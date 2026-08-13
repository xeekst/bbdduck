//! 隧道执行引擎：基于 russh + tokio。
//! - 本地转发（-L）：本机监听，经 SSH 转发到服务器可达的目标
//! - 远程转发（-R）：SSH 服务器监听，经 SSH 转发回本机可达的目标
//! - 动态转发（-D）：本机 SOCKS5 代理，按需经 SSH 访问任意目标
//! - TCP 直接使用 SSH 的 direct-tcpip 通道；UDP 通过 exec 通道内的
//!   轻量帧协议 + 远端 python 中继实现（需远程主机有 python3/python）
//!
//! 每个隧道运行在独立任务中，watch 通道实现随时停止；连接断开且开启
//! 自动重连时以 5 秒间隔自动重试，SSH 层配置了 keepalive 保活。

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex as TokioMutex;

use crate::ssh_tunnel::manager::TunnelRuntime;
use crate::ssh_tunnel::model::*;
use crate::sync::model::now_secs;

// ---------------- 事件回调 --------------

/// 隧道运行过程中的日志与状态回调（由 TunnelManager 实现，测试中可替换为空实现）
pub trait TunnelEvents: Send + Sync {
    fn log(&self, rt: &Arc<TunnelRuntime>, level: &str, message: String);
    fn emit_state(&self, rt: &Arc<TunnelRuntime>);
}

// ---------------- 错误 ----------------

pub struct RunError {
    pub message: String,
    /// fatal = 不可恢复（配置/绑定错误），不再自动重连
    pub fatal: bool,
}

impl std::fmt::Debug for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunError")
            .field("message", &self.message)
            .field("fatal", &self.fatal)
            .finish()
    }
}

impl RunError {
    fn fatal(message: impl Into<String>) -> Self {
        RunError {
            message: message.into(),
            fatal: true,
        }
    }
    fn transient(message: impl Into<String>) -> Self {
        RunError {
            message: message.into(),
            fatal: false,
        }
    }
}

// ---------------- 停止信号 ----------------

pub async fn stop_signal(rt: &TunnelRuntime) {
    let mut rx = rt.stop_rx.clone();
    if *rx.borrow() {
        return;
    }
    let _ = rx.changed().await;
}

/// 等待一段时间；若期间被停止则返回 true
pub async fn sleep_or_stop(rt: &TunnelRuntime, d: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => false,
        _ = stop_signal(rt) => true,
    }
}

// ---------------- 主循环 ----------------

/// 隧道任务主循环：连接 → 运行 → 断线自动重连，直到被停止
pub async fn run_tunnel(mgr: Arc<dyn TunnelEvents>, rt: Arc<TunnelRuntime>) {
    let mut attempt: u32 = 0;
    loop {
        if rt.is_stopped() {
            break;
        }
        attempt += 1;
        rt.set_state(STATE_CONNECTING, None);
        mgr.emit_state(&rt);
        mgr.log(
            &rt,
            "info",
            format!("第 {attempt} 次尝试连接 SSH {} …", rt.config.ssh_server_display()),
        );
        match run_attempt(&mgr, &rt).await {
            Ok(()) => break,
            Err(e) => {
                if rt.is_stopped() {
                    break;
                }
                mgr.log(&rt, "error", e.message.clone());
                rt.set_state(STATE_ERROR, Some(e.message));
                mgr.emit_state(&rt);
                if e.fatal || !rt.config.auto_reconnect {
                    break;
                }
                mgr.log(&rt, "info", "已开启自动重连，5 秒后重试…".into());
                if sleep_or_stop(&rt, Duration::from_secs(5)).await {
                    break;
                }
            }
        }
    }
    rt.set_state(STATE_STOPPED, None);
    mgr.emit_state(&rt);
    mgr.log(&rt, "info", "隧道已停止".into());
}

/// 一次完整连接尝试：建连 → 认证 → 按类型运行转发逻辑
pub async fn run_attempt(
    mgr: &Arc<dyn TunnelEvents>,
    rt: &Arc<TunnelRuntime>,
) -> Result<(), RunError> {
    let cfg = rt.config.clone();
    let handle = connect_or_stop(mgr, rt, &cfg).await?;

    rt.connections.store(0, Ordering::Relaxed);
    rt.set_connected_at(Some(now_secs()));
    rt.set_state(STATE_RUNNING, None);
    mgr.emit_state(rt);
    mgr.log(&rt, "info", "SSH 连接建立，认证成功，密钥协商完成".into());

    let result = match (cfg.tunnel_type, cfg.proto) {
        (TunnelType::Local, TunnelProto::Tcp) => {
            let session = Arc::new(handle);
            run_local_tcp(mgr, rt, &session).await
        }
        (TunnelType::Local, TunnelProto::Udp) => {
            let session = Arc::new(handle);
            run_local_udp(mgr, rt, &session).await
        }
        (TunnelType::Remote, TunnelProto::Tcp) => run_remote_tcp(mgr, rt, handle).await,
        (TunnelType::Remote, TunnelProto::Udp) => {
            let session = Arc::new(handle);
            run_remote_udp(mgr, rt, &session).await
        }
        (TunnelType::Dynamic, _) => {
            let session = Arc::new(handle);
            run_dynamic_socks(mgr, rt, &session).await
        }
    };
    rt.abort_tasks().await;
    result
}

// ---------------- SSH 连接与认证 ----------------

struct ClientHandler {
    mgr: Arc<dyn TunnelEvents>,
    rt: Arc<TunnelRuntime>,
    /// 远程转发时：SSH 服务器上来的连接要转发的本地目标
    target_host: String,
    target_port: u16,
}

impl russh::client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        // 简单工具场景：接受任意主机密钥（日志中已提示）
        Ok(true)
    }

    /// 远程转发：SSH 服务器上有客户端连到监听端口，把通道转接给本机目标
    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        _connected_address: &str,
        _connected_port: u32,
        originator_address: &str,
        originator_port: u32,
        reply: russh::client::ChannelOpenHandle,
        _session: &mut russh::client::Session,
    ) -> Result<(), Self::Error> {
        let target_host = self.target_host.clone();
        let target_port = self.target_port;
        let mgr = self.mgr.clone();
        let rt = self.rt.clone();
        let origin = format!("{originator_address}:{originator_port}");
        let rt_task = rt.clone();
        rt.spawn_task(async move {
            match TcpStream::connect((target_host.as_str(), target_port)).await {
                Ok(mut socket) => {
                    reply.accept().await;
                    rt_task.connections.fetch_add(1, Ordering::Relaxed);
                    mgr.log(&rt_task, "info", format!("远程转发: {origin} → {target_host}:{target_port}"));
                    let mut stream = channel.into_stream();
                    match tokio::io::copy_bidirectional(&mut socket, &mut stream).await {
                        Ok((up, down)) => {
                            rt_task.bytes_up.fetch_add(up, Ordering::Relaxed);
                            rt_task.bytes_down.fetch_add(down, Ordering::Relaxed);
                            mgr.log(&rt_task, "info", format!("远程转发: {origin} 断开（↑{up} B ↓{down} B）"));
                        }
                        Err(e) => mgr.log(&rt_task, "warn", format!("远程转发: {origin} 中断: {e}")),
                    }
                    rt_task.connections.fetch_sub(1, Ordering::Relaxed);
                }
                Err(e) => {
                    mgr.log(
                        &rt_task,
                        "warn",
                        format!("远程转发: 无法连接本地目标 {target_host}:{target_port}: {e}"),
                    );
                    reply
                        .reject(russh::ChannelOpenFailure::ConnectFailed)
                        .await;
                }
            }
        });
        Ok(())
    }
}

async fn connect_auth(
    mgr: Arc<dyn TunnelEvents>,
    rt: Arc<TunnelRuntime>,
    cfg: &TunnelConfig,
) -> Result<russh::client::Handle<ClientHandler>, RunError> {
    let mut client_cfg = russh::client::Config::default();
    client_cfg.keepalive_interval =
        Some(Duration::from_secs(cfg.keepalive_secs.clamp(5, 3600) as u64));
    client_cfg.keepalive_max = 3;
    let client_cfg = Arc::new(client_cfg);

    let handler = ClientHandler {
        mgr,
        rt,
        target_host: cfg.target_host.clone(),
        target_port: cfg.target_port,
    };
    let mut handle = russh::client::connect(
        client_cfg,
        (cfg.ssh_host.clone(), cfg.ssh_port),
        handler,
    )
    .await
    .map_err(|e| RunError::transient(format!("SSH 连接失败: {e}")))?;

    let auth_res = match cfg.auth {
        AuthKind::Password => {
            let password = cfg
                .password
                .clone()
                .ok_or_else(|| RunError::fatal("未设置密码"))?;
            handle
                .authenticate_password(cfg.username.clone(), password)
                .await
                .map_err(|e| RunError::fatal(format!("认证失败: {e}")))?
        }
        AuthKind::Key => {
            let path = cfg
                .key_path
                .clone()
                .ok_or_else(|| RunError::fatal("未设置私钥路径"))?;
            let content = std::fs::read_to_string(&path)
                .map_err(|e| RunError::fatal(format!("读取私钥文件失败: {e}")))?;
            let pass = cfg.key_passphrase.clone().filter(|s| !s.is_empty());
            let key = russh::keys::decode_secret_key(&content, pass.as_deref())
                .map_err(|e| RunError::fatal(format!("解析私钥失败: {e}")))?;
            let key = russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), None);
            handle
                .authenticate_publickey(cfg.username.clone(), key)
                .await
                .map_err(|e| RunError::fatal(format!("公钥认证失败: {e}")))?
        }
    };
    if !auth_res.success() {
        return Err(RunError::fatal("认证失败（用户名或密码/私钥错误）"));
    }
    Ok(handle)
}

async fn connect_or_stop(
    mgr: &Arc<dyn TunnelEvents>,
    rt: &Arc<TunnelRuntime>,
    cfg: &TunnelConfig,
) -> Result<russh::client::Handle<ClientHandler>, RunError> {
    let fut = connect_auth(mgr.clone(), rt.clone(), cfg);
    tokio::pin!(fut);
    tokio::select! {
        r = &mut fut => r,
        _ = stop_signal(rt) => Err(RunError { message: "已停止".into(), fatal: true }),
    }
}

// ---------------- 本地 TCP 转发 ----------------

async fn run_local_tcp(
    mgr: &Arc<dyn TunnelEvents>,
    rt: &Arc<TunnelRuntime>,
    handle: &Arc<russh::client::Handle<ClientHandler>>,
) -> Result<(), RunError> {
    let cfg = rt.config.clone();
    let bind = format!("{}:{}", cfg.listen_host, cfg.listen_port);
    let listener = TcpListener::bind(&bind)
        .await
        .map_err(|e| RunError::fatal(format!("本地监听 {bind} 失败: {e}")))?;
    let actual = listener.local_addr().map(|a| a.to_string()).unwrap_or(bind);
    rt.set_listen_addr(&actual);
    mgr.emit_state(rt);
    mgr.log(
        rt,
        "info",
        format!(
            "本地转发已启动: {actual} ⇄ SSH({}) ⇄ {}:{}",
            cfg.ssh_host, cfg.target_host, cfg.target_port
        ),
    );

    loop {
        tokio::select! {
            _ = stop_signal(rt) => break,
            accepted = listener.accept() => {
                let (socket, peer) = match accepted {
                    Ok(x) => x,
                    Err(e) => {
                        mgr.log(rt, "warn", format!("accept 失败: {e}"));
                        continue;
                    }
                };
                let handle = handle.clone();
                let mgr = mgr.clone();
                let rt = rt.clone();
                let target_host = cfg.target_host.clone();
                let target_port = cfg.target_port;
                let rt_task = rt.clone();
                rt.spawn_task(async move {
                    rt_task.connections.fetch_add(1, Ordering::Relaxed);
                    mgr.log(&rt_task, "info", format!("新连接 {peer} → {target_host}:{target_port}"));
                    match handle
                        .channel_open_direct_tcpip(&target_host, target_port as u32, "127.0.0.1", 0)
                        .await
                    {
                        Ok(channel) => {
                            let mut stream = channel.into_stream();
                            let mut socket = socket;
                            match tokio::io::copy_bidirectional(&mut socket, &mut stream).await {
                                Ok((up, down)) => {
                                    rt_task.bytes_up.fetch_add(up, Ordering::Relaxed);
                                    rt_task.bytes_down.fetch_add(down, Ordering::Relaxed);
                                    mgr.log(&rt_task, "info", format!("{peer} 断开（↑{up} B ↓{down} B）"));
                                }
                                Err(e) => mgr.log(&rt_task, "warn", format!("{peer} 连接中断: {e}")),
                            }
                        }
                        Err(e) => mgr.log(&rt_task, "warn", format!("{peer} 打开 SSH 通道失败: {e}")),
                    }
                    rt_task.connections.fetch_sub(1, Ordering::Relaxed);
                });
            }
        }
    }
    Ok(())
}

// ---------------- 远程 TCP 转发 ----------------

async fn run_remote_tcp(
    mgr: &Arc<dyn TunnelEvents>,
    rt: &Arc<TunnelRuntime>,
    handle: russh::client::Handle<ClientHandler>,
) -> Result<(), RunError> {
    let cfg = rt.config.clone();
    let chosen = handle
        .tcpip_forward(cfg.listen_host.as_str(), cfg.listen_port as u32)
        .await
        .map_err(|e| {
            RunError::fatal(format!(
                "请求远程端口转发失败（若监听非 127.0.0.1，需服务器允许 GatewayPorts）: {e}"
            ))
        })?;
    let actual_port: u16 = if chosen != 0 { chosen as u16 } else { cfg.listen_port };
    rt.set_listen_addr(&format!("{}:{}", cfg.ssh_host, actual_port));
    mgr.emit_state(rt);
    mgr.log(
        rt,
        "info",
        format!(
            "远程转发已启动: {}:{} ⇄ 本机 ⇄ {}:{}",
            cfg.ssh_host, actual_port, cfg.target_host, cfg.target_port
        ),
    );

    // 保持会话直到停止或连接断开
    tokio::select! {
        _ = stop_signal(rt) => Ok(()),
        r = handle => match r {
            Ok(()) => Err(RunError::transient("SSH 连接已关闭")),
            Err(e) => Err(RunError::transient(format!("SSH 连接断开: {e}"))),
        },
    }
}

// ---------------- 动态转发（SOCKS5） ----------------

async fn run_dynamic_socks(
    mgr: &Arc<dyn TunnelEvents>,
    rt: &Arc<TunnelRuntime>,
    handle: &Arc<russh::client::Handle<ClientHandler>>,
) -> Result<(), RunError> {
    let cfg = rt.config.clone();
    let bind = format!("{}:{}", cfg.listen_host, cfg.listen_port);
    let listener = TcpListener::bind(&bind)
        .await
        .map_err(|e| RunError::fatal(format!("本地 SOCKS5 监听 {bind} 失败: {e}")))?;
    let actual = listener.local_addr().map(|a| a.to_string()).unwrap_or(bind);
    rt.set_listen_addr(&actual);
    mgr.emit_state(rt);
    mgr.log(
        rt,
        "info",
        format!(
            "SOCKS5 动态转发已启动: {actual} ⇄ SSH({})（客户端经此代理按需访问任意目标）",
            cfg.ssh_host
        ),
    );

    loop {
        tokio::select! {
            _ = stop_signal(rt) => break,
            accepted = listener.accept() => {
                let (socket, peer) = match accepted {
                    Ok(x) => x,
                    Err(e) => {
                        mgr.log(rt, "warn", format!("accept 失败: {e}"));
                        continue;
                    }
                };
                let handle = handle.clone();
                let mgr = mgr.clone();
                let rt = rt.clone();
                let rt_task = rt.clone();
                rt.spawn_task(async move {
                    rt_task.connections.fetch_add(1, Ordering::Relaxed);
                    mgr.log(&rt_task, "info", format!("SOCKS5 新连接 {peer}"));
                    let mut socket = socket;
                    match socks5_connect(&mut socket, &handle).await {
                        Ok((target, up, down)) => {
                            rt_task.bytes_up.fetch_add(up, Ordering::Relaxed);
                            rt_task.bytes_down.fetch_add(down, Ordering::Relaxed);
                            mgr.log(&rt_task, "info", format!("SOCKS5 {peer} → {target} 完成（↑{up} B ↓{down} B）"));
                        }
                        Err(e) => mgr.log(&rt_task, "warn", format!("SOCKS5 {peer}: {e}")),
                    }
                    rt_task.connections.fetch_sub(1, Ordering::Relaxed);
                });
            }
        }
    }
    Ok(())
}

/// 处理一个 SOCKS5 客户端连接（无认证，仅 CONNECT）
async fn socks5_connect(
    socket: &mut TcpStream,
    handle: &Arc<russh::client::Handle<ClientHandler>>,
) -> Result<(String, u64, u64), String> {
    let t = Duration::from_secs(15);
    let mut buf = [0u8; 512];

    // 握手：VER NMETHODS METHODS
    timeout_io(t, socket.read_exact(&mut buf[..2])).await?;
    if buf[0] != 5 {
        return Err("不是 SOCKS5 协议".into());
    }
    let nmethods = buf[1] as usize;
    if nmethods == 0 || nmethods > 255 {
        return Err("SOCKS5 方法数非法".into());
    }
    timeout_io(t, socket.read_exact(&mut buf[..nmethods])).await?;
    if !buf[..nmethods].contains(&0) {
        let _ = socket.write_all(&[5, 0xff]).await;
        return Err("不支持客户端提出的认证方式".into());
    }
    socket.write_all(&[5, 0]).await.map_err(|e| e.to_string())?;

    // 请求：VER CMD RSV ATYP
    timeout_io(t, socket.read_exact(&mut buf[..4])).await?;
    let (cmd, atyp) = (buf[1], buf[3]);
    let host = match atyp {
        1 => {
            timeout_io(t, socket.read_exact(&mut buf[..4])).await?;
            IpAddr::from([buf[0], buf[1], buf[2], buf[3]]).to_string()
        }
        4 => {
            timeout_io(t, socket.read_exact(&mut buf[..16])).await?;
            let mut v6 = [0u8; 16];
            v6.copy_from_slice(&buf[..16]);
            IpAddr::from(v6).to_string()
        }
        3 => {
            timeout_io(t, socket.read_exact(&mut buf[..1])).await?;
            let len = buf[0] as usize;
            timeout_io(t, socket.read_exact(&mut buf[..len])).await?;
            String::from_utf8_lossy(&buf[..len]).to_string()
        }
        _ => {
            let _ = socket.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await;
            return Err("不支持的地址类型".into());
        }
    };
    timeout_io(t, socket.read_exact(&mut buf[..2])).await?;
    let port = u16::from_be_bytes([buf[0], buf[1]]);

    if cmd != 1 {
        let _ = socket.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await;
        return Err("仅支持 CONNECT 命令".into());
    }

    let channel = handle
        .channel_open_direct_tcpip(host.clone(), port as u32, "127.0.0.1", 0)
        .await
        .map_err(|e| format!("打开 SSH 通道失败: {e}"))?;
    socket
        .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
        .await
        .map_err(|e| e.to_string())?;

    let mut stream = channel.into_stream();
    let (up, down) = tokio::io::copy_bidirectional(socket, &mut stream)
        .await
        .map_err(|e| format!("传输中断: {e}"))?;
    Ok((host, up, down))
}

async fn timeout_io<F, T>(d: Duration, fut: F) -> Result<T, String>
where
    F: Future<Output = std::io::Result<T>>,
{
    tokio::time::timeout(d, fut)
        .await
        .map_err(|_| "SOCKS5 客户端超时".to_string())?
        .map_err(|e| e.to_string())
}

// ---------------- UDP 转发（SSH exec 通道 + 远端 python 中继） ----------------

/// 帧头：op(1) + rsv(1) + flow(4) + len(4)，payload 最大 64KB
const FRAME_HEADER: usize = 10;
const MAX_FRAME: usize = 65536;
const UDP_FLOW_MAX: usize = 512;
const UDP_FLOW_IDLE: Duration = Duration::from_secs(120);

pub async fn read_frame<R: AsyncRead + Unpin>(
    r: &mut R,
) -> std::io::Result<(u8, u32, Vec<u8>)> {
    let mut hdr = [0u8; FRAME_HEADER];
    r.read_exact(&mut hdr).await?;
    let op = hdr[0];
    let fid = u32::from_le_bytes([hdr[2], hdr[3], hdr[4], hdr[5]]);
    let len = u32::from_le_bytes([hdr[6], hdr[7], hdr[8], hdr[9]]) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "UDP 帧过大",
        ));
    }
    let mut data = vec![0u8; len];
    r.read_exact(&mut data).await?;
    Ok((op, fid, data))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    op: u8,
    fid: u32,
    data: &[u8],
) -> std::io::Result<()> {
    let mut hdr = [0u8; FRAME_HEADER];
    hdr[0] = op;
    hdr[2..6].copy_from_slice(&fid.to_le_bytes());
    hdr[6..10].copy_from_slice(&(data.len() as u32).to_le_bytes());
    w.write_all(&hdr).await?;
    if !data.is_empty() {
        w.write_all(data).await?;
    }
    w.flush().await?;
    Ok(())
}

/// 远端 UDP 中继脚本（py2/py3 兼容，注意：脚本内不能出现单引号）
const UDP_RELAY_PY: &str = r#"
import sys, socket, struct, select, time, threading
try:
    stdin = sys.stdin.buffer
    stdout = sys.stdout.buffer
except AttributeError:
    stdin = sys.stdin
    stdout = sys.stdout
HOST = sys.argv[1]
PORT = int(sys.argv[2])
MODE = sys.argv[3]
lock = threading.Lock()
flows = {}
by_addr = {}
next_fid = [0]
IDLE = 120.0
def alloc_fid():
    with lock:
        fid = next_fid[0]
        next_fid[0] = (fid + 1) & 0xFFFFFFFF
        return fid
def send_frame(fid, data, op=1):
    head = struct.pack("<BBII", op, 0, fid, len(data))
    stdout.write(head + data)
    stdout.flush()
def recvn(n):
    buf = b""
    while len(buf) < n:
        chunk = stdin.read(n - len(buf))
        if not chunk:
            raise EOFError()
        buf += chunk
    return buf
def reader(sock):
    try:
        while True:
            h = recvn(10)
            op, _, fid, ln = struct.unpack("<BBII", h)
            if ln > 65536:
                continue
            data = recvn(ln) if ln else b""
            if op == 2:
                with lock:
                    item = flows.pop(fid, None)
                if item is None:
                    continue
                if MODE == "connect":
                    try:
                        item[0].close()
                    except Exception:
                        pass
                else:
                    with lock:
                        by_addr.pop(item[0], None)
                continue
            with lock:
                item = flows.get(fid)
                if item is None:
                    if MODE == "connect":
                        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                        s.settimeout(0.0)
                        s.connect((HOST, PORT))
                        item = [s, time.time()]
                    else:
                        item = [None, time.time()]
                    flows[fid] = item
            if MODE == "connect":
                try:
                    item[0].send(data)
                except Exception:
                    pass
                with lock:
                    item[1] = time.time()
            else:
                with lock:
                    item = flows.get(fid)
                if item is not None and item[0] is not None:
                    try:
                        sock.sendto(data, item[0])
                    except Exception:
                        pass
                    with lock:
                        item[1] = time.time()
    except Exception:
        return
def prune():
    now = time.time()
    with lock:
        stale = [fid for fid, item in flows.items() if now - item[1] > IDLE]
    for fid in stale:
        with lock:
            item = flows.pop(fid, None)
        if item is None:
            continue
        if MODE == "listen":
            with lock:
                by_addr.pop(item[0], None)
        else:
            try:
                item[0].close()
            except Exception:
                pass
        try:
            send_frame(fid, b"", 2)
        except Exception:
            pass
def main_loop():
    if MODE == "listen":
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.settimeout(0.5)
        s.bind((HOST, PORT))
        t = threading.Thread(target=reader, args=(s,))
        t.daemon = True
        t.start()
        while True:
            try:
                data, addr = s.recvfrom(65535)
            except Exception:
                data = None
            if data is not None:
                with lock:
                    fid = by_addr.get(addr)
                    if fid is None:
                        fid = alloc_fid()
                        by_addr[addr] = fid
                        flows[fid] = [addr, time.time()]
                send_frame(fid, data)
            prune()
    else:
        t = threading.Thread(target=reader, args=(None,))
        t.daemon = True
        t.start()
        while True:
            with lock:
                items = [(fid, item[0]) for fid, item in flows.items() if item[0] is not None]
            if items:
                socks = [it[1] for it in items]
                try:
                    r, _, _ = select.select(socks, [], [], 0.5)
                except Exception:
                    r = []
                for sock in r:
                    try:
                        data = sock.recv(65535)
                    except Exception:
                        data = None
                    if data:
                        fid = None
                        for f0, f1 in items:
                            if f1 is sock:
                                fid = f0
                                break
                        if fid is not None:
                            send_frame(fid, data)
                            with lock:
                                item = flows.get(fid)
                                if item is not None:
                                    item[1] = time.time()
            else:
                time.sleep(0.5)
            prune()
main_loop()
"#;

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn udp_relay_command(host: &str, port: u16, mode: &str) -> String {
    let qscript = shell_quote(UDP_RELAY_PY);
    let qhost = shell_quote(host);
    let qmode = shell_quote(mode);
    format!(
        "python3 -u -c {qscript} {qhost} {port} {qmode} || python -u -c {qscript} {qhost} {port} {qmode}"
    )
}

async fn open_exec(
    mgr: &Arc<dyn TunnelEvents>,
    rt: &Arc<TunnelRuntime>,
    handle: &Arc<russh::client::Handle<ClientHandler>>,
    command: &str,
) -> Result<russh::Channel<russh::client::Msg>, RunError> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| RunError::fatal(format!("打开会话通道失败: {e}")))?;
    channel
        .exec(true, command.as_bytes().to_vec())
        .await
        .map_err(|e| {
            RunError::fatal(format!(
                "启动远程 UDP 中继失败（需要远程主机装有 python3 或 python）: {e}"
            ))
        })?;
    mgr.log(rt, "info", "远程 UDP 中继已通过 exec 通道启动".into());
    Ok(channel)
}

struct UdpFlowLocal {
    fid: u32,
    last: Instant,
}

/// 本地 UDP 转发：本机 UDP 监听 → SSH exec 通道（python 中继）→ 远端目标 UDP
async fn run_local_udp(
    mgr: &Arc<dyn TunnelEvents>,
    rt: &Arc<TunnelRuntime>,
    handle: &Arc<russh::client::Handle<ClientHandler>>,
) -> Result<(), RunError> {
    let cfg = rt.config.clone();
    let bind = format!("{}:{}", cfg.listen_host, cfg.listen_port);
    let sock = Arc::new(
        UdpSocket::bind(&bind)
            .await
            .map_err(|e| RunError::fatal(format!("本地 UDP 监听 {bind} 失败: {e}")))?,
    );
    let actual = sock.local_addr().map(|a| a.to_string()).unwrap_or(bind);
    rt.set_listen_addr(&actual);
    mgr.emit_state(rt);

    let cmd = udp_relay_command(&cfg.target_host, cfg.target_port, "connect");
    let channel = open_exec(mgr, rt, handle, &cmd).await?;
    let stream = Box::pin(channel.into_stream());
    let (mut read_half, write_half) = tokio::io::split(stream);
    let writer: Arc<TokioMutex<_>> = Arc::new(TokioMutex::new(write_half));

    mgr.log(
        rt,
        "info",
        format!(
            "UDP 本地转发已启动: {actual} ⇄ SSH({}) ⇄ {}:{} (UDP)",
            cfg.ssh_host, cfg.target_host, cfg.target_port
        ),
    );

    let flows: Arc<TokioMutex<HashMap<SocketAddr, UdpFlowLocal>>> = Arc::default();
    let next_fid = Arc::new(AtomicU32::new(1));

    // 读方向：中继通道 → 本地 UDP 客户端
    {
        let mgr = mgr.clone();
        let rt_task = rt.clone();
        let flows = flows.clone();
        let sock = sock.clone();
        rt.spawn_task(async move {
            loop {
                match read_frame(&mut read_half).await {
                    Ok((1, fid, data)) => {
                        let peer = flows
                            .lock()
                            .await
                            .iter()
                            .find(|(_, f)| f.fid == fid)
                            .map(|(p, _)| *p);
                        if let Some(peer) = peer {
                            match sock.send_to(&data, peer).await {
                                Ok(_) => {
                                    rt_task.bytes_down.fetch_add(data.len() as u64, Ordering::Relaxed);
                                }
                                Err(e) => mgr.log(&rt_task, "warn", format!("UDP 回包发送失败: {e}")),
                            }
                        }
                    }
                    Ok((2, fid, _)) => {
                        flows.lock().await.retain(|_, f| f.fid != fid);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        mgr.log(&rt_task, "warn", format!("UDP 中继通道读取结束: {e}"));
                        break;
                    }
                }
            }
        });
    }

    // 清理任务：淘汰空闲流并向远端发 CLOSE
    {
        let rt_task = rt.clone();
        let flows = flows.clone();
        let writer = writer.clone();
        rt.spawn_task(async move {
            loop {
                if sleep_or_stop(&rt_task, Duration::from_secs(30)).await {
                    break;
                }
                let now = Instant::now();
                let stale: Vec<u32> = flows
                    .lock()
                    .await
                    .iter()
                    .filter(|(_, f)| now.duration_since(f.last) > UDP_FLOW_IDLE)
                    .map(|(_, f)| f.fid)
                    .collect();
                for fid in stale {
                    flows.lock().await.retain(|_, f| f.fid != fid);
                    let _ = write_frame(&mut *writer.lock().await, 2, fid, &[]).await;
                }
            }
        });
    }

    // 主循环：本地 UDP 客户端 → 中继通道
    let mut buf = [0u8; MAX_FRAME];
    loop {
        tokio::select! {
            _ = stop_signal(rt) => break,
            r = sock.recv_from(&mut buf) => {
                let (n, peer) = r
                    .map_err(|e| RunError::transient(format!("UDP 接收失败: {e}")))?;
                let fid = {
                    let mut fl = flows.lock().await;
                    match fl.get_mut(&peer) {
                        Some(f) => {
                            f.last = Instant::now();
                            f.fid
                        }
                        None => {
                            if fl.len() >= UDP_FLOW_MAX {
                                drop(fl);
                                mgr.log(rt, "warn", "UDP 并发流数达到上限，丢弃数据包".into());
                                continue;
                            }
                            let fid = next_fid.fetch_add(1, Ordering::Relaxed);
                            fl.insert(peer, UdpFlowLocal { fid, last: Instant::now() });
                            fid
                        }
                    }
                };
                let mut w = writer.lock().await;
                write_frame(&mut *w, 1, fid, &buf[..n])
                    .await
                    .map_err(|e| RunError::transient(format!("UDP 隧道写入失败: {e}")))?;
                rt.bytes_up.fetch_add(n as u64, Ordering::Relaxed);
            }
        }
    }
    Ok(())
}

/// 远程 UDP 转发：SSH 服务器上 python 监听 UDP → exec 通道 → 本机目标 UDP
async fn run_remote_udp(
    mgr: &Arc<dyn TunnelEvents>,
    rt: &Arc<TunnelRuntime>,
    handle: &Arc<russh::client::Handle<ClientHandler>>,
) -> Result<(), RunError> {
    let cfg = rt.config.clone();
    let cmd = udp_relay_command(&cfg.listen_host, cfg.listen_port, "listen");
    let channel = open_exec(mgr, rt, handle, &cmd).await?;
    let stream = Box::pin(channel.into_stream());
    let (mut read_half, write_half) = tokio::io::split(stream);
    let writer: Arc<TokioMutex<_>> = Arc::new(TokioMutex::new(write_half));

    rt.set_listen_addr(&format!("{}:{} (UDP)", cfg.ssh_host, cfg.listen_port));
    mgr.emit_state(rt);
    mgr.log(
        rt,
        "info",
        format!(
            "UDP 远程转发已启动: {}:{} (UDP) ⇄ 本机 ⇄ {}:{}",
            cfg.ssh_host, cfg.listen_port, cfg.target_host, cfg.target_port
        ),
    );

    // fid -> 与本地目标相连的 UDP 套接字；spawned 记录是否已派生出回包读取任务
    let flows: Arc<TokioMutex<HashMap<u32, Arc<UdpSocket>>>> = Arc::default();
    let spawned: Arc<TokioMutex<std::collections::HashSet<u32>>> = Arc::default();

    loop {
        tokio::select! {
            _ = stop_signal(rt) => break,
            frame = read_frame(&mut read_half) => {
                let (op, fid, data) = frame.map_err(|e| RunError::transient(format!("UDP 中继通道读取失败: {e}")))?;
                if op == 2 {
                    flows.lock().await.remove(&fid);
                    spawned.lock().await.remove(&fid);
                    continue;
                }
                if op != 1 {
                    continue;
                }
                let (sock, is_new) = {
                    let mut fl = flows.lock().await;
                    match fl.get(&fid) {
                        Some(s) => (s.clone(), false),
                        None => {
                            let target = format!("{}:{}", cfg.target_host, cfg.target_port);
                            match UdpSocket::bind("0.0.0.0:0").await {
                                Ok(s) => {
                                    let _ = s.connect(&target).await;
                                    let s = Arc::new(s);
                                    fl.insert(fid, s.clone());
                                    (s, true)
                                }
                                Err(e) => {
                                    mgr.log(rt, "warn", format!("创建本地 UDP 套接字失败: {e}"));
                                    continue;
                                }
                            }
                        }
                    }
                };
                match sock.send(&data).await {
                    Ok(_) => {
                        rt.bytes_down.fetch_add(data.len() as u64, Ordering::Relaxed);
                    }
                    Err(e) => mgr.log(rt, "warn", format!("UDP 发送到本地目标失败: {e}")),
                }
                // 每个流只派生一个回包读取任务（空闲 120s 自动退出）
                if is_new && !spawned.lock().await.insert(fid) {
                    let rt_task = rt.clone();
                    let writer = writer.clone();
                    rt.spawn_task(async move {
                        let mut buf = [0u8; MAX_FRAME];
                        loop {
                            let r = tokio::time::timeout(UDP_FLOW_IDLE, sock.recv(&mut buf)).await;
                            match r {
                                Err(_) | Ok(Err(_)) => break,
                                Ok(Ok(0)) => break,
                                Ok(Ok(n)) => {
                                    let mut w = writer.lock().await;
                                    if write_frame(&mut *w, 1, fid, &buf[..n]).await.is_err() {
                                        break;
                                    }
                                    rt_task.bytes_up.fetch_add(n as u64, Ordering::Relaxed);
                                }
                            }
                        }
                    });
                }
            }
        }
    }
    Ok(())
}

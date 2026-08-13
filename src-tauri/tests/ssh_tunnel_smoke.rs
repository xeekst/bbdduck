//! SSH 隧道冒烟测试：
//! 在本机启动一个真实 russh SSH 服务器（密码认证）和一个 TCP echo 服务器，
//! 用 runner 的「本地 TCP 转发」打通数据链路，验证连接、认证、通道与双向拷贝；
//! 另对 UDP 中继帧协议做往返测试。

use std::sync::Arc;
use std::time::Duration;

use bbdduck_lib::ssh_tunnel::manager::TunnelRuntime;
use bbdduck_lib::ssh_tunnel::model::{AuthKind, TunnelConfig, TunnelProto, TunnelType};
use bbdduck_lib::ssh_tunnel::runner::{self, TunnelEvents};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 测试专用主机密钥（仅用于本测试的本地 SSH 服务器）
const HOST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBs7IV1Y2vtEXNsPnIByok33xIuRKPsd8g8mRb72jrjOgAAAJhABf72QAX+
9gAAAAtzc2gtZWQyNTUxOQAAACBs7IV1Y2vtEXNsPnIByok33xIuRKPsd8g8mRb72jrjOg
AAAEDGPfhg1nNOoLk1bN1G0JvGEGe0+C3RDWIQ2csQSSoHFGzshXVja+0Rc2w+cgHKiTff
Ei5Eo+x3yDyZFvvaOuM6AAAAEngxQExBUFRPUC05TjI0MlI2TAECAw==
-----END OPENSSH PRIVATE KEY-----
";

// ---------------- 空事件实现 ----------------

struct NoopEvents;

impl TunnelEvents for NoopEvents {
    fn log(&self, _rt: &Arc<TunnelRuntime>, _level: &str, _message: String) {}
    fn emit_state(&self, _rt: &Arc<TunnelRuntime>) {}
}

// ---------------- 测试 SSH 服务器 ----------------

#[derive(Clone)]
struct TestServerHandler;

impl russh::server::Handler for TestServerHandler {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<russh::server::Auth, Self::Error> {
        if user == "test" && password == "secret" {
            Ok(russh::server::Auth::Accept)
        } else {
            Ok(russh::server::Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            })
        }
    }

    /// 接受客户端的 direct-tcpip 通道请求（模拟 sshd 的端口转发行为）
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        let target = format!("{host_to_connect}:{port_to_connect}");
        tokio::spawn(async move {
            match tokio::net::TcpStream::connect(&target).await {
                Ok(mut socket) => {
                    reply.accept().await;
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                }
                Err(_) => {
                    reply
                        .reject(russh::ChannelOpenFailure::ConnectFailed)
                        .await;
                }
            }
        });
        Ok(())
    }
}

/// 启动 SSH 服务器，返回监听端口
async fn spawn_ssh_server() -> u16 {
    let key = russh::keys::decode_secret_key(HOST_KEY, None).expect("解析主机密钥失败");
    let config = Arc::new(russh::server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::from_millis(100),
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("SSH 服务器监听失败");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        // 保持会话句柄存活，避免后台任务被取消
        let mut sessions: Vec<russh::server::RunningSession<TestServerHandler>> = Vec::new();
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            let cfg = config.clone();
            match russh::server::run_stream(cfg, socket, TestServerHandler).await {
                Ok(s) => sessions.push(s),
                Err(_) => {}
            }
        }
    });
    port
}

/// 启动 TCP echo 服务器，返回监听端口
async fn spawn_echo_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("echo 服务器监听失败");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                loop {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if socket.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    port
}

fn reserve_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn local_tcp_config(ssh_port: u16, echo_port: u16, listen_port: u16) -> TunnelConfig {
    TunnelConfig {
        id: 1,
        name: "smoke".into(),
        tunnel_type: TunnelType::Local,
        proto: TunnelProto::Tcp,
        ssh_host: "127.0.0.1".into(),
        ssh_port,
        username: "test".into(),
        auth: AuthKind::Password,
        password: Some("secret".into()),
        key_path: None,
        key_passphrase: None,
        listen_host: "127.0.0.1".into(),
        listen_port,
        target_host: "127.0.0.1".into(),
        target_port: echo_port,
        keepalive_secs: 5,
        auto_reconnect: false,
        enabled: false,
        created_at: 0,
    }
}

// ---------------- 测试 ----------------

#[tokio::test(flavor = "multi_thread")]
async fn local_tcp_forward_roundtrip() {
    let ssh_port = spawn_ssh_server().await;
    let echo_port = spawn_echo_server().await;
    let listen_port = reserve_port();

    let rt = Arc::new(TunnelRuntime::new(local_tcp_config(
        ssh_port, echo_port, listen_port,
    )));
    let mgr: Arc<dyn TunnelEvents> = Arc::new(NoopEvents);
    let rt2 = rt.clone();
    let task = tokio::spawn(async move { runner::run_attempt(&mgr, &rt2).await });

    // 等待本地转发监听就绪
    let mut client = None;
    for _ in 0..100 {
        if let Ok(s) = tokio::net::TcpStream::connect(("127.0.0.1", listen_port)).await {
            client = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut client = client.expect("本地转发监听未就绪");

    // 通过 SSH 隧道发送数据，echo 服务器原样返回
    client
        .write_all(b"hello-over-ssh")
        .await
        .expect("写入失败");
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(5), client.read(&mut buf))
        .await
        .expect("等待回包超时")
        .expect("读取失败");
    assert_eq!(&buf[..n], b"hello-over-ssh", "回包内容不一致");

    // 停止并确认运行任务退出
    let _ = rt.stop_tx.send(true);
    let res = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("运行任务未在停止后退出");
    assert!(res.is_ok(), "运行任务异常: {res:?}");
}

#[tokio::test]
async fn udp_frame_roundtrip() {
    let (mut a, mut b) = tokio::io::duplex(65536);

    runner::write_frame(&mut a, 1, 0x1234_5678, b"payload")
        .await
        .unwrap();
    let (op, fid, data) = runner::read_frame(&mut b).await.unwrap();
    assert_eq!(op, 1);
    assert_eq!(fid, 0x1234_5678);
    assert_eq!(data, b"payload");

    // 空负载 CLOSE 帧
    runner::write_frame(&mut b, 2, 7, &[]).await.unwrap();
    let (op, fid, data) = runner::read_frame(&mut a).await.unwrap();
    assert_eq!((op, fid, data.len()), (2, 7, 0));
}

//! 网络工具（仅 IPv4）：本机网络信息（网卡/网关/DNS/路由表）、
//! TCP 端口检测、ICMP Ping。探测过程通过 `net-log` 事件推送到前端日志框。

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

// ---------------- 数据结构（serde camelCase 对齐前端） ----------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalNetInfo {
    pub hostname: String,
    pub os: String,
    pub interfaces: Vec<InterfaceInfo>,
    pub routes: Vec<RouteInfo>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceInfo {
    pub name: String,
    pub mac: Option<String>,
    pub ips: Vec<IpInfo>,
    pub gateways: Vec<String>,
    pub dns: Vec<String>,
    #[serde(skip)]
    pub if_index: u32,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IpInfo {
    pub addr: String,
    pub prefix_len: u8,
    pub netmask: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RouteInfo {
    pub dest: String,
    pub gateway: String,
    pub interface: String,
    pub metric: u32,
    #[serde(skip)]
    pub prefix_len: u8,
    #[serde(skip)]
    pub dest_u32: u32,
    #[serde(skip)]
    pub mask_u32: u32,
    #[serde(skip)]
    pub if_index: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub success: bool,
    pub host: String,
    pub resolved_ip: Option<String>,
    pub port: u16,
    pub elapsed_ms: u128,
    pub state: String,
    pub reason: Option<String>,
    pub route: Option<RouteInfo>,
    pub source_ip: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingResult {
    pub success: bool,
    pub host: String,
    pub resolved_ip: Option<String>,
    pub sent: u32,
    pub received: u32,
    pub avg_ms: Option<f64>,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub loss_percent: f64,
    pub reason: Option<String>,
}

// ---------------- 事件 ----------------

const EVT_NET_LOG: &str = "net-log";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NetLogPayload {
    level: String,
    message: String,
    time: i64,
}

fn log_info<R: Runtime>(app: &AppHandle<R>, message: impl Into<String>) {
    let _ = app.emit(
        EVT_NET_LOG,
        NetLogPayload {
            level: "info".into(),
            message: message.into(),
            time: now_secs(),
        },
    );
}

fn log_error<R: Runtime>(app: &AppHandle<R>, message: impl Into<String>) {
    let _ = app.emit(
        EVT_NET_LOG,
        NetLogPayload {
            level: "error".into(),
            message: message.into(),
            time: now_secs(),
        },
    );
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------- 本机信息 ----------------

struct Adapters {
    interfaces: Vec<InterfaceInfo>,
    names: HashMap<u32, String>,
}

fn hostname() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::WindowsProgramming::GetComputerNameW;
        let mut buf = [0u16; 260];
        let mut size = buf.len() as u32;
        if unsafe { GetComputerNameW(buf.as_mut_ptr(), &mut size) } != 0 {
            return String::from_utf16_lossy(&buf[..size as usize]);
        }
        "未知".into()
    }
    #[cfg(not(windows))]
    {
        std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "未知".into())
    }
}

/// 枚举网卡：名称、MAC、IPv4 地址（含前缀/掩码）、网关、DNS。
/// 同时返回 ifindex -> 名称 映射，供路由表显示接口名。
#[cfg(windows)]
fn collect_adapters() -> Result<Adapters, String> {
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST,
        IP_ADAPTER_ADDRESSES_LH,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    const FLAGS: u32 = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut size: u32 = 16 * 1024;
    let mut buf = vec![0u8; size as usize];
    let mut ret = unsafe {
        GetAdaptersAddresses(
            AF_INET as u32,
            FLAGS,
            std::ptr::null(),
            buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
            &mut size,
        )
    };
    if ret == ERROR_BUFFER_OVERFLOW {
        buf.resize(size as usize, 0);
        ret = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                FLAGS,
                std::ptr::null(),
                buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH,
                &mut size,
            )
        };
    }
    if ret != NO_ERROR {
        return Err(format!(
            "获取网卡信息失败：GetAdaptersAddresses 错误码 {ret}"
        ));
    }

    let mut interfaces = Vec::new();
    let mut names = HashMap::new();
    let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
    while !cur.is_null() {
        let a = unsafe { &*cur };
        let if_index = unsafe { a.Anonymous1.Anonymous.IfIndex };
        let name = unsafe { pwstr(a.FriendlyName) };
        let mac = if a.PhysicalAddressLength > 0 {
            Some(
                a.PhysicalAddress[..a.PhysicalAddressLength as usize]
                    .iter()
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join("-"),
            )
        } else {
            None
        };

        let mut ips: Vec<IpInfo> = Vec::new();
        let mut ua = a.FirstUnicastAddress;
        while !ua.is_null() {
            let u = unsafe { &*ua };
            if let Some(ip) = unsafe { sockaddr_ipv4(u.Address.lpSockaddr) } {
                let prefix = u.OnLinkPrefixLength;
                ips.push(IpInfo {
                    addr: ip.to_string(),
                    prefix_len: prefix,
                    netmask: netmask_str(prefix),
                });
            }
            ua = u.Next;
        }

        let mut gateways: Vec<String> = Vec::new();
        let mut ga = a.FirstGatewayAddress;
        while !ga.is_null() {
            let g = unsafe { &*ga };
            if let Some(ip) = unsafe { sockaddr_ipv4(g.Address.lpSockaddr) } {
                push_unique(&mut gateways, ip.to_string());
            }
            ga = g.Next;
        }

        let mut dns: Vec<String> = Vec::new();
        let mut da = a.FirstDnsServerAddress;
        while !da.is_null() {
            let d = unsafe { &*da };
            if let Some(ip) = unsafe { sockaddr_ipv4(d.Address.lpSockaddr) } {
                push_unique(&mut dns, ip.to_string());
            }
            da = d.Next;
        }

        if !ips.is_empty() {
            interfaces.push(InterfaceInfo {
                name: name.clone(),
                mac,
                ips,
                gateways,
                dns,
                if_index,
            });
        }
        names.insert(if_index, name);

        cur = a.Next;
    }
    Ok(Adapters { interfaces, names })
}

#[cfg(not(windows))]
fn collect_adapters() -> Result<Adapters, String> {
    Ok(Adapters {
        interfaces: vec![],
        names: HashMap::new(),
    })
}

#[cfg(windows)]
unsafe fn pwstr(p: *mut u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

#[cfg(windows)]
unsafe fn sockaddr_ipv4(
    sa: *mut windows_sys::Win32::Networking::WinSock::SOCKADDR,
) -> Option<Ipv4Addr> {
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
    if sa.is_null() {
        return None;
    }
    let sin = &*(sa as *const SOCKADDR_IN);
    if sin.sin_family != AF_INET as u16 {
        return None;
    }
    Some(Ipv4Addr::from(u32::from_be(sin.sin_addr.S_un.S_addr)))
}

fn netmask_str(prefix: u8) -> String {
    let mask = if prefix >= 32 {
        u32::MAX
    } else if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    Ipv4Addr::from(mask).to_string()
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

/// 读取 Windows IPv4 路由表。
#[cfg(windows)]
fn collect_routes() -> Result<Vec<RouteInfo>, String> {
    use windows_sys::Win32::Foundation::NO_ERROR;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIpForwardTable2, MIB_IPFORWARD_TABLE2,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    let ret = unsafe { GetIpForwardTable2(AF_INET as u16, &mut table) };
    if ret != NO_ERROR {
        return Err(format!("获取路由表失败：GetIpForwardTable2 错误码 {ret}"));
    }
    let mut out = Vec::new();
    unsafe {
        let num = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), num);
        let adapters = collect_adapters().unwrap_or(Adapters {
            interfaces: vec![],
            names: HashMap::new(),
        });
        for row in rows {
            let dest = u32::from_be(row.DestinationPrefix.Prefix.Ipv4.sin_addr.S_un.S_addr);
            let prefix = row.DestinationPrefix.PrefixLength;
            let gw_raw = u32::from_be(row.NextHop.Ipv4.sin_addr.S_un.S_addr);
            let mask = if prefix >= 32 {
                u32::MAX
            } else if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            let gateway = if gw_raw == 0 {
                "直连".to_string()
            } else {
                Ipv4Addr::from(gw_raw).to_string()
            };
            let name = adapters
                .names
                .get(&row.InterfaceIndex)
                .cloned()
                .unwrap_or_else(|| format!("接口 {}", row.InterfaceIndex));
            out.push(RouteInfo {
                dest: format!("{}/{}", Ipv4Addr::from(dest), prefix),
                gateway,
                interface: name,
                metric: row.Metric,
                prefix_len: prefix,
                dest_u32: dest,
                mask_u32: mask,
                if_index: row.InterfaceIndex,
            });
        }
        FreeMibTable(table as *const _);
    }
    Ok(out)
}

#[cfg(not(windows))]
fn collect_routes() -> Result<Vec<RouteInfo>, String> {
    Ok(vec![])
}

/// 最长前缀匹配：找到目标 IP 命中的路由。
fn best_route(ip: Ipv4Addr, routes: &[RouteInfo]) -> Option<RouteInfo> {
    let ip_u32 = u32::from(ip);
    routes
        .iter()
        .filter(|r| (ip_u32 & r.mask_u32) == (r.dest_u32 & r.mask_u32))
        .max_by_key(|r| r.prefix_len)
        .cloned()
}

/// 命中的路由对应接口上的本机出口 IP。
fn source_ip_for(route: &RouteInfo, adapters: &Adapters) -> Option<String> {
    if let Some(iface) = adapters
        .interfaces
        .iter()
        .find(|i| i.if_index == route.if_index)
    {
        if let Some(ip) = iface
            .ips
            .iter()
            .map(|i| i.addr.clone())
            .find(|a| !a.starts_with("127."))
        {
            return Some(ip);
        }
    }
    adapters
        .interfaces
        .iter()
        .flat_map(|i| i.ips.iter())
        .map(|i| i.addr.clone())
        .find(|a| !a.starts_with("127."))
}

// ---------------- TCP 端口检测 ----------------

fn resolve_ipv4(host: &str) -> Result<Ipv4Addr, String> {
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Ok(ip);
    }
    let mut addrs = (host, 0u16)
        .to_socket_addrs()
        .map_err(|e| format!("DNS 解析失败：{e}"))?
        .filter(|a| a.is_ipv4());
    match addrs.next() {
        Some(SocketAddr::V4(v)) => Ok(*v.ip()),
        _ => Err(format!("DNS 解析失败：{host} 未解析到 IPv4 地址")),
    }
}

fn run_tcp_probe(app: &AppHandle, host: &str, port: u16, timeout_ms: u64) -> ProbeResult {
    let timeout = Duration::from_millis(timeout_ms.max(100));
    log_info(
        app,
        format!(
            "开始检测 TCP {host}:{port}（超时 {} ms）",
            timeout_ms.max(100)
        ),
    );

    let ip = match resolve_ipv4(host) {
        Ok(ip) => {
            log_info(app, format!("解析主机名 {host} → {ip}"));
            ip
        }
        Err(e) => {
            log_error(app, &e);
            return ProbeResult {
                success: false,
                host: host.into(),
                resolved_ip: None,
                port,
                elapsed_ms: 0,
                state: "dnsFailed".into(),
                reason: Some(e),
                route: None,
                source_ip: None,
            };
        }
    };

    let adapters = collect_adapters().unwrap_or(Adapters {
        interfaces: vec![],
        names: HashMap::new(),
    });
    let routes = collect_routes().unwrap_or_default();
    let route = best_route(ip, &routes);
    match &route {
        Some(r) => log_info(
            app,
            format!(
                "路由：{} → 网关 {}，接口 {}，Metric {}",
                r.dest, r.gateway, r.interface, r.metric
            ),
        ),
        None => log_info(app, "路由：未匹配到可达目标的路由"),
    }
    let source_ip = route.as_ref().and_then(|r| source_ip_for(r, &adapters));
    if let Some(s) = &source_ip {
        log_info(app, format!("本机出口 IP：{s}"));
    }

    log_info(app, format!("发送 SYN → {ip}:{port}"));
    let start = Instant::now();
    let result = TcpStream::connect_timeout(&SocketAddr::new(ip.into(), port), timeout);
    let elapsed = start.elapsed().as_millis();

    match result {
        Ok(_) => {
            log_info(
                app,
                format!("收到 SYN-ACK，TCP 握手成功，耗时 {elapsed} ms"),
            );
            log_info(app, "检测结束：连接成功");
            ProbeResult {
                success: true,
                host: host.into(),
                resolved_ip: Some(ip.to_string()),
                port,
                elapsed_ms: elapsed,
                state: "connected".into(),
                reason: None,
                route,
                source_ip,
            }
        }
        Err(e) => {
            let (state, reason, line) = classify_tcp_error(&e);
            log_error(app, line);
            log_error(app, format!("失败原因：{reason}"));
            log_info(app, format!("检测结束：连接失败（{elapsed} ms）"));
            ProbeResult {
                success: false,
                host: host.into(),
                resolved_ip: Some(ip.to_string()),
                port,
                elapsed_ms: elapsed,
                state,
                reason: Some(reason),
                route,
                source_ip,
            }
        }
    }
}

/// 将 TCP 连接错误映射为「包状态 + 人类可读原因 + 日志行」。
fn classify_tcp_error(e: &io::Error) -> (String, String, String) {
    match e.raw_os_error() {
        // WSAECONNREFUSED
        Some(10061) => (
            "reset".into(),
            "连接被拒绝：服务器在线但端口未监听，或防火墙返回 RST".into(),
            "收到 TCP RST（连接被拒绝）".into(),
        ),
        // WSAECONNRESET
        Some(10054) => (
            "reset".into(),
            "连接被重置（收到 RST）".into(),
            "收到 TCP RST（连接被重置）".into(),
        ),
        // WSAETIMEDOUT
        Some(10060) => (
            "timeout".into(),
            format!("连接超时：SYN 无响应（{e}），数据包可能被防火墙丢弃"),
            "超时：未收到任何 TCP 回应（SYN 可能被丢弃）".into(),
        ),
        // WSAENETUNREACH
        Some(10051) => (
            "unreachable".into(),
            "网络不可达：没有到目标网络的路径".into(),
            "ICMP：网络不可达".into(),
        ),
        // WSAEHOSTUNREACH
        Some(10065) => (
            "unreachable".into(),
            "主机不可达：目标主机可能离线或未配置".into(),
            "ICMP：主机不可达".into(),
        ),
        // WSAEADDRNOTAVAIL
        Some(10049) => (
            "badAddr".into(),
            format!("地址不可用：{e}"),
            format!("地址不可用：{e}"),
        ),
        _ => match e.kind() {
            io::ErrorKind::ConnectionRefused => (
                "reset".into(),
                "连接被拒绝：收到 RST".into(),
                "收到 TCP RST（连接被拒绝）".into(),
            ),
            io::ErrorKind::TimedOut => (
                "timeout".into(),
                "连接超时：SYN 无响应".into(),
                "超时：未收到任何 TCP 回应".into(),
            ),
            io::ErrorKind::HostUnreachable => (
                "unreachable".into(),
                "主机不可达".into(),
                "ICMP：主机不可达".into(),
            ),
            io::ErrorKind::NetworkUnreachable => (
                "unreachable".into(),
                "网络不可达".into(),
                "ICMP：网络不可达".into(),
            ),
            _ => (
                "other".into(),
                format!("连接失败：{e}"),
                format!("连接失败：{e}"),
            ),
        },
    }
}

// ---------------- ICMP Ping ----------------

/// 单次 ICMP Echo。返回 (rtt_ms)。失败返回 (状态码, 原因)。
/// 通过 Windows IcmpSendEcho API 实现（与 ping.exe 相同，无需管理员权限）。
#[cfg(windows)]
fn ping_once(ip: Ipv4Addr, timeout_ms: u32) -> Result<u32, (u32, String)> {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, IP_OPTION_INFORMATION,
    };

    let handle: HANDLE = unsafe { IcmpCreateFile() };
    if handle == INVALID_HANDLE_VALUE {
        return Err((0, "无法创建 ICMP 句柄".into()));
    }

    let payload = [0u8; 32];
    let reply_size = 64 + payload.len();
    let mut reply = vec![0u8; reply_size];
    let dest = u32::from(ip).to_be(); // 网络字节序
    let opts: IP_OPTION_INFORMATION = unsafe { std::mem::zeroed() };

    let n = unsafe {
        IcmpSendEcho(
            handle,
            dest,
            payload.as_ptr() as *const _,
            payload.len() as u16,
            &opts,
            reply.as_mut_ptr() as *mut _,
            reply_size as u32,
            timeout_ms,
        )
    };
    unsafe { IcmpCloseHandle(handle) };

    if n == 0 {
        let code = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        return Err((0, format!("IcmpSendEcho 失败：错误码 {code}")));
    }
    // ICMP_ECHO_REPLY 头部布局：offset 0 = Address，4 = Status，8 = RoundTripTime(ms)
    let status = u32::from_ne_bytes([reply[4], reply[5], reply[6], reply[7]]);
    let rtt_ms = u32::from_ne_bytes([reply[8], reply[9], reply[10], reply[11]]);
    if status == 0 {
        Ok(rtt_ms)
    } else {
        Err((status, icmp_status_str(status)))
    }
}

#[cfg(not(windows))]
fn ping_once(_ip: Ipv4Addr, _timeout_ms: u32) -> Result<u32, (u32, String)> {
    Err((0, "当前平台不支持 ICMP Ping".into()))
}

fn icmp_status_str(status: u32) -> String {
    match status {
        0 => "成功".into(),
        11002 => "目标网络不可达".into(),
        11003 => "目标主机不可达".into(),
        11005 => "目标端口不可达".into(),
        11010 => "请求超时（无回应）".into(),
        11050 => "一般性失败".into(),
        11051 => "目标不可达".into(),
        11052 => "TTL 超时".into(),
        11055 => "连接被重置".into(),
        _ => format!("ICMP 状态码 {status}"),
    }
}

fn run_ping(app: &AppHandle, host: &str, count: u32, timeout_ms: u64) -> PingResult {
    let count = count.clamp(1, 10);
    let timeout = timeout_ms.max(100);
    log_info(
        app,
        format!("开始 Ping {host}（{count} 次，超时 {timeout} ms）"),
    );

    let ip = match resolve_ipv4(host) {
        Ok(ip) => {
            log_info(app, format!("解析主机名 {host} → {ip}"));
            ip
        }
        Err(e) => {
            log_error(app, &e);
            return PingResult {
                success: false,
                host: host.into(),
                resolved_ip: None,
                sent: 0,
                received: 0,
                avg_ms: None,
                min_ms: None,
                max_ms: None,
                loss_percent: 100.0,
                reason: Some(e),
            };
        }
    };

    let mut received = 0u32;
    let mut rtts: Vec<f64> = Vec::new();
    for seq in 0..count {
        log_info(
            app,
            format!("发送 ICMP Echo 请求 → {ip}（第 {}/{} 次）", seq + 1, count),
        );
        match ping_once(ip, timeout as u32) {
            Ok(rtt) => {
                received += 1;
                let ms = rtt as f64;
                rtts.push(ms);
                log_info(app, format!("收到 ICMP Echo 回复，RTT {ms:.1} ms"));
            }
            Err((_, msg)) => {
                log_error(app, format!("第 {} 次失败：{msg}", seq + 1));
            }
        }
        if seq + 1 < count {
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    let loss = (count - received) as f64 / count as f64 * 100.0;
    let (avg, min, max) = if rtts.is_empty() {
        (None, None, None)
    } else {
        let sum: f64 = rtts.iter().sum();
        let mn = rtts.iter().cloned().fold(f64::INFINITY, f64::min);
        let mx = rtts.iter().cloned().fold(0.0f64, f64::max);
        (Some(sum / rtts.len() as f64), Some(mn), Some(mx))
    };
    let success = received > 0;
    let summary = match avg {
        Some(a) => format!(
            "平均 RTT {a:.1} ms（最小 {:.1}，最大 {:.1}）",
            min.unwrap_or(0.0),
            max.unwrap_or(0.0)
        ),
        None => "无回应".to_string(),
    };
    log_info(
        app,
        format!("Ping 统计：{received}/{count} 成功，丢包率 {loss:.0}%，{summary}"),
    );
    PingResult {
        success,
        host: host.into(),
        resolved_ip: Some(ip.to_string()),
        sent: count,
        received,
        avg_ms: avg,
        min_ms: min,
        max_ms: max,
        loss_percent: loss,
        reason: if success {
            None
        } else {
            Some("所有请求均失败".into())
        },
    }
}

// ---------------- 对外逻辑函数（Tauri 命令包装在 lib.rs） ----------------

pub fn net_local_info() -> Result<LocalNetInfo, String> {
    let adapters = collect_adapters()?;
    let routes = collect_routes()?;
    Ok(LocalNetInfo {
        hostname: hostname(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        interfaces: adapters.interfaces,
        routes,
    })
}

pub async fn net_tcp_probe(
    app: AppHandle,
    host: String,
    port: u16,
    timeout_ms: u64,
) -> Result<ProbeResult, String> {
    tauri::async_runtime::spawn_blocking(move || run_tcp_probe(&app, &host, port, timeout_ms))
        .await
        .map_err(|e| e.to_string())
}

pub async fn net_ping(
    app: AppHandle,
    host: String,
    count: u32,
    timeout_ms: u64,
) -> Result<PingResult, String> {
    tauri::async_runtime::spawn_blocking(move || run_ping(&app, &host, count, timeout_ms))
        .await
        .map_err(|e| e.to_string())
}

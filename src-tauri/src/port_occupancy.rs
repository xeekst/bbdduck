use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortEndpoint {
    pub protocol: String,
    pub address_family: String,
    pub local_ip: String,
    pub local_port: u16,
    pub remote_ip: Option<String>,
    pub remote_port: Option<u16>,
    pub state: String,
    pub listening: bool,
    pub wildcard: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessTreeNode {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub thread_count: u32,
    pub is_target: bool,
    pub children: Vec<ProcessTreeNode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortOccupyingProcess {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub path: Option<String>,
    pub command_line: Option<String>,
    pub app_type: String,
    pub session_id: u32,
    pub started_at: Option<u64>,
    pub thread_count: u32,
    pub endpoints: Vec<PortEndpoint>,
    pub parent_chain: Vec<ProcessTreeNode>,
    pub process_tree: ProcessTreeNode,
    pub tree_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortOccupancyScanResult {
    pub port: u16,
    pub occupied: bool,
    pub listener_count: usize,
    pub endpoint_count: usize,
    pub elapsed_ms: u128,
    pub processes: Vec<PortOccupyingProcess>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpStateCount {
    pub state: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpConnectionDetail {
    pub address_family: String,
    pub local_ip: String,
    pub local_port: u16,
    pub remote_ip: Option<String>,
    pub remote_port: Option<u16>,
    pub state: String,
    pub pid: u32,
    pub process_name: String,
    pub process_path: Option<String>,
    pub process_started_at: Option<u64>,
    pub bytes_sent: Option<u64>,
    pub bytes_received: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TcpConnectionStatistics {
    pub port: u16,
    pub source_ip: Option<String>,
    pub local_ip: Option<String>,
    pub total_connections: usize,
    pub listener_count: usize,
    pub process_count: usize,
    pub total_bytes_sent: u64,
    pub total_bytes_received: u64,
    pub traffic_available_connections: usize,
    pub traffic_unavailable_connections: usize,
    pub traffic_newly_enabled_connections: usize,
    pub traffic_permission_denied: bool,
    pub state_counts: Vec<TcpStateCount>,
    pub connections: Vec<TcpConnectionDetail>,
    pub details_truncated: bool,
    pub captured_at: u64,
    pub elapsed_ms: u128,
}

#[cfg(windows)]
mod platform {
    use super::{
        PortEndpoint, PortOccupancyScanResult, PortOccupyingProcess, ProcessTreeNode,
        TcpConnectionDetail, TcpConnectionStatistics, TcpStateCount,
    };
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::ffi::c_void;
    use std::mem::{align_of, size_of};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::path::Path;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};
    use windows_sys::Wdk::System::Threading::{
        NtQueryInformationProcess, ProcessCommandLineInformation,
    };
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, FILETIME, HANDLE,
        INVALID_HANDLE_VALUE, NO_ERROR,
    };
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetExtendedUdpTable, GetPerTcp6ConnectionEStats,
        GetPerTcpConnectionEStats, SetPerTcp6ConnectionEStats, SetPerTcpConnectionEStats,
        TCP_ESTATS_DATA_ROD_v0, TCP_ESTATS_DATA_RW_v0, TcpConnectionEstatsData, MIB_TCP6ROW,
        MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_LH, MIB_TCPROW_LH_0, MIB_TCPROW_OWNER_PID,
        MIB_TCP_STATE_CLOSED, MIB_TCP_STATE_CLOSE_WAIT, MIB_TCP_STATE_CLOSING,
        MIB_TCP_STATE_DELETE_TCB, MIB_TCP_STATE_ESTAB, MIB_TCP_STATE_FIN_WAIT1,
        MIB_TCP_STATE_FIN_WAIT2, MIB_TCP_STATE_LAST_ACK, MIB_TCP_STATE_LISTEN,
        MIB_TCP_STATE_SYN_RCVD, MIB_TCP_STATE_SYN_SENT, MIB_TCP_STATE_TIME_WAIT,
        MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, IN6_ADDR, IN6_ADDR_0};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, IsProcessCritical, OpenProcess, QueryFullProcessImageNameW,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const WINDOWS_TO_UNIX_SECONDS: u64 = 11_644_473_600;
    const MAX_TREE_DEPTH: usize = 12;
    const MAX_TREE_NODES: usize = 300;
    const MAX_CONNECTION_DETAILS: usize = 5_000;
    const TCP_STATES: [&str; 12] = [
        "CLOSED",
        "LISTENING",
        "SYN_SENT",
        "SYN_RECEIVED",
        "ESTABLISHED",
        "FIN_WAIT_1",
        "FIN_WAIT_2",
        "CLOSE_WAIT",
        "CLOSING",
        "LAST_ACK",
        "TIME_WAIT",
        "DELETE_TCB",
    ];

    #[derive(Debug, Clone)]
    struct SnapshotProcess {
        pid: u32,
        parent_pid: u32,
        name: String,
        thread_count: u32,
    }

    struct RawEndpoint {
        pid: u32,
        endpoint: PortEndpoint,
        traffic: Option<TrafficRead>,
    }

    enum TrafficRead {
        Available {
            sent: u64,
            received: u64,
            newly_enabled: bool,
        },
        AccessDenied,
        Unavailable,
    }

    #[derive(Clone)]
    struct ConnectionProcessMetadata {
        name: String,
        path: Option<String>,
        started_at: Option<u64>,
    }

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *const u16,
    }

    pub fn scan(port: u16) -> Result<PortOccupancyScanResult, String> {
        if port == 0 {
            return Err("端口必须在 1-65535 之间".into());
        }

        let started = Instant::now();
        let mut raw_endpoints = Vec::new();
        collect_tcp_v4(port, &mut raw_endpoints, false)?;
        collect_tcp_v6(port, &mut raw_endpoints, false)?;
        collect_udp_v4(port, &mut raw_endpoints)?;
        collect_udp_v6(port, &mut raw_endpoints)?;

        raw_endpoints.sort_by(|a, b| {
            a.pid
                .cmp(&b.pid)
                .then(b.endpoint.listening.cmp(&a.endpoint.listening))
                .then(a.endpoint.protocol.cmp(&b.endpoint.protocol))
                .then(a.endpoint.address_family.cmp(&b.endpoint.address_family))
                .then(a.endpoint.local_ip.cmp(&b.endpoint.local_ip))
        });

        let endpoint_count = raw_endpoints.len();
        let listener_count = raw_endpoints
            .iter()
            .filter(|item| item.endpoint.listening)
            .count();
        let snapshot = process_snapshot().unwrap_or_default();
        let mut children_by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
        for process in snapshot.values() {
            children_by_parent
                .entry(process.parent_pid)
                .or_default()
                .push(process.pid);
        }
        for children in children_by_parent.values_mut() {
            children.sort_by(|a, b| {
                let a_name = snapshot.get(a).map(|p| p.name.as_str()).unwrap_or("");
                let b_name = snapshot.get(b).map(|p| p.name.as_str()).unwrap_or("");
                a_name
                    .to_ascii_lowercase()
                    .cmp(&b_name.to_ascii_lowercase())
                    .then(a.cmp(b))
            });
        }

        let mut endpoints_by_pid: BTreeMap<u32, Vec<PortEndpoint>> = BTreeMap::new();
        for item in raw_endpoints {
            endpoints_by_pid
                .entry(item.pid)
                .or_default()
                .push(item.endpoint);
        }

        let mut processes = Vec::with_capacity(endpoints_by_pid.len());
        for (pid, endpoints) in endpoints_by_pid {
            let snapshot_process = snapshot.get(&pid).cloned().unwrap_or(SnapshotProcess {
                pid,
                parent_pid: 0,
                name: if pid == 0 {
                    "System Idle Process".into()
                } else {
                    format!("进程 {pid}")
                },
                thread_count: 0,
            });
            let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            let (path, command_line, started_at, session_id, critical) = if handle.is_null() {
                (None, None, None, process_session_id(pid), pid == 4)
            } else {
                let start = process_start_time(handle).and_then(filetime_to_unix_seconds);
                let values = (
                    process_image_path(handle),
                    process_command_line(handle),
                    start,
                    process_session_id(pid),
                    process_is_critical(handle),
                );
                unsafe { CloseHandle(handle) };
                values
            };
            let name = path
                .as_deref()
                .and_then(|value| Path::new(value).file_name())
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| snapshot_process.name.clone());

            let (process_tree, tree_truncated) =
                build_process_tree(pid, &snapshot, &children_by_parent, &snapshot_process);
            processes.push(PortOccupyingProcess {
                pid,
                parent_pid: snapshot_process.parent_pid,
                name,
                path,
                command_line,
                app_type: if critical { "critical" } else { "regular" }.into(),
                session_id,
                started_at,
                thread_count: snapshot_process.thread_count,
                endpoints,
                parent_chain: build_parent_chain(pid, &snapshot),
                process_tree,
                tree_truncated,
            });
        }

        processes.sort_by(|a, b| {
            let a_listens = a.endpoints.iter().any(|endpoint| endpoint.listening);
            let b_listens = b.endpoints.iter().any(|endpoint| endpoint.listening);
            b_listens
                .cmp(&a_listens)
                .then(
                    a.name
                        .to_ascii_lowercase()
                        .cmp(&b.name.to_ascii_lowercase()),
                )
                .then(a.pid.cmp(&b.pid))
        });

        Ok(PortOccupancyScanResult {
            port,
            occupied: endpoint_count > 0,
            listener_count,
            endpoint_count,
            elapsed_ms: started.elapsed().as_millis(),
            processes,
        })
    }

    pub fn tcp_statistics(
        port: u16,
        source_ip: Option<String>,
        local_ip: Option<String>,
    ) -> Result<TcpConnectionStatistics, String> {
        if port == 0 {
            return Err("端口必须在 1-65535 之间".into());
        }
        let started = Instant::now();
        let source_filter = parse_ip_filter("来源 IP", source_ip)?;
        let local_filter = parse_ip_filter("本地 IP", local_ip)?;

        let mut endpoints = Vec::new();
        collect_tcp_v4(port, &mut endpoints, true)?;
        collect_tcp_v6(port, &mut endpoints, true)?;
        endpoints.retain(|item| {
            matches_source_ip(&item.endpoint, source_filter)
                && matches_local_ip(&item.endpoint, local_filter)
        });
        endpoints.sort_by(|a, b| {
            state_order(&a.endpoint.state)
                .cmp(&state_order(&b.endpoint.state))
                .then(a.endpoint.address_family.cmp(&b.endpoint.address_family))
                .then(a.endpoint.local_ip.cmp(&b.endpoint.local_ip))
                .then(a.endpoint.remote_ip.cmp(&b.endpoint.remote_ip))
                .then(a.endpoint.remote_port.cmp(&b.endpoint.remote_port))
                .then(a.pid.cmp(&b.pid))
        });

        let total_connections = endpoints.len();
        let listener_count = endpoints
            .iter()
            .filter(|item| item.endpoint.listening)
            .count();
        let process_count = endpoints
            .iter()
            .map(|item| item.pid)
            .collect::<HashSet<_>>()
            .len();
        let mut counts: BTreeMap<&'static str, usize> =
            TCP_STATES.iter().map(|state| (*state, 0)).collect();
        for item in &endpoints {
            if let Some(value) = counts.get_mut(item.endpoint.state.as_str()) {
                *value += 1;
            }
        }
        let state_counts = TCP_STATES
            .iter()
            .map(|state| TcpStateCount {
                state: (*state).into(),
                count: counts.get(state).copied().unwrap_or(0),
            })
            .collect();

        let mut total_bytes_sent = 0u64;
        let mut total_bytes_received = 0u64;
        let mut traffic_available_connections = 0usize;
        let mut traffic_unavailable_connections = 0usize;
        let mut traffic_newly_enabled_connections = 0usize;
        let mut traffic_permission_denied = false;
        for item in &endpoints {
            match item.traffic.as_ref() {
                Some(TrafficRead::Available {
                    sent,
                    received,
                    newly_enabled,
                }) => {
                    total_bytes_sent = total_bytes_sent.saturating_add(*sent);
                    total_bytes_received = total_bytes_received.saturating_add(*received);
                    traffic_available_connections += 1;
                    if *newly_enabled {
                        traffic_newly_enabled_connections += 1;
                    }
                }
                Some(TrafficRead::AccessDenied) => {
                    traffic_unavailable_connections += 1;
                    traffic_permission_denied = true;
                }
                Some(TrafficRead::Unavailable) => traffic_unavailable_connections += 1,
                None => {}
            }
        }

        let details_truncated = total_connections > MAX_CONNECTION_DETAILS;
        endpoints.truncate(MAX_CONNECTION_DETAILS);
        let snapshot = process_snapshot().unwrap_or_default();
        let mut metadata = HashMap::<u32, ConnectionProcessMetadata>::new();
        let mut connections = Vec::with_capacity(endpoints.len());
        for item in endpoints {
            let process = metadata
                .entry(item.pid)
                .or_insert_with(|| connection_process_metadata(item.pid, &snapshot));
            let (bytes_sent, bytes_received) = match item.traffic {
                Some(TrafficRead::Available { sent, received, .. }) => (Some(sent), Some(received)),
                _ => (None, None),
            };
            connections.push(TcpConnectionDetail {
                address_family: item.endpoint.address_family,
                local_ip: item.endpoint.local_ip,
                local_port: item.endpoint.local_port,
                remote_ip: item.endpoint.remote_ip,
                remote_port: item.endpoint.remote_port,
                state: item.endpoint.state,
                pid: item.pid,
                process_name: process.name.clone(),
                process_path: process.path.clone(),
                process_started_at: process.started_at,
                bytes_sent,
                bytes_received,
            });
        }

        Ok(TcpConnectionStatistics {
            port,
            source_ip: source_filter.map(|ip| ip.to_string()),
            local_ip: local_filter.map(|ip| ip.to_string()),
            total_connections,
            listener_count,
            process_count,
            total_bytes_sent,
            total_bytes_received,
            traffic_available_connections,
            traffic_unavailable_connections,
            traffic_newly_enabled_connections,
            traffic_permission_denied,
            state_counts,
            connections,
            details_truncated,
            captured_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            elapsed_ms: started.elapsed().as_millis(),
        })
    }

    fn parse_ip_filter(label: &str, value: Option<String>) -> Result<Option<IpAddr>, String> {
        let Some(value) = value else {
            return Ok(None);
        };
        let value = value.trim();
        if value.is_empty() {
            return Ok(None);
        }
        let value = value
            .strip_prefix('[')
            .and_then(|text| text.strip_suffix(']'))
            .unwrap_or(value);
        value
            .parse::<IpAddr>()
            .map(Some)
            .map_err(|_| format!("{label}格式不正确：{value}"))
    }

    fn endpoint_ip(value: &str) -> Option<IpAddr> {
        value
            .split_once('%')
            .map(|(address, _)| address)
            .unwrap_or(value)
            .parse()
            .ok()
    }

    fn matches_source_ip(endpoint: &PortEndpoint, filter: Option<IpAddr>) -> bool {
        match filter {
            None => true,
            Some(filter) => endpoint
                .remote_ip
                .as_deref()
                .and_then(endpoint_ip)
                .is_some_and(|address| address == filter),
        }
    }

    fn matches_local_ip(endpoint: &PortEndpoint, filter: Option<IpAddr>) -> bool {
        match filter {
            None => true,
            Some(filter) if endpoint.wildcard => {
                (filter.is_ipv4() && endpoint.address_family == "IPv4")
                    || (filter.is_ipv6() && endpoint.address_family == "IPv6")
            }
            Some(filter) => {
                endpoint_ip(&endpoint.local_ip).is_some_and(|address| address == filter)
            }
        }
    }

    fn state_order(state: &str) -> usize {
        TCP_STATES
            .iter()
            .position(|candidate| *candidate == state)
            .unwrap_or(TCP_STATES.len())
    }

    fn connection_process_metadata(
        pid: u32,
        snapshot: &BTreeMap<u32, SnapshotProcess>,
    ) -> ConnectionProcessMetadata {
        let fallback_name = snapshot
            .get(&pid)
            .map(|process| process.name.clone())
            .unwrap_or_else(|| {
                if pid == 0 {
                    "System Idle Process".into()
                } else {
                    format!("进程 {pid}")
                }
            });
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return ConnectionProcessMetadata {
                name: fallback_name,
                path: None,
                started_at: None,
            };
        }
        let path = process_image_path(handle);
        let started_at = process_start_time(handle).and_then(filetime_to_unix_seconds);
        unsafe { CloseHandle(handle) };
        let name = path
            .as_deref()
            .and_then(|value| Path::new(value).file_name())
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or(fallback_name);
        ConnectionProcessMetadata {
            name,
            path,
            started_at,
        }
    }

    fn collect_tcp_v4(
        port: u16,
        output: &mut Vec<RawEndpoint>,
        collect_traffic: bool,
    ) -> Result<(), String> {
        let storage = query_tcp_table(AF_INET as u32)?;
        for row in table_rows::<MIB_TCPROW_OWNER_PID>(&storage) {
            if decode_port(row.dwLocalPort) != port {
                continue;
            }
            let state = tcp_state(row.dwState);
            let listening = row.dwState as i32 == MIB_TCP_STATE_LISTEN;
            let local_ip = Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()).to_string();
            let remote_port = decode_port(row.dwRemotePort);
            let remote_ip = Ipv4Addr::from(row.dwRemoteAddr.to_ne_bytes());
            output.push(RawEndpoint {
                pid: row.dwOwningPid,
                traffic: (collect_traffic && !listening).then(|| tcp_v4_traffic(&row)),
                endpoint: PortEndpoint {
                    protocol: "TCP".into(),
                    address_family: "IPv4".into(),
                    wildcard: local_ip == "0.0.0.0",
                    local_ip,
                    local_port: port,
                    remote_ip: (!listening && remote_port > 0).then(|| remote_ip.to_string()),
                    remote_port: (!listening && remote_port > 0).then_some(remote_port),
                    state: state.into(),
                    listening,
                },
            });
        }
        Ok(())
    }

    fn collect_tcp_v6(
        port: u16,
        output: &mut Vec<RawEndpoint>,
        collect_traffic: bool,
    ) -> Result<(), String> {
        let storage = query_tcp_table(AF_INET6 as u32)?;
        for row in table_rows::<MIB_TCP6ROW_OWNER_PID>(&storage) {
            if decode_port(row.dwLocalPort) != port {
                continue;
            }
            let state = tcp_state(row.dwState);
            let listening = row.dwState as i32 == MIB_TCP_STATE_LISTEN;
            let local_addr = Ipv6Addr::from(row.ucLocalAddr);
            let local_ip = format_ipv6(local_addr, row.dwLocalScopeId);
            let remote_port = decode_port(row.dwRemotePort);
            let remote_addr = Ipv6Addr::from(row.ucRemoteAddr);
            output.push(RawEndpoint {
                pid: row.dwOwningPid,
                traffic: (collect_traffic && !listening).then(|| tcp_v6_traffic(&row)),
                endpoint: PortEndpoint {
                    protocol: "TCP".into(),
                    address_family: "IPv6".into(),
                    wildcard: local_addr.is_unspecified(),
                    local_ip,
                    local_port: port,
                    remote_ip: (!listening && remote_port > 0)
                        .then(|| format_ipv6(remote_addr, row.dwRemoteScopeId)),
                    remote_port: (!listening && remote_port > 0).then_some(remote_port),
                    state: state.into(),
                    listening,
                },
            });
        }
        Ok(())
    }

    fn tcp_v4_traffic(owner_row: &MIB_TCPROW_OWNER_PID) -> TrafficRead {
        let row = MIB_TCPROW_LH {
            Anonymous: MIB_TCPROW_LH_0 {
                dwState: owner_row.dwState,
            },
            dwLocalAddr: owner_row.dwLocalAddr,
            dwLocalPort: owner_row.dwLocalPort,
            dwRemoteAddr: owner_row.dwRemoteAddr,
            dwRemotePort: owner_row.dwRemotePort,
        };
        match get_tcp_v4_traffic(&row) {
            Ok(Some((sent, received))) => {
                return TrafficRead::Available {
                    sent,
                    received,
                    newly_enabled: false,
                }
            }
            Err(ERROR_ACCESS_DENIED) => return TrafficRead::AccessDenied,
            Ok(None) | Err(_) => {}
        }

        let enable = TCP_ESTATS_DATA_RW_v0 {
            EnableCollection: 1,
        };
        let status = unsafe {
            SetPerTcpConnectionEStats(
                &row,
                TcpConnectionEstatsData,
                (&enable as *const TCP_ESTATS_DATA_RW_v0).cast::<u8>(),
                0,
                size_of::<TCP_ESTATS_DATA_RW_v0>() as u32,
                0,
            )
        };
        if status == ERROR_ACCESS_DENIED {
            return TrafficRead::AccessDenied;
        }
        if status != NO_ERROR {
            return TrafficRead::Unavailable;
        }
        match get_tcp_v4_traffic(&row) {
            Ok(Some((sent, received))) => TrafficRead::Available {
                sent,
                received,
                newly_enabled: true,
            },
            Err(ERROR_ACCESS_DENIED) => TrafficRead::AccessDenied,
            Ok(None) | Err(_) => TrafficRead::Unavailable,
        }
    }

    fn get_tcp_v4_traffic(row: &MIB_TCPROW_LH) -> Result<Option<(u64, u64)>, u32> {
        let mut config: TCP_ESTATS_DATA_RW_v0 = unsafe { std::mem::zeroed() };
        let mut data: TCP_ESTATS_DATA_ROD_v0 = unsafe { std::mem::zeroed() };
        let status = unsafe {
            GetPerTcpConnectionEStats(
                row,
                TcpConnectionEstatsData,
                (&mut config as *mut TCP_ESTATS_DATA_RW_v0).cast::<u8>(),
                0,
                size_of::<TCP_ESTATS_DATA_RW_v0>() as u32,
                std::ptr::null_mut(),
                0,
                0,
                (&mut data as *mut TCP_ESTATS_DATA_ROD_v0).cast::<u8>(),
                0,
                size_of::<TCP_ESTATS_DATA_ROD_v0>() as u32,
            )
        };
        if status != NO_ERROR {
            return Err(status);
        }
        if config.EnableCollection == 0 {
            return Ok(None);
        }
        Ok(Some((data.DataBytesOut, data.DataBytesIn)))
    }

    fn tcp_v6_traffic(owner_row: &MIB_TCP6ROW_OWNER_PID) -> TrafficRead {
        let row = MIB_TCP6ROW {
            State: owner_row.dwState as i32,
            LocalAddr: IN6_ADDR {
                u: IN6_ADDR_0 {
                    Byte: owner_row.ucLocalAddr,
                },
            },
            dwLocalScopeId: owner_row.dwLocalScopeId,
            dwLocalPort: owner_row.dwLocalPort,
            RemoteAddr: IN6_ADDR {
                u: IN6_ADDR_0 {
                    Byte: owner_row.ucRemoteAddr,
                },
            },
            dwRemoteScopeId: owner_row.dwRemoteScopeId,
            dwRemotePort: owner_row.dwRemotePort,
        };
        match get_tcp_v6_traffic(&row) {
            Ok(Some((sent, received))) => {
                return TrafficRead::Available {
                    sent,
                    received,
                    newly_enabled: false,
                }
            }
            Err(ERROR_ACCESS_DENIED) => return TrafficRead::AccessDenied,
            Ok(None) | Err(_) => {}
        }

        let enable = TCP_ESTATS_DATA_RW_v0 {
            EnableCollection: 1,
        };
        let status = unsafe {
            SetPerTcp6ConnectionEStats(
                &row,
                TcpConnectionEstatsData,
                (&enable as *const TCP_ESTATS_DATA_RW_v0).cast::<u8>(),
                0,
                size_of::<TCP_ESTATS_DATA_RW_v0>() as u32,
                0,
            )
        };
        if status == ERROR_ACCESS_DENIED {
            return TrafficRead::AccessDenied;
        }
        if status != NO_ERROR {
            return TrafficRead::Unavailable;
        }
        match get_tcp_v6_traffic(&row) {
            Ok(Some((sent, received))) => TrafficRead::Available {
                sent,
                received,
                newly_enabled: true,
            },
            Err(ERROR_ACCESS_DENIED) => TrafficRead::AccessDenied,
            Ok(None) | Err(_) => TrafficRead::Unavailable,
        }
    }

    fn get_tcp_v6_traffic(row: &MIB_TCP6ROW) -> Result<Option<(u64, u64)>, u32> {
        let mut config: TCP_ESTATS_DATA_RW_v0 = unsafe { std::mem::zeroed() };
        let mut data: TCP_ESTATS_DATA_ROD_v0 = unsafe { std::mem::zeroed() };
        let status = unsafe {
            GetPerTcp6ConnectionEStats(
                row,
                TcpConnectionEstatsData,
                (&mut config as *mut TCP_ESTATS_DATA_RW_v0).cast::<u8>(),
                0,
                size_of::<TCP_ESTATS_DATA_RW_v0>() as u32,
                std::ptr::null_mut(),
                0,
                0,
                (&mut data as *mut TCP_ESTATS_DATA_ROD_v0).cast::<u8>(),
                0,
                size_of::<TCP_ESTATS_DATA_ROD_v0>() as u32,
            )
        };
        if status != NO_ERROR {
            return Err(status);
        }
        if config.EnableCollection == 0 {
            return Ok(None);
        }
        Ok(Some((data.DataBytesOut, data.DataBytesIn)))
    }

    fn collect_udp_v4(port: u16, output: &mut Vec<RawEndpoint>) -> Result<(), String> {
        let storage = query_udp_table(AF_INET as u32)?;
        for row in table_rows::<MIB_UDPROW_OWNER_PID>(&storage) {
            if decode_port(row.dwLocalPort) != port {
                continue;
            }
            let local_ip = Ipv4Addr::from(row.dwLocalAddr.to_ne_bytes()).to_string();
            output.push(RawEndpoint {
                pid: row.dwOwningPid,
                traffic: None,
                endpoint: PortEndpoint {
                    protocol: "UDP".into(),
                    address_family: "IPv4".into(),
                    wildcard: local_ip == "0.0.0.0",
                    local_ip,
                    local_port: port,
                    remote_ip: None,
                    remote_port: None,
                    state: "BOUND".into(),
                    listening: true,
                },
            });
        }
        Ok(())
    }

    fn collect_udp_v6(port: u16, output: &mut Vec<RawEndpoint>) -> Result<(), String> {
        let storage = query_udp_table(AF_INET6 as u32)?;
        for row in table_rows::<MIB_UDP6ROW_OWNER_PID>(&storage) {
            if decode_port(row.dwLocalPort) != port {
                continue;
            }
            let local_addr = Ipv6Addr::from(row.ucLocalAddr);
            output.push(RawEndpoint {
                pid: row.dwOwningPid,
                traffic: None,
                endpoint: PortEndpoint {
                    protocol: "UDP".into(),
                    address_family: "IPv6".into(),
                    local_ip: format_ipv6(local_addr, row.dwLocalScopeId),
                    local_port: port,
                    remote_ip: None,
                    remote_port: None,
                    state: "BOUND".into(),
                    listening: true,
                    wildcard: local_addr.is_unspecified(),
                },
            });
        }
        Ok(())
    }

    fn query_tcp_table(address_family: u32) -> Result<Vec<usize>, String> {
        let mut byte_len = 0u32;
        let initial = unsafe {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                &mut byte_len,
                1,
                address_family,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if initial != ERROR_INSUFFICIENT_BUFFER && initial != NO_ERROR {
            return Err(format!("读取 TCP 端点表失败（Windows 错误 {initial}）"));
        }
        query_table_buffer(
            byte_len,
            |buffer, size| unsafe {
                GetExtendedTcpTable(buffer, size, 1, address_family, TCP_TABLE_OWNER_PID_ALL, 0)
            },
            "TCP",
        )
    }

    fn query_udp_table(address_family: u32) -> Result<Vec<usize>, String> {
        let mut byte_len = 0u32;
        let initial = unsafe {
            GetExtendedUdpTable(
                std::ptr::null_mut(),
                &mut byte_len,
                1,
                address_family,
                UDP_TABLE_OWNER_PID,
                0,
            )
        };
        if initial != ERROR_INSUFFICIENT_BUFFER && initial != NO_ERROR {
            return Err(format!("读取 UDP 端点表失败（Windows 错误 {initial}）"));
        }
        query_table_buffer(
            byte_len,
            |buffer, size| unsafe {
                GetExtendedUdpTable(buffer, size, 1, address_family, UDP_TABLE_OWNER_PID, 0)
            },
            "UDP",
        )
    }

    fn query_table_buffer(
        mut byte_len: u32,
        query: impl Fn(*mut c_void, &mut u32) -> u32,
        protocol: &str,
    ) -> Result<Vec<usize>, String> {
        byte_len = byte_len.max(64);
        for _ in 0..4 {
            let word_count = (byte_len as usize).div_ceil(size_of::<usize>());
            let mut storage = vec![0usize; word_count];
            let mut actual_len = (storage.len() * size_of::<usize>()) as u32;
            let status = query(storage.as_mut_ptr().cast::<c_void>(), &mut actual_len);
            if status == NO_ERROR {
                return Ok(storage);
            }
            if status != ERROR_INSUFFICIENT_BUFFER {
                return Err(format!(
                    "读取 {protocol} 端点表失败（Windows 错误 {status}）"
                ));
            }
            byte_len = actual_len.max(byte_len.saturating_mul(2));
        }
        Err(format!("{protocol} 端点表持续变化，请重试"))
    }

    fn table_rows<T: Copy>(storage: &[usize]) -> Vec<T> {
        let total_bytes = storage.len() * size_of::<usize>();
        if total_bytes < size_of::<u32>() {
            return Vec::new();
        }
        let base = storage.as_ptr().cast::<u8>();
        let reported = unsafe { std::ptr::read_unaligned(base.cast::<u32>()) } as usize;
        let offset = align_up(size_of::<u32>(), align_of::<T>());
        if total_bytes < offset || size_of::<T>() == 0 {
            return Vec::new();
        }
        let available = (total_bytes - offset) / size_of::<T>();
        let count = reported.min(available);
        unsafe { std::slice::from_raw_parts(base.add(offset).cast::<T>(), count) }.to_vec()
    }

    fn align_up(value: usize, alignment: usize) -> usize {
        (value + alignment - 1) & !(alignment - 1)
    }

    fn decode_port(raw: u32) -> u16 {
        u16::from_be(raw as u16)
    }

    fn format_ipv6(address: Ipv6Addr, scope_id: u32) -> String {
        if scope_id > 0 && !address.is_unspecified() {
            format!("{address}%{scope_id}")
        } else {
            address.to_string()
        }
    }

    fn tcp_state(state: u32) -> &'static str {
        match state as i32 {
            MIB_TCP_STATE_CLOSED => "CLOSED",
            MIB_TCP_STATE_LISTEN => "LISTENING",
            MIB_TCP_STATE_SYN_SENT => "SYN_SENT",
            MIB_TCP_STATE_SYN_RCVD => "SYN_RECEIVED",
            MIB_TCP_STATE_ESTAB => "ESTABLISHED",
            MIB_TCP_STATE_FIN_WAIT1 => "FIN_WAIT_1",
            MIB_TCP_STATE_FIN_WAIT2 => "FIN_WAIT_2",
            MIB_TCP_STATE_CLOSE_WAIT => "CLOSE_WAIT",
            MIB_TCP_STATE_CLOSING => "CLOSING",
            MIB_TCP_STATE_LAST_ACK => "LAST_ACK",
            MIB_TCP_STATE_TIME_WAIT => "TIME_WAIT",
            MIB_TCP_STATE_DELETE_TCB => "DELETE_TCB",
            _ => "UNKNOWN",
        }
    }

    fn process_snapshot() -> Result<BTreeMap<u32, SnapshotProcess>, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(format!(
                "无法读取进程快照：{}",
                std::io::Error::last_os_error()
            ));
        }

        let mut processes = BTreeMap::new();
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
        while ok != 0 {
            let name_len = entry
                .szExeFile
                .iter()
                .position(|character| *character == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..name_len]);
            processes.insert(
                entry.th32ProcessID,
                SnapshotProcess {
                    pid: entry.th32ProcessID,
                    parent_pid: entry.th32ParentProcessID,
                    name,
                    thread_count: entry.cntThreads,
                },
            );
            ok = unsafe { Process32NextW(snapshot, &mut entry) };
        }
        unsafe { CloseHandle(snapshot) };
        Ok(processes)
    }

    fn build_parent_chain(
        target_pid: u32,
        snapshot: &BTreeMap<u32, SnapshotProcess>,
    ) -> Vec<ProcessTreeNode> {
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut next = snapshot.get(&target_pid).map(|process| process.parent_pid);
        while let Some(pid) = next {
            if pid == 0 || pid == target_pid || !seen.insert(pid) || chain.len() >= 16 {
                break;
            }
            let Some(process) = snapshot.get(&pid) else {
                break;
            };
            chain.push(tree_node(process, target_pid, Vec::new()));
            next = Some(process.parent_pid);
        }
        chain.reverse();
        chain
    }

    fn build_process_tree(
        target_pid: u32,
        snapshot: &BTreeMap<u32, SnapshotProcess>,
        children_by_parent: &HashMap<u32, Vec<u32>>,
        fallback: &SnapshotProcess,
    ) -> (ProcessTreeNode, bool) {
        let mut remaining = MAX_TREE_NODES;
        let mut visited = HashSet::new();
        let mut truncated = false;
        let root = build_tree_node(
            target_pid,
            target_pid,
            snapshot,
            children_by_parent,
            fallback,
            0,
            &mut remaining,
            &mut visited,
            &mut truncated,
        );
        (root, truncated)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_tree_node(
        pid: u32,
        target_pid: u32,
        snapshot: &BTreeMap<u32, SnapshotProcess>,
        children_by_parent: &HashMap<u32, Vec<u32>>,
        fallback: &SnapshotProcess,
        depth: usize,
        remaining: &mut usize,
        visited: &mut HashSet<u32>,
        truncated: &mut bool,
    ) -> ProcessTreeNode {
        let process = snapshot.get(&pid).unwrap_or(fallback);
        if *remaining == 0 || !visited.insert(pid) {
            *truncated = true;
            return tree_node(process, target_pid, Vec::new());
        }
        *remaining -= 1;

        let child_pids = children_by_parent.get(&pid).cloned().unwrap_or_default();
        if depth >= MAX_TREE_DEPTH {
            if !child_pids.is_empty() {
                *truncated = true;
            }
            return tree_node(process, target_pid, Vec::new());
        }

        let mut children = Vec::new();
        for child_pid in child_pids {
            if *remaining == 0 {
                *truncated = true;
                break;
            }
            if let Some(child) = snapshot.get(&child_pid) {
                children.push(build_tree_node(
                    child_pid,
                    target_pid,
                    snapshot,
                    children_by_parent,
                    child,
                    depth + 1,
                    remaining,
                    visited,
                    truncated,
                ));
            }
        }
        tree_node(process, target_pid, children)
    }

    fn tree_node(
        process: &SnapshotProcess,
        target_pid: u32,
        children: Vec<ProcessTreeNode>,
    ) -> ProcessTreeNode {
        ProcessTreeNode {
            pid: process.pid,
            parent_pid: process.parent_pid,
            name: process.name.clone(),
            thread_count: process.thread_count,
            is_target: process.pid == target_pid,
            children,
        }
    }

    fn process_image_path(handle: HANDLE) -> Option<String> {
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
        (ok != 0).then(|| String::from_utf16_lossy(&buffer[..size as usize]))
    }

    fn process_command_line(handle: HANDLE) -> Option<String> {
        let mut required = 0u32;
        unsafe {
            NtQueryInformationProcess(
                handle,
                ProcessCommandLineInformation,
                std::ptr::null_mut(),
                0,
                &mut required,
            );
        }
        if required < size_of::<UnicodeString>() as u32 || required > 4 * 1024 * 1024 {
            return None;
        }
        let word_count = (required as usize).div_ceil(size_of::<usize>());
        let mut storage = vec![0usize; word_count];
        let mut actual = required;
        let status = unsafe {
            NtQueryInformationProcess(
                handle,
                ProcessCommandLineInformation,
                storage.as_mut_ptr().cast::<c_void>(),
                (storage.len() * size_of::<usize>()) as u32,
                &mut actual,
            )
        };
        if status < 0 {
            return None;
        }
        let value = unsafe { &*storage.as_ptr().cast::<UnicodeString>() };
        if value.buffer.is_null() || value.length == 0 || value.length % 2 != 0 {
            return None;
        }
        let start = storage.as_ptr() as usize;
        let end = start.checked_add(storage.len() * size_of::<usize>())?;
        let buffer_start = value.buffer as usize;
        let buffer_end = buffer_start.checked_add(value.length as usize)?;
        if buffer_start < start || buffer_end > end {
            return None;
        }
        let text = unsafe {
            std::slice::from_raw_parts(value.buffer, value.length as usize / size_of::<u16>())
        };
        let command_line = String::from_utf16_lossy(text);
        (!command_line.trim().is_empty()).then_some(command_line)
    }

    fn process_start_time(handle: HANDLE) -> Option<FILETIME> {
        let mut creation: FILETIME = unsafe { std::mem::zeroed() };
        let mut exit: FILETIME = unsafe { std::mem::zeroed() };
        let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
        let mut user: FILETIME = unsafe { std::mem::zeroed() };
        if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0
        {
            None
        } else {
            Some(creation)
        }
    }

    fn process_is_critical(handle: HANDLE) -> bool {
        let mut critical = 0;
        unsafe { IsProcessCritical(handle, &mut critical) != 0 && critical != 0 }
    }

    fn process_session_id(pid: u32) -> u32 {
        let mut session_id = 0u32;
        if unsafe { ProcessIdToSessionId(pid, &mut session_id) } == 0 {
            0
        } else {
            session_id
        }
    }

    fn filetime_to_unix_seconds(value: FILETIME) -> Option<u64> {
        let ticks = ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64;
        if ticks == 0 {
            return None;
        }
        (ticks / 10_000_000).checked_sub(WINDOWS_TO_UNIX_SECONDS)
    }
}

#[cfg(windows)]
pub use platform::{scan, tcp_statistics};

#[cfg(not(windows))]
pub fn scan(_port: u16) -> Result<PortOccupancyScanResult, String> {
    Err("端口占用检测目前仅支持 Windows".into())
}

#[cfg(not(windows))]
pub fn tcp_statistics(
    _port: u16,
    _source_ip: Option<String>,
    _local_ip: Option<String>,
) -> Result<TcpConnectionStatistics, String> {
    Err("TCP 连接统计目前仅支持 Windows".into())
}

#[cfg(all(test, windows))]
mod tests {
    use std::net::{TcpListener, TcpStream, UdpSocket};

    #[test]
    fn finds_current_process_tcp_listener() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let result = super::scan(port).unwrap();
        let current_pid = std::process::id();
        assert!(result.processes.iter().any(|process| {
            process.pid == current_pid
                && process.endpoints.iter().any(|endpoint| {
                    endpoint.protocol == "TCP"
                        && endpoint.local_ip == "127.0.0.1"
                        && endpoint.local_port == port
                        && endpoint.listening
                })
        }));
    }

    #[test]
    fn tcp_statistics_counts_and_filters_listener() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        let result = super::tcp_statistics(port, None, Some("127.0.0.1".into())).unwrap();
        assert!(result.total_connections >= 1);
        assert!(result
            .state_counts
            .iter()
            .any(|state| state.state == "LISTENING" && state.count >= 1));
        assert!(result.connections.iter().any(|connection| {
            connection.pid == std::process::id()
                && connection.local_ip == "127.0.0.1"
                && connection.local_port == port
                && connection.state == "LISTENING"
        }));

        let filtered = super::tcp_statistics(port, Some("127.0.0.1".into()), None).unwrap();
        assert_eq!(filtered.total_connections, 0);
    }

    #[test]
    fn tcp_statistics_filters_established_connection_by_source_ip() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let (server, _) = listener.accept().unwrap();

        let result =
            super::tcp_statistics(port, Some("127.0.0.1".into()), Some("127.0.0.1".into()))
                .unwrap();
        assert!(result
            .state_counts
            .iter()
            .any(|state| state.state == "ESTABLISHED" && state.count >= 1));
        assert!(result.connections.iter().any(|connection| {
            connection.local_port == port
                && connection.remote_ip.as_deref() == Some("127.0.0.1")
                && connection.state == "ESTABLISHED"
        }));
        assert_eq!(
            result.traffic_available_connections + result.traffic_unavailable_connections,
            result.total_connections
        );
        for connection in result
            .connections
            .iter()
            .filter(|connection| connection.bytes_sent.is_some())
        {
            assert!(connection.bytes_received.is_some());
        }

        drop(server);
        drop(client);
    }

    #[test]
    fn finds_current_process_udp_binding() {
        let socket = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let port = socket.local_addr().unwrap().port();
        let result = super::scan(port).unwrap();
        let current_pid = std::process::id();
        assert!(result.processes.iter().any(|process| {
            process.pid == current_pid
                && process.endpoints.iter().any(|endpoint| {
                    endpoint.protocol == "UDP"
                        && endpoint.local_ip == "127.0.0.1"
                        && endpoint.local_port == port
                        && endpoint.listening
                })
        }));
    }
}

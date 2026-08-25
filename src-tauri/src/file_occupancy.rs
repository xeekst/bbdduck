use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedFileHandle {
    pub handle_value: String,
    pub path: String,
    pub granted_access: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OccupyingProcess {
    pub pid: u32,
    pub process_token: String,
    pub name: String,
    pub path: Option<String>,
    pub app_type: String,
    pub session_id: u32,
    pub started_at: Option<u64>,
    pub can_terminate: bool,
    pub handles: Vec<MatchedFileHandle>,
    pub handle_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OccupancyScanResult {
    pub query: String,
    pub scanned_handles: usize,
    pub file_handles: usize,
    pub matched_handles: usize,
    pub inaccessible_processes: usize,
    pub unresolved_handles: usize,
    pub truncated: bool,
    pub elapsed_ms: u128,
    pub processes: Vec<OccupyingProcess>,
}

#[cfg(windows)]
mod platform {
    use super::{MatchedFileHandle, OccupancyScanResult, OccupyingProcess};
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::fs::File;
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::time::Instant;
    use windows_sys::Wdk::System::SystemInformation::NtQuerySystemInformation;
    use windows_sys::Win32::Foundation::{
        CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, FILETIME, HANDLE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileType, GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, FILE_TYPE_DISK,
        VOLUME_NAME_DOS,
    };
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, IsProcessCritical, OpenProcess,
        QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, PROCESS_DUP_HANDLE,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };

    const SYSTEM_EXTENDED_HANDLE_INFORMATION: i32 = 64;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;
    const INITIAL_HANDLE_BUFFER: usize = 1024 * 1024;
    const MAX_HANDLE_BUFFER: usize = 128 * 1024 * 1024;
    const MAX_MATCHED_HANDLES: usize = 500;
    const WINDOWS_TO_UNIX_SECONDS: u64 = 11_644_473_600;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SystemHandleEntry {
        object: usize,
        unique_process_id: usize,
        handle_value: usize,
        granted_access: u32,
        creator_back_trace_index: u16,
        object_type_index: u16,
        handle_attributes: u32,
        reserved: u32,
    }

    pub fn scan(input: String) -> Result<OccupancyScanResult, String> {
        let started = Instant::now();
        let query = input.trim().to_string();
        if query.is_empty() {
            return Err("请输入要搜索的文件或文件夹名称".into());
        }
        let query_chars: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
        let (scanned_handles, file_entries) = snapshot_file_handles()?;
        let file_handles = file_entries.len();

        let mut by_pid: BTreeMap<u32, Vec<SystemHandleEntry>> = BTreeMap::new();
        for entry in file_entries {
            if entry.unique_process_id <= u32::MAX as usize {
                by_pid
                    .entry(entry.unique_process_id as u32)
                    .or_default()
                    .push(entry);
            }
        }

        let current_pid = unsafe { GetCurrentProcessId() };
        let current_process = unsafe { GetCurrentProcess() };
        let mut processes = Vec::new();
        let mut inaccessible_processes = 0usize;
        let mut unresolved_handles = 0usize;
        let mut matched_handles = 0usize;
        let mut truncated = false;

        for (pid, entries) in by_pid {
            if matched_handles >= MAX_MATCHED_HANDLES {
                truncated = true;
                break;
            }
            let access = PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION;
            let process_handle = unsafe { OpenProcess(access, 0, pid) };
            if process_handle.is_null() {
                inaccessible_processes += 1;
                continue;
            }

            let mut matches = Vec::new();
            for entry in entries {
                if matched_handles >= MAX_MATCHED_HANDLES {
                    truncated = true;
                    break;
                }
                let path = match duplicate_file_path(process_handle, current_process, &entry) {
                    Ok(Some(path)) => path,
                    Ok(None) => continue,
                    Err(()) => {
                        unresolved_handles += 1;
                        continue;
                    }
                };
                if !path_matches_query(&path, &query_chars) {
                    continue;
                }
                matches.push(MatchedFileHandle {
                    handle_value: format!("0x{:X}", entry.handle_value),
                    path,
                    granted_access: format!("0x{:08X}", entry.granted_access),
                });
                matched_handles += 1;
            }

            if !matches.is_empty() {
                let process_path = process_image_path(process_handle);
                let name = process_path
                    .as_deref()
                    .and_then(|value| Path::new(value).file_name())
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("进程 {pid}"));
                let start_time = process_start_time(process_handle);
                let critical = process_is_critical(process_handle);
                let handle_count = matches.len();
                processes.push(OccupyingProcess {
                    pid,
                    process_token: process_token(pid, start_time),
                    name,
                    path: process_path,
                    app_type: if critical {
                        "critical".into()
                    } else {
                        "regular".into()
                    },
                    session_id: process_session_id(pid),
                    started_at: start_time.and_then(filetime_to_unix_seconds),
                    can_terminate: pid > 4
                        && pid != current_pid
                        && !critical
                        && start_time.is_some(),
                    handles: matches,
                    handle_count,
                });
            }

            unsafe {
                CloseHandle(process_handle);
            }
        }

        processes.sort_by(|a, b| {
            b.handle_count
                .cmp(&a.handle_count)
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then(a.pid.cmp(&b.pid))
        });

        Ok(OccupancyScanResult {
            query,
            scanned_handles,
            file_handles,
            matched_handles,
            inaccessible_processes,
            unresolved_handles,
            truncated,
            elapsed_ms: started.elapsed().as_millis(),
            processes,
        })
    }

    pub fn terminate(pid: u32, process_token: String) -> Result<(), String> {
        if pid == 0 || pid == 4 || pid == unsafe { GetCurrentProcessId() } {
            return Err("出于安全原因，不能终止该系统进程或当前应用".into());
        }
        let (token_pid, expected_high, expected_low) = parse_process_token(&process_token)?;
        if token_pid != pid || (expected_high == 0 && expected_low == 0) {
            return Err("进程标识无效，请重新检测后再试".into());
        }

        let access = PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            return Err(last_os_error(
                "无法打开进程；它可能已退出，或需要管理员权限",
            ));
        }

        let result = (|| {
            if process_is_critical(handle) {
                return Err("出于安全原因，不能终止 Windows 关键进程".into());
            }
            let actual =
                process_start_time(handle).ok_or_else(|| last_os_error("无法校验进程启动时间"))?;
            if actual.dwHighDateTime != expected_high || actual.dwLowDateTime != expected_low {
                return Err("PID 已被其他进程复用，已取消终止操作；请重新检测".into());
            }
            if unsafe { TerminateProcess(handle, 1) } == 0 {
                return Err(last_os_error("终止进程失败；请尝试以管理员身份运行"));
            }
            unsafe {
                WaitForSingleObject(handle, 2000);
            }
            Ok(())
        })();

        unsafe {
            CloseHandle(handle);
        }
        result
    }

    fn snapshot_file_handles() -> Result<(usize, Vec<SystemHandleEntry>), String> {
        let executable =
            std::env::current_exe().map_err(|error| format!("无法定位当前程序：{error}"))?;
        let probe = File::open(executable).map_err(|error| format!("无法创建文件探针：{error}"))?;
        let probe_value = probe.as_raw_handle() as usize;
        let current_pid = unsafe { GetCurrentProcessId() } as usize;
        let entries = query_system_handles()?;
        let scanned_handles = entries.len();
        let file_type_index = entries
            .iter()
            .find(|entry| {
                entry.unique_process_id == current_pid && entry.handle_value == probe_value
            })
            .map(|entry| entry.object_type_index)
            .ok_or_else(|| "无法识别 Windows 文件 Handle 类型".to_string())?;
        let file_entries = entries
            .into_iter()
            .filter(|entry| entry.object_type_index == file_type_index)
            .collect();
        Ok((scanned_handles, file_entries))
    }

    fn query_system_handles() -> Result<Vec<SystemHandleEntry>, String> {
        let mut byte_len = INITIAL_HANDLE_BUFFER;
        for _ in 0..8 {
            let words_len = byte_len.div_ceil(size_of::<usize>());
            let mut storage = vec![0usize; words_len];
            let mut required = 0u32;
            let status = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_EXTENDED_HANDLE_INFORMATION,
                    storage.as_mut_ptr().cast::<c_void>(),
                    (storage.len() * size_of::<usize>()) as u32,
                    &mut required,
                )
            };
            if status == 0 {
                let header_size = size_of::<usize>() * 2;
                let available_bytes = storage.len() * size_of::<usize>();
                if available_bytes < header_size {
                    return Err("Windows 返回了无效的 Handle 表".into());
                }
                let reported = unsafe { *(storage.as_ptr() as *const usize) };
                let available = (available_bytes - header_size) / size_of::<SystemHandleEntry>();
                let count = reported.min(available);
                let first = unsafe { storage.as_ptr().cast::<u8>().add(header_size) }
                    as *const SystemHandleEntry;
                let entries = unsafe { std::slice::from_raw_parts(first, count) }.to_vec();
                return Ok(entries);
            }
            if status != STATUS_INFO_LENGTH_MISMATCH {
                return Err(format!(
                    "枚举系统 Handle 失败（NTSTATUS 0x{:08X}）",
                    status as u32
                ));
            }
            let requested = required as usize;
            byte_len = requested.saturating_add(64 * 1024).max(byte_len * 2);
            if byte_len > MAX_HANDLE_BUFFER {
                return Err("系统 Handle 数量过多，超过安全扫描上限".into());
            }
        }
        Err("系统 Handle 表持续变化，请重试".into())
    }

    fn duplicate_file_path(
        source_process: HANDLE,
        current_process: HANDLE,
        entry: &SystemHandleEntry,
    ) -> Result<Option<String>, ()> {
        let mut duplicated: HANDLE = std::ptr::null_mut();
        let ok = unsafe {
            DuplicateHandle(
                source_process,
                entry.handle_value as HANDLE,
                current_process,
                &mut duplicated,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if ok == 0 || duplicated.is_null() {
            return Err(());
        }

        let file_type = unsafe { GetFileType(duplicated) };
        let path = if file_type == FILE_TYPE_DISK {
            final_path(duplicated).map(Some).ok_or(())
        } else {
            Ok(None)
        };
        unsafe {
            CloseHandle(duplicated);
        }
        path
    }

    fn final_path(handle: HANDLE) -> Option<String> {
        let mut buffer = vec![0u16; 32768];
        let length = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        };
        if length == 0 || length as usize >= buffer.len() {
            return None;
        }
        let raw = String::from_utf16_lossy(&buffer[..length as usize]);
        let display = display_path(&raw);
        if is_filesystem_path(&display) {
            Some(display)
        } else {
            None
        }
    }

    fn display_path(raw: &str) -> String {
        if let Some(value) = raw.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{value}")
        } else if let Some(value) = raw.strip_prefix(r"\\?\") {
            value.to_string()
        } else {
            raw.to_string()
        }
    }

    fn is_filesystem_path(path: &str) -> bool {
        let bytes = path.as_bytes();
        (bytes.len() >= 2 && bytes[1] == b':') || path.starts_with(r"\\")
    }

    fn path_matches_query(path: &str, needle: &[char]) -> bool {
        path.split(|ch| ch == '\\' || ch == '/')
            .any(|component| fuzzy_component_match(component, needle))
    }

    fn fuzzy_component_match(component: &str, needle: &[char]) -> bool {
        if needle.is_empty() {
            return false;
        }
        let mut index = 0usize;
        for ch in component.chars().flat_map(char::to_lowercase) {
            if ch == needle[index] {
                index += 1;
                if index == needle.len() {
                    return true;
                }
            }
        }
        false
    }

    fn process_image_path(handle: HANDLE) -> Option<String> {
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
        if ok == 0 {
            None
        } else {
            Some(String::from_utf16_lossy(&buffer[..size as usize]))
        }
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

    fn process_token(pid: u32, start: Option<FILETIME>) -> String {
        match start {
            Some(value) => format!("{pid}:{}:{}", value.dwHighDateTime, value.dwLowDateTime),
            None => format!("{pid}:0:0"),
        }
    }

    fn parse_process_token(token: &str) -> Result<(u32, u32, u32), String> {
        let values: Vec<&str> = token.split(':').collect();
        if values.len() != 3 {
            return Err("进程标识无效，请重新检测".into());
        }
        Ok((
            values[0].parse().map_err(|_| "进程标识无效")?,
            values[1].parse().map_err(|_| "进程标识无效")?,
            values[2].parse().map_err(|_| "进程标识无效")?,
        ))
    }

    fn filetime_to_unix_seconds(value: FILETIME) -> Option<u64> {
        let ticks = ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64;
        if ticks == 0 {
            return None;
        }
        (ticks / 10_000_000).checked_sub(WINDOWS_TO_UNIX_SECONDS)
    }

    fn last_os_error(action: &str) -> String {
        format!("{action}：{}", std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
pub use platform::{scan, terminate};

#[cfg(not(windows))]
pub fn scan(_query: String) -> Result<OccupancyScanResult, String> {
    Err("文件 Handle 搜索目前仅支持 Windows".into())
}

#[cfg(not(windows))]
pub fn terminate(_pid: u32, _process_token: String) -> Result<(), String> {
    Err("进程终止功能目前仅支持 Windows".into())
}

#[cfg(all(test, windows))]
mod tests {
    use std::fs::{self, OpenOptions};

    #[test]
    fn finds_an_open_file_handle_by_fuzzy_name() {
        let marker = format!(
            "bbdduck_handle_marker_{}_{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(&marker);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();

        let result = super::scan(marker.replace("handle", "hndl")).unwrap();
        let current_pid = std::process::id();
        assert!(result.processes.iter().any(|process| {
            process.pid == current_pid
                && process
                    .handles
                    .iter()
                    .any(|handle| handle.path.ends_with(&marker))
        }));

        drop(file);
        fs::remove_file(path).unwrap();
    }
}

//! Sync diagnostics shared by the client engine and the server.
//! Every entry is emitted to the UI and appended to one UTC-dated log file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, Runtime};

use super::model::{now_secs, LogPayload, EVT_LOG};

static LOG_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub fn emit<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    source: &str,
    level: &str,
    message: impl Into<String>,
) {
    let message = message.into();
    let timestamp = now_secs();
    let (date, time) = utc_date_time(timestamp);
    let file = append_log(app, &date, &time, timestamp, id, source, level, &message);
    let _ = app.emit(
        EVT_LOG,
        LogPayload {
            id: id.to_string(),
            source: source.to_string(),
            level: level.to_string(),
            message,
            time: timestamp,
            file: file.map(|path| path.to_string_lossy().into_owned()),
        },
    );
}

pub fn server_info<R: Runtime>(app: &AppHandle<R>, message: impl Into<String>) {
    emit(app, "server", "server", "info", message);
}

pub fn server_warn<R: Runtime>(app: &AppHandle<R>, message: impl Into<String>) {
    emit(app, "server", "server", "warn", message);
}

pub fn server_error<R: Runtime>(app: &AppHandle<R>, message: impl Into<String>) {
    emit(app, "server", "server", "error", message);
}

fn append_log<R: Runtime>(
    app: &AppHandle<R>,
    date: &str,
    time: &str,
    timestamp: i64,
    id: &str,
    source: &str,
    level: &str,
    message: &str,
) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?.join("logs");
    fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("sync-{date}.log"));
    let line = format!(
        "[{date} {time} UTC] [{timestamp}] [{}] [{}] [{}] {}\n",
        source.to_uppercase(),
        level.to_uppercase(),
        id,
        message.replace(['\r', '\n'], " ")
    );
    let _guard = LOG_WRITE_LOCK.lock().ok()?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    file.write_all(line.as_bytes()).ok()?;
    Some(path)
}

/// Convert a Unix timestamp to a UTC date/time without adding a date-time
/// dependency to the desktop bundle.
fn utc_date_time(timestamp: i64) -> (String, String) {
    let days = timestamp.div_euclid(86_400);
    let seconds = timestamp.rem_euclid(86_400);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;

    // Howard Hinnant's civil_from_days algorithm.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };

    (
        format!("{year:04}-{month:02}-{day:02}"),
        format!("{hour:02}:{minute:02}:{second:02}"),
    )
}

#[cfg(test)]
mod tests {
    use super::utc_date_time;

    #[test]
    fn formats_unix_epoch() {
        assert_eq!(utc_date_time(0), ("1970-01-01".into(), "00:00:00".into()));
        assert_eq!(
            utc_date_time(1_735_689_599),
            ("2024-12-31".into(), "23:59:59".into())
        );
    }
}

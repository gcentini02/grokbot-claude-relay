//! Optional bodyless wake-up ping to a webhook that wakes the Grok app. Off unless `grok-webhook.url` exists.
//! The URL goes to curl through a temp config file (`-K`), never on the command line, and never into output.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::store::Paths;

pub enum Outcome {
    NotConfigured,
    Sent,
    Failed(&'static str),
}

const ALERT_EVERY_SECS: u64 = 3600;

/// Removes the temp curl config file when dropped.
struct TempConf(std::path::PathBuf);

impl Drop for TempConf {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn ping(paths: &Paths) -> Outcome {
    let file = paths.webhook();
    let meta = match fs::metadata(&file) {
        Ok(m) => m,
        Err(_) => return Outcome::NotConfigured,
    };
    if meta.permissions().mode() & 0o777 != 0o600 {
        return Outcome::Failed("webhook file mode is not 0600");
    }
    let url = match fs::read_to_string(&file) {
        Ok(text) => text.trim().to_string(),
        Err(_) => return Outcome::Failed("webhook file is unreadable"),
    };
    if !valid_url(&url) {
        return Outcome::Failed("webhook URL is malformed");
    }

    let conf = paths
        .home
        .join(format!(".curl-{}.conf", std::process::id()));
    let _ = fs::remove_file(&conf);
    let guard = TempConf(conf.clone());
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&conf)
        .and_then(|mut f| writeln!(f, "url = \"{url}\""));
    if written.is_err() {
        return Outcome::Failed("could not write curl config");
    }

    let status = Command::new("curl")
        .arg("-K")
        .arg(&conf)
        .args(["-fsS", "-m", "10", "-X", "POST", "--data", ""])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    drop(guard);
    match status {
        Ok(s) if s.success() => Outcome::Sent,
        Ok(_) => Outcome::Failed("webhook returned an error"),
        Err(_) => Outcome::Failed("curl could not run"),
    }
}

/// Only plain http(s) URLs with no whitespace, quotes, backslashes or control characters.
fn valid_url(url: &str) -> bool {
    let scheme_ok = url.starts_with("https://") || url.starts_with("http://");
    let chars_ok = url
        .chars()
        .all(|c| !c.is_whitespace() && !c.is_control() && c != '"' && c != '\\');
    scheme_ok && chars_ok && url.len() > 8
}

/// True when a failure alert may be sent now. Records the time so the next alert waits an hour.
pub fn claim_alert_slot(paths: &Paths) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let last = fs::read_to_string(paths.wake_stamp())
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());
    if let Some(last) = last
        && now.saturating_sub(last) < ALERT_EVERY_SECS
    {
        return false;
    }
    let _ = fs::write(paths.wake_stamp(), now.to_string());
    true
}

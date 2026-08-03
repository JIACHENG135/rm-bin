//! The one thing rm-bin still needs to know about talking to the tablet over
//! ssh: how to open the connection, and how to read back what went wrong.

use std::process::Command;

/// Shared with `upload`: same host, same credentials, same "fail fast rather
/// than sit at a password prompt" options.
pub(super) fn ssh_base(host: &str, port: u16) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args([
        "-p",
        &port.to_string(),
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ConnectTimeout=6",
        &format!("root@{host}"),
    ]);
    cmd
}

/// Pull the one interesting line out of the remote command's stderr. dd
/// always reports its "N+M records in/out" tally there even on success, so
/// the last line is usually noise — an actual complaint is what's wanted.
pub(super) fn remote_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .map(str::trim)
        .find(|l| l.contains("dd:") || l.contains("rror") || l.contains("denied"))
        .or_else(|| text.lines().map(str::trim).find(|l| !l.is_empty() && !l.contains("records")))
        .unwrap_or("ssh 退出异常")
        .to_string()
}

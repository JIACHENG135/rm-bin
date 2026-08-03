//! Putting files onto the tablet by writing straight into xochitl's document
//! store over ssh — the PDF importer's fallback when the USB web interface
//! isn't reachable.

use std::io::Write;
use std::process::Stdio;

use crate::rm::device::ssh_base;
use crate::rm::rmfile;

/// xochitl's document store. Same path on both generations.
const STORE: &str = "/home/root/.local/share/remarkable/xochitl";

/// One file to place in the document store: a name relative to it, and the
/// bytes.
pub struct Entry {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// The shell run on the tablet: write every file, flush, restart.
///
/// One ssh connection rather than one per file — the handshakes are most of
/// the wall clock of this path, and a single `set -e` script means the
/// restart cannot happen after a half-written document.
///
/// `dd bs=N count=1 iflag=fullblock`, not `head -c N`. All the files arrive
/// down one stdin, so each command has to consume *exactly* its own bytes and
/// leave the rest for the next one. busybox `head -c` reads in blocks and can
/// swallow past its limit; `dd` with a `bs` equal to the whole file and
/// `iflag=fullblock` (so a short pipe read doesn't end it early) takes that
/// many bytes and stops. Same lesson as `device::push`, for the same reason.
///
/// dd's own "N+0 records in" tally goes to stderr on success too, which is why
/// stderr isn't silenced here: `device::remote_error` already knows to skip
/// those lines, and silencing them would also silence the real complaints.
pub(super) fn script_for(files: &[Entry], dirs: &[String]) -> String {
    let mut s = format!(
        "set -e\n\
         d={STORE}\n\
         [ -d \"$d\" ] || {{ echo 'xochitl data directory missing'; exit 1; }}\n"
    );
    for dir in dirs {
        s.push_str(&format!("mkdir -p \"$d/{dir}\"\n"));
    }
    for f in files {
        s.push_str(&format!(
            "dd bs={} count=1 iflag=fullblock of=\"$d/{}\"\n",
            f.bytes.len(),
            f.name
        ));
    }
    s.push_str("sync\nsystemctl restart xochitl\n");
    s
}

/// Place a set of files in xochitl's document store and restart it.
pub fn install_files(host: &str, port: u16, files: &[Entry], dirs: &[String]) -> Result<(), String> {
    let script = script_for(files, dirs);

    let mut child = ssh_base(host, port)
        .arg(script)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 ssh: {e}"))?;

    let mut stdin = child.stdin.take().expect("piped");
    // Order matters: it is the order the script consumes them in.
    let mut ok = true;
    for f in files {
        if stdin.write_all(&f.bytes).is_err() {
            ok = false;
            break;
        }
    }
    ok &= stdin.flush().is_ok();
    drop(stdin);

    let out = child.wait_with_output().map_err(|e| format!("ssh 失败：{e}"))?;
    if out.status.success() && ok {
        return Ok(());
    }
    // A write failure here is almost always EPIPE — the remote script already
    // died and said why on stderr, which is the useful half.
    Err(format!(
        "上传失败：{}",
        crate::rm::device::remote_error(&out.stderr)
    ))
}

/// A random-looking UUID, without pulling in a dependency for it.
///
/// These name a notebook nobody looks up by name; the only thing riding on
/// them is that two notebooks don't collide and overwrite each other. A
/// splitmix64 seeded from the nanosecond clock and the address of a local is
/// far past sufficient for that, and it keeps the dependency list honest.
pub(crate) fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let local = 0u8;
    let mut state = nanos
        ^ (&local as *const u8 as u64)
        ^ ((std::process::id() as u64) << 32);

    let mut next = || {
        // splitmix64
        state = state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    };

    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&next().to_le_bytes());
    b[8..].copy_from_slice(&next().to_le_bytes());
    // Version 4, variant 1 — xochitl doesn't check, but a UUID that says what
    // it is costs two bytes of masking.
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    rmfile::uuid_string(&b)
}

/// A document name from the dropped file: the image's own name, since that is
/// what the person will look for in the document list.
pub fn name_from_path(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        // xochitl shows the name verbatim and `rmfile::metadata` interpolates
        // it into JSON, so anything that would break out of the string has to
        // go. Control characters would also render as boxes.
        .map(|s| {
            s.chars()
                .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
                .take(60)
                .collect::<String>()
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "RM Bin".into())
}

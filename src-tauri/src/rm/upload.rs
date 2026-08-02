//! Putting a finished notebook onto the tablet.
//!
//! `rmfile` produces the three files xochitl wants — `<doc>.metadata`,
//! `<doc>.content` and `<doc>/<page>.rm` — and this puts them in its document
//! store. That store is a plain directory, so "installing" a notebook is
//! writing three files into it; the only awkward part is that xochitl reads
//! the directory once, at startup, and has no reload anyone can call. So the
//! last thing this does is restart it.
//!
//! The restart is the price of this path. What it buys, against replaying the
//! pen: the drawing lands whole in about a second instead of crawling in over
//! minutes, it lands on a *new* page instead of on top of whatever was open,
//! and it can't blunder into the toolbar, because there is no pen involved.

use std::io::Write;
use std::process::Stdio;

use crate::rm::device::ssh_base;
use crate::rm::rmfile;

/// xochitl's document store. Same path on both generations.
const STORE: &str = "/home/root/.local/share/remarkable/xochitl";

/// A notebook as three files, named and ready to be written.
pub struct Notebook {
    pub doc: String,
    pub page: String,
    pub metadata: String,
    pub content: String,
    pub page_bytes: Vec<u8>,
}

impl Notebook {
    /// Total bytes that will cross the wire.
    pub fn len(&self) -> usize {
        self.metadata.len() + self.content.len() + self.page_bytes.len()
    }
}

/// Serialise `strokes` (already in page coordinates) as a one-page notebook
/// called `name`.
pub fn build(name: &str, strokes: &[Vec<rmfile::Point>], now_ms: u128) -> Notebook {
    let doc = uuid();
    let page = uuid();
    Notebook {
        metadata: rmfile::metadata(name, now_ms),
        content: rmfile::content(&page, now_ms),
        page_bytes: rmfile::page(strokes),
        doc,
        page,
    }
}

/// The shell run on the tablet: write the three files, flush, restart.
///
/// One ssh connection rather than one per file — three handshakes is most of
/// the wall clock of this whole path, and doing it in a single `set -e` script
/// means the restart can't happen after a half-written notebook.
///
/// `dd bs=N count=1 iflag=fullblock`, not `head -c N`. All three files arrive
/// down one stdin, so each command has to consume *exactly* its own bytes and
/// leave the rest for the next one. busybox `head -c` reads in blocks and can
/// swallow past its limit; `dd` with a `bs` equal to the whole file and
/// `iflag=fullblock` (so a short pipe read doesn't end it early) takes that
/// many bytes and stops. Same lesson as `device::push`, for the same reason.
///
/// dd's own "N+0 records in" tally goes to stderr on success too, which is why
/// stderr isn't silenced here: `device::remote_error` already knows to skip
/// those lines, and silencing them would also silence the real complaints.
pub(super) fn script(nb: &Notebook) -> String {
    format!(
        "set -e\n\
         d={STORE}\n\
         [ -d \"$d\" ] || {{ echo 'xochitl data directory missing'; exit 1; }}\n\
         mkdir -p \"$d/{doc}\"\n\
         dd bs={n_meta} count=1 iflag=fullblock of=\"$d/{doc}.metadata\"\n\
         dd bs={n_content} count=1 iflag=fullblock of=\"$d/{doc}.content\"\n\
         dd bs={n_page} count=1 iflag=fullblock of=\"$d/{doc}/{page}.rm\"\n\
         sync\n\
         systemctl restart xochitl\n",
        doc = nb.doc,
        page = nb.page,
        n_meta = nb.metadata.len(),
        n_content = nb.content.len(),
        n_page = nb.page_bytes.len(),
    )
}

/// Write `nb` into the tablet's document store and restart xochitl so it
/// appears. Returns once the restart has been asked for — xochitl takes a few
/// seconds more to come back up on its own.
pub fn install(host: &str, port: u16, nb: &Notebook) -> Result<(), String> {
    let mut child = ssh_base(host, port)
        .arg(script(nb))
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 ssh: {e}"))?;

    let mut stdin = child.stdin.take().expect("piped");
    // Order matters: it's the order `script` consumes them in.
    let ok = stdin
        .write_all(nb.metadata.as_bytes())
        .and_then(|_| stdin.write_all(nb.content.as_bytes()))
        .and_then(|_| stdin.write_all(&nb.page_bytes))
        .and_then(|_| stdin.flush())
        .is_ok();
    drop(stdin);

    let out = child
        .wait_with_output()
        .map_err(|e| format!("ssh 失败：{e}"))?;
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
fn uuid() -> String {
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

/// A notebook name from the dropped file: the image's own name, since that is
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

//! What the two reMarkable generations look like from the *host* side.
//!
//! rm-agent encodes all of this with `#[cfg(target_pointer_width)]`, because
//! it runs on the tablet and so is compiled for exactly one of them. rm-bin
//! runs on the Mac and talks to whichever tablet is plugged in, so the same
//! facts have to be runtime values instead: `input_event` is 16 bytes on the
//! rM2's 32-bit kernel and 24 on Paper Pro's 64-bit one, the pen lives on a
//! different `/dev/input/eventN`, and the digitizer ranges differ.
//!
//! Calibration numbers are the ones rm-agent verified on real hardware
//! (`evtest` per device, plus a goMarkableStream `/screenshot` for the pixel
//! dimensions) — see `rm-agent/src/evdev.rs` for how each was measured.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Rm2,
    PaperPro,
}

#[derive(Clone, Copy, Debug)]
pub struct Calib {
    pub model: Model,
    /// Pen digitizer node — the file raw events get written into.
    pub pen_device: &'static str,
    /// Pen ABS_X / ABS_Y full-scale range.
    pub max_x: f64,
    pub max_y: f64,
    /// Framebuffer/screenshot pixel dimensions (square pixels).
    pub screen_w: f64,
    pub screen_h: f64,
    /// `sizeof(struct input_event)` on the tablet's kernel.
    pub event_size: usize,
}

pub const RM2: Calib = Calib {
    model: Model::Rm2,
    pen_device: "/dev/input/event1",
    max_x: 20966.0,
    max_y: 15725.0,
    screen_w: 1404.0,
    screen_h: 1872.0,
    event_size: 16,
};

pub const PAPER_PRO: Calib = Calib {
    model: Model::PaperPro,
    pen_device: "/dev/input/event2",
    max_x: 11180.0,
    max_y: 15340.0,
    screen_w: 1632.0,
    screen_h: 2154.0,
    event_size: 24,
};

impl Calib {
    /// Screen-space (u, v) — both 0..1, origin top-left, as you'd read the
    /// page — to raw pen digitizer units.
    ///
    /// On Paper Pro the pen axes map straight onto the screen axes. On rM2
    /// pen space is rotated 90°: pen_y runs along the screen's *width* and
    /// pen_x runs *up* the screen's height (pen_x = 0 is the bottom edge).
    /// This is the inverse of rm-agent's `pen_to_screenshot_px`, which is
    /// where both mappings were confirmed.
    pub fn pen_from_screen(&self, u: f64, v: f64) -> (f64, f64) {
        match self.model {
            Model::Rm2 => ((1.0 - v) * self.max_x, u * self.max_y),
            Model::PaperPro => (u * self.max_x, v * self.max_y),
        }
    }
}

// ————— raw Linux input_event encoding —————

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_ABS: u16 = 3;
pub const ABS_X: u16 = 0;
pub const ABS_Y: u16 = 1;
pub const ABS_PRESSURE: u16 = 24;
pub const BTN_TOOL_PEN: u16 = 320;
pub const BTN_TOUCH: u16 = 330;

pub const PRESSURE: i32 = 3200;
/// Max distance (pen units) between two consecutive emitted samples; longer
/// jumps get interpolated so xochitl draws a line rather than a teleport.
pub const STEP: i32 = 40;

/// Append one `struct input_event`. Layout is `struct timeval {long,long};
/// u16 type; u16 code; s32 value` — the `long`s are 4 bytes on the rM2 and 8
/// on Paper Pro, which is the whole reason `event_size` is dynamic. We always
/// write a zero timestamp; xochitl doesn't look at it for injected events.
pub fn push_ev(buf: &mut Vec<u8>, event_size: usize, typ: u16, code: u16, value: i32) {
    let t = event_size - 8; // timeval: 8 bytes on rM2, 16 on Paper Pro
    buf.resize(buf.len() + t, 0);
    buf.extend_from_slice(&typ.to_le_bytes());
    buf.extend_from_slice(&code.to_le_bytes());
    buf.extend_from_slice(&value.to_le_bytes());
}

/// BTN_TOOL_PEN down — the tablet now believes a pen is hovering.
pub fn pen_prologue(c: &Calib) -> Vec<u8> {
    let mut buf = Vec::new();
    push_ev(&mut buf, c.event_size, EV_KEY, BTN_TOOL_PEN, 1);
    push_ev(&mut buf, c.event_size, EV_SYN, 0, 0);
    buf
}

/// BTN_TOOL_PEN up — the pen leaves the page. Must always be sent, or
/// xochitl is left thinking a pen is still hovering.
pub fn pen_epilogue(c: &Calib) -> Vec<u8> {
    let mut buf = Vec::new();
    push_ev(&mut buf, c.event_size, EV_KEY, BTN_TOOL_PEN, 0);
    push_ev(&mut buf, c.event_size, EV_SYN, 0, 0);
    buf
}

/// Events for a set of polylines already in pen-digitizer coordinates, each
/// drawn as one pen-down..pen-up stroke, interpolated at STEP resolution.
/// Same shape as rm-agent's `build_tool_strokes`, minus the tool prologue /
/// epilogue so many calls can be concatenated into a single pen-down session.
pub fn stroke_events(c: &Calib, strokes: &[Vec<(f64, f64)>]) -> Vec<u8> {
    let mut buf = Vec::new();
    for stroke in strokes {
        let mut down = false;
        let mut prev: Option<(i32, i32)> = None;
        let emit = |buf: &mut Vec<u8>, x: i32, y: i32, down: &mut bool| {
            push_ev(buf, c.event_size, EV_ABS, ABS_X, x);
            push_ev(buf, c.event_size, EV_ABS, ABS_Y, y);
            push_ev(buf, c.event_size, EV_ABS, ABS_PRESSURE, PRESSURE);
            if !*down {
                push_ev(buf, c.event_size, EV_KEY, BTN_TOUCH, 1);
                *down = true;
            }
            push_ev(buf, c.event_size, EV_SYN, 0, 0);
        };
        for &(px, py) in stroke {
            let x = px.round().clamp(0.0, c.max_x) as i32;
            let y = py.round().clamp(0.0, c.max_y) as i32;
            match prev {
                Some((x0, y0)) => {
                    let (dx, dy) = (x - x0, y - y0);
                    let n = (dx.abs().max(dy.abs()) / STEP).max(1);
                    for k in 1..=n {
                        emit(&mut buf, x0 + dx * k / n, y0 + dy * k / n, &mut down);
                    }
                }
                None => emit(&mut buf, x, y, &mut down),
            }
            prev = Some((x, y));
        }
        if down {
            push_ev(&mut buf, c.event_size, EV_KEY, BTN_TOUCH, 0);
            push_ev(&mut buf, c.event_size, EV_SYN, 0, 0);
        }
    }
    buf
}

// ————— transport —————

fn ssh_base(host: &str, port: u16) -> Command {
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

/// Ask the tablet what it is. The kernel's machine name is the one bit that
/// distinguishes the two generations without needing xochitl to be reachable:
/// armv7l on the rM2, aarch64 on Paper Pro. It also happens to be exactly the
/// fact that decides `event_size`, so there's nothing to infer separately.
pub fn detect(host: &str, port: u16) -> Result<Calib, String> {
    let out = ssh_base(host, port)
        .arg("uname -m")
        .output()
        .map_err(|e| format!("无法启动 ssh: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim().lines().last().unwrap_or("连接失败");
        return Err(format!("连不上设备（root@{host}:{port}）：{err}"));
    }
    match String::from_utf8_lossy(&out.stdout).trim() {
        "aarch64" => Ok(PAPER_PRO),
        "armv7l" | "armv7" => Ok(RM2),
        other => Err(format!("不认识的设备架构：{other}")),
    }
}

/// Chunk size and delay for `push`. Writing a large buffer in one shot makes
/// xochitl log "Dropped pen event!" and desync its BTN_TOOL_PEN state — this
/// pacing (carried over from the Python prototype, where it was tuned against
/// real hardware over exactly this SSH pipe) mimics a plausible digitizer
/// rate instead. The chunk is a multiple of `event_size` so a write never
/// splits one event in half, which is the very desync being avoided.
const EVENTS_PER_CHUNK: usize = 20;
const CHUNK_DELAY: Duration = Duration::from_millis(4);

/// How long to hover the pen before the first stroke. xochitl collapses its
/// toolbar out of the way when a pen approaches, and until it has, that
/// toolbar is an overlay that turns any stroke crossing it into a button
/// press — observed the hard way: a full-bleed test card drew correctly *and*
/// flipped the page to landscape, opened the overflow menu and added a page.
/// draw.rs's `MARGIN` keeps the ink clear of the docked toolbar; this waits
/// for the collapse so the two together leave nothing to hit.
const SETTLE: Duration = Duration::from_millis(400);

/// Stream stroke events into the tablet's pen device over SSH, framed by the
/// pen-down / pen-up the tablet needs to see around them, and calling
/// `on_bytes` with the running byte count after each paced chunk lands.
///
/// The pacing is what makes progress reporting meaningful at all: because we
/// hand the tablet events at roughly the rate it can draw them, "bytes we
/// have written" tracks "ink on the page" closely enough to drive an
/// animation from it. `data` is stroke bytes only, so the count `on_bytes`
/// reports lines up with `Plan`'s progress table.
pub fn push(
    host: &str,
    port: u16,
    c: &Calib,
    data: &[u8],
    mut on_bytes: impl FnMut(usize),
) -> Result<(), String> {
    let chunk = c.event_size * EVENTS_PER_CHUNK;

    // `dd ... iflag=fullblock`, not `cat`. The kernel rejects a write to an
    // evdev node that isn't a whole number of `struct input_event`s, and the
    // remote program's own buffer size decides how much lands per write():
    // busybox `cat` writes 4096 at a time, which is a multiple of the rM2's
    // 16-byte events but *not* of Paper Pro's 24-byte ones, so `cat` fails
    // there with EINVAL after the very first block. (This is why the Python
    // prototype's `cat > /dev/input/event1` never hit it — rM2 only.)
    // `iflag=fullblock` is the load-bearing half: without it dd writes short
    // reads straight through, and a pipe hands out whatever happens to have
    // arrived, so most blocks come out misaligned. With it, dd fills `bs`
    // first, and both `bs` and the total are multiples of `event_size`, so
    // every write — including the final short one — is aligned.
    let mut child = ssh_base(host, port)
        .arg(format!("dd of={} bs={chunk} iflag=fullblock", c.pen_device))
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 ssh: {e}"))?;

    let mut stdin = child.stdin.take().expect("piped");
    // A write error anywhere below is nearly always EPIPE, meaning the
    // *remote* end already died — the useful diagnosis is in its stderr, not
    // in this errno, so stop and go read that instead of reporting "broken
    // pipe". `write` returns false once that has happened.
    let mut broke = false;
    let write = |stdin: &mut std::process::ChildStdin, part: &[u8]| {
        stdin.write_all(part).and_then(|_| stdin.flush()).is_ok()
    };

    if write(&mut stdin, &pen_prologue(c)) {
        std::thread::sleep(SETTLE);
    } else {
        broke = true;
    }

    let mut written = 0usize;
    if !broke {
        for part in data.chunks(chunk) {
            if !write(&mut stdin, part) {
                broke = true;
                break;
            }
            written += part.len();
            on_bytes(written);
            std::thread::sleep(CHUNK_DELAY);
        }
    }
    // Always lift the pen, even after a failure — leaving BTN_TOOL_PEN down
    // leaves xochitl believing a stylus is still hovering over the page.
    if !broke {
        broke = !write(&mut stdin, &pen_epilogue(c));
    }
    drop(stdin);

    let out = child.wait_with_output().map_err(|e| format!("ssh 失败：{e}"))?;
    if out.status.success() && !broke {
        return Ok(());
    }
    Err(format!("设备写入失败：{}", remote_error(&out.stderr)))
}

/// Pull the one interesting line out of the remote command's stderr. dd
/// always reports its "N+M records in/out" tally there even on success, so
/// the last line is usually noise — an actual complaint is what's wanted.
fn remote_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .map(str::trim)
        .find(|l| l.contains("dd:") || l.contains("rror") || l.contains("denied"))
        .or_else(|| text.lines().map(str::trim).find(|l| !l.is_empty() && !l.contains("records")))
        .unwrap_or("ssh 退出异常")
        .to_string()
}

//! Putting an image on the panel itself, rather than into a document.
//!
//! The other two paths both produce *ink*: strokes replayed through the pen,
//! or a `.rm` page handed to xochitl. Both are therefore limited to what a pen
//! can draw — a traced skeleton, black on white. A photograph comes out of
//! that as a thicket of lines.
//!
//! This path gives up the document to get the picture. `rmfb-agent` (see
//! ../../../rmfb) runs on the tablet with xochitl stopped and paints straight
//! into the vendor framebuffer, so what lands is the actual image at
//! 1620x2160 in eight-bit grey. Nothing is saved: the panel holds it, and the
//! moment xochitl comes back it repaints over it.
//!
//! Everything interesting is here rather than on the device. The agent takes
//! a rectangle and some bytes and decides nothing; scaling, tone mapping, the
//! order the image appears in and how fast are all decided on this side,
//! where they can be changed without cross-compiling against a proprietary Qt.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Stdio};

use crate::rm::device::ssh_base;

/// Where the agent and its shim live on the tablet.
const REMOTE_DIR: &str = "/home/root/rmfb";

/// The vendor library isn't on the default search path.
const REMOTE_LD_PATH: &str = "/home/root/rmfb:/usr/lib/plugins/scenegraph:/usr/lib";

/// Horizontal bands the image is sent in.
///
/// Bands are what makes this feel like something arriving rather than a file
/// transfer with a flash at the end: each one is painted as it lands. They
/// are also the progress signal — an acknowledgement comes back only after
/// the panel has finished that band, so "bands acked" is ink on glass, not
/// bytes in a socket.
const BANDS: u32 = 18;

/// EPScreenMode. Bands go up in the fastest waveform because there will be a
/// clean pass over the whole screen afterwards; that final pass is the one
/// that gets to be slow and good.
const MODE_FASTEST: u8 = 0;
const MODE_FULL: u8 = 4;

/// How long the picture stays before the tablet is given back, unless
/// something asks sooner. Long enough to actually look at it; short enough
/// that a tablet left on a desk doesn't stay hostage to a drawing.
const HOLD_SECS: u32 = 600;

/// One protocol header. Sixteen bytes, little-endian, matching the struct
/// `rmfb-agent` reads — the two are a pair and neither is any use if they
/// disagree about a single offset, which is what `screen_test` checks.
pub(crate) fn header(op: u8, mode: u8, refresh: u8, x: u32, y: u32, w: u32, h: u32) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..4].copy_from_slice(b"RMFB");
    b[4] = op;
    b[5] = mode;
    b[6] = refresh;
    b[8..10].copy_from_slice(&(x as u16).to_le_bytes());
    b[10..12].copy_from_slice(&(y as u16).to_le_bytes());
    b[12..14].copy_from_slice(&(w as u16).to_le_bytes());
    b[14..16].copy_from_slice(&(h as u16).to_le_bytes());
    b
}

pub struct Panel {
    pub width: u32,
    pub height: u32,
    /// QImage::Format the agent reported. Nothing on this side needs it —
    /// the wire is always eight-bit grey and the agent expands it — but it is
    /// the one field that would change if reMarkable moved the panel to
    /// another pixel layout, so it is carried and logged rather than dropped.
    #[allow(dead_code)]
    pub format: i32,
}

/// A live agent: xochitl is stopped and the panel is ours until this is
/// dropped.
pub struct Screen {
    child: Child,
    pub panel: Panel,
}

/// A content hash of the two binaries, for deciding whether the tablet's copy
/// is current.
///
/// This was file sizes, with a comment claiming two builds would not collide
/// on both. The very next build of the agent changed a few instructions and
/// came out at exactly the same size, so the device kept running the old one
/// and the bug being fixed appeared to survive the fix. FNV-1a instead: it is
/// eight lines, needs no dependency, and — the part that matters — the hash
/// is computed *here*, over bytes already in memory, so the device still does
/// nothing but store a string.
pub(crate) fn stamp_of(agent: &[u8], shim: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in agent.iter().chain(shim) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}-{}-{}", agent.len(), shim.len())
}

/// Copy the agent and its shim to the tablet if what's there isn't current.
pub fn deploy(host: &str, port: u16, agent: &[u8], shim: &[u8]) -> Result<(), String> {
    let stamp = stamp_of(agent, shim);

    let out = ssh_base(host, port)
        .arg(format!("cat {REMOTE_DIR}/.stamp 2>/dev/null"))
        .output()
        .map_err(|e| format!("无法启动 ssh: {e}"))?;
    if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == stamp {
        return Ok(());
    }

    // Same framing trick as the notebook upload: one connection, and each
    // `dd` takes exactly its own file's bytes off the shared stdin.
    let script = format!(
        "set -e\n\
         mkdir -p {REMOTE_DIR}\n\
         dd bs={} count=1 iflag=fullblock of={REMOTE_DIR}/rmfb-agent\n\
         dd bs={} count=1 iflag=fullblock of={REMOTE_DIR}/librmfb.so\n\
         chmod +x {REMOTE_DIR}/rmfb-agent\n\
         printf %s '{stamp}' > {REMOTE_DIR}/.stamp\n",
        agent.len(),
        shim.len()
    );

    let mut child = ssh_base(host, port)
        .arg(script)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 ssh: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped");
    let wrote = stdin
        .write_all(agent)
        .and_then(|_| stdin.write_all(shim))
        .and_then(|_| stdin.flush())
        .is_ok();
    drop(stdin);

    let out = child.wait_with_output().map_err(|e| format!("ssh 失败：{e}"))?;
    if out.status.success() && wrote {
        Ok(())
    } else {
        Err(format!(
            "无法安装绘制程序：{}",
            crate::rm::device::remote_error(&out.stderr)
        ))
    }
}

impl Screen {
    /// Stop xochitl and start the agent. The tablet's own watchdog is armed
    /// first: whatever happens to this process, the ssh link or the Mac,
    /// `systemctl start xochitl` runs on the device after `HOLD_SECS`. A
    /// tablet that needs a laptop to come back is not an acceptable failure.
    pub fn open(host: &str, port: u16) -> Result<Screen, String> {
        let cmd = format!(
            "nohup sh -c 'sleep {HOLD_SECS}; systemctl start xochitl' >/dev/null 2>&1 &\n\
             systemctl stop xochitl 2>/dev/null\n\
             sleep 1\n\
             cd {REMOTE_DIR} && LD_LIBRARY_PATH={REMOTE_LD_PATH} exec ./rmfb-agent\n"
        );
        // stderr is kept rather than discarded: the vendor library narrates
        // its startup there (panel model, waveform table, pmic rails), and
        // when the handshake fails that narration is the only account of why.
        let mut child = ssh_base(host, port)
            .arg(cmd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("无法启动 ssh: {e}"))?;

        // The agent's first line reports the panel, so nothing here has to
        // hardcode 1620x2160 — a different tablet just says something else.
        let stdout = child.stdout.take().expect("piped");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let hello = reader.read_line(&mut line);
        let panel = match hello {
            Ok(n) if n > 0 => parse_hello(&line),
            _ => None,
        };
        let Some(panel) = panel else {
            // Closing stdin is what actually stops the agent — killing the
            // local ssh process leaves it running on the device with the
            // panel to itself and xochitl still stopped, which is how the
            // first attempt at this left a tablet showing nothing at all.
            drop(child.stdin.take());
            let why = child
                .wait_with_output()
                .map(|o| crate::rm::device::remote_error(&o.stderr))
                .unwrap_or_else(|e| e.to_string());
            // And give the tablet back: a failure here has already stopped
            // xochitl, so leaving is not an option.
            let _ = restore(host, port);
            return Err(format!("设备端绘制程序没有回应：{why}"));
        };

        // Put the reader back as the thing we read acks from.
        let child_out = reader.into_inner();
        let mut screen = Screen { child, panel };
        screen.child.stdout = Some(child_out);
        Ok(screen)
    }

    /// Wait for the agent to say the panel has finished. Reading the ack is
    /// also the back-pressure: without it we would push the whole image into
    /// the ssh socket and learn nothing about when any of it appeared.
    fn wait_ack(&mut self) -> Result<(), String> {
        let out = self.child.stdout.as_mut().ok_or("绘制程序已关闭")?;
        let mut ack = [0u8; 1];
        out.read_exact(&mut ack).map_err(|e| format!("设备无响应：{e}"))?;
        match ack[0] {
            b'.' => Ok(()),
            b'!' => Err("设备拒绝了这一块画面".into()),
            other => Err(format!("设备返回了意外的应答 {other}")),
        }
    }

    /// Paint one band and wait for it.
    #[allow(clippy::too_many_arguments)] // a rectangle, its pixels, and how to show them
    pub fn blit(&mut self, x: u32, y: u32, w: u32, h: u32, grey: &[u8], mode: u8, refresh: bool)
        -> Result<(), String>
    {
        debug_assert_eq!(grey.len(), (w * h) as usize);
        let stdin = self.child.stdin.as_mut().ok_or("绘制程序已关闭")?;
        stdin
            .write_all(&header(1, mode, refresh as u8, x, y, w, h))
            .and_then(|_| stdin.write_all(grey))
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("发送失败：{e}"))?;
        self.wait_ack()
    }

    /// Refresh a region already in the buffer — no pixels crossing the wire.
    pub fn refresh(&mut self, x: u32, y: u32, w: u32, h: u32, mode: u8) -> Result<(), String> {
        let stdin = self.child.stdin.as_mut().ok_or("绘制程序已关闭")?;
        stdin
            .write_all(&header(3, mode, 1, x, y, w, h))
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("发送失败：{e}"))?;
        self.wait_ack()
    }
}

impl Drop for Screen {
    /// Close stdin so the agent leaves its loop and the vendor library shuts
    /// down in its own order. xochitl is *not* restarted here: the picture is
    /// the point, and repainting the home screen over it a second later would
    /// be an odd thing to do. The device-side timer is what eventually gives
    /// the tablet back; `restore` is how to ask for it sooner.
    fn drop(&mut self) {
        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(&header(2, 0, 0, 0, 0, 0, 0));
            let _ = stdin.flush();
        }
        let _ = self.child.wait();
    }
}

/// Give the tablet's interface back now, rather than when the timer fires.
pub fn restore(host: &str, port: u16) -> Result<(), String> {
    let out = ssh_base(host, port)
        .arg("systemctl start xochitl")
        .output()
        .map_err(|e| format!("无法启动 ssh: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "无法恢复设备界面：{}",
            crate::rm::device::remote_error(&out.stderr)
        ))
    }
}

pub(crate) fn parse_hello(line: &str) -> Option<Panel> {
    let mut it = line.split_whitespace();
    if it.next()? != "RMFB" {
        return None;
    }
    Some(Panel {
        width: it.next()?.parse().ok()?,
        height: it.next()?.parse().ok()?,
        format: it.next().unwrap_or("0").parse().unwrap_or(0),
    })
}

/// Fit an image onto the panel: greyscale, scaled to fit whole, centered on
/// white.
///
/// Scaled to *fit* rather than fill — a photograph cropped by the panel's
/// aspect ratio is a worse outcome than one with paper showing at the sides,
/// and this is the path whose entire reason to exist is showing the picture
/// as it is.
pub fn fit(image_path: &str, panel: &Panel) -> Result<Vec<u8>, String> {
    use image::imageops::FilterType;
    use image::GenericImageView;

    let img = image::open(image_path).map_err(|e| format!("读不了这张图：{e}"))?;
    let (sw, sh) = img.dimensions();
    if sw == 0 || sh == 0 {
        return Err("这张图是空的".into());
    }

    let scale = (panel.width as f64 / sw as f64).min(panel.height as f64 / sh as f64);
    let dw = ((sw as f64 * scale).round() as u32).clamp(1, panel.width);
    let dh = ((sh as f64 * scale).round() as u32).clamp(1, panel.height);

    // Lanczos: this is the one path where the source detail survives all the
    // way to the panel, so the resample is worth paying for.
    let scaled = img.resize_exact(dw, dh, FilterType::Lanczos3).to_luma8();

    let mut out = vec![0xffu8; (panel.width * panel.height) as usize];
    let x0 = (panel.width - dw) / 2;
    let y0 = (panel.height - dh) / 2;
    for y in 0..dh {
        let dst = ((y0 + y) * panel.width + x0) as usize;
        let src = (y * dw) as usize;
        out[dst..dst + dw as usize].copy_from_slice(&scaled.as_raw()[src..src + dw as usize]);
    }
    Ok(out)
}

/// Send a full-panel image, band by band, reporting 0..1 as each band lands.
///
/// The bands go up in the fastest waveform and then the whole screen gets one
/// clean pass: painting each band at full quality would trade a slightly
/// better intermediate state for a much slower and flashier arrival, and the
/// intermediate state is not the one anybody keeps.
pub fn show(screen: &mut Screen, grey: &[u8], mut on_progress: impl FnMut(f64)) -> Result<(), String> {
    let (w, h) = (screen.panel.width, screen.panel.height);
    let band = h.div_ceil(BANDS).max(1);

    let mut y = 0;
    while y < h {
        let rows = band.min(h - y);
        let start = (y * w) as usize;
        let end = start + (rows * w) as usize;
        screen.blit(0, y, w, rows, &grey[start..end], MODE_FASTEST, false)?;
        y += rows;
        on_progress(y as f64 / h as f64);
    }

    // The settling pass: same pixels, full waveform, no ghosts.
    screen.refresh(0, 0, w, h, MODE_FULL)?;
    Ok(())
}

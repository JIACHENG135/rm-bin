// rmfb-agent — the dumb half.
//
// This runs on the tablet and does as little as possible: read a rectangle of
// 8-bit grey pixels from stdin, put them in the vendor framebuffer, ask the
// panel to show them. It decides nothing. Scaling, dithering, what a stroke
// looks like, how fast a drawing appears — all of that is Rust on the Mac,
// which is where it can be read, tested and changed without cross-compiling
// anything against a proprietary Qt.
//
// That split is deliberate. The one thing that genuinely has to live here is
// the C++ ABI boundary in ../shim, and it is the thing we least want to keep
// rebuilding: it depends on a closed library, a specific Qt, and a device we
// can only reach over ssh. So it is kept small and dumb enough to be finished.
//
// Protocol, little-endian, on stdin:
//
//     'R' 'M' 'F' 'B'   u8 op   u8 mode   u8 refresh  u8 pad
//     u16 x   u16 y   u16 w   u16 h
//     w*h bytes of 8-bit grey            (op = BLIT only)
//
// op 1 BLIT — paint the rect and swap.  op 2 QUIT — leave.
// op 3 SWAP — refresh a rect that is already in the buffer, sending no
//   pixels. This is what makes a progressive draw cheap: bands go up one at a
//   time in the fastest waveform, and the clean full-quality pass at the end
//   costs a header rather than another screenful.
// One byte per pixel rather than four: the wire is an ssh pipe over wifi, and
// a full screen is 3.4 MB of grey against 13 MB of RGB32.
//
// After each BLIT the agent writes one byte back ('.' for done, '!' for a
// rejected rect) so the sender can pace itself against real completions
// instead of guessing — the same idea as the pen path's paced writes, where
// what has been acknowledged is a decent proxy for what is on the paper.

#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "rmfb.h"

#define OP_BLIT 1
#define OP_QUIT 2
#define OP_SWAP 3

/// Read exactly `n` bytes or fail. A pipe hands over whatever has arrived, so
/// a single read() is not a read of a message.
static int read_exact(uint8_t *buf, size_t n) {
    size_t got = 0;
    while (got < n) {
        ssize_t r = read(STDIN_FILENO, buf + got, n - got);
        if (r <= 0) return -1;
        got += (size_t)r;
    }
    return 0;
}

static uint16_t le16(const uint8_t *p) { return (uint16_t)(p[0] | (p[1] << 8)); }

int main(void) {
    // The vendor library logs to *stdout* — panel model, waveform file, pmic
    // rails, all of it — and stdout here is the protocol channel. The first
    // version of this agent shared the two, so the sender read
    // "...panel tft: C0F" where it expected the handshake and gave up with
    // the screen already taken over.
    //
    // So take a private duplicate of the real pipe and point fd 1 at stderr.
    // After this the vendor can print whatever it likes: it goes to the ssh
    // session's stderr, which the caller discards, and it cannot reach the
    // one descriptor that carries meaning.
    int proto = dup(STDOUT_FILENO);
    if (proto < 0) {
        fprintf(stderr, "rmfb-agent: cannot duplicate stdout\n");
        return 1;
    }
    dup2(STDERR_FILENO, STDOUT_FILENO);

    if (rmfb_init(0) != RMFB_OK) {
        fprintf(stderr, "rmfb-agent: cannot open the framebuffer\n");
        return 1;
    }
    RmfbInfo info;
    uint8_t *fb = rmfb_buffer(&info);
    if (!fb) {
        fprintf(stderr, "rmfb-agent: no buffer\n");
        return 1;
    }
    // Announce the panel so the sender never has to hardcode it.
    char hello[64];
    int n = snprintf(hello, sizeof hello, "RMFB %d %d %d\n", info.width, info.height,
                     info.format);
    if (write(proto, hello, (size_t)n) != n) return 1;

    // One row of incoming grey, reused. The panel is 1620 wide; this is
    // generous so a future panel doesn't overflow it.
    static uint8_t row[8192];

    for (;;) {
        uint8_t hdr[16];
        if (read_exact(hdr, sizeof hdr)) break; // EOF: the sender is done
        if (memcmp(hdr, "RMFB", 4) != 0) {
            fprintf(stderr, "rmfb-agent: lost sync\n");
            return 1;
        }
        uint8_t op = hdr[4], mode = hdr[5], refresh = hdr[6];
        int x = le16(hdr + 8), y = le16(hdr + 10);
        int w = le16(hdr + 12), h = le16(hdr + 14);

        if (op == OP_QUIT) break;
        if (op != OP_BLIT && op != OP_SWAP) {
            fprintf(stderr, "rmfb-agent: unknown op %d\n", op);
            return 1;
        }

        int fits = x >= 0 && y >= 0 && w > 0 && h > 0 && (size_t)w <= sizeof row &&
                   x + w <= info.width && y + h <= info.height;

        for (int r = 0; op == OP_BLIT && r < h; r++) {
            if (read_exact(row, (size_t)w)) return 1; // always drain the payload
            if (!fits) continue;
            uint8_t *dst = fb + (long long)(y + r) * info.bytes_per_line;
            switch (info.format) {
                case 24: // Grayscale8
                    memcpy(dst + x, row, (size_t)w);
                    break;
                default: { // RGB32 and friends: grey on all three channels
                    uint8_t *p = dst + (long long)x * 4;
                    for (int i = 0; i < w; i++) {
                        uint8_t v = row[i];
                        p[0] = v; p[1] = v; p[2] = v; p[3] = 0xff;
                        p += 4;
                    }
                    break;
                }
            }
        }

        if (fits) rmfb_swap(x, y, w, h, RMFB_CONTENT_MONO, mode, refresh);
        // Acknowledge after the swap, so "acknowledged" means "on the panel".
        char ack = fits ? '.' : '!';
        if (write(proto, &ack, 1) != 1) return 1;
    }
    return 0;
}

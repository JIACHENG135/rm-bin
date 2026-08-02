// One-shot check that the whole route works: bring up the vendor
// framebuffer, report what it actually is, paint something unmistakable, and
// push it to the panel.
//
// It prints far more than it needs to because it runs on a device nobody can
// attach a debugger to, with xochitl stopped and the screen taken over — a
// run that fails should still say why.
//
// Deliberately not Rust. Everything it exercises is the C++ boundary itself;
// once this passes, the Rust side is four function calls and a byte buffer.

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <dlfcn.h>
#include <unistd.h>

#include "rmfb.h"

/// QImage::Format values that matter here.
static const char *format_name(int f) {
    switch (f) {
        case 3: return "Indexed8";
        case 4: return "RGB32";
        case 5: return "ARGB32";
        case 6: return "ARGB32_Premultiplied";
        case 7: return "RGB16";
        case 24: return "Grayscale8";
        default: return "?";
    }
}

/// Write one pixel as whatever the buffer's format wants. Only the two
/// formats the panel has ever been seen using are handled; anything else is
/// reported rather than guessed at, since a wrong guess paints garbage on a
/// screen we cannot see from here.
static bool put(uint8_t *row, int format, int x, uint8_t grey) {
    switch (format) {
        case 24: // Grayscale8
            row[x] = grey;
            return true;
        case 4:  // RGB32
        case 5:
        case 6: {
            uint8_t *p = row + x * 4;
            p[0] = grey; p[1] = grey; p[2] = grey; p[3] = 0xff;
            return true;
        }
        default:
            return false;
    }
}

/// Report whether a symbol resolves, without calling it. The interposition
/// only works if the real implementations can still be reached through
/// `RTLD_NEXT`, and Qt 6 versions its symbols (`@Qt_6`) — a lookup that comes
/// back null there is the difference between "this approach is wrong" and
/// "one name is misspelled", which is worth knowing before anything jumps to
/// an address.
static void check(const char *sym) {
    void *p = dlsym(RTLD_DEFAULT, sym);
    printf("[probe]   %-60s %s\n", sym, p ? "ok" : "MISSING");
}

int main(int argc, char **argv) {
    bool hold = argc > 1 && strcmp(argv[1], "--hold") == 0;

    printf("[probe] stage 0: symbol lookup\n");
    check("_ZN13EPFramebuffer8instanceEv");
    check("_ZN13EPFramebuffer11swapBuffersE5QRect13EPContentType12EPScreenMode6QFlagsINS_10UpdateFlagEE");
    check("_ZN6QImage4bitsEv");
    check("_ZNK6QImage12bytesPerLineEv");
    check("_ZN16QCoreApplicationC1ERiPPci");
    check("_ZN16QCoreApplication4selfE");
    fflush(stdout);

    printf("[probe] stage 1: QCoreApplication\n");
    fflush(stdout);
    int rc = rmfb_ensure_qapp();
    printf("[probe] stage 1 -> %d\n", rc);
    fflush(stdout);
    if (rc != RMFB_OK) return 1;

    printf("[probe] stage 2: rmfb_init (EPFramebuffer::instance)\n");
    fflush(stdout);
    rc = rmfb_init(1);
    if (rc != RMFB_OK) {
        printf("[probe] rmfb_init failed: %d\n", rc);
        return 1;
    }

    RmfbInfo info;
    uint8_t *bits = rmfb_buffer(&info);
    if (!bits) {
        printf("[probe] no buffer\n");
        return 1;
    }
    printf("[probe] buffer %p  %dx%d  bpl=%lld  format=%d (%s)\n", bits,
           info.width, info.height, info.bytes_per_line, info.format,
           format_name(info.format));
    fflush(stdout);

    // A pattern that cannot be mistaken for a coincidence: a black border, a
    // horizontal grey ramp, and a filled square a third of the way down. If
    // the geometry is wrong the border shows it; if the format is wrong the
    // ramp comes out as colour noise; if the buffer is the wrong one nothing
    // appears at all.
    const int w = info.width, h = info.height;
    const long long bpl = info.bytes_per_line;
    bool ok = true;
    for (int y = 0; y < h && ok; y++) {
        uint8_t *row = bits + (long long)y * bpl;
        for (int x = 0; x < w; x++) {
            uint8_t v = 0xff;
            bool border = x < 8 || y < 8 || x >= w - 8 || y >= h - 8;
            bool square = x > w / 4 && x < w * 3 / 4 && y > h / 3 && y < h / 3 + w / 2;
            if (border) v = 0x00;
            else if (square) v = 0x40;
            else if (y > h / 8 && y < h / 8 + h / 12) v = (uint8_t)(x * 255 / w);
            if (!put(row, info.format, x, v)) {
                printf("[probe] unhandled QImage format %d — not painting\n", info.format);
                ok = false;
                break;
            }
        }
    }
    if (!ok) return 1;

    printf("[probe] painted, swapping (full refresh)...\n");
    fflush(stdout);
    rmfb_swap(0, 0, w, h, RMFB_CONTENT_MONO, RMFB_QUALITY_FULL, 1);
    printf("[probe] swap returned\n");
    fflush(stdout);

    // The panel keeps the last image without power, so there is nothing to
    // hold open for — but while debugging it is useful to keep the process
    // (and whatever the vendor library set up) alive.
    if (hold) {
        printf("[probe] holding for 20s\n");
        fflush(stdout);
        sleep(20);
    }
    return 0;
}

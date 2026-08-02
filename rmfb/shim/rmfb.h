// The C ABI over `libqsgepaper.so`.
//
// Everything above this line is Rust. The C++ below it exists only because
// `EPFramebuffer::swapBuffers` takes Qt objects by value and reference, and
// Rust cannot construct a QRect or a QImage — those have layouts, vtables and
// destructors that only a C++ compiler knows. So this is the whole of the
// C++: a handful of functions that take integers and hand back a pointer to
// pixels.

#ifndef RMFB_H
#define RMFB_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define RMFB_OK 0
#define RMFB_ERR_NO_INSTANCE -1
#define RMFB_ERR_NO_BUFFER -2
#define RMFB_ERR_NO_QAPP -3

/// `QRect` as the ABI sees it: four ints, with right and bottom *inclusive*.
typedef struct {
    int x1, y1, x2, y2;
} QRectAbi;

/// Shape of the framebuffer. `format` is a `QImage::Format` value — 4 is
/// RGB32, 24 is Grayscale8 — and decides how pixels are written.
typedef struct {
    int width;
    int height;
    long long bytes_per_line;
    int format;
} RmfbInfo;

/// EPContentType.
#define RMFB_CONTENT_MONO 0
#define RMFB_CONTENT_COLOR 1

/// EPScreenMode. Fastest is what you want while something is being drawn
/// stroke by stroke; Full is the slow, clean, ghost-clearing refresh.
#define RMFB_QUALITY_FASTEST 0
#define RMFB_QUALITY_FAST 1
#define RMFB_QUALITY_3 3
#define RMFB_QUALITY_FULL 4
#define RMFB_QUALITY_5 5

/// The live `EPFramebuffer*`. Exposed so a caller can tell "initialised" from
/// "not" without keeping its own flag.
extern void *rmfb_instance;

/// Create the QCoreApplication the vendor code expects to exist, if there
/// isn't one. `rmfb_init` calls this; it is exposed so a caller can do it
/// earlier, and separately, when narrowing down where a crash happens.
int rmfb_ensure_qapp(void);

/// Bring up the vendor framebuffer and capture its image. Must be called
/// before anything else, and only once. Returns `RMFB_OK` or one of the
/// errors above.
int rmfb_init(int verbose_logging);

/// The pixels, and their shape. Valid until shutdown; not owned by the caller.
uint8_t *rmfb_buffer(RmfbInfo *out);

/// Push a rectangle of the buffer to the panel. `w`/`h` are a size, not
/// inclusive edges — the conversion happens inside.
int rmfb_swap(int x, int y, int w, int h, int content_type, int screen_mode,
              int full_refresh);

#ifdef __cplusplus
}
#endif

#endif

// Getting at the Paper Pro's e-ink framebuffer, by letting the vendor's own
// code hand it to us.
//
// The panel is not a framebuffer you can paint. `/dev/dri/card0` is real —
// imx-drm, LVDS, dumb buffers supported — but its mode is 405x1084, and the
// panel is 1620x2160. 405*4 == 1620 exactly: what is scanned out at 85Hz is
// packed *waveform* data, phase codes that drive each pixel through a
// multi-frame sequence, not grey levels. Reimplementing that is the months of
// reverse engineering everyone else declined to do too.
//
// `libqsgepaper.so` already does it. It is closed, undocumented, and exports
// `EPFramebuffer`, whose `swapBuffers` takes a rectangle and pushes whatever
// is in its internal QImage to the panel. So the only problem is getting a
// pointer to that QImage — and it has no accessor.
//
// The trick (asivery's, from epfb-re) is to make the vendor code tell us:
// interpose QImage's constructors, call `EPFramebuffer::instance()` with a
// flag set, and note every QImage that gets built while it runs. Two of them
// are the framebuffers.
//
// Interposition works here even though libqsgepaper's references are
// versioned (`@Qt_6`) and our definitions are not: the loader accepts an
// unversioned definition for a versioned reference, which is exactly how
// LD_PRELOAD interposes versioned glibc symbols. What it does need is for
// this object to come *first* in the lookup order — link it ahead of Qt.
//
// Nothing here includes a Qt header. Qt's headers are not available for the
// device's 6.8.2 and a mismatched set would inline the wrong struct layouts;
// every symbol below was instead read straight out of the device's own
// libQt6Gui.so.6.8.2 and libqsgepaper.so with `nm`, so the declarations
// describe that binary rather than some other build of Qt.

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <map>

#include "rmfb.h"

namespace {

// ————— Qt, as the device's binaries actually export it —————
//
// Non-virtual member functions in the Itanium ABI are plain functions taking
// `this` first, so they can be declared and called as C.

extern "C" {
// QImage::bits()
uint8_t *_ZN6QImage4bitsEv(void *self);
// QImage::width() / height() / bytesPerLine() / format(), all const
int _ZNK6QImage5widthEv(const void *self);
int _ZNK6QImage6heightEv(const void *self);
long long _ZNK6QImage12bytesPerLineEv(const void *self);
int _ZNK6QImage6formatEv(const void *self);

// EPFramebuffer::instance()
void *_ZN13EPFramebuffer8instanceEv();

// QCoreApplication::QCoreApplication(int &argc, char **argv, int flags)
//
// A reference is a pointer at the ABI, so `int&` is declared as `int*` here.
// `flags` is Qt's `ApplicationFlags`, which is just QT_VERSION — the library
// compares it against its own to catch a headers/runtime mismatch.
void _ZN16QCoreApplicationC1ERiPPci(void *self, int *argc, char **argv, int flags);

// `QCoreApplication::instance()` is `static inline { return self; }` in the
// header, so it is compiled away and never exported — the only thing to ask
// is the static member it reads.
extern void *_ZN16QCoreApplication4selfE;
// EPFramebuffer::swapBuffers(QRect, EPContentType, EPScreenMode,
//                            QFlags<UpdateFlag>)
//
// QRect is four ints — left, top, right, bottom, with right/bottom
// *inclusive*. The enums and QFlags are int-sized. Passed by value, which on
// AArch64 means registers; declaring the struct is enough for the compiler to
// match the ABI.
unsigned long _ZN13EPFramebuffer11swapBuffersE5QRect13EPContentType12EPScreenMode6QFlagsINS_10UpdateFlagEE(
    void *self, QRectAbi rect, int content_type, int screen_mode, int flags);
}

/// While this is set, every QImage the vendor code builds is recorded.
bool watching = false;

struct Seen {
    int width, height, format;
    long long bytes_per_line;
};
std::map<void *, Seen> seen;

/// The two the vendor code kept — identified after the fact by their shape,
/// not by construction order, which is not something to rely on.
void *aux_image = nullptr;
void *main_image = nullptr;

bool verbose = false;

} // namespace

// C linkage comes from the declaration in rmfb.h; repeating `extern "C"` on
// the definition would only warn about an initialised extern.
void *rmfb_instance = nullptr;

// ————— the interposed constructors —————
//
// Each forwards to the real implementation via RTLD_NEXT and then, only while
// `watching`, records what was built. `dlsym` is looked up once per symbol.

// Resolving to null and calling it is a segfault a long way from its cause,
// which on a tablet with the screen taken over is the worst possible place to
// debug. Every interposed function checks, says which symbol went missing,
// and returns — a QImage that was never constructed is survivable; a jump to
// address zero is not.
#define REAL(sym, ret, ...)                                                    \
    static ret (*real)(__VA_ARGS__) = nullptr;                                 \
    static bool tried = false;                                                 \
    if (!tried) {                                                              \
        tried = true;                                                          \
        real = reinterpret_cast<ret (*)(__VA_ARGS__)>(dlsym(RTLD_NEXT, sym));  \
        if (!real) fprintf(stderr, "[rmfb] dlsym(RTLD_NEXT, %s) failed\n", sym); \
    }                                                                          \
    if (!real) return

extern "C" void _ZN6QImageC1EPhiixNS_6FormatEPFvPvES2_(void *self, uint8_t *data,
                                                       int w, int h, long long bpl,
                                                       int format, void *cleanup,
                                                       void *cleanup_info) {
    REAL("_ZN6QImageC1EPhiixNS_6FormatEPFvPvES2_", void, void *, uint8_t *, int,
         int, long long, int, void *, void *);
    real(self, data, w, h, bpl, format, cleanup, cleanup_info);
    if (!watching) return;
    seen[self] = Seen{w, h, format, bpl};
    if (verbose)
        fprintf(stderr, "[rmfb] QImage(data) %p  %dx%d  bpl=%lld  fmt=%d\n", self,
                w, h, bpl, format);
}

extern "C" void _ZN6QImageC1ERKS_(void *self, void *other) {
    REAL("_ZN6QImageC1ERKS_", void, void *, void *);
    real(self, other);
    if (!watching) return;
    // A framebuffer that gets copied is still that framebuffer: QImage is
    // implicitly shared, so the copy points at the same pixels.
    auto it = seen.find(other);
    if (it != seen.end()) {
        seen[self] = it->second;
        if (verbose) fprintf(stderr, "[rmfb] QImage copy %p <- %p\n", self, other);
    }
}

extern "C" void _ZN6QImageaSERKS_(void *self, void *other) {
    REAL("_ZN6QImageaSERKS_", void, void *, void *);
    real(self, other);
    if (!watching) return;
    auto it = seen.find(other);
    if (it != seen.end()) seen[self] = it->second;
}

extern "C" void _ZN6QImageD1Ev(void *self) {
    REAL("_ZN6QImageD1Ev", void, void *);
    real(self);
    // Drop it whether or not we are watching: a pointer that has been
    // destroyed must never be handed out later, and addresses get reused.
    if (!seen.empty()) seen.erase(self);
}

// ————— the C ABI —————

/// Storage for the QCoreApplication. `QCoreApplication` adds no data members
/// to `QObject`, which is a vptr plus a d-pointer, so 16 bytes is the real
/// size — this is generous rather than clever, because the exact figure is
/// not something we can include a header to learn.
static long long app_storage[32];
static int app_argc = 1;
static char app_arg0[] = "rmfb";
static char *app_argv[] = {app_arg0, nullptr};

/// QT_VERSION for the device's Qt (6.8.2). QCoreApplication's constructor
/// takes this and compares it with the version it was built against.
static const int QT_VERSION_6_8_2 = 0x060802;

extern "C" int rmfb_ensure_qapp(void) {
    if (_ZN16QCoreApplication4selfE) return RMFB_OK;
    // EPFramebuffer is a QObject and expects to be created with an
    // application object in existence — without one it dereferences a null
    // QCoreApplication and takes the process with it.
    _ZN16QCoreApplicationC1ERiPPci(app_storage, &app_argc, app_argv,
                                   QT_VERSION_6_8_2);
    return _ZN16QCoreApplication4selfE ? RMFB_OK : RMFB_ERR_NO_QAPP;
}

extern "C" int rmfb_init(int verbose_logging) {
    verbose = verbose_logging != 0;

    if (int rc = rmfb_ensure_qapp()) {
        fprintf(stderr, "[rmfb] no QCoreApplication\n");
        return rc;
    }

    watching = true;
    void *fb = _ZN13EPFramebuffer8instanceEv();
    watching = false;

    if (!fb) {
        fprintf(stderr, "[rmfb] EPFramebuffer::instance() returned null\n");
        return RMFB_ERR_NO_INSTANCE;
    }

    // On this firmware the vendor keeps two panel-sized images and they are
    // told apart by format, not by size: the one we paint is colour (RGB32 on
    // the Gallery 3 panel, RGB16 on earlier ones), and the other is the
    // Grayscale8 copy the library renders from. Sizes are identical —
    // 1620x2160 both — so anything that sorts by area is deciding by map
    // order, which is to say by luck.
    for (auto &kv : seen) {
        switch (kv.second.format) {
            case 4: // RGB32
            case 5: // ARGB32
            case 6: // ARGB32_Premultiplied
            case 7: // RGB16
                if (!aux_image) aux_image = kv.first;
                break;
            case 24: // Grayscale8
                if (!main_image) main_image = kv.first;
                break;
            default:
                break;
        }
    }

    if (verbose) {
        fprintf(stderr, "[rmfb] instance=%p, %zu image(s) survived\n", fb, seen.size());
        for (auto &kv : seen)
            fprintf(stderr, "[rmfb]   %p  %dx%d  bpl=%lld  fmt=%d%s\n", kv.first,
                    kv.second.width, kv.second.height, kv.second.bytes_per_line,
                    kv.second.format, kv.first == aux_image ? "   <- aux" : "");
    }
    if (!aux_image) {
        fprintf(stderr, "[rmfb] no framebuffer image was constructed\n");
        return RMFB_ERR_NO_BUFFER;
    }

    rmfb_instance = fb;
    return RMFB_OK;
}

extern "C" uint8_t *rmfb_buffer(RmfbInfo *out) {
    if (!aux_image) return nullptr;
    // bits() detaches, but a QImage wrapping foreign memory with a single
    // reference has nothing to detach from, so this is the real pixels.
    uint8_t *bits = _ZN6QImage4bitsEv(aux_image);
    if (out) {
        out->width = _ZNK6QImage5widthEv(aux_image);
        out->height = _ZNK6QImage6heightEv(aux_image);
        out->bytes_per_line = _ZNK6QImage12bytesPerLineEv(aux_image);
        out->format = _ZNK6QImage6formatEv(aux_image);
    }
    return bits;
}

extern "C" int rmfb_swap(int x, int y, int w, int h, int content_type,
                         int screen_mode, int full_refresh) {
    if (!rmfb_instance) return RMFB_ERR_NO_INSTANCE;
    // QRect's right and bottom edges are inclusive — an off-by-one here shows
    // up as a one-pixel column of stale ink down the edge of every update.
    QRectAbi rect{x, y, x + w - 1, y + h - 1};
    _ZN13EPFramebuffer11swapBuffersE5QRect13EPContentType12EPScreenMode6QFlagsINS_10UpdateFlagEE(
        rmfb_instance, rect, content_type, screen_mode, full_refresh ? 1 : 0);
    return RMFB_OK;
}

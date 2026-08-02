#!/usr/bin/env bash
#
# Build the on-device half: librmfb.so (the C++ ABI boundary) and rmfb-agent.
#
# Needs an aarch64 Linux host with g++, and the three libraries this links
# against, which are *the device's own* and are not redistributable:
#
#     /usr/lib/plugins/scenegraph/libqsgepaper.so
#     /usr/lib/libQt6Core.so.6.8.2
#     /usr/lib/libQt6Gui.so.6.8.2
#
# Fetch them from your tablet first:
#
#     ./build.sh --pull root@10.11.99.1
#
# Compiling on an older glibc than the device's (2.35 vs 2.39) is the safe
# direction; libstdc++ is linked statically so the device's version never
# matters. Qt headers are deliberately not used — see shim/epfb.cpp.
set -euo pipefail
cd "$(dirname "$0")"

LIBS=vendor
OUT=dist

if [[ "${1:-}" == "--pull" ]]; then
    host="${2:?usage: build.sh --pull root@10.11.99.1}"
    mkdir -p "$LIBS"
    scp -O "$host:/usr/lib/plugins/scenegraph/libqsgepaper.so" \
           "$host:/usr/lib/libQt6Core.so.6.8.2" \
           "$host:/usr/lib/libQt6Gui.so.6.8.2" "$LIBS/"
    echo "pulled into $LIBS/"
    exit 0
fi

for f in libqsgepaper.so libQt6Core.so.6.8.2 libQt6Gui.so.6.8.2; do
    [[ -f "$LIBS/$f" ]] || { echo "missing $LIBS/$f — run: $0 --pull root@<device>" >&2; exit 1; }
done

mkdir -p "$OUT"
# SONAME-shaped names, so the linker finds them by -l and the loader by NEEDED
ln -sf libQt6Core.so.6.8.2 "$LIBS/libQt6Core.so"
ln -sf libQt6Gui.so.6.8.2 "$LIBS/libQt6Gui.so"
ln -sf libQt6Core.so.6.8.2 "$LIBS/libQt6Core.so.6"
ln -sf libQt6Gui.so.6.8.2 "$LIBS/libQt6Gui.so.6"

g++ -std=c++17 -O2 -fPIC -shared -Ishim -o "$OUT/librmfb.so" shim/epfb.cpp \
    -L"$LIBS" -lqsgepaper -lQt6Gui -lQt6Core -ldl \
    -static-libstdc++ -static-libgcc

# librmfb must come first so its QImage definitions win the lookup — that is
# the whole interposition. The device has Qt6Quick/Qml/DBus/icu; we carry only
# what we link against, so transitive references are left to the loader.
common=(-L"$LIBS" -lrmfb -lqsgepaper -lQt6Gui -lQt6Core -ldl
        -static-libstdc++ -static-libgcc
        -Wl,--allow-shlib-undefined -Wl,--unresolved-symbols=ignore-in-shared-libs
        -Wl,-rpath,'$ORIGIN')

gcc -std=c11 -O2 -Ishim -o "$OUT/rmfb-agent" agent/main.c \
    -L"$OUT" "${common[@]}" 2>/dev/null ||
gcc -std=c11 -O2 -Ishim -o "$OUT/rmfb-agent" agent/main.c \
    -L"$OUT" -L"$LIBS" -lrmfb -lqsgepaper -lQt6Gui -lQt6Core -ldl \
    -Wl,--allow-shlib-undefined -Wl,--unresolved-symbols=ignore-in-shared-libs \
    -Wl,-rpath,'$ORIGIN'

g++ -std=c++17 -O2 -Ishim -o "$OUT/rmfb-probe" shim/probe.cpp \
    -L"$OUT" -L"$LIBS" -lrmfb -lqsgepaper -lQt6Gui -lQt6Core -ldl \
    -static-libstdc++ -static-libgcc \
    -Wl,--allow-shlib-undefined -Wl,--unresolved-symbols=ignore-in-shared-libs \
    -Wl,-rpath,'$ORIGIN'

ls -l "$OUT"

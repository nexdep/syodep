#!/usr/bin/env bash
# Validate libraries intentionally supplied by the host rather than AppImage.
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 APPDIR" >&2
    exit 2
fi

appdir=$(realpath "$1")
executable="$appdir/usr/bin/syodep"

if [[ ! -x "$executable" ]]; then
    echo "syodep executable is missing: $executable" >&2
    exit 1
fi

bundled_xkbcommon=$(find "$appdir" \( -type f -o -type l \) \
    \( -name 'libxkbcommon.so*' -o -name 'libxkbcommon-x11.so*' \) \
    -print -quit)
if [[ -n "$bundled_xkbcommon" ]]; then
    echo "xkbcommon must resolve from the host, but the AppDir bundles: $bundled_xkbcommon" >&2
    exit 1
fi

ldd_output=$(ldd "$executable")
if grep -q 'not found' <<< "$ldd_output"; then
    printf '%s\n' "$ldd_output" >&2
    echo "syodep has unresolved dynamic-library dependencies" >&2
    exit 1
fi

resolved_xkbcommon=$(awk '$1 == "libxkbcommon.so.0" { print $3; exit }' \
    <<< "$ldd_output")
if [[ -z "$resolved_xkbcommon" || ! -e "$resolved_xkbcommon" ]]; then
    echo "libxkbcommon.so.0 did not resolve from the host" >&2
    exit 1
fi

resolved_xkbcommon=$(realpath "$resolved_xkbcommon")
case "$resolved_xkbcommon" in
    "$appdir"/*)
        echo "libxkbcommon.so.0 resolved inside the AppDir: $resolved_xkbcommon" >&2
        exit 1
        ;;
esac

echo "libxkbcommon.so.0 resolves from the host: $resolved_xkbcommon"

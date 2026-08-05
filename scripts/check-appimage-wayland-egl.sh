#!/usr/bin/env bash
# Validate that an extracted syodep AppDir can load Qt's nested Wayland EGL
# client integration from the bundle itself, without help from host Qt libs.
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 APPDIR" >&2
    exit 2
fi

if ! command -v patchelf >/dev/null 2>&1; then
    echo "patchelf is required to inspect the Wayland EGL plugin RUNPATH" >&2
    exit 2
fi

appdir=$(realpath "$1")
platform_dir=$(find "$appdir" -type d -path '*/plugins/platforms' -print -quit)
if [[ -z "$platform_dir" ]]; then
    echo "Qt platform plugin directory is missing from $appdir" >&2
    exit 1
fi

plugin_root=$(dirname "$platform_dir")
wayland_egl="$plugin_root/wayland-graphics-integration-client/libqt-plugin-wayland-egl.so"
client_name=libQt6WaylandEglClientHwIntegration.so.6
client_lib="$appdir/usr/lib/$client_name"

if [[ ! -f "$wayland_egl" ]]; then
    echo "Wayland EGL client integration plugin is missing: $wayland_egl" >&2
    exit 1
fi
if [[ ! -e "$client_lib" ]]; then
    echo "Wayland EGL client library is missing: $client_lib" >&2
    exit 1
fi

runpath=$(patchelf --print-rpath "$wayland_egl")
expected_runpath='$ORIGIN/../../lib'
runpath_found=false
IFS=: read -ra runpath_entries <<< "$runpath"
for entry in "${runpath_entries[@]}"; do
    if [[ "$entry" == "$expected_runpath" ]]; then
        runpath_found=true
        break
    fi
done
if [[ "$runpath_found" != true ]]; then
    echo "Wayland EGL plugin RUNPATH does not contain $expected_runpath: $runpath" >&2
    exit 1
fi

ldd_output=$(ldd "$wayland_egl")
printf '%s\n' "$ldd_output"
if grep -q 'not found' <<< "$ldd_output"; then
    echo "Wayland EGL client integration has unresolved dependencies" >&2
    exit 1
fi

resolved_client=$(awk -v name="$client_name" '$1 == name { print $3; exit }' <<< "$ldd_output")
if [[ -z "$resolved_client" ]]; then
    echo "$client_name was not found in the plugin dependency list" >&2
    exit 1
fi
if [[ $(realpath "$resolved_client") != $(realpath "$client_lib") ]]; then
    echo "$client_name resolved outside the AppDir: $resolved_client" >&2
    exit 1
fi

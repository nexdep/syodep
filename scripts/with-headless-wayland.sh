#!/usr/bin/env bash
# Run a command under an isolated headless Weston compositor. CI uses this
# instead of Xvfb/offscreen so Linux shell tests exercise the shipped Wayland
# path. Mesa software GL keeps the OpenGL run deterministic on GPU-less hosts.
set -euo pipefail

if [[ $# -eq 0 ]]; then
    echo "usage: $0 COMMAND [ARG ...]" >&2
    exit 2
fi

runtime_dir=$(mktemp -d)
weston_log="$runtime_dir/weston.log"
chmod 700 "$runtime_dir"

export XDG_RUNTIME_DIR="$runtime_dir"
export WAYLAND_DISPLAY=syodep-ci-wayland
export LIBGL_ALWAYS_SOFTWARE=1
unset DISPLAY QT_QPA_PLATFORM QT_OPENGL

if weston --help 2>&1 | grep -q -- '--renderer'; then
    weston_renderer=(--renderer=gl)
else
    weston_renderer=(--use-gl)
fi

weston --backend=headless-backend.so \
    "${weston_renderer[@]}" \
    --socket="$WAYLAND_DISPLAY" \
    --idle-time=0 \
    --no-config \
    --log="$weston_log" &
weston_pid=$!

cleanup() {
    kill "$weston_pid" 2>/dev/null || true
    wait "$weston_pid" 2>/dev/null || true
    rm -rf -- "$runtime_dir"
}
trap cleanup EXIT

# Cold llvmpipe startup can cross five seconds on a fresh CI runner. Keep the
# wait bounded, but leave enough headroom for shader/driver initialization.
for _ in {1..300}; do
    if [[ -S "$runtime_dir/$WAYLAND_DISPLAY" ]]; then
        break
    fi
    if ! kill -0 "$weston_pid" 2>/dev/null; then
        echo "Weston exited before creating its Wayland socket" >&2
        sed -n '1,240p' "$weston_log" >&2
        exit 1
    fi
    sleep 0.05
done

if [[ ! -S "$runtime_dir/$WAYLAND_DISPLAY" ]]; then
    echo "Weston did not create its Wayland socket" >&2
    sed -n '1,240p' "$weston_log" >&2
    exit 1
fi

set +e
"$@"
status=$?
set -e
if [[ $status -ne 0 ]]; then
    echo "--- weston.log ---" >&2
    sed -n '1,240p' "$weston_log" >&2
    exit "$status"
fi

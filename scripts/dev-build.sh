#!/usr/bin/env bash
# Incrementally build and launch a local development copy of syodep.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo_root=$(cd -- "$script_dir/.." && pwd -P)
build_dir="$repo_root/build/dev"
fixture="$build_dir/dev-fixture.pdf"
executable="$build_dir/ui-qt/syodep"

usage() {
    cat <<'EOF'
usage: scripts/dev-build.sh [PDF]
       scripts/dev-build.sh --build-only
       scripts/dev-build.sh --smoke

With no arguments, build syodep, generate a five-page PDF fixture, and open it.
Pass a PDF to open that document instead. --smoke runs the OpenGL and raster
headless smoke tests after building.
EOF
}

mode=launch
document=

if [[ $# -gt 1 ]]; then
    usage >&2
    exit 2
fi

if [[ $# -eq 1 ]]; then
    case $1 in
        --build-only)
            mode=build-only
            ;;
        --smoke)
            mode=smoke
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --*)
            echo "unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
        *)
            if [[ ! -f $1 ]]; then
                echo "PDF does not exist: $1" >&2
                exit 2
            fi
            document=$(cd -- "$(dirname -- "$1")" && pwd -P)/$(basename -- "$1")
            ;;
    esac
fi

for command in cargo cmake ninja; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "required command not found: $command" >&2
        exit 127
    fi
done

cmake -S "$repo_root" -B "$build_dir" -G Ninja \
    -DCMAKE_BUILD_TYPE=Debug \
    -DSYODEP_RUST_PROFILE=dev \
    -DSYODEP_BUILD_CHANNEL=development
cmake --build "$build_dir"

if [[ $mode == build-only ]]; then
    echo "Development build ready: $executable"
    exit 0
fi

if [[ -z $document ]]; then
    cargo run --manifest-path "$repo_root/Cargo.toml" \
        -p syodep-pdf --features test-support --example make_fixture -- \
        "$fixture" 5
    document=$fixture
fi

# Keep files produced relative to the process working directory inside /build,
# which is ignored by Git even if a launch or smoke test exits unexpectedly.
cd -- "$build_dir"

if [[ $mode == smoke ]]; then
    if ! command -v weston >/dev/null 2>&1; then
        echo "required command not found for --smoke: weston" >&2
        exit 127
    fi
    bash "$repo_root/scripts/with-headless-wayland.sh" \
        "$executable" --renderer=opengl --smoke-test "$document"
    bash "$repo_root/scripts/with-headless-wayland.sh" \
        "$executable" --renderer=raster --smoke-test "$document"
    echo "OpenGL and raster smoke tests passed"
    exit 0
fi

exec "$executable" "$document"

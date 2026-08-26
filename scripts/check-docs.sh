#!/usr/bin/env bash
# Documentation presence/consistency checks, run by CI (docs job).
#
# 1. All required documentation files exist and are non-empty.
# 2. Every command in the core's command registry is documented.
# 3. Every default keybinding is documented.
# 4. Every config option is documented.
# 5. The AppImage build/runtime baseline is consistent across workflow and docs.
# 6. Workflow artifact retention is explicit and documented.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
err() {
    echo "DOCS CHECK FAILED: $*" >&2
    fail=1
}

required_docs=(
    README.md
    docs/architecture.md
    docs/development-log.md
    docs/commands.md
    docs/commands-normal-mode.md
    docs/commands-focus-mode.md
    docs/commands-visual-mode.md
    docs/commands-highlight-mode.md
    docs/keybindings.md
    docs/config.md
    docs/testing.md
    docs/packaging.md
    docs/roadmap.md
)
for doc in "${required_docs[@]}"; do
    [ -s "$doc" ] || err "missing or empty: $doc"
done

# Every command name in ALL_COMMANDS must appear on a per-mode commands page
# (docs/commands.md is the index; the tables live in the per-mode pages).
command_docs=(docs/commands-normal-mode.md docs/commands-focus-mode.md \
    docs/commands-visual-mode.md docs/commands-highlight-mode.md)
while IFS= read -r command; do
    grep -q "\`$command\`" "${command_docs[@]}" || err "command not documented: $command"
done < <(grep -oP '^\s*\("\K[a-z0-9_]+(?=",)' crates/syodep-core/src/command.rs)

# Every default binding's command in default_keybindings() must appear in
# docs/keybindings.md (key syntax itself is too fiddly to grep literally).
while IFS= read -r binding; do
    grep -qF "$binding" docs/keybindings.md || err "default keybinding not documented: $binding"
done < <(grep -oP '^\s*\("\K[^"]+(?=", ")' crates/syodep-config/src/lib.rs)

# Every focus-mode binding's command (default_focus_keybindings) must appear in
# docs/keybindings.md, so focus mode stays documented. One block covers every
# scope, because one keymap does.
while IFS= read -r command; do
    grep -q "\`$command\`" docs/keybindings.md \
        || err "focus keybinding command not documented: $command"
done < <(awk '/pub fn default_focus_keybindings/,/^}/' crates/syodep-config/src/lib.rs \
    | grep -oP '", "\K[a-z_]+(?="\))')

# Likewise for visual-mode bindings (default_visual_keybindings).
while IFS= read -r command; do
    grep -q "\`$command\`" docs/keybindings.md \
        || err "visual keybinding command not documented: $command"
done < <(awk '/pub fn default_visual_keybindings/,/^}/' crates/syodep-config/src/lib.rs \
    | grep -oP '", "\K[a-z_]+(?="\))')

# Likewise for highlight-mode bindings (default_highlight_keybindings).
while IFS= read -r command; do
    grep -q "\`$command\`" docs/keybindings.md \
        || err "highlight keybinding command not documented: $command"
done < <(awk '/pub fn default_highlight_keybindings/,/^}/' crates/syodep-config/src/lib.rs \
    | grep -oP '", "\K[a-z_]+(?="\))')

# Every [view], [window] and [input] config field must appear in docs/config.md.
for section in ViewConfig WindowConfig InputConfig; do
    while IFS= read -r option; do
        grep -q "\`$option\`" docs/config.md \
            || err "config option not documented: $option"
    done < <(awk "/pub struct $section/,/^}/" crates/syodep-config/src/lib.rs \
        | grep -oP '^\s*pub \K[a-z_]+(?=:)')
done

# Cargo.toml holds the only manually maintained base version. CMake derives a
# channel-aware build identity from it and injects that exact value into both
# the shell and core. These checks guard that construction rather than compare
# redundant version copies.
grep -q 'project(syodep VERSION \${SYODEP_VERSION_NUMERIC}' CMakeLists.txt \
    || err "CMakeLists.txt must derive the version from Cargo.toml, not hardcode it"

grep -q 'setApplicationVersion(QStringLiteral(SYODEP_BUILD_VERSION))' ui-qt/src/main.cpp \
    || err "ui-qt must report SYODEP_BUILD_VERSION, not a hardcoded version string"

grep -q 'option_env!("SYODEP_BUILD_VERSION")' crates/syodep-ffi/src/lib.rs \
    || err "syodep-ffi must report the injected build identity"

# The AppImage build userland determines the package's glibc floor. A distro
# bump must update the cache boundary and every active compatibility statement
# in the same change.
grep -qF 'container: ubuntu:24.04' .github/workflows/appimage.yml \
    || err "AppImage workflow must use the documented Ubuntu 24.04 container"
grep -qF 'shared-key: appimage-ubuntu-24.04' .github/workflows/appimage.yml \
    || err "AppImage cache key must identify the Ubuntu 24.04 userland"
grep -qF 'glibc ≥ 2.39' README.md \
    || err "README must document the AppImage glibc 2.39 floor"
grep -qF '`ubuntu:24.04` container' docs/packaging.md \
    || err "packaging docs must document the Ubuntu 24.04 AppImage container"
grep -qF 'glibc 2.39' docs/packaging.md \
    || err "packaging docs must document the AppImage glibc 2.39 floor"
grep -qF 'ubuntu:24.04 container build' docs/roadmap.md \
    || err "roadmap must record the Ubuntu 24.04 AppImage baseline"

# Workflow artifacts are transfers/debugging aids; release assets are the
# durable downloads. Keep the short-retention policy and conditional Windows
# installer upload synchronized with the packaging documentation.
retention_expression="retention-days: \${{ github.ref == 'refs/heads/main' && 3 || 7 }}"
grep -qF "$retention_expression" .github/workflows/ci.yml \
    || err "CI artifact must use the documented 3/7-day retention policy"
grep -qF "$retention_expression" .github/workflows/appimage.yml \
    || err "AppImage artifact must use the documented 3/7-day retention policy"
grep -qF 'name: Upload continuous Windows transfer artifact' .github/workflows/release.yml \
    || err "release workflow must have a main-only Windows transfer upload"
grep -qF 'name: Upload release or development Windows artifact' .github/workflows/release.yml \
    || err "release workflow must preserve tag/manual installer artifacts"
grep -qF 'retention-days: 3' .github/workflows/release.yml \
    || err "main Windows transfer artifact must be retained for 3 days"
grep -qF 'retention-days: 7' .github/workflows/release.yml \
    || err "tag/manual Windows artifacts must be retained for 7 days"
grep -qF 'a `main` push are retained for 3 days.' docs/packaging.md \
    || err "packaging docs must explain main artifact retention"
grep -qF 'Branch, tag, and manual-run artifacts' docs/packaging.md \
    || err "packaging docs must explain non-main artifact retention"

# The installer script is a shipped artifact source, not a doc, but losing it
# would silently drop the Windows installer from releases.
[ -s packaging/syodep.nsi ] || err "missing or empty: packaging/syodep.nsi"
grep -q "NSIS" docs/packaging.md \
    || err "docs/packaging.md no longer documents the NSIS installer"

if [ "$fail" -eq 0 ]; then
    echo "docs check OK"
fi
exit "$fail"

#!/usr/bin/env bash
# Documentation presence/consistency checks, run by CI (docs job).
#
# 1. All required documentation files exist and are non-empty.
# 2. Every command in the core's command registry is documented.
# 3. Every default keybinding is documented.
# 4. Every config option is documented.
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
    docs/commands-caret-focus-mode.md
    docs/commands-line-focus-mode.md
    docs/commands-word-focus-mode.md
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
command_docs=(docs/commands-normal-mode.md docs/commands-caret-focus-mode.md docs/commands-line-focus-mode.md docs/commands-word-focus-mode.md)
while IFS= read -r command; do
    grep -q "\`$command\`" "${command_docs[@]}" || err "command not documented: $command"
done < <(grep -oP '^\s*\("\K[a-z0-9_]+(?=",)' crates/syodep-core/src/command.rs)

# Every default binding's command in default_keybindings() must appear in
# docs/keybindings.md (key syntax itself is too fiddly to grep literally).
while IFS= read -r binding; do
    grep -qF "$binding" docs/keybindings.md || err "default keybinding not documented: $binding"
done < <(grep -oP '^\s*\("\K[^"]+(?=", ")' crates/syodep-config/src/lib.rs)

# Every caret-focus binding's command (default_caret_focus_keybindings) must
# appear in docs/keybindings.md, so caret focus mode stays documented.
while IFS= read -r command; do
    grep -q "\`$command\`" docs/keybindings.md \
        || err "caret keybinding command not documented: $command"
done < <(awk '/pub fn default_caret_focus_keybindings/,/^}/' crates/syodep-config/src/lib.rs \
    | grep -oP '", "\K[a-z_]+(?="\))')

# Likewise for line-focus bindings (default_line_focus_keybindings).
while IFS= read -r command; do
    grep -q "\`$command\`" docs/keybindings.md \
        || err "line keybinding command not documented: $command"
done < <(awk '/pub fn default_line_focus_keybindings/,/^}/' crates/syodep-config/src/lib.rs \
    | grep -oP '", "\K[a-z_]+(?="\))')

# Likewise for word-focus bindings (default_word_focus_keybindings).
while IFS= read -r command; do
    grep -q "\`$command\`" docs/keybindings.md \
        || err "word keybinding command not documented: $command"
done < <(awk '/pub fn default_word_focus_keybindings/,/^}/' crates/syodep-config/src/lib.rs \
    | grep -oP '", "\K[a-z_]+(?="\))')

# Every [view] config field must appear in docs/config.md.
while IFS= read -r option; do
    grep -q "\`$option\`" docs/config.md || err "config option not documented: $option"
done < <(awk '/pub struct ViewConfig/,/^}/' crates/syodep-config/src/lib.rs \
    | grep -oP '^\s*pub \K[a-z_]+(?=:)')

if [ "$fail" -eq 0 ]; then
    echo "docs check OK"
fi
exit "$fail"

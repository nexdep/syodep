# syodep

A keyboard-first, Vim-like academic PDF reader and annotation tool for
Windows and Linux. Inspired by [Sioyek](https://github.com/ahrm/sioyek)
as a product, built clean-room on a different architecture.

The core loop syodep is built around:

> Open PDF → read with keyboard → move quickly through text → select text →
> highlight → annotate → search/retrieve notes → export annotations.

## Status

Milestone 1 (MVP foundation) is complete:

- open a local PDF (CLI argument, `o` key, or file dialog)
- continuous scrollable rendering of the entire document (MuPDF)
- Vim-like keyboard navigation with count prefixes: `j`/`k`/`h`/`l`,
  `J`/`K` (pages), `<C-d>`/`<C-u>`/`<C-f>`/`<C-b>`, `gg`/`G`/`{n}G`
- zoom: `+`/`-`, `zw` fit-width, `z0` reset
- TOML config with user keybindings
- SQLite persistence; last reading position restored per document
- text extraction API (foundation for selection/search in phase 2)

See `docs/roadmap.md` for what comes next.

## Installing

### Windows (Scoop)

```powershell
scoop bucket add syodep https://github.com/nexdep/syodep
scoop install syodep
```

### Windows (portable zip)

Download `syodep-vX.Y.Z-win64.zip` from the
[releases page](https://github.com/nexdep/syodep/releases), unzip
anywhere, run `syodep.exe`. No installation required.

On Linux, build from source for now (AppImage planned, see
`docs/packaging.md`).

## Building

Requirements: Rust (stable), CMake ≥ 3.21, Ninja (recommended), Qt 6
(Widgets + OpenGLWidgets), a C/C++ toolchain. MuPDF and SQLite are built
from source by cargo automatically (no system packages needed for them).

Debian/Ubuntu packages:

```bash
sudo apt install build-essential cmake ninja-build qt6-base-dev \
    libqt6opengl6-dev libgl1-mesa-dev libfontconfig1-dev libfreetype-dev \
    clang libclang-dev
```

Build and run:

```bash
cmake -B build -G Ninja
cmake --build build
./build/ui-qt/syodep path/to/document.pdf
```

The Rust core builds and tests standalone, without Qt:

```bash
cargo test --workspace
```

## Configuration

`~/.config/syodep/config.toml` (Linux) or `%APPDATA%\syodep\config.toml`
(Windows). A documented sample lives at `config/default-config.toml`.
Reference: `docs/config.md`, `docs/keybindings.md`, `docs/commands.md`.

User data (reading positions, and later bookmarks/highlights/notes) is
stored in SQLite at `~/.local/share/syodep/syodep.sqlite3` (Linux) or
`%APPDATA%\syodep\syodep.sqlite3` (Windows). It is never stored in TOML.

## Repository layout

| Path                    | Purpose                                              |
| ----------------------- | ---------------------------------------------------- |
| `crates/syodep-core`    | UI-independent app core: state, commands, input, layout |
| `crates/syodep-pdf`     | Safe PDF backend wrapping MuPDF                      |
| `crates/syodep-storage` | SQLite persistence + migrations                      |
| `crates/syodep-config`  | TOML config + key chord syntax                       |
| `crates/syodep-ffi`     | C ABI consumed by the Qt shell                       |
| `ui-qt/`                | Thin Qt 6 desktop shell (window, canvas, dialogs)    |
| `docs/`                 | Architecture, commands, keybindings, config, roadmap… |
| `config/`               | Documented sample configuration                      |
| `bucket/`               | Scoop manifest (this repo is also a Scoop bucket)    |

Architecture rule: the Rust core never depends on Qt; the Qt shell contains
no document logic. Details in `docs/architecture.md`.

## Documentation

- `docs/architecture.md` – module boundaries, data flow, decisions
- `docs/commands.md` – every command
- `docs/keybindings.md` – every default binding + key syntax
- `docs/config.md` – every config option
- `docs/testing.md` – test strategy and how to run tests
- `docs/packaging.md` – build/packaging/release pipeline
- `docs/development-log.md` – milestone-by-milestone log
- `docs/roadmap.md` – phases and planned features
- `AGENTS.md` – ground rules and verify steps for AI agents / contributors

## License

syodep is released under the
[GNU Affero General Public License v3.0 or later](LICENSE) (AGPL-3.0-or-later).

This matches the license of [MuPDF](https://mupdf.com/), which syodep
embeds for PDF parsing and rendering: distributing combined builds requires
AGPL terms anyway, so licensing syodep itself under the AGPL keeps the
whole work compliant and unambiguous.

Clean-room note: Sioyek is GPLv3 and serves as product inspiration only.
syodep contains no Sioyek code, assets, or file structure; Sioyek-derived
*ideas* are recorded as conceptual inspiration in `docs/architecture.md`.

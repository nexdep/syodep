# Keybindings

## Key syntax

Bindings use a Vim-flavored textual syntax (shared between the config file
and the internals):

- Plain printable characters bind themselves: `j`, `G`, `+`. Case matters —
  `G` means shift+g and is written as the uppercase character.
- Special keys use angle brackets: `<Esc>`, `<CR>` (Enter), `<Tab>`,
  `<Space>`, `<BS>` (Backspace), `<Up>`, `<Down>`, `<Left>`, `<Right>`,
  `<PageUp>`, `<PageDown>`, `<Home>`, `<End>`.
- Modifiers go inside the brackets: `<C-d>` (ctrl), `<A-x>` (alt),
  `<C-A-Left>` (both). Shift on letters is expressed by case: `<C-G>`.
- A *sequence* concatenates chords: `gg`, `zw`, `g<C-d>`.

**Counts are not part of bindings.** Typing digits before a binding
(`5j`, `12G`) passes a count to the command at runtime. `0` only continues
a count that has already started (so `0` itself is bindable).

**Disambiguation rule:** if a sequence is both a complete binding and a
prefix of a longer one (e.g. binding both `g` and `gg`), syodep waits for
more input rather than firing eagerly; press `<Esc>` to cancel pending
input. There is no timeout — behavior is fully deterministic. The defaults
avoid such overlaps.

## Default bindings

Scrolling:

| Keys | Command |
|---|---|
| `j`, `<Down>` | `scroll_down` |
| `k`, `<Up>` | `scroll_up` |
| `h`, `<Left>` | `scroll_left` |
| `l`, `<Right>` | `scroll_right` |
| `<C-d>` | `scroll_half_page_down` |
| `<C-u>` | `scroll_half_page_up` |
| `<C-f>` | `scroll_page_down` |
| `<C-b>` | `scroll_page_up` |

Page navigation:

| Keys | Command |
|---|---|
| `J`, `<PageDown>` | `next_page` |
| `K`, `<PageUp>` | `prev_page` |
| `gg` | `goto_first_page` (with count: go to that page) |
| `G` | `goto_last_page` (with count: go to that page) |

Zoom:

| Keys | Command |
|---|---|
| `+`, `=` | `zoom_in` |
| `-` | `zoom_out` |
| `zw` | `fit_width` |
| `z0` | `zoom_reset` |

Application:

| Keys | Command |
|---|---|
| `o` | `open_file` |
| `q` | `quit` |
| `<Esc>` | `cancel` |

The mouse wheel (and horizontal trackpad scrolling) also scrolls the view;
this is a convenience, not the primary workflow.

## Customizing

Add a `[keys]` table to your config file (see `docs/config.md` for its
location). Entries **add to or override** the defaults — list only your
changes:

```toml
[keys]
"J"     = "scroll_half_page_down"  # rebind a default
"<C-o>" = "open_file"              # add a new binding
```

Invalid entries (bad key syntax or unknown command names) are reported in
the status bar at startup, with all other bindings staying functional.
Valid command names are listed in `docs/commands.md`.

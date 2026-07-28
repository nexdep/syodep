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
prefix of a longer one (e.g. binding both `o` and `ow`), syodep waits for
more input rather than firing eagerly; press `<Esc>` to cancel pending
input. There is no timeout — behavior is fully deterministic.

If the wait ends in a sequence that is bound to nothing, syodep falls back
to the **longest prefix that is itself a complete binding**: that command
runs, and the leftover keys are replayed. So with both `o` and `ow` bound,
`ow` runs `ow`, while `oj` runs `o` and then `j`. Without this, a binding
that is also a prefix of a longer one could never be triggered on its own.
The decision is still made by the next key press, never by elapsed time.

Counts survive the replay, and they go to the command that resolves first:
`5oj` gives the count to `o`, whereas `o5j` runs `o` and then gives the
count to `j`.

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

Focus (see "Focus mode" below). The same chords change the scope from inside
focus mode:

| Keys | Command |
|---|---|
| `cc` | `focus_enter_char` |
| `ce` | `focus_enter_line` |
| `cw` | `focus_enter_word` |
| `cs` | `focus_enter_sentence` |
| `cp` | `focus_enter_paragraph` |

Selection (see "Visual mode" below):

| Keys | Command |
|---|---|
| `v` | `visual_enter` — inherit the focus scope |
| `vc` | `visual_enter_char` |
| `ve` | `visual_enter_line` |
| `vw` | `visual_enter_word` |
| `vs` | `visual_enter_sentence` |
| `vp` | `visual_enter_paragraph` |

Application:

| Keys | Command |
|---|---|
| `<C-o>` | `open_file` |
| `q` | `quit` |
| `<Esc>` | `cancel` |

The mouse wheel (and horizontal trackpad scrolling) also scrolls the view;
this is a convenience, not the primary workflow.

Dragging a PDF onto the window opens it. Anything that is not a `.pdf` is
refused while still being dragged, so nothing happens on release; dropping
several at once opens the first and says so in the status bar.

## Focus mode

In **normal mode** (the default) `hjkl` scroll the page. Press `cc`, `cw`,
`ce`, `cs` or `cp` to switch to **focus mode**, where one position in the
document's content — text characters and images — is highlighted and `hjkl`
move it:

| Keys | Command |
|---|---|
| `h`, `<Left>` | `focus_left` — back one unit of the active scope |
| `l`, `<Right>` | `focus_right` — forward one unit of the active scope |
| `k`, `<Up>` | `focus_up` — up a line, or the previous unit |
| `j`, `<Down>` | `focus_down` — down a line, or the next unit |
| `w` | `focus_next_word` — next word start |
| `e` | `focus_end_word` — current/next word end |
| `b` | `focus_prev_word` — current/previous word start |
| `<Esc>` | `focus_exit` — back to normal mode |

**One keymap covers every scope.** `focus_left` is a character in char scope, a
word in word scope, a column jump in line scope and the previous unit in
sentence or paragraph scope — the command dispatches on the scope, so the same
keys keep doing the same thing as you change granularity:

| Keys | Scope | `h` / `l` | `j` / `k` |
|---|---|---|---|
| `cc` | char | one character (wraps across lines and pages) | one line, keeping the goal column |
| `cw` | word | one word run | one line, nearest the goal column |
| `ce` | line | previous/next **column** (multi-column pages) | one line |
| `cs` | sentence | previous/next sentence | previous/next sentence |
| `cp` | paragraph | previous/next paragraph | previous/next paragraph |

Line scope is `ce`, not `cl`: `l` is the forward motion in every mode.

`w`/`e`/`b` move by Vim-like word runs in *every* scope: letters/digits/
underscore together, punctuation/symbols separately, whitespace skipped. Each
image is a single stop.

**The entry chords also change the scope, in place.** Pressing `ce` while
already focused on a word highlights the line you are on — it does not move
you. There is one position and the scope reinterprets it.

The view scrolls to keep the highlight visible, and counts work (`5l`, `3j`,
`2w`). Every other binding (page scroll, page navigation, zoom, `q`, `o`, …)
still works in focus mode — only `hjkl`/`w`/`e`/`b`/`<Esc>` change meaning.
Scroll and page-jump commands additionally carry the highlight to the top of
the newly visible content; zoom leaves it in place. The status bar shows
`-- FOCUS (word) --` with the highlighted line and column. See
`docs/commands-focus-mode.md` for the full list.

Customize focus-mode keys with a `[focus_keys]` table (see `docs/config.md`);
it overlays the normal bindings while focus mode is active. Focus is the
foundation for highlighting and search in later phases
(`docs/roadmap.md`).

## Visual mode

Press `v` (`visual_enter`) to switch to **visual mode** and select a range. A
bare `v` inherits the granularity of the mode you were in, so `cw` then `v`
selects word by word; `vc`/`ve`/`vw`/`vs`/`vp` name the granularity instead.

Motion moves one end of the selection — the **head** — while the other stays
anchored:

| Keys | Command |
|---|---|
| `h`, `<Left>` | `visual_left` — back one unit of the active scope |
| `l`, `<Right>` | `visual_right` — forward one unit of the active scope |
| `k`, `<Up>` | `visual_up` — up a line, or back one unit for sentence/paragraph |
| `j`, `<Down>` | `visual_down` — down a line, or forward one unit for sentence/paragraph |
| `w` | `visual_next_word` — next word, whatever the scope |
| `b` | `visual_prev_word` — previous word, whatever the scope |
| `e` | `visual_end_word` — end of the current word, whatever the scope |
| `<Esc>` | `visual_exit` — back to the mode visual mode was entered from |

Each end has its own scope. `v` acts on the end that is moving, `o` on the
other one:

| Keys | Command |
|---|---|
| `o` | `visual_swap_ends` — move the other end from now on |
| `oo` | `visual_swap_ends` — swap without waiting for a motion |
| `oc`, `oe`, `ow`, `os`, `op` | `visual_other_char`, `visual_other_line`, `visual_other_word`, `visual_other_sentence`, `visual_other_paragraph` — switch ends *and* set that end's scope |
| `vc`, `ve`, `vw`, `vs`, `vp` | `visual_scope_char`, `visual_scope_line`, `visual_scope_word`, `visual_scope_sentence`, `visual_scope_paragraph` — set the active end's scope |
| `v` | `visual_exit` |

So from a line selection, `o` then `j`/`k` moves the beginning of the selection
line by line, while `ow` moves that same beginning word by word and leaves the
other end line-granular. Note that `v` and `o` are each both a binding and the
start of longer ones, so they take effect together with the key that follows
(see the disambiguation rule above); `oo` swaps ends on its own.

The ends may cross freely, and counts work (`3l`). Scroll and page jumps leave
the selection where it is rather than dragging it to the visible content, and
only the on-screen part of a selection is drawn. The status bar shows
`-- VISUAL (scope) --`, or `-- VISUAL (head/anchor) --` when the two ends have
different scopes. See `docs/commands-visual-mode.md` for the full list.

Customize visual-mode keys with a `[visual_keys]` table (see `docs/config.md`);
it overlays the normal bindings while visual mode is active.

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

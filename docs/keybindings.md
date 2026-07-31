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
- `<leader>` stands for the leader key, `[input] leader` (`<Space>` by
  default). It is expanded when the config is read, so `<leader>w` is simply
  the leader's chords followed by `w` — a leader is a naming convenience, not a
  mechanism of its own, and it disambiguates by the ordinary prefix rules
  below. `<leader>` is only valid on the left of a binding; the leader itself is
  written out in full.

**Counts are not part of bindings.** Typing digits before a binding
(`5j`, `12G`) passes a count to the command at runtime. `0` only continues
a count that has already started (so `0` itself is bindable).

**Disambiguation rule:** if a sequence is both a complete binding and a
prefix of a longer one (e.g. binding both `o` and `ow`), syodep waits
rather than firing eagerly. The wait ends in one of three ways: the next
key press, **a pause**, or `<Esc>`, which cancels pending input.

If the wait ends in a sequence that is bound to nothing, syodep falls back
to the **longest prefix that is itself a complete binding**: that command
runs, and the leftover keys are replayed. So with both `o` and `ow` bound,
`ow` runs `ow`, while `oj` runs `o` and then `j`. Without this, a binding
that is also a prefix of a longer one could never be triggered on its own.

**The pause** (`[input] timeout_ms`, 500 ms by default) is what lets such a
binding be used on its own: press `o`, pause, and it runs. Sequences typed at
normal speed never reach it — `cw` is word focus, while `c`, pause, `w` is
focus mode followed by a word motion. Two details worth knowing:

- a half-typed sequence bound to nothing (`g` on its own) is **dropped** by
  the pause rather than waiting indefinitely;
- a bare count never times out, so `12`, a pause, then `G` still jumps to
  page 12.

Set `timeout_ms = 0` to switch the pause off entirely, leaving the next key
press as the only thing that ends a wait.

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
| `c` | `focus_enter` — enter focus keeping the current scope (needs the pause) |
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

Highlighting (see "Highlight mode" below). Bound in focus and visual mode, not
in normal mode, where there is nothing selected to highlight:

| Keys | Command |
|---|---|
| `a` | `highlight_enter` — turn the focus highlight or selection into a highlight |

Application:

| Keys | Command |
|---|---|
| `<leader>o` | `open_file` — open the native file picker |
| `<leader>w` | `save_document` — overwrite the PDF with the highlights embedded |
| `<leader>q` | `quit` — save the reading position and quit; asks first if there are highlights not yet saved to the PDF |
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
| `h`, `<Left>` | `focus_left` — back one unit, or previous column for line/sentence/paragraph |
| `l`, `<Right>` | `focus_right` — forward one unit, or next column for line/sentence/paragraph |
| `k`, `<Up>` | `focus_up` — up a line, or the previous unit |
| `j`, `<Down>` | `focus_down` — down a line, or the next unit |
| `w` | `focus_next_word` — next word start |
| `e` | `focus_next_line` — next line start |
| `s` | `focus_next_sentence` — next sentence start |
| `p` | `focus_next_paragraph` — next paragraph start |
| `b` | `focus_prev_word` — current/previous word start |
| `<Esc>` | `focus_exit` — back to normal mode |

**One keymap covers every scope.** `focus_left` is a character in char scope, a
word in word scope, and a column jump in line, sentence or paragraph scope —
the command dispatches on the scope, so the same keys keep doing the same
thing as you change granularity:

| Keys | Scope | `h` / `l` | `j` / `k` |
|---|---|---|---|
| `cc` | char | one character (wraps across lines and pages) | one line, keeping the goal column |
| `cw` | word | one word run | one line, nearest the goal column |
| `ce` | line | previous/next **column** (multi-column pages) | one line |
| `cs` | sentence | previous/next **column** (multi-column pages) | previous/next sentence |
| `cp` | paragraph | previous/next **column** (multi-column pages) | previous/next paragraph |

Line scope is `ce`, not `cl`: `l` is the forward motion in every mode — and `e`
is the line letter for the same reason, since `l` was already taken.

`w`/`b` move by Vim-like word runs in *every* scope: letters/digits/underscore
together, punctuation/symbols separately, whitespace skipped. Each image is a
single stop. `e` does the same for lines, `s` and `p` for sentences and
paragraphs — `e` always lands at column 0, a line's own start, rather than
keeping the goal column `hjkl` was aiming for.

**These are motions, not scope changes.** In word focus, `s` jumps to the first
word of the next sentence and the highlight stays word-sized; `cs` stays put and
makes the highlight a whole sentence. There is no backward sentence or
paragraph key — use `cs`/`cp` and then `h`.

**The entry chords also change the scope, in place.** Pressing `ce` while
already focused on a word highlights the line you are on — it does not move
you. There is one position and the scope reinterprets it.

The view scrolls to keep the highlight visible, and counts work (`5l`, `3j`,
`2w`). Every other binding (page scroll, page navigation, zoom, `<leader>q`,
`<leader>o`, …) still works in focus mode — only `hjkl`/`w`/`e`/`b`/`<Esc>`
change meaning.
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
| `h`, `<Left>` | `visual_left` — back one unit, or previous column for line/sentence/paragraph |
| `l`, `<Right>` | `visual_right` — forward one unit, or next column for line/sentence/paragraph |
| `k`, `<Up>` | `visual_up` — up a line, or back one unit for sentence/paragraph |
| `j`, `<Down>` | `visual_down` — down a line, or forward one unit for sentence/paragraph |
| `w` | `visual_next_word` — next word, whatever the scope |
| `b` | `visual_prev_word` — previous word, whatever the scope |
| `e` | `visual_next_line` — next line start, whatever the scope |
| `s` | `visual_next_sentence` — next sentence start, whatever the scope |
| `p` | `visual_next_paragraph` — next paragraph start, whatever the scope |
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

## Highlight mode

Press `a` (`highlight_enter`) while focused or selecting to turn what is
highlighted or selected into a highlight, in the highlight colour. It stays
adjustable: **every visual-mode motion works, bound to the very same commands**,
so a key cannot mean one thing while selecting and another while highlighting.

| Keys | Command |
|---|---|
| `h`, `j`, `k`, `l` and the arrows | `visual_left`, `visual_down`, `visual_up`, `visual_right` |
| `w`, `b`, `e` | `visual_next_word`, `visual_prev_word`, `visual_next_line` |
| `s`, `p` | `visual_next_sentence`, `visual_next_paragraph` |
| `o`, `oo` | `visual_swap_ends` |
| `oc`, `oe`, `ow`, `os`, `op` | `visual_other_char`, `visual_other_line`, `visual_other_word`, `visual_other_sentence`, `visual_other_paragraph` |

Only three keys mean something specific to a highlight:

| Keys | Command |
|---|---|
| `a` | `highlight_commit` — keep it, back to visual mode with the same text selected |
| `<Esc>` | `highlight_discard` — throw it away, restoring the mode and selection `a` was pressed on |
| `<BS>` | `highlight_discard` |

`v` and `c`, with or without a scope letter, also **keep** the highlight and go
to the mode they name — they are not bound here at all, but fall through to
`[keys]`, where `visual_enter*` and `focus_enter*` store the pending highlight on
the way out. So `vw` means "keep it and carry on selecting by word", and the only
way to lose a highlight is to ask for it with `<Esc>` or `<BS>`.

Entering from focus mode gives the highlight a second end where the focus was,
so `w` and `o` can still grow a highlight that began on one word; discarding
takes that end away again.

Committing stores the highlight in syodep's database, so it comes back when the
document is reopened. `<leader>w` (`save_document`) overwrites the PDF with every
stored highlight embedded as a real PDF annotation, which is what makes them
visible in other readers.

The status bar shows `-- HIGHLIGHT (scope) --`, in the same shape as visual
mode's. See `docs/commands-highlight-mode.md` for the full list.

Customize highlight-mode keys with a `[highlight_keys]` table (see
`docs/config.md`); it overlays the normal bindings while a highlight is pending.

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

# Focus mode commands

**Focus mode** highlights one position in the document's content — text
characters and images — and moves it with `hjkl`, independently of scrolling.
Each image is a single stop.

What "one position" covers is the **scope**: a character, a word, a line, a
sentence or a paragraph. The scope is a *setting of the mode*, not a mode of its
own, so there is one focus mode with five granularities rather than five modes.
The status bar shows `-- FOCUS (word) --` with the highlighted line and column,
matching visual mode's `-- VISUAL (word) --`.

Press `<Esc>` (`focus_exit`) to return to normal mode; the position and scope
are remembered.

Counts work here too (`5l`, `3j`).

## Entering, and changing scope

| Command | Effect | Count |
|---|---|---|
| `focus_enter_char` | focus character by character | — |
| `focus_enter_word` | focus word by word | — |
| `focus_enter_line` | focus line by line | — |
| `focus_enter_sentence` | focus sentence by sentence | — |
| `focus_enter_paragraph` | focus paragraph by paragraph | — |
| `focus_exit` | leave focus mode (the position and scope are remembered) | — |

Bound to `cc` / `cw` / `ce` / `cs` / `cp`. Line scope is `ce`, not `cl`: `l` is
the forward motion in every mode.

**The same chords change the scope from inside focus mode**, and they do it
*in place* — `cw` then `ce` highlights the line you are already on, it does not
jump you somewhere else. There is only one position, and changing the scope
reinterprets it. (Five separate focus modes each kept their own mark, so
switching between them teleported you to wherever you last were at that
granularity.)

## Moving

| Command | Effect | Count |
|---|---|---|
| `focus_left` | move back one unit of the active scope | repeats N times |
| `focus_right` | move forward one unit of the active scope | repeats N times |
| `focus_up` | move up a line, or back one unit for the linear scopes | repeats N times |
| `focus_down` | move down a line, or forward one unit for the linear scopes | repeats N times |
| `focus_next_word` | move to the start of the next word run | repeats N times |
| `focus_prev_word` | move to the start of the current word run, or the previous run if already at a start | repeats N times |
| `focus_end_word` | move to the end of the current word run, or the next run if already at an end | repeats N times |

`hjkl` and the arrow keys move by the active scope. What that means per scope:

| Scope | `h` / `l` | `j` / `k` |
|---|---|---|
| char | one character (wraps to the previous/next line and page) | one line, keeping the goal column |
| word | one word run | one line, landing on the word nearest the goal column |
| line | the line in the previous/next **column** (multi-column pages only) | one line |
| sentence | previous/next sentence | previous/next sentence |
| paragraph | previous/next paragraph | previous/next paragraph |

Two of those are worth spelling out. **Line scope swaps the axes**: `h`/`l` jump
between columns on a multi-column page rather than moving within the line, and
`j`/`k` set the row those jumps aim at. **Sentence and paragraph have no second
axis**, so all four directions collapse to previous/next.

`w`, `b` and `e` always move by a word, whatever the scope is — they are
word-named motions, and the highlight still snaps out to the active scope
afterwards. This mirrors visual mode exactly.

Word motions use Vim-like lowercase boundaries: letters/digits/underscore form
word runs, punctuation/symbols form separate runs, whitespace is skipped, and
each image is a single stop.

The view auto-scrolls to keep the highlight on screen as it moves.

## Inherited view commands

Every normal-mode command stays available in focus mode with its normal
binding — only `hjkl`, `w`/`e`/`b`, the arrow keys and `<Esc>` are remapped (to
scope motion / exit). So the page-scroll, page-navigation and zoom commands
below all work here too. (The plain line-scroll commands `scroll_down` /
`scroll_up` / `scroll_left` / `scroll_right` are *not* reachable from the
keyboard, since their default `hjkl`/arrow bindings move the highlight
instead.)

**Scroll and page jumps reposition the highlight.** After any of these
commands, the highlight jumps to the top-most content now visible in the
window, keeping its goal column — so it follows the scroll instead of being
left behind off-screen.

| Command | Effect | Count |
|---|---|---|
| `scroll_half_page_down` | scroll down half a window, then move the highlight to the top of the new view | multiplies |
| `scroll_half_page_up` | scroll up half a window, then move the highlight to the top of the new view | multiplies |
| `scroll_page_down` | scroll down a full window, then move the highlight to the top of the new view | multiplies |
| `scroll_page_up` | scroll up a full window, then move the highlight to the top of the new view | multiplies |
| `next_page` | jump to the next page; the highlight moves onto it | advances N pages |
| `prev_page` | jump to the previous page; the highlight moves onto it | goes back N pages |
| `goto_first_page` | go to the first page; the highlight moves onto it | **with count N: page N** (1-based) |
| `goto_last_page` | go to the last page; the highlight moves onto it | **with count N: page N** (1-based) |

**Zoom keeps the highlight in place.** Zoom changes magnification around the
window center; the highlight stays on the same content.

| Command | Effect | Count |
|---|---|---|
| `zoom_in` | multiply zoom by `view.zoom_step` | applies N times |
| `zoom_out` | divide zoom by `view.zoom_step` | applies N times |
| `fit_width` | fit the widest page to the window width | — |
| `zoom_reset` | set zoom to 100% (72 dpi) | — |

The application commands `open_file`, `quit` and `cancel` also keep their
normal-mode behavior. See `docs/commands-normal-mode.md` for those.

## Relationship to visual mode

A focus highlight is a selection whose two ends coincide. Pressing `v` from
focus mode enters visual mode inheriting the focus scope, and `<Esc>` comes
back to focus mode at wherever the head ended up. The two modes share one
per-scope motion table, so a scope cannot mean one thing in focus mode and
something else in visual mode. See `docs/commands-visual-mode.md`.

## Customizing

Focus bindings live in the `[focus_keys]` config table, which overlays the
normal `[keys]` while focus mode is active. One table covers every scope,
because the motion commands dispatch on the active scope rather than being
named after it. See `docs/config.md` and `docs/keybindings.md`.

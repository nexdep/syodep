# Visual mode commands

**Visual mode** selects a *range* of content. It is entered with `v` from
normal mode or from focus mode, and unlike focus mode it has two ends:
an **anchor** that stays put and a **head** that motions move. The status bar
shows `-- VISUAL (scope) --` with the selected line range.

Each end carries its own **scope** - the granularity it moves and snaps by -
so a selection can be line-granular at one edge and word-granular at the other.
A bare `v` inherits the focus scope, so `cw` then `v` starts selecting word by
word with the same keys you were already using.

Press `<Esc>` (`visual_exit`) to return to the mode visual mode was entered
from. The moving end *is* the focus position, so both where you are and the
granularity you were last using carry straight over — `cw`, `v`, `ve`, `<Esc>`
leaves you in line focus at the head, not back where you started.

Counts work here too (`3l`, `2w`).

## Entering

| Command | Effect | Count |
|---|---|---|
| `visual_enter` | start selecting, inheriting the focus scope | - |
| `visual_enter_char` | start selecting character by character | - |
| `visual_enter_line` | start selecting line by line | - |
| `visual_enter_word` | start selecting word by word | - |
| `visual_enter_sentence` | start selecting sentence by sentence | - |
| `visual_enter_paragraph` | start selecting paragraph by paragraph | - |
| `visual_exit` | leave visual mode, returning to the prior mode | - |

Bound to `v` (inherit) and `vc` / `ve` / `vw` / `vs` / `vp` (explicit). Because
`v` is both a binding and the start of the longer ones, it takes effect
together with the next key you press - see the disambiguation rule in
`docs/keybindings.md`.

## Moving the selection

| Command | Effect | Count |
|---|---|---|
| `visual_left` | move the active end back one unit of its scope | repeats N times |
| `visual_right` | move the active end forward one unit of its scope | repeats N times |
| `visual_up` | move the active end up a line, or back one unit for the linear scopes | repeats N times |
| `visual_down` | move the active end down a line, or forward one unit for the linear scopes | repeats N times |
| `visual_next_word` | move the active end to the start of the next word | repeats N times |
| `visual_prev_word` | move the active end to the start of the previous word | repeats N times |
| `visual_end_word` | move the active end to the end of the current word | repeats N times |

`hjkl` and the arrow keys move by the active end's scope: one character in char
scope, one word in word scope, one line in line scope, and so on. In sentence
and paragraph scope the unit is a linear sequence, so all four directions
collapse to previous/next - the same shape as focus mode at those scopes.

`w`, `b` and `e` always move by a word, whatever the scope is. In line scope
the head still moves a word at a time while the edge snaps out to the whole
line.

The ends may cross freely: the selection always runs from the earlier end to
the later one, so passing the anchor and coming back leaves the selection
exactly as it was.

## Choosing the end and its scope

**`v` acts on the end that is moving, `o` on the other one.**

| Command | Effect | Count |
|---|---|---|
| `visual_swap_ends` | make the other end the one motions move | - |
| `visual_scope_char` | set the active end's scope to characters | - |
| `visual_scope_word` | set the active end's scope to words | - |
| `visual_scope_line` | set the active end's scope to lines | - |
| `visual_scope_sentence` | set the active end's scope to sentences | - |
| `visual_scope_paragraph` | set the active end's scope to paragraphs | - |
| `visual_other_char` | switch ends and set that end's scope to characters | - |
| `visual_other_word` | switch ends and set that end's scope to words | - |
| `visual_other_line` | switch ends and set that end's scope to lines | - |
| `visual_other_sentence` | switch ends and set that end's scope to sentences | - |
| `visual_other_paragraph` | switch ends and set that end's scope to paragraphs | - |

So in a line selection, `o` starts moving the beginning of the selection line
by line, and `ow` starts moving that same beginning word by word while the
other end stays line-granular. `vw` changes the scope of the end that is
already moving, without switching.

Like `v`, a bare `o` is also the start of `oc`/`ow`/`oe`/`os`/`op`, so it takes
effect together with the key that follows it (`oj` swaps ends and then moves
down). Type `oo` to swap ends on its own.

When the two ends have different scopes the status bar shows both, active end
first: `-- VISUAL (word/line) --`.

## Inherited view commands

Every normal-mode command stays available in visual mode with its normal
binding - only `hjkl`, `w`/`b`/`e`, `o`, `v`, the arrow keys and `<Esc>` are
remapped. So the page-scroll, page-navigation and zoom commands all work here
too.

**Scrolling and page jumps leave the selection alone.** Unlike focus mode,
scrolling does not drag the selection to the newly visible content - a
selection is an explicit range, and moving it out from under the reader would
lose work. Only the parts of the selection on screen are drawn.

**Entering focus mode discards the selection but keeps your place.** The
focus entry chords (`cc`, `ce`, `cw`, `cs`, `cp`) still work in visual mode;
they drop the anchor and leave you focused on the moving end at the scope you
named. Leaving by a `c` chord and leaving by `<Esc>` differ only in whether you
also change the scope.

The application commands `open_file`, `quit` and `cancel` also keep their
normal-mode behavior. See `docs/commands-normal-mode.md` for those.

## Customizing

Visual-mode bindings live in the `[visual_keys]` config table, which overlays
the normal `[keys]` while visual mode is active. See `docs/config.md` and
`docs/keybindings.md`.

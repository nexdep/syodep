# Visual mode commands

**Visual mode** selects a *range* of content — text, images and tables. It is
entered with `v` from
normal mode or from focus mode, and unlike focus mode it has two ends:
an **anchor** that stays put and a **head** that motions move. The status bar
shows `-- VISUAL (scope) --` with the selected line range.

Each end carries its own **scope** - the granularity it moves and snaps by -
so a selection can be line-granular at one edge and word-granular at the other.
A bare `v` inherits the focus scope, so `fw` then `v` starts selecting word by
word with the same keys you were already using.

Press `<Esc>` (`visual_exit`) to return to the mode visual mode was entered
from. The moving end *is* the focus position, so both where you are and the
granularity you were last using carry straight over — `fw`, `v`, `ve`, `<Esc>`
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
`v` is both a binding and the start of the longer ones, it takes effect either
with the next key you press or after a brief pause - see the disambiguation
rule in `docs/keybindings.md`. The same is true of `o`, so pausing after it
swaps the ends without needing `oo`.

Entering with an explicit scope (and changing the head's scope with `vw` /
`ve` / …) **snaps** the moving end and refreshes `focus_span` the same way
focus enter does, so the head's cached span always matches what
`scope_span` would draw.

## Moving the selection

| Command | Effect | Count |
|---|---|---|
| `visual_left` | move the active end back one unit of its scope, or to the previous column for line/sentence/paragraph | repeats N times |
| `visual_right` | move the active end forward one unit of its scope, or to the next column for line/sentence/paragraph | repeats N times |
| `visual_up` | move the active end up a line, or back one unit for sentence/paragraph | repeats N times |
| `visual_down` | move the active end down a line, or forward one unit for sentence/paragraph | repeats N times |
| `visual_next_word` | move the active end to the start of the next word | repeats N times |
| `visual_prev_word` | move the active end to the start of the previous word | repeats N times |
| `visual_next_line` | move the active end to the start of the next line | repeats N times |
| `visual_next_sentence` | move the active end to the start of the next sentence | repeats N times |
| `visual_next_paragraph` | move the active end to the start of the next paragraph | repeats N times |

`hjkl` and the arrow keys move by the active end's scope: one character in char
scope, one word in word scope, one line in line scope, and so on. Line,
sentence and paragraph swap the axes on multi-column pages — `h`/`l` jump
columns (landing on the unit at the goal row) while `j`/`k` step
previous/next — the same shape as focus mode at those scopes. On a
single-column page `h`/`l` are a no-op there; use `j`/`k` (or `s`/`p`) to
grow the selection by a unit.

`w`, `b`, `e`, `s` and `p` always move by their own unit, whatever the active
end's scope is: word, word, line, sentence and paragraph respectively. In word
scope the head still moves to the next line's start (or a sentence, or a
paragraph) while the edge snaps back out to the whole word it lands on.

`s` and `p` do not change either end's scope — `vs` and `vp` do that. So `s`
grows the selection by a sentence while keeping word-granular edges.

The ends may cross freely: the selection always runs from the earlier end to
the later one, so passing the anchor and coming back leaves the selection
exactly as it was.

### Tables, images and equations

A table, an image or a display equation is one unit here too: from line scope
up, a single motion extends the selection across the whole of it. A selection
that covers one entirely is drawn as **one rectangle** over the whole object
rather than a ragged stack of row boxes — however the selection came to cover
it, so dragging an end across a whole formula squares it off too.

Word and char scope reach inside a table or an equation, and their ends are not
expanded. Because scope is per end, this is decided independently at each edge:
that is how you select a single figure from a table while the other end selects
whole paragraphs. An image stays one unit at word scope, having no words inside.

### Headings

A heading is one step for `s` and `p` here too, so `vs` then `s` selects a whole
section heading and the next `s` takes the paragraph under it. At line, word or
char scope the ends move through a heading normally, since its wrapped lines are
real reading lines — see `docs/commands-focus-mode.md`.

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
`center_view` scrolls so the selection is at the viewport center without
moving either end.

**Entering focus mode discards the selection but keeps your place.** The
focus entry chords (`fc`, `fe`, `fw`, `fs`, `fp`) still work in visual mode;
they drop the anchor and leave you focused on the moving end at the scope you
named. Leaving by a `f` chord and leaving by `<Esc>` differ only in whether you
also change the scope.

The application commands `open_file`, `save_document`,
`toggle_highlights_sidebar`, `toggle_annotations_sidebar`, `close_sidebar`,
`toggle_keybindings_overlay`, `create_annotation`, `quit` and `cancel` also
keep their normal-mode behavior. See `docs/commands-normal-mode.md` for those.

## Customizing

Visual-mode bindings live in the `[visual_keys]` config table, which overlays
the normal `[keys]` while visual mode is active. See `docs/config.md` and
`docs/keybindings.md`.

# Focus mode commands

**Focus mode** highlights one position in the document's content — text
characters, images and tables — and moves it with `hjkl`, independently of
scrolling. From line scope up, each image, detected table and display equation
is a single stop: one motion lands on it, the next lands past it, however many
lines it covers. Word and char scope reach inside — see "Tables and images"
below.

What "one position" covers is the **scope**: a character, a word, a line, a
sentence or a paragraph. The scope is a *setting of the mode*, not a mode of its
own, so there is one focus mode with five granularities rather than five modes.
The status bar shows `-- FOCUS (word) --` with the highlighted line and column,
matching visual mode's `-- VISUAL (word) --`.

Press `<Esc>` (`focus_exit`) to return to normal mode. The position is
remembered, but the scope resets to char: normal mode has no granularity of its
own, so it does not keep one — a later bare `v` always starts by character.

Counts work here too (`5l`, `3j`).

## Entering, and changing scope

| Command | Effect | Count |
|---|---|---|
| `focus_enter` | enter focus mode keeping the current scope | — |
| `focus_enter_char` | focus character by character | — |
| `focus_enter_word` | focus word by word | — |
| `focus_enter_line` | focus line by line | — |
| `focus_enter_sentence` | focus sentence by sentence | — |
| `focus_enter_paragraph` | focus paragraph by paragraph | — |
| `focus_exit` | leave focus mode (the position and scope are remembered) | — |

Bound to `fc` / `fw` / `fe` / `fs` / `fp`. Line scope is `fe`, not `cl`: `l` is
the forward motion in every mode.

`focus_enter` is bound to a bare `f`, which acts once you **pause** — `f` is
also the start of the five chords above, so it waits to see whether another key
follows (see the disambiguation rule in `docs/keybindings.md`). It keeps
whatever scope is live: char coming from normal mode, which resets the scope,
and the selection's scope coming from visual mode.

**The same chords change the scope from inside focus mode**, and they do it
*in place* — `fw` then `fe` highlights the line you are already on, it does not
jump you somewhere else. There is only one position, and changing the scope
reinterprets it. (Five separate focus modes each kept their own mark, so
switching between them teleported you to wherever you last were at that
granularity.)

## Moving

| Command | Effect | Count |
|---|---|---|
| `focus_left` | move back one unit of the active scope, or to the previous column for line/sentence/paragraph | repeats N times |
| `focus_right` | move forward one unit of the active scope, or to the next column for line/sentence/paragraph | repeats N times |
| `focus_up` | move up a line, or back one unit for sentence/paragraph | repeats N times |
| `focus_down` | move down a line, or forward one unit for sentence/paragraph | repeats N times |
| `focus_next_word` | move to the start of the next word run | repeats N times |
| `focus_prev_word` | move to the start of the current word run, or the previous run if already at a start | repeats N times |
| `focus_next_line` | move to the start of the next line | repeats N times |
| `focus_next_sentence` | move to the start of the next sentence | repeats N times |
| `focus_next_paragraph` | move to the start of the next paragraph | repeats N times |

`hjkl` and the arrow keys move by the active scope. What that means per scope:

| Scope | `h` / `l` | `j` / `k` |
|---|---|---|
| char | one character (wraps to the previous/next line and page) | one line, keeping the goal column |
| word | one word run | one line, landing on the word nearest the goal column |
| line | the line in the previous/next **column** (multi-column pages only) | one line |
| sentence | the sentence in the previous/next **column** (multi-column pages only) | previous/next sentence |
| paragraph | the paragraph in the previous/next **column** (multi-column pages only) | previous/next paragraph |

Two of those are worth spelling out. **Line, sentence and paragraph swap the
axes**: `h`/`l` jump between columns on a multi-column page (landing on the
unit that contains the line nearest the goal row), and `j`/`k` set the row
those jumps aim at while also stepping previous/next unit. On a single-column
page `h`/`l` are a no-op at those scopes; use `j`/`k` (or `s`/`p`) to move.

`w`, `b`, `e`, `s` and `p` always move by their own unit, whatever the active
scope is — they are *motions*, and the highlight still snaps out to the active
scope afterwards. This mirrors visual mode exactly.

| Key | Moves by |
|---|---|
| `w` / `b` | next / previous word start |
| `e` | next line start |
| `s` | next sentence |
| `p` | next paragraph |

`e` is the line letter for the same reason `fe` is: `l` is the forward motion
in every mode, so line scope's own letter can't be `l`. This is also why `e`
always lands at column 0 — a line's start *is* column 0 — rather than
preserving whatever column `hjkl` was aiming for.

**A motion is not a scope change.** In word focus, `s` jumps to the first word
of the next sentence and the highlight stays *word*-sized; `fs` stays where you
are and makes the highlight a whole sentence. Sentence and paragraph have no
dedicated backward motion key — press `fs` or `fp` and use `k`, which walks
backwards a unit at a time.

Word motions use Vim-like lowercase boundaries: letters/digits/underscore form
word runs, punctuation/symbols form separate runs, whitespace is skipped, and
each image is a single stop. Word motions run *through* a table or an equation,
stopping on the words inside them.

**Line-final colons.** A colon whose only followers on its line are spaces (or
nothing) ends the sentence and starts a new paragraph at the next line:
`A lead-in:` then `Continued text.` are two sentences and two paragraphs, even
when the vertical gap between the lines is tight. A mid-line colon stays inert
— `Note: more words here.` is still one sentence — and a colon inside a URL is
unchanged.

**Lists.** Each list item is one sentence, so `s` steps through a list item by
item even though items rarely end in a full stop. The line introducing a list
does not run into its first item, and the last item does not run on into the
prose after the list: an item covers its marker and the lines wrapped under it,
and ends where the text returns to the marker's own margin or where a
paragraph-sized gap opens — the same threshold paragraph motion uses, so a
hanging-indent list whose following prose sits past the marker still stops
cleanly. A numbered item is one sentence including its `1.`, not two.

Both shapes of item count: a bullet followed by its text, and a bullet that
extraction leaves on a line of its own with the text below *or* above it
(MuPDF sometimes emits the marker after the citation). A lone bullet is paired
with the nearest indented neighbour in a small vertical window. A marker counts
only when another item of the same kind lines up with it, so a sentence that
merely opens with a numeral is not a list. Uppercase single-letter labels
(`T. Author`) are not enumerators — only digits, roman numerals, and lowercase
`a.`/`b.` — so citation initials cannot form a false list beside real bullets.

Items bound sentences only. `w` still walks the marker and the words after it,
and items do not split a list into paragraphs — though a list set with generous
space between items may still be split by the ordinary paragraph-gap rule. A
colon lead-in before a list is its own paragraph (and sentence) by the
line-final-colon rule above; `p` then skips the list that follows in one step.

**Abbreviations.** `e.g.`, `i.e.`, `U.S.`, `Ph.D.` and the like are one word and
never break a sentence: the stops inside them are inert. Runs of initials
joined by stops are recognised by shape, so nothing has to be listed. A set of
common abbreviations that no rule can infer is also known — `etc.`, `cf.`,
`vs.`, `et al.`, `Fig.`, `Eq.`, `Sec.`, `vol.`, `Dr.`, month and day names and
others.

Punctuation set around one is not part of it. `(e.g.,` is three stops — `(`,
`e.g.`, `,` — and the abbreviation is recognised inside its brackets, so a
parenthetical aside does not break the sentence carrying it. The construct is a
hard edge for `w` in both directions: neither the bracket before it nor the
comma after it is swallowed into it.

Their *closing* stop still ends a sentence when a new one visibly follows it, so
`…apples, oranges, etc. The next one` splits correctly while `…etc. and then
more` does not. That capitalisation test is used only where an abbreviation is
already suspected — applied to prose generally it merges real sentences, since
technical writing constantly starts one with a lower-case identifier. A comma,
semicolon or colon reached before the capital vetoes it, which is what keeps a
citation like `(e.g., Smith 2020)` in one piece. A closing bracket after the
stop is unaffected: `…and magic (etc.) Then more` still splits.

A number is always one word, however it is punctuated: `3.14` and `1,234.56` are
each a single stop, because a separator with digits on both sides belongs to the
figure. The same rule keeps a decimal point from ending a sentence — `pi is 3.14
exactly.` is one sentence, not two. A full stop that merely follows a number
still ends both, since nothing follows it: `it costs 3.` behaves as before.

More generally, a full stop with alphanumeric (or `_`) sides and no space —
`VII.0`, `file.txt`, `a.b.c` — is the same kind of join: one word, and never a
sentence boundary. That is what keeps library versions like `ENDF/B-VII.0` from
splitting under `s`.

Scientific notation and percentages come with it. `1.5e-10` and `2.3E+5` are one
word each: the sign of an exponent joins when an `e`/`E` with a digit behind it
sits in front of it, so `cache+1` is still three stops. A proportion sign
written tight against a figure joins backwards to it, making `45.5%` one word,
while `the % sign` and `45.5 %` keep the separate stops they should have. Units
(`37°C`, `5kg`) are not covered — the letters after them are a question of their
own.

A hyphenated compound is likewise one word: `well-known`, `state-of-the-art` and
`COVID-19` are each a single stop for `w`, `e` and `b`, because a hyphen with
word characters on both sides joins them. A dash that is not doing that keeps
the stop of its own it has always had — `one - two`, `well- known`, `one--two`,
and the en and em dashes (`–`, `—`), which punctuate a sentence rather than
build a word. A word broken across a line break stays two stops: word runs never
cross lines, and that hyphen belongs to the typesetting rather than to the word.

**Links.** A URL or an email address is one word, and the stops inside it never
end a sentence — `See https://example.com/a.html for it.` is one sentence with
one stop in it. Recognised forms: anything with a scheme (`https://…`,
`mailto:…`, `doi:…`), a `www.` host, a host with a path (`doi.org/10.1000/182`),
and a plain email address. The punctuation around a link is not part of it, so
`(https://example.com),` is three stops and the full stop in `…example.com.`
still ends the sentence — while a bracket the address itself opened stays in,
as in `…/Glob_(pattern)`.

A bare host with no path and no `www.` is deliberately *not* treated as a link:
extraction that drops a space leaves `sentence.Next` looking exactly like one,
and swallowing that would glue two words together and lose a sentence boundary.
`and/or`, `km/h` and `src/lib.rs` are unaffected for the same reason — nothing
before the slash is a host.

The view auto-scrolls to keep the highlight on screen as it moves, stopping
`view.scroll_off` pixels short of the top and bottom edges so there is always
context past the highlight rather than the highlighted line sitting flush
against the border. The buffer is given up at the very start and end of the
document, where there is nothing left to scroll to, so the first and last
lines stay reachable. Set `view.scroll_off = 0.0` to let the highlight reach
the edge.

## Tables and images

A table or an image is **one unit** from line scope up. `e`, `s`, `p` and
`hjkl` all step onto it once and then step past it, no matter how many lines it
spans, and the highlight covers the whole thing as a single rectangle. A
paragraph or sentence next to a table never reaches into it either, so `p` on
the prose above a figure highlights just that prose.

Word and char scope reach **inside** a table, because its cells are text you may
well want a part of. `fw` then `w` walks the words in its cells, and `fc` then
`h`/`l` walks its characters, so a single number in a cell stays selectable.
Their highlights shrink to the word or character rather than covering the whole
table. Switching back to line scope or coarser snaps to the whole table again.

An **image** is the exception: it is one unit at word scope too, since there are
no words inside one to walk through.

A caption or a paragraph set close under a table stays outside it: its own
sentence, its own stop, and no tint over it. Detection reports the table's
*ruled* area, which reaches past the last row, so the edges of what it claims
are trimmed back to the rows themselves and the highlight is held back from the
lines above and below.

Images are always single stops. Tables are found by a detection pass that can
be turned off with `view.detect_tables` — see `docs/config.md`.

## Headings

A heading is one step at **sentence and paragraph scope**: `s` lands on it, the
next `s` lands on the body beneath. Without this a heading would be swallowed by
the paragraph that follows it, because headings rarely end in a full stop. A
numbered heading like `2.12. Recommended checking order` is still one sentence,
not three — including when it is set at body size, where typography alone would
miss it — and a heading that wraps onto two lines is one step across both.

A heading goes further than a table does: `w` walks its individual words *and*
`j` at line scope still moves through it line by line. Its wrapped lines are
real reading lines, unlike a table's rows or an equation's, so only the two
scopes that group text into runs treat it as a unit.

Turn detection off with `view.detect_headings` — see `docs/config.md`.

## Equations

A display equation is **one unit from line scope up**, exactly as a table is.
`e`, `s` and `p` land on the whole formula and the next press lands on the prose
after it, however many rows the equation runs to — an aligned system is one
unit, and an equation number set on a line of its own (`(3.4)`) comes with it. A
stop inside a formula — `f(x) = 0.` — does not split it, the same way `2.12.`
does not split a heading. The highlight is the formula's whole box, so an
aligned system tints as one rectangle rather than one ragged strip per row.

Its rows are not reading lines, which is why `j` at line scope steps over the
whole thing: stopping on row two of a system is never what you meant. That is
the one way it differs from a heading, whose wrapped lines *are* reading lines
and keep a stop each.

Word and char scope still reach inside: `w` steps through its terms and `fc`
then `h`/`l` walks its characters, so a single variable or coefficient stays
selectable, with the highlight shrinking to match.

A line reads as a display equation when it is set apart from the prose (it does
not fill the column), reads as mathematics by its fonts or by rich math
characters (Greek / unicode operators — not ASCII `+`/`=` alone), carries an
operator or relation, and carries almost no ordinary words. **Maths inline in a
sentence is left alone** — making it a unit would mean making it a region, and
that would split the sentence around it. Turn detection off with
`view.detect_equations` — see `docs/config.md`.

## Page furniture

Running headers, page numbers, sideways margin stamps and inclined watermarks
are not part of the reading flow, so the caret never lands on them at any scope
and no selection covers them. They are still drawn on the page. Headers and
footers are recognised by repeating across pages rather than by where they sit,
so a title or a section heading near the top of a page is never mistaken for
one. Turn it off with `view.skip_page_furniture` — see `docs/config.md`.

`Ln N` in the status line counts navigable lines, so `Ln 1` is the first line of
body text rather than the running header above it.

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
window below the `view.scroll_off` buffer, keeping its goal column — so it
follows the scroll instead of being left behind off-screen, and lands where it
would have come to rest had you walked there, rather than pinned to the edge.

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
| `center_view` | scroll so the focus highlight is at the viewport center (Vim `zz`) | — |

The application commands `open_file`, `save_document`,
`toggle_highlights_sidebar`, `toggle_annotations_sidebar`, `create_annotation`,
`quit` and `cancel` also keep their
normal-mode behavior. See `docs/commands-normal-mode.md` for those.

## Relationship to visual mode

A focus highlight is a selection whose two ends coincide. Pressing `v` from
focus mode enters visual mode inheriting the focus scope; visual mode's moving
end *is* the focus position, so leaving it — by `<Esc>` or by naming a new
scope with a `f` chord — always leaves you where that end was. The two modes
share one per-scope motion table, so a scope cannot mean one thing in focus
mode and something else in visual mode. See `docs/commands-visual-mode.md`.

## Customizing

Focus bindings live in the `[focus_keys]` config table, which overlays the
normal `[keys]` while focus mode is active. One table covers every scope,
because the motion commands dispatch on the active scope rather than being
named after it. See `docs/config.md` and `docs/keybindings.md`.

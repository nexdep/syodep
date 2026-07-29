# Development log

Newest entries first. Each entry records what was implemented, the tests
that cover it, and decisions worth remembering. Future contributors (human
or agent): read `docs/architecture.md` first, then the latest entries here,
then `docs/roadmap.md` for what to build next.

---

## 2026-07-29 — An abbreviation is one word and does not end a sentence

`e.g.` was four word stops and two sentences. Now abbreviations are single words
whose internal stops are inert, in the same shape as the number rule: one
predicate consulted from both `same_word_run` and `sentence_boundary_after`, so
word runs and sentence runs cannot disagree about where the construct ends.

Two kinds, deliberately handled differently. **Dotted initials** — `e.g.`,
`i.e.`, `U.S.`, `Ph.D.`, `a.k.a.` — are recognised by *shape*: a run of one- or
two-letter groups joined by stops. No list, nothing to maintain, and no sentence
ever ends in the middle of one. **Everything else** needs naming, because `Fig.`
is indistinguishable from a word ending a sentence, so there is a curated list:
Latin and citation forms, references, bibliographic forms, titles, months, days,
organisations and measurement.

### The capitalisation test, and why it is not applied generally

`etc.` and `U.S.` genuinely can end a sentence. So the closing stop of an
abbreviation still ends one — but only when a capital follows. `…oranges, etc.
The next` splits; `…etc. and then` does not.

The tempting generalisation is to apply that everywhere: "a stop followed by a
lower-case word is not an ending". Measured on the reference document first,
that rule would have merged **nine genuine sentence boundaries** — after
`match.`, `data.`, `bits.`, `rule.`, `byte.`, `type.` — because a technical
document constantly begins a sentence with a lower-case identifier. So the
capitalisation signal is consulted *only* where an abbreviation is already
suspected, which is precisely where the evidence says it is safe. A test pins
that: `an_ordinary_word_before_a_lower_case_word_still_ends_the_sentence`.

Several list entries (`no.`, `min.`, `co.`) are also ordinary words that can
close a sentence. That is safe for the same reason — the list only says "suspect
an abbreviation here", never "this is never an ending".

### Result

On the reference document, sentence stops went 415 → 413: exactly the two
spurious boundaries inside its single `e.g.`, with all nine lower-case-follows
boundaries correctly left alone.

11 new tests, 353 total.

---

## 2026-07-29 — A list item is a region, so it has an end

The first cut of list support gave items a *start* and nothing else: a
`list_starts` vector of line indices, plus two bespoke filters in the sentence
walkers refusing to cross one. That made each item a sentence but left the last
item of every list joining the prose below it, since nothing said where a list
ended.

The fix was not a `list_ends` vector beside the first. A list item is a region
like a heading or a table, and a region has two edges by construction. Giving
items an extent deletes both filters and `App::starts_list_item` outright:
`next_cell_in_region`/`prev_cell_in_region` revert character-for-character to
the one-line `same_region` filters they were before lists existed. The feature
stops existing in the motion code entirely.

`ObjectKind` grew a second predicate to pay for it. Regions now read as a chain:
a table or image is one stop at every scope; a heading is one step for `s` and
`p`; a list item is one step for `s` only, because a list is one paragraph made
of many items.

### The extent rule, and two measurements that shaped it

An item covers its marker and every following line indented past that marker,
stopping at the next marker, at any object already claimed, at a paragraph-sized
gap, or when the text returns to the marker's margin. Measured on the reference
document: markers at x0=119.6, item text and wrapped continuations at 129.5,
prose resuming at 119.6.

**A design review caught a bug that would have shipped.** On a two-column page
the first line of column two is trivially "indented past" a marker in column
one, so an item swallowed the head of the next column. A y-reset guard fixes it,
and `an_item_never_crosses_a_column_break` fails without it — verified by
removing the guard and watching the item become `(1,2)`.

**The spike then caught the guard being too strict.** An exact "did we move up?"
test cut every item off at its marker on the pages where MuPDF emits the bullet
as its own line: a bullet's box starts a point or two *below* its text's,
because the glyph is small and the text has ascenders. The guard needs a
one-line tolerance. Under-extension on the real document went from 13 items to
zero.

The remaining stops all proved to be the gap guard doing real work. In this
document the body prose sits *right* of the list markers, so the indent rule
alone can never end an item there — the reviewer predicted exactly this, and it
is why the gap guard is load-bearing rather than polish.

### Also folded in

A numbered item no longer splits at its own marker. Rather than exporting the
marker grammar a second time, the rule is *a sentence never ends inside the
first token of a list item* — exact, because detection only accepts a marker
that is followed by a space or is the whole line, so on any line it accepted the
first token **is** the marker. No character-to-cell index mapping either, which
would have diverged the moment a bullet were drawn as an image.

Detection also moved inside `content_objects`, judged against the objects
already claimed rather than against raw heading ranges. A marker inside a table
is a table row; an item reaching a figure truncates at it instead of being
discarded for touching one. Disjointness is now by construction — the scan
breaks on a blocked line — with a `debug_assert` over the sorted result.

On the reference document: 36 items, 6 tables and 27 headings, all unchanged.

18 new tests, 343 total.

---

## 2026-07-29 — A list item always starts a sentence

List items rarely end in a full stop, so a whole bulleted list — and the line
introducing it, which usually ends in a colon — read as a single sentence. `s`
now stops at the start of every item.

Extraction shapes items two ways, and both had to work. Usually the bullet and
its text are one line (`• A standard way to install…`), but where the indent is
wide enough MuPDF emits the bullet as a line of its own followed by the text as
another. Keying on "this line opens with a marker" covers both: in the split
case the bullet line is the item's start and its text line simply continues it.

Detection is shape plus corroboration. A marker counts only when at least one
other line of the same kind starts at the same left edge, which is what
separates a real list from a sentence opening `1998. That year…`. Numbered
section headings are indistinguishable from enumerated items by shape, so lines
already known to be headings are excluded outright — without that, every `2.1.
Directory layout` in the reference document became a list item. On that document
the rule finds 36 item starts, all genuine bullets, no false positives.

The two sentence-walking choke points added the check, so nothing else moved.
Lists bound sentences only: `w` still walks the marker and the words after it,
and paragraph scope still reads a list as one paragraph, which is a coherent
model — the list is the paragraph, each item a sentence within it.

**Known limit, pinned by a test:** item *starts* are boundaries and nothing
marks where a list *ends*, so the final item joins whatever prose follows it,
exactly as any unterminated line always has. Fixing it needs list extents —
knowing which lines are continuations rather than the next paragraph.
`the_last_item_runs_on_into_the_prose_after_the_list` is the test that will
change the day that lands. *(It landed the same day: see the entry above, where
items became regions with extents and that test inverted.)*

9 new tests, 323 total. New fixture: `pdf_with_list`.

---

## 2026-07-29 — A number is one word and never ends a sentence

`3.14` used to be three word stops and, worse, two sentences: the decimal point
was read as a full stop, so `s` landed in the middle of a figure and a sentence
span stopped short. Now a separator with digits on **both** sides belongs to the
number, so `3.14` and `1,234.56` are each a single word and pass through sentence
detection untouched. The rule composes, which is what makes the grouped case work
without special-casing it.

A full stop that merely follows a number is unaffected — `it costs 3.` still ends
the word and the sentence, because nothing follows the stop. That asymmetry is
the whole rule: digits on both sides, or it is punctuation as before.

Only the same line counts. A figure is not carried across a line break, and
joining one would splice text that merely happens to end and begin with digits.

Both halves come from one pure predicate, `is_inside_number(before, sep, after)`,
consulted from `same_word_run` and `sentence_boundary_after`. Abbreviations
(`Mr.`, `e.g.`) remain a known simplification — they need a dictionary, not a
shape test.

9 new tests, 314 total.

---

## 2026-07-29 — Page furniture is out of the caret's path

Moving through a paper meant stepping through the running header on every page,
the folio at the foot, any sideways stamp down the margin and the big inclined
watermark preprints carry. None of it is reading matter. It is now dropped from
the navigable content layer entirely — not skipped by motion, *removed* — so
every motion, span and overlay ignores it with no change to the caret at all.
The lines are kept in `PageContent::furniture` rather than discarded.

### Two rules

**Rotation.** A line more than 10&deg; off the page's *dominant* direction.
Dominant rather than horizontal is what lets a page laid out sideways keep
everything, and it makes the rule provably unable to empty a page: the dominant
cluster is by construction the majority and is never flagged. The angle comes
from the character quads — `ur - ul` runs along the baseline and is well-defined
even for a single glyph — as a circular mean rather than a bucketed vote, since
bucketing splits one physical direction across the ±180&deg; wraparound.

**Repetition.** A margin-band line whose digit-masked text and baseline recur
across sampled pages. Position is never evidence on its own, which is why a
paper's title survives: it appears once. Digit masking means a folio matches
itself across pages, and `Chapter 7 of 9` is correctly one running head rather
than nine headings.

### Thresholds, and the observations that set them

Measured on the shared-mime-info spec before any code was written. Its running
header sits at baseline **56.19 on 18 of 19 pages — zero jitter** — so the 2.5pt
tolerance is generous; the same text appears once more at 88.82, as the *title*
on page 0, and correctly survives. Every ordinary line reported an angle of
exactly `0.0`, so 10&deg; is enormously slack. Eight sampling passes cost 13.4ms.

Baselines, not bounding boxes, are the position key: a descender shifts a box by
points, which would make a header match on some pages and not others.

Sampling is four anchors of **two consecutive pages**, not evenly spaced singles:
100 pages sampled 8 times steps by 14 and lands on one parity, so a book that
alternates verso and recto running heads would never see the recto one repeat.

The bare-folio rule was **cut**. Digit masking already makes page numbers repeat
like anything else, and a standalone "bottom-band number" rule would have
deleted the bare-number body lines this document carries at 674–686pt. The 30%
repeat floor is load-bearing for the same reason: two body lines coincidentally
sharing a baseline score 25% and are rejected.

### Three things the tests caught

- **Table detection would have died silently.** `content_objects` discards a
  table claiming every non-empty line — a guard that passes today only because
  the header and folio pad the count. Remove them and a full-page table trips
  it. The guards now count lines *plus* furniture.
- **`image_lines` indices needed remapping.** They point into the unfiltered
  vector; dropping lines without remapping compiles cleanly and labels a line of
  text an image.
- **The upright tiebreak was unreachable.** If an upright cluster carried ≥0.9×
  the leader, the leader could never hold 55% of the page, so the branch could
  not fire. Deleted; the dominance floor already gives the safe outcome.

### One deliberate override

A design review argued a divider page holding only a header and a folio *should*
end up with zero lines, since page stepping already skips empty pages. The test
suite disagreed loudly: the shared fixture gives each page one line, `Page N
text`, which normalises identically across pages, and the whole document became
unnavigable. That is the worst failure this feature can produce — text plainly
visible that the caret cannot reach — so the repetition rule now never takes a
page's last line. Cheap insurance against a catastrophic mode, at the cost of
leaving two lines on a genuinely blank divider page.

### Known limitations

A table continued across pages whose column-header row sits at the same height
each time will be removed if it falls inside the top band; long tables usually
start lower, and a gap test cannot separate this from a real running head.
Chapter titles that change per chapter are not caught — the principled
escalation is matching band text against `Document::outline()`, which already
exists. Images are never furniture, so a logo inside a running header survives.

`Ln N` in the status line now counts body lines: the running header no longer
occupies line 1. Nothing persists it, so no migration.

23 new tests, 305 total. New fixtures: `pdf_with_running_header`,
`pdf_with_rotated_text`.

---

## 2026-07-29 — Headings are single sentence and paragraph steps

Headings rarely end in a full stop, so the sentence walker ran straight through
them and glued a section heading to the paragraph below it; paragraph scope had
the same problem whenever a heading sat close to its body text. Now a heading is
one step for `s` and `p`: `s` lands on it, the next `s` lands on the body
beneath. A numbered heading such as `2.12. Recommended checking order` counts as
one sentence rather than three, and a heading that wraps is one step across both
lines.

### Why a heading is not a table

Deliberately **not** atomic. `w` still walks a heading's individual words and
`j` at line scope still moves line by line, because a heading is ordinary prose
you may want to select a phrase of — unlike a table, which has nothing useful
inside it to traverse.

That distinction is the whole change in the core. `ObjectKind` gained
`is_atomic()`, false only for `Heading`, and the accessors that were serving two
purposes at once split in two: `atomic_object_at`/`atomic_id_at` (motion,
highlighting, snapping) exclude headings, while `region_at`/`region_id_at`
(sentence-run bounds, paragraph splitting) include them. Being a region is
already enough to be one sentence and one paragraph — `split_segments_at_objects`
and `next_cell_in_region` needed no new logic — so `step_scope_atomic` was not
touched at all. Had the new variant simply been added, every object-consuming
site was kind-agnostic and headings would have become atomic everywhere.

One extra rule: `sentence_boundary_after` ignores punctuation inside a heading,
so `2.12.` cannot split one. Tables never needed this because their atomicity
masked it.

### Detection

From typography, not structure. Body size is the character-count mode of the
page — body text dominates by volume on every page, including title pages, which
makes it far steadier than a mean or median. A line is a heading when it is
`>= 1.15x` body size, or entirely bold at body size *and* narrower than 90% of
the widest line on the page (that last clause separates a bold subsection
heading from a bold lead-in sentence). Adjacent flagged lines of equal size and
weight merge, so a wrapped title is one heading.

Both signals come free from the existing extraction pass — `TextChar::size()` is
always populated and `TextCharFlags::BOLD` is set from the font's own bold flag —
so unlike table detection there is **no second pass and no runtime cost**.

Thresholds were set against a real document rather than guessed. 1.15 rather
than 1.10 because of an observed failure: on pages dominated by 9pt code
listings, ordinary 10pt prose is 1.11x the computed body size and was being
flagged wholesale. The runaway guard is 50% rather than the tables' stricter
shape because a title page legitimately is mostly large type. On the
shared-mime-info spec the result is 27 headings — the title, the author block
and every numbered section plus `References` — with no body text flagged.

MuPDF has its own heading detection (`FZ_STEXT_PARAGRAPH_BREAK`) and it is a
dead end: it wraps headings in structure nodes whose children the Rust bindings
cannot walk, the same wall the table work hit, and it keys on bold alone.

### Headings yield to tables

Bold table column headers (`Attribute`, `Required?`, `Value`) look exactly like
bold subheadings, and on the sample document they were flagged on precisely the
six pages where tables are detected. A heading range overlapping any existing
object is dropped, which removes all of them.

### Tests

19 new tests, 268 total. Detection is a pure function tested directly against
each threshold, and the behavioural tests inject a hand-built `PageContent` with
evenly-spaced lines — spacing at which the paragraph gap heuristic alone would
merge the whole page, so the tests prove the heading edges are doing the work.
`word_motion_still_walks_through_a_heading` is the guard on the accessor split.
New fixture: `pdf_with_heading`.

---

## 2026-07-29 — Tables and images are single navigation units

Moving through a paper used to mean crawling through its tables: a table was
just many short lines, so `w`, `s` and `p` stepped through it cell by cell and
a selection could only ever grab a fragment of it. Now a table or an image is
**one unit** at every scope above char — one motion lands on it, the next lands
past it — and selecting it takes the whole thing, drawn as a single rectangle.

Char scope is deliberately left raw: `cc` then `h`/`l` still walks a table's
individual characters, so a single number in a cell stays selectable. That is
the escape hatch, and it is also why the split is at char rather than making
tables opaque everywhere.

### How it is built

- `syodep-pdf` gained `ContentObject` (a run of lines that behaves as one unit)
  and `PageContent { lines, objects }`. `page_content` now takes
  `ContentOptions`.
- Tables come from a **second** structured-text pass with
  `TABLE_HUNT | COLLECT_VECTORS`, from which only the bounding boxes are taken.
  Two passes are needed because the detection pass rewrites the page: it moves a
  table's text into a structure node the Rust bindings cannot walk into, and
  splits lines while filling cells. Its geometry is therefore unusable for text,
  and the text pass is left exactly as it was.
- `COLLECT_VECTORS` is load-bearing, not a nicety. MuPDF hunts for tables among
  a page's ruled rectangles; with no vectors collected that list is empty and it
  falls back to hunting the whole page at a loose threshold. Measured on a real
  spec document: with `TABLE_HUNT` alone, two prose pages came back as one
  page-sized "table" and nothing else was found; adding `COLLECT_VECTORS` found
  six real tables with tight boxes and left the prose pages as the only false
  positives.
- Those remaining false positives are killed by one guard: a box whose lines are
  *every* line on the page is discarded. On the same document that guard was
  exactly precise — it rejected both bad pages and no good ones. Boxes mapping
  to a non-contiguous set of lines are dropped too. Degrading to line-by-line
  navigation is always safe; a wrong atomic unit is a very visible bug.
- Cost is ~2 ms per page for the second pass, and content is already cached per
  page for the session. `view.detect_tables` (default `true`) turns it off.

### Why the motion table was not touched

`step_scope` remains the pure per-scope description of what a word, line,
sentence or paragraph is. Atomicity is one wrapper over it — `step_scope_atomic`
— which after a step keeps stepping while the caret is still inside the object
it started in, and snaps to an object's start when it lands in a new one.
Because the wrapper has the same signature, counts (`5w`) and all six call sites
work unchanged and a table costs exactly one repetition. `e` is the exception,
landing on the object's *end* so a following `e` leaves rather than walking back
through it.

Two things the wrapper cannot fix, because they are about how far a span
*reaches* rather than where a step *lands*, and both needed their own change:
paragraph segments are cut at object boundaries afterwards
(`split_segments_at_objects`), and sentence runs stop expanding at an object
edge — a table cell rarely ends in `.`, so a sentence would otherwise run
straight through the table. Searching for the next sentence still crosses
freely, which is what makes a table simply become a sentence of its own.

### Tests

19 new tests, 249 total. The mapping from boxes to line ranges is a pure
function tested directly (including every discard rule), and the motion tests
inject a hand-built `PageContent` instead of relying on the heuristic — so a
future change in MuPDF's detection can only fail the one detection test rather
than the behavioural suite. New fixture: `pdf_with_table`.

---

## 2026-07-29 — 0.8.0

Since 0.7.0. **Existing configs keep working** — unlike 0.7.0, nothing was
removed. The new `[input]` section and the new command names are additive.

- **A pause now completes a key sequence.** `c` or `v` on its own enters focus
  or visual mode after a brief stop, keeping whatever granularity is live.
  Sequences typed at normal speed are unaffected: `cw` is still word focus.
  Tunable with `[input] timeout_ms` (500 ms default, `0` disables).
- **`s` and `p` move by sentence and paragraph**, at any scope, the way `w`
  already moved by word. They are motions, not scope changes: in word focus,
  `s` jumps to the next sentence's first word and the highlight stays
  word-sized, where `cs` stays put and highlights the whole sentence.
- **Open file moved from `o` to `<C-o>`.** `o` swaps the selection ends while
  selecting, so open-file was the one command that silently had no binding in a
  mode. This is the change most likely to disturb muscle memory.
- **Returning to normal mode resets the granularity to characters.** Normal
  mode has none of its own, so it no longer remembers one: `cs`, `<Esc>`, `v`
  now starts a character selection rather than a sentence one.
- **Leaving a selection by naming a scope keeps your place.** Previously only
  `<Esc>` did; `cw` and friends dropped you back where the selection started.

### If you rebound `o`

A config that sets `"o" = "open_file"` under `[keys]` keeps that binding — user
entries extend the defaults rather than replacing them — so `o` will still open
files for you *and* `<C-o>` will too. Remove the line to follow the new default.

---

## 2026-07-29 — `s` and `p` move by sentence and paragraph

### Implemented

Focus and visual mode gain two motions: `s` to the next sentence, `p` to the
next paragraph. Like `w`/`b`/`e` they work at *every* scope and leave the scope
alone — the highlight stays whatever size the active scope makes it.

Forward only, by choice: going back a sentence is `cs` then `h`. Not bound in
normal mode, matching `w`/`e`/`b` — a motion moves the highlight, and normal
mode has none.

New commands: `focus_next_sentence`, `focus_next_paragraph`,
`visual_next_sentence`, `visual_next_paragraph`.

### Why it was mostly a deletion

`step_scope(caret, scope, dir, …)` already maps a scope and a direction onto a
motion, and its `Scope::Word` arm calls exactly the functions the word commands
called:

```rust
Scope::Word => match dir {
    Dir::Left  => self.step_prev_word_start(caret),   // == focus_prev_word
    Dir::Right => self.step_next_word_start(caret),   // == focus_next_word
```

So "move one unit of a *named* scope, ignoring the active one" already existed
— `w` and `b` were the word instance of it, written out longhand. The feature
is the sentence and paragraph instances of the same idea.

The change was therefore a generalisation: one `focus_scope_motion(scope, dir,
count)` (and its visual twin) now backs `w`, `b`, `s` and `p`. `e` is the only
leftover — it targets a word run's *end* rather than a unit's start, which no
scope motion expresses — so `WordMotion` collapsed from three variants to one
and was deleted in favour of `focus_word_end` / `visual_word_end`.

### Test strategy

The refactor half was verified by the **absence** of test changes: routing
`w`/`b` through `step_scope` left all 211 existing tests passing untouched,
which is the proof it was behaviour-preserving. Only then were the new
commands added.

Six new tests cover motion-at-every-scope (char, word and line, asserting the
highlight keeps the *active* scope's width rather than the motion's), counts,
clamping at the document end, growing a visual selection, and that a bare `s`
does not shadow the `cs` chord.

The distinguishing property — motion versus scope change — is a rendering
question no unit test answers, so it was checked under Xvfb on a
two-sentence fixture:

| Keys | Result |
|---|---|
| `cw` | "First" highlighted, word-sized |
| `s` | "Second" highlighted — next sentence, **still word-sized** |
| `cs` | same place, now "Second sentence here." — whole sentence |

The plan predicted no FFI or Qt change (commands and keymaps only);
`git diff --stat crates/syodep-ffi ui-qt` came back empty, checked rather than
assumed.

### Notes / remaining

- `s`/`p` are bare keys while `cs`/`cp` and `vs`/`vp` are two-chord sequences
  on a different trie path, so all of them keep working. `s` *moves* by a
  sentence; `cs` *focuses by* sentence.
- No backward sentence/paragraph keys. `S` and `P` are still free if that
  changes.
- Bare scope letters are dropped as an idea; this covers the motion half of
  what it was for.

---

## 2026-07-28 — A pause commits a key sequence; `o` frees up; the scope resets

Three items from the post-0.7.0 review, shipped as three commits.

### `open_file` moves to `<C-o>`

`o` opened a file in normal and focus mode but swapped the selection ends in
visual mode, so open-file was the one normal-mode command that silently had no
binding in a mode. `o` is now free everywhere and the command works in all
three modes. The empty-state status line advertised the old key, so it moved
too.

### Returning to normal mode resets the scope

Normal mode has no granularity of its own, so it cannot sensibly remember one —
but it did: `cs`, `<Esc>`, `v` started a *sentence* selection. Every path back
to normal now goes through `enter_normal_mode`, which resets the scope and
keeps the position. Leaving visual back into *focus* still carries the scope,
since focus does have a granularity worth remembering.

### A pause ends a wait

**This reverses a deliberate design decision**, which is the reason it gets its
own section. `input.rs` said in three places that the state machine was
timer-free — *"the decision is made by the next key press, never by elapsed
time"* — and `docs/keybindings.md` advertised "no timeout — behavior is fully
deterministic". Those claims are now gone rather than left contradicting the
code.

A sequence that is both a binding and a prefix (`c`, `v`, `o` while selecting)
could previously only fire by following it with an unrelated key: `vk` entered
visual mode and then moved up. Now a pause resolves it, so `v` alone works.
`InputState::timeout` reuses the existing longest-prefix walk with one change —
the range is inclusive, so the full pending sequence is itself a candidate.
That one character is the whole feature.

Two rules that are not obvious:

- **A bare count never times out.** `12`, a pause, then `G` still jumps to page
  12. Only a partial *sequence* resolves, which is why `Effects::pending_input`
  is driven by a new `has_pending_sequence()` rather than `has_pending()`.
- **A junk sequence is dropped.** A half-typed `g` clears itself instead of
  waiting indefinitely for a key that may never come.

Bare `c` needed a command to fire: `focus_enter`, which is
`enter_focus(self.focus_scope)` — no new state. Combined with the scope reset
above, that gives exactly the requested behaviour: char from normal, the live
scope from focus or visual.

**The clock lives in the shell.** `InputState::timeout` is called by a QTimer
the widget arms when `pending_input` is set; the core never reads a clock. So
the core stays deterministic, and a test "waits" by calling `timeout` directly.
New config: `[input] timeout_ms`, default 500 ms, `0` to disable.

### Test strategy

Six unit tests in `input.rs` (fire a bound prefix, keep the count, leave a bare
count alone, drop an unbound partial, inert when nothing is pending, replay
leftovers) and two app-level ones covering `c`/`v` entry per source mode and
the `pending_input` flag.

Because the timer is shell-side, none of that proves the feature works, so it
was also driven under Xvfb: `c` + wait gives a one-character highlight with no
second key; `cw` typed fast gives word focus *without* also advancing a word;
`c`, wait, `w` gives char focus and then a word motion. The middle case is the
one that matters — it is the regression a too-short pause would cause.

The new `check-docs.sh` guard for `[input]` was verified by breaking it on
purpose: renaming the documented option made the script exit 1. A guard that
has never failed is not known to guard anything.

### Notes / remaining

- `o` is now unbound in normal and focus mode. Left that way deliberately: it
  is the obvious home for a future binding.
- Drive-by: `docs/roadmap.md` still said `vl`, stale since the `e` rename.
- Bare scope letters remains blocked on `w`/`e`/`b` being word motions in both
  focus and visual mode.

---

## 2026-07-28 — The visual head *is* the focus position

### Implemented

Visual mode no longer stores its moving end. It stores only the anchored end
(`VisualAnchor`); the head is the app's `focus`/`focus_scope` — the same pair
focus mode uses. `VisualSelection` survives as a read-only view assembled by
`visual_selection()`.

Removed: `visual_goal_x`, `visual_goal_y`, `update_visual_goal_x`,
`update_visual_goal_y`, and `VisualSelection::swap_ends` (now `App::swap_visual_ends`,
which exchanges the anchor with the focus position).

Two user-visible changes fall out:

- **Leaving visual mode with a `c` chord keeps your place.** Previously
  `cw`/`ce`/… restored the position from *before* the selection started and
  discarded the head. `<Esc>` was correct; nothing else was.
- **The scope carries out too.** `cw`, `v`, `ve`, `<Esc>` now leaves you in
  *line* focus rather than reverting to word. Position and scope had been
  obeying different rules.

### Why

`self.focus` and `self.visual.head` both claimed to hold "where you are", and
were only reconciled inside `exit_visual`. Any exit that did not go through
that function read a stale value — and `enter_focus`, reached by the `c`
chords, is exactly such an exit.

This is the same failure mode the 0.7.0 collapse eliminated — derivable state
stored twice and kept in sync by discipline — surviving in the one place that
refactor did not reach. Patching the single bad read was the cheaper option and
was rejected: it fixes the symptom and leaves the trap armed for the next
caller. Deleting the duplicate makes the bug unrepresentable, which is why
`enter_focus` needed **no change at all** in the end.

### Test strategy

Both bug tests were written first and observed to fail:

```
leaving_visual_by_scope_chord_keeps_the_position
  left:  Some(Caret { page: 0, line: 0, cell: 0 })    <- pre-selection
  right: Some(Caret { page: 0, line: 0, cell: 6 })    <- the head
scope_carries_out_of_visual
  left:  (Focus, Word)    right: (Focus, Line)
```

The first version of that test exited at *line* scope, which snaps the column
to 0 — indistinguishable from the bug on a one-line fixture. It kept failing
after the fix was correct. Rewritten to exit at word scope (where the landing
cell is the head's word run) plus a two-line case for line scope, it
distinguishes the two properly. A test that cannot tell success from the bug it
targets is worse than no test.

`swapping_ends_exchanges_positions_and_scopes` moved from a `caret.rs` unit
test of the pure swap to an app-level `oo` test, since the swap now spans two
pieces of state. 202 tests pass.

The plan predicted the FFI and Qt would need no change, since `VisualSelection`
never crossed the crate boundary; `git diff --stat crates/syodep-ffi ui-qt`
came back empty, as a check rather than an assumption.

Verified visually under Xvfb: `cw` highlights the first word, `v`+`l` grows a
grey selection over two words, and `cw` then leaves the blue focus on the
*second* word — where the head was.

### Notes / remaining

- No version bump; 0.7.0 shipped hours earlier. This rides the next release.
- Still open from the same review: `o` (open file) is unreachable in visual
  mode, since `o` is swap-ends there; and `v` from normal mode inherits the
  last-used scope rather than defaulting to char.
- Bare scope letters remains blocked on `w`/`e`/`b` already being word motions
  in both modes.

---

## 2026-07-28 — 0.7.0

Since 0.6.0. **This release breaks existing configs.** See the migration below.

- **The five focus modes are now one focus mode with a scope.** `Mode` went
  from seven variants to three (`Normal`, `Focus`, `Visual`). Granularity —
  char, word, line, sentence, paragraph — is a *setting* of focus mode, not a
  mode of its own, matching how visual mode has always worked.
- **Changing granularity no longer teleports you.** Pressing `ce` while focused
  on a word now highlights the line you are on. It used to restore wherever you
  last left line focus, possibly pages away, because each mode kept its own
  mark. There is one position now, so the bug cannot occur.
- **The entry chords double as scope switches.** `cc`/`cw`/`ce`/`cs`/`cp` work
  from inside focus mode and change the scope in place.
- **`w`/`e`/`b` move by a word at every scope**, mirroring visual mode. They
  used to be bound only in caret focus, and meant scope motion in word focus.
- **The line scope is identified by `e`, not `l`** (`ce`, `ve`, `oe`). `l` is
  the forward motion in every mode and could not also name a scope.

Your keys are otherwise unchanged: `cc`/`cw`/`ce`/`cs`/`cp` still enter, `hjkl`
still move, `<Esc>` still exits.

### Migrating a config

If your config has none of the tables below, nothing to do.

The five focus key tables became one. Rename whichever you have to
`[focus_keys]`, merging them if you had more than one, and replace the command
names:

| was | now |
|---|---|
| `[caret_focus_keys]`, `[line_focus_keys]`, `[word_focus_keys]`, `[sentence_focus_keys]`, `[paragraph_focus_keys]` | `[focus_keys]` |
| `caret_focus_left`, `line_focus_left`, `word_focus_left`, `sentence_focus_prev`, `paragraph_focus_prev` | `focus_left` |
| the `*_right` / `*_next` equivalents | `focus_right` |
| the `*_up` equivalents | `focus_up` |
| the `*_down` equivalents | `focus_down` |
| `caret_focus_next_word` / `_end_word` / `_prev_word` | `focus_next_word` / `focus_end_word` / `focus_prev_word` |
| `*_focus_exit` | `focus_exit` |
| `caret_focus_enter`, `line_focus_enter`, … | `focus_enter_char`, `focus_enter_line`, … |

The motion commands dispatch on the active scope, which is why five sets
collapse to one: `focus_left` is a character in char scope, a word in word
scope, a column jump in line scope and the previous unit in sentence or
paragraph scope.

**An unmigrated config fails to load entirely**, not just its key table —
syodep rejects unknown fields so a typo cannot silently do nothing. The error
message names the stale tables and their replacement. A fresh, fully commented
reference config is at `config/default-config.toml`, and `syodep --defaults`
writes one.

### For anyone embedding the core

The C ABI changed shape. `SyoCaret`, `SyoSentence` and `SyoSelection`, plus
`syo_app_caret`, `syo_app_line`, `syo_app_word`, `syo_app_sentence`,
`syo_app_paragraph` and `syo_sentence_free`, are replaced by one `SyoOverlay`
with `syo_app_focus`, `syo_app_selection` and `syo_overlay_free`. Focus and
selection overlays are the same shape because a focus highlight is a selection
whose two ends coincide.

---

## 2026-07-28 — Five focus modes collapse into one mode with a scope

### Implemented

`Mode` went from seven variants to three: `Normal`, `Focus`, `Visual`.
Granularity is no longer a mode — `Focus` carries a `Scope` (char, word, line,
sentence, paragraph), exactly as each end of a visual selection already did.

| | before | after |
|---|---|---|
| `Mode` variants | 7 | 3 |
| Focus commands | 29 | 13 |
| Focus keymaps / config tables | 5 | 1 |
| FFI overlay getters | 5 | 1 (`syo_app_focus`) |
| Focus state fields | 8 | 5 |
| Per-mode docs pages | 5 | 1 |

The app now stores one `Caret` plus a `Scope`. What is *drawn* is derived by
`scope_span(caret, scope)` and cached in `focus_span`, mirroring how
`visual_span` has always worked. The five stored marks (`line_mark`,
`word_mark`, `sentence_mark`, `paragraph_mark`, plus three goal values) are
gone: they were derivable state the code stored and then failed to keep
consistent.

Breaking changes, all deliberate:

- `[caret_focus_keys]`, `[line_focus_keys]`, `[word_focus_keys]`,
  `[sentence_focus_keys]` and `[paragraph_focus_keys]` are **removed**,
  replaced by one `[focus_keys]`.
- The `caret_focus_*` / `line_focus_*` / `word_focus_*` / `sentence_focus_*` /
  `paragraph_focus_*` command names are **removed**, replaced by
  `focus_enter_{char,word,line,sentence,paragraph}`, `focus_exit`,
  `focus_{left,right,up,down}` and `focus_{next,prev,end}_word`.
- The status bar reads `-- FOCUS (word) --`, matching `-- VISUAL (word) --`.
- FFI: `SyoCaret`, `SyoSentence`, `SyoSelection`, `syo_app_caret`,
  `syo_app_line`, `syo_app_word`, `syo_app_sentence`, `syo_app_paragraph` and
  `syo_sentence_free` are replaced by `SyoOverlay`, `syo_app_focus`,
  `syo_app_selection` and `syo_overlay_free`.

Default *keys* are unchanged: `cc`/`cw`/`ce`/`cs`/`cp` still enter, `hjkl` still
move, `<Esc>` still exits.

### Why

A bug fixed by construction. `enter_*_focus` seeded its mark only when it was
`None`, so switching granularity landed you on a *stale* position: work in word
focus, scroll to page 30, press `ce`, and you were back wherever you last left
line focus. With one position and a scope field the bug is unrepresentable —
there is nothing to be stale. `changing_scope_keeps_the_position` covers it and
fails on the old code.

Three earlier items were symptoms of the same duplication: unifying five
hardcoded overlay colours, `Mode::CaretFocus` appearing in a dozen
enumerations that grew with every mode, and the planned bare-scope-letter
feature being self-contradictory under five modes ("stay in the mode" *is* a
mode change under the old model; under the new one it is a field assignment).

The unifying idea, now written into `docs/architecture.md`: **a focus highlight
is a selection whose two ends coincide.** That is why one `scope_span` derives
both extents, one `step_scope` table serves both motions, and one
`span_screen_rects` produces both overlays.

The entry chords doubling as scope switches falls out for free — the focus
keymap is the normal keymap plus overrides, and `c` is not overridden, so `cw`
already worked inside focus mode. It now changes the scope in place instead of
switching modes. No `focus_scope_*` commands were needed.

### Test strategy

The two de-risking commits landed first and separately: extracting the shared
`step_scope` motion table (which passed the existing suite with **zero test
edits**, converting "focus and visual already agree scope-for-scope" from an
assumption into a result — and exposing that Line scope did *not* agree), then
renaming `VisualScope` to `Scope`.

For the collapse itself, the ~100 test call sites that read the old per-scope
marks were kept working by **deriving** those shapes from `focus_span` in
`#[cfg(test)]` helpers. That is not a compatibility shim for its own sake: it
means every one of those assertions now checks what is actually drawn rather
than a parallel field, so they kept their value instead of being rewritten into
something weaker. 198 tests pass.

New tests: `changing_scope_keeps_the_position` (the bug above),
`visual_inherits_and_returns_every_focus_scope` (round-trips all five scopes
through visual and back), `word_motions_work_at_every_scope`, and
`pre_collapse_focus_tables_get_a_migration_hint`.

Verified beyond the suite: `cargo fmt`, `clippy -D warnings`,
`./scripts/check-docs.sh`, the Qt build, the offscreen smoke test, and an Xvfb
screenshot per scope confirming each highlight still renders at the right
extent in one uniform colour with no borders — plus a visual-mode capture
confirming the two overlays still differ.

### Notes / remaining

- **Old configs fail to parse entirely.** `deny_unknown_fields` rejects the
  whole file, so a config still naming `[word_focus_keys]` loses `[view]` and
  `[files]` too. The clean break stands, but the parse error now appends a
  migration hint naming the stale tables and the replacement — the bare serde
  message did not say what to do.
- **Paragraph highlights now render per line** rather than as one solid block,
  matching sentence and visual paragraph scope. The right edge is ragged where
  it used to be flush. Deliberate: it is the same span machinery for everything.
- Entering focus mode now always starts from the top-most visible line, for
  every scope. Char and line focus previously started at line 0 of the current
  page while word, sentence and paragraph used the viewport. The viewport
  behaviour is the better one and is now uniform.
- Warrants a **0.7.0** release with the breaking-change note.
- The bare-scope-letter feature (`w`/`e`/`s`/`p`/`c` switching scope in place)
  is now coherent to specify, and is the natural next step. Note the tension it
  still has to resolve: `w`, `e` and `b` are already word *motions* in both
  focus and visual mode.

---

## 2026-07-28 — Line scope is identified by `e`, not `l`

### Implemented

Renamed the *line scope identifier* in every chord:

| was | now |
|---|---|
| `cl` | `ce` — enter line focus |
| `vl` | `ve` — enter visual with line scope |
| `vl` (in visual) | `ve` — set the active end to line scope |
| `ol` (in visual) | `oe` — switch ends and set line scope |

`l` as a *motion* is untouched: it still moves forward in every mode and
scrolls right in normal mode.

### Why

Groundwork for bare scope letters (pressing `w`, `s`, `p`, … inside a focus or
visual mode to switch granularity in place). That feature needs one letter per
scope, and `l` cannot be it: `l` is the forward motion in **all seven** modes
(`scroll_right`, `caret_focus_right`, `line_focus_right`, `word_focus_right`,
`sentence_focus_next`, `paragraph_focus_next`, `visual_right`), so binding it
to a scope would gut navigation everywhere.

`e` costs far less: it is only bound in caret focus (`caret_focus_end_word`)
and visual (`visual_end_word`), and those keep working — this change touches
only the `c`-, `v`- and `o`-prefixed chords, which live on different trie paths
from the bare `e` binding.

Doing the rename first, on its own, keeps it separable from the behavioural
change that follows.

### Test strategy

No new behaviour, so no new tests: the existing suite covers the rename by
construction. The three FFI and app-level tests that drove line focus through
`c`+`l` now use `c`+`e`, and one comment that read "a single `c` is only the
first half of `cl`" was updated. `config/default-config.toml` was regenerated
from `default_config_doc()` rather than hand-edited; the diff is exactly the
five renamed bindings and the one prose line.

### Notes / remaining

- **This breaks muscle memory and existing user configs.** Anyone with `cl` in
  a `[keys]` table keeps it working — user tables extend the defaults rather
  than replacing them — but `cl` no longer enters line focus by default, and a
  config that rebinds `cl` to something else now leaves `ce` live as well.
- Historical dev-log entries still say `cl`/`vl`/`ol`. They describe what was
  true when written and are deliberately left alone.

---

## 2026-07-28 — 0.6.0

Since 0.5.0:

- **Drag and drop.** A PDF dragged onto the window opens it. Non-PDFs are
  refused while still being dragged, so nothing happens on release; dropping
  several opens the first and says how many were ignored.
- **Overlay colours are consistent and configurable.** All five focus modes
  share one colour and the selection has its own, set through
  `[view] focus_color`/`visual_color` and their opacities. No borders, and
  overlapping boxes are merged before filling so a multi-line highlight is one
  flat block instead of a banded ladder.
- **`[view] background` works.** It had been defined, documented and ignored
  since it was introduced — the shell hardcoded `#1e1e1e`.

---

## 2026-07-28 — One overlay colour per mode, configurable

### Implemented

- **All five focus modes share one colour** (`[view] focus_color`, default the
  light blue `#add8e6`); the selection uses `visual_color` (light grey
  `#d3d3d3`). Previously each mode had its own hardcoded accent — caret blue,
  line orange, word green, paragraph purple, sentence red, visual teal — so the
  highlight encoded *which scope* rather than the useful signal, focus versus
  selection.
- **No borders anywhere.** Five overlays used to draw an opaque 1px outline
  plus a fill at alpha 70 while the selection was fill-only at alpha 110, so
  two overlays with identical geometry semantics looked unrelated.
- **Overlapping boxes no longer double-blend.** Rectangles go into a
  `QPainterPath` that is `simplified()` before a single fill, which merges
  intersecting subpaths into an outline with no intersecting edges. This was a
  real defect, not a hypothetical: both multi-rect overlays take y from
  `line.bbox`, which is MuPDF's `line.bounds()` — the union of glyph quads
  including ascenders and descenders — so consecutive lines genuinely overlap,
  and the `width < 2.0` guard widens rects rightwards as well.
- **`[view] background` works again.** It was defined, documented and *dead*:
  `canvas_widget.cpp` hardcoded `#1e1e1e`, and the `setBackgroundColor()` hook
  had never been called from anywhere. Fixed with the same plumbing rather than
  left as a documented option that does nothing.
- Colour and opacity are separate options, so opacity can be tuned without
  rewriting a hex value.

### Design

Colours are resolved **once in `syo_app_new`** and cached on `SyoApp`, exactly
like `open_dir`: an unparseable value falls back to the built-in default and
reports the problem, so a typo degrades instead of producing an invisible
overlay. Opacity is clamped to `0.0..=1.0`. A new `#[repr(C)] SyoColor` and
three getters carry them to Qt — these are the **first `[view]` values ever
exposed over the FFI**; every other one is consumed inside the core.

Parsing lives in Rust rather than letting `QColor` do it, so bad values surface
through the same channel as every other config problem. `#rrggbb` only:
opacity is separate, so an eight-digit value is rejected rather than silently
reinterpreted.

The five focus getters were left as they are. Collapsing them into one
`syo_app_focus_rects()` would make the single colour structural rather than
conventional, but that is an FFI change beyond this task; the Qt-side union
gives the same visible result.

### Test strategy

Rust: `parse_hex_color` accept/reject cases, opacity clamping, config
defaults + override, and FFI tests that a configured colour reaches the getter
and that an invalid one falls back and warns.

Colour cannot be checked by exit code, so it was verified on screen under Xvfb:

- caret, word and line focus all render the *same* light blue,
- a paragraph selection is one flat grey block — sampling it gives a single
  value, `#EDEDED`, which is exactly the predicted blend of `#d3d3d3` at 0.4
  over white (211 × 0.4 + 255 × 0.6 = 237) and is identical on different
  lines. A double-blended overlap would show a second, darker value,
- a config setting `background`, `focus_color` and `visual_color` to red/blue/
  dark green takes effect for all three — the background pixel reads `#204020`,
  which was impossible before,
- `focus_color = "lightblue"` falls back to the default and shows
  `ERROR: invalid focus_color "lightblue" in [view]; using the default #add8e6`.

### Notes / remaining

- That warning lives in `last_error`, which `open_document` clears on success,
  so it is visible until a document is opened. Pre-existing behaviour shared
  with the `open_dir` warning, not introduced here.
- The default opacity of 0.4 was chosen by looking at the result, not derived.
  Pale fills with no border need more presence than the old saturated bordered
  boxes (alpha 70 ≈ 0.27).

---

## 2026-07-28 — Open a PDF dropped onto the window

### Implemented

- `MainWindow` accepts drops (`ui-qt/src/main_window.{h,cpp}`): a `.pdf`
  dragged onto the window opens it through the existing
  `MainWindow::openDocument`, which already backs the CLI argument and the
  file dialog. No new document logic, and nothing added to the core.
- Anything that is not a `.pdf` is **refused during the drag**, so the cursor
  shows "no entry" and releasing does nothing — the user finds out before
  letting go rather than getting an error afterwards.
- Dropping several PDFs opens the first and reports the rest as ignored in a
  transient status message, so the discard is visible instead of silent.

### Why it lives in the Qt shell

`AGENTS.md` says new behaviour goes in the core as a `Command`, never in Qt
event handlers. That rule is about *document/navigation behaviour*; a drop is
an OS input gesture, the same category as the mouse wheel and the CLI
argument, neither of which is a `Command` either (`wheelEvent` calls
`syo_app_scroll_by` directly; `main.cpp` calls `openDocument` directly). The
document logic was already in the core and already reachable — the handler
extracts a path and calls it.

Only `MainWindow` needed `setAcceptDrops(true)`. `CanvasWidget` is a
`QOpenGLWidget` covering the whole window, but it never enables drops, and Qt
delivers drag events to the nearest ancestor that accepts them rather than
stopping at a child that does not. That was the one assumption worth proving
rather than believing, and the test below proves it.

### Test strategy

No Rust changed, and `docs/testing.md` puts Qt widget behaviour outside the
unit-test boundary by design — so the workspace suite and `--smoke-test` only
show nothing regressed. `runSmokeTest` builds a `MainWindow` but never calls
`openDocument`, and a drop is not a key event, so neither touches this code.

`xdotool` cannot originate a drag (XDND needs a real drag *source*), so a
throwaway ~30-line Qt drag-source app was built in the scratchpad and driven
against syodep under `Xvfb`, with `xdotool` moving and releasing the mouse.
All three cases were confirmed by screenshot:

- a `.pdf` dropped on the window opens (status line `dropme.pdf [1/4] 118%`),
- a `.txt` is refused — still `no document - press 'o' to open a PDF`, and
  notably **no error**, because the drag was rejected rather than
  accepted-then-failed,
- two and three PDFs produce `Opened dropme.pdf - 1 other file ignored` and
  `- 2 other files ignored`.

That last check caught a real blemish: the message first used `tr()`'s `%n`
plural form, which needs a translation catalogue to resolve. With none loaded
`tr()` returns the source string unchanged, so users would have seen the
literal `1 other file(s) ignored`. Spelled out explicitly instead.

### Notes / remaining

- No config option to disable it; `[files]` would be the natural home, but a
  toggle for a standard gesture nobody triggers by accident is not worth the
  surface.
- No `QFileOpenEvent` handling (the macOS "open with" event) — there is no
  macOS build.
- Dropping onto the taskbar/desktop icon is file association, already covered
  by `packaging/syodep.desktop` and the Windows installer.

---

## 2026-07-28 — 0.5.0

First release carrying the Windows installer, and the first where
`--version` is trustworthy.

Since 0.4.0:

- **Visual selection mode** — a two-ended selection over the content layer.
  `v` inherits the current focus mode's granularity, `vc`/`vw`/`vl`/`vs`/`vp`
  name it, and each end carries its own scope so `o` plus a scope letter
  re-scopes the far end independently. Required teaching the input state
  machine longest-prefix fallback, which is a behaviour change in its own
  right.
- **Windows installer** — `syodep-vX.Y.Z-win64-setup.exe`: per-user, no UAC,
  silent-capable, opt-in PDF handler, and an uninstaller that leaves
  `%APPDATA%\syodep` alone because Scoop and portable installs share it.
- **One version source.** `Cargo.toml` is now the only place the version
  exists; CMake reads it and the shell reports it. 0.4.0 shipped binaries that
  told users they were 0.3.0.
- **App icon.** `syodep.exe` finally has one, plus a version resource, and the
  SVG no longer depends on a font — which also fixes the AppImage icon.
- **Release pipeline fix.** The AppImage build had been failing on a missing
  `unzip` in the container, so nothing was published between 2026-06-26 and
  2026-07-27.

Bumping `Cargo.toml` is the whole of a version bump now: `syodep --version`
reported 0.5.0 from a clean rebuild with no other file touched.

---

## 2026-07-27 — Windows NSIS installer

### Implemented

- **`packaging/syodep.nsi`**, compiled by `makensis` in `release-build-windows`
  over the `syodep-win64/` tree the staged smoke test has already validated. The
  script installs files; it never builds them.
- Per-user install to `%LOCALAPPDATA%\Programs\syodep` (no UAC), Start-menu
  shortcut, Add/Remove Programs entry including `QuietUninstallString`, silent
  `/S` install and uninstall, `/ASSOCIATE` to opt into the PDF picker.
- Attached to **tagged releases only** as `syodep-vX.Y.Z-win64-setup.exe`;
  `continuous` still carries just the zip and AppImage.
- **A tag-vs-`Cargo.toml` assertion** in the installer build: tagging `v0.5.0`
  without bumping would otherwise ship an installer and a Scoop manifest
  asserting a version the binary does not report — the same class of bug as the
  version drift fixed earlier today, one layer up.
- **The script is compiled on Linux in the `rust-lint` CI job.** `makensis` is
  cross-platform, so a broken script fails in ~1 minute rather than after the
  12-minute Windows build.

### Why the local-first workflow mattered

Installing `nsis` locally (3.10, the same version the runner has) paid for
itself immediately: the very first compile failed with

    warning 6000: unknown variable/constant "{SecAssoc}" detected, ignoring

because `.onInit` referenced the section before it was defined — NSIS resolves
`${SecAssoc}` at parse time, so the reference silently expanded to nothing and
`/ASSOCIATE` would have been a no-op. Caught in seconds; it would otherwise have
been a twelve-minute round trip, and `-WX` is what turned the warning into a
failure rather than a silently broken installer.

The same compile also revealed that **NSIS resolves a relative `OutFile` against
the script's directory, not the working directory** — the first successful build
dropped a 563 KB binary into `packaging/`. `OutFile` is now `${OUTFILE}`, passed
explicitly, with `packaging/*.exe` gitignored as a backstop.

Two more only surfaced in CI, both because the local invocation differed from
the CI one in a way that hid them.

`-DSRCDIR=syodep-win64` (relative) made makensis report *"Error while loading
icon from syodep-win64\\syodep.ico: can't open file"* — the same
script-relative resolution rule as `OutFile`, so it hunted for
`packaging/syodep-win64/`. The error names the icon, which reads like a missing
file rather than a wrong base directory. Every local test had passed an
absolute path and never exercised the relative case. All paths handed to
makensis are now absolute, and the script says so where the defines are
declared.

The other: Ubuntu's Ubuntu's
`imagemagick` package is ImageMagick **6**, whose command is `convert`, while
`magick` exists only in 7, so the lint job died with `magick: command not
found`.
`ui-qt/CMakeLists.txt` already accepted either via
`find_program(... NAMES magick convert)`; the workflow now does the same.

And running the CI lint step locally caught a third: **`makensis` aborts with
`free(): double free detected` (SIGABRT, exit 134) when `MUI_ICON` points at an
invalid `.ico`**, rather than reporting a readable error. The stub tree had
created the icon with `touch`, so it was zero bytes. The lint job now generates
a real icon from `packaging/syodep.svg`, which has the side benefit of failing
that job if the SVG ever stops rendering.

### Decisions

- **Uninstall leaves `%APPDATA%\syodep` alone.** It is shared with Scoop and
  portable installs, so wiping it would destroy reading positions belonging to a
  syodep this installer never owned — silently, via `uninstall.exe /S`. This
  reverses an earlier decision, taken before that sharing was noticed.
- **The PDF checkbox cannot claim the default handler**, and does not pretend
  to. Since Windows 8 the effective association lives in a hash-protected
  `UserChoice` key; the script registers a ProgID, `Applications\syodep.exe`
  and `OpenWithProgids` so syodep *appears* in the picker, and the wizard text
  says "Offer syodep as a PDF handler" rather than promising a default.
- **`SetErrorLevel 2` before every `Abort`.** NSIS exits 0 by default, so a
  silent install can fail invisibly — which would have made the whole CI gate
  theatre.
- **`RMDir /r "$INSTDIR"` is guarded** (path length, plus `syodep.exe` and
  `Uninstall.exe` present) because `$INSTDIR` is user-controllable through
  `/D=`. A hand-maintained file manifest was rejected: `windeployqt` emits a
  Qt-version-dependent tree that would go stale on every Qt bump.
- **Upgrades delete the stale payload in place** rather than running the old
  uninstaller. `File /r` overwrites but never removes, and a leftover Qt DLL
  from an older Qt is a startup crash with no useful message.

### Test strategy

CI-only change; no core logic touched, so no new Rust tests. The script compiles
locally against a stub tree and produces a valid PE.

The CI verification is a real gate rather than a compile check: silent install,
assert the payload and Qt plugin, assert the ARP values (including a
plausibility range on `EstimatedSize`, which catches the classic bytes-for-KiB
slip), assert the association was **not** set under a plain `/S`, run the
*installed* binary offscreen with Qt stripped from `PATH`, then uninstall and
assert everything is gone.

Three traps it is written around:

- `uninstall.exe /S` normally relaunches from `%TEMP%` and returns immediately,
  so every assertion after it races and passes only on a fast runner. `_?=`
  keeps it in place; it must be the last argument, and it means the uninstaller
  cannot delete itself.
- The "user data survives" assertion would be **vacuous** without planting a
  sentinel first: `--smoke-test` runs `syo_app_new(nullptr, nullptr)` and never
  creates `%APPDATA%\syodep`.
- A **negative test** (install into an unwritable path must exit non-zero) is
  what proves `SetErrorLevel` works. Without it every other assertion rests on
  an installer that might always exit 0. Two things had to be right for it.
  The target has to be unwritable *regardless of privilege* — the first attempt
  used `C:\Windows\System32`, which CI can write to because it runs elevated.
  And the check has to run in `.onInit`, not in a section: `Abort` in an install
  section cancels the section but the process still exits 0. Under `/S` there is
  no directory page, so `$INSTDIR` is already final at `.onInit` and can be
  rejected there.

  **The test's premise was impossible.** Logging `${GetParameters}` and
  `$INSTDIR` from `.onInit` on the runner showed `$INSTDIR` was the *default*
  install directory, not the unwritable one: **NSIS validates `/D=` and
  silently falls back to `InstallDir` when the path is unusable.** So the
  installer installed itself to its default location and correctly exited 0.
  `CheckWritable` passed because the directory it was handed genuinely was
  writable. Nothing was ever aimed at the bad path.

  There is therefore no way to provoke a failed install through `/D=`, and the
  negative test was removed rather than kept in a form that tests nothing.
  `CheckWritable` remains unexercised by CI; it is reachable only through a
  directory chosen interactively.

  Seven dispatches went into this, each testing a hypothesis about NSIS
  internals, when the fault was that the installer was never receiving the
  input the test claimed to give it. Two process errors made it worse: the
  instrumentation that settled it in one run should have gone in after the
  second failure rather than the seventh, and it was reverted once before the
  fix was confirmed, which cost another cycle. When something "does not fire",
  log its input before theorising about its logic.

  Chasing this is what motivated running the installer under **Wine** locally,
  which turned a 12-minute dispatch into a few seconds and settled three
  questions by experiment rather than by guessing: `SetErrorLevel` + `Abort` in
  `.onInit` under `/S` *does* yield exit 2; `/D=` *is* already visible in
  `.onInit`; and `CreateDirectory`/`FileOpen` do **not** reliably raise the
  error flag. So `CheckWritable` now tests outcomes — does the directory exist,
  is the handle non-empty, did the probe file actually land — rather than
  trusting the flag.

  Wine's limits are worth recording too: it happily "succeeds" at creating a
  directory beneath a regular file, so it cannot reproduce Windows filesystem
  failures. It is good for exit-code and control-flow semantics, useless for
  permission semantics.

### Notes / remaining

- **Unsigned.** SmartScreen will warn until the binary earns reputation, and
  reputation is per-publisher, so an unsigned build never accrues any. Signing
  needs an Authenticode certificate on FIPS hardware, meaning a cloud signing
  service rather than a secret in CI. Tracked as a new roadmap bullet.
- Antivirus false positives are common for unsigned NSIS installers.
- Scoop and the installer can coexist (Scoop shortcuts live under
  `Scoop Apps\`), but both appear in the picker and whichever `syodep.exe` is
  on `PATH` wins for CLI use.

---

## 2026-07-27 — App icon: font-free SVG, generated .ico, Windows resource

### Implemented

- **`packaging/syodep.svg` no longer contains text.** The Vim-style `:` was a
  `<text>` element at `font-family="monospace"`; it is now two `<circle>`s.
- **`syodep.ico` is generated, not committed** (`AGENTS.md:66` forbids checked-in
  generated binaries): the Qt build runs ImageMagick over the SVG to produce a
  7-frame icon (16/24/32/48/64/128/256).
- **`syodep.exe` now carries an icon and a VERSIONINFO block**, via
  `ui-qt/syodep.rc.in` configured by CMake. `enable_language(RC)` is called only
  under `if(WIN32)`, so non-Windows builds never look for a resource compiler.
  Version strings come from `SYODEP_VERSION`, so the file-properties dialog
  cannot disagree with `--version`.
- **Graceful degradation**: `find_program(magick)` — when ImageMagick is absent
  the whole resource is skipped with a warning and the build still works.
- The release job copies the generated icon into `syodep-win64/` and verifies
  it before shipping.

### Why

An icon built from a font glyph renders differently on every machine: the face,
weight and metrics depend on what the system resolves for `monospace`, and at
16px an unhinted glyph turns to mush. This was already live in the **AppImage**,
which ships this SVG as its icon — so the fix is not merely preparatory for
Windows.

Separately, `syodep.exe` had no icon or version resource at all, so Explorer,
the taskbar and Add/Remove Programs showed a blank generic document.

A design review claimed ImageMagick's internal renderer ignores `text-anchor`
outright, which would have meant the glyph was clipping outside the page rect.
**That could not be reproduced**: the local ImageMagick has the rsvg delegate,
renders the two anchorings differently, and `msvg:` did not bypass it. The
change was made on the ground that a shape-only icon renders identically under
every renderer — not on the unverified claim.

### Test strategy

No core logic touched, so no new Rust tests. Verified by rendering and *looking
at* the output rather than trusting exit codes: before/after at 256px, and the
16/32/48px frames pixel-magnified. At 16px the dots disappear and the icon
reduces to "document with a yellow highlight" — acceptable, and no worse than
the glyph, which was equally invisible there.

The CI blank-icon guard was validated both ways locally: the real icon scores
sd=0.387 and passes; a deliberately empty `.ico` scores sd=0 and is rejected.
A broken SVG render produces a structurally *valid* but empty icon, which a
frame count alone would not catch.

Linux builds were confirmed unaffected — a from-scratch configure and build
emits no warnings, generates no `.ico`, and produces a working binary.

### Notes / remaining

- The 16px frame is muddy: the outer rounded square plus the page inset leaves
  little room. Worth revisiting if the placeholder art is ever replaced.
- The taskbar prefers the *window* icon over the exe resource. Fully fixing the
  taskbar would need `QApplication::setWindowIcon` fed from a `.qrc`; not done.

---

## 2026-07-27 — One version source, not three

### Implemented

- `CMakeLists.txt` **reads the version out of `Cargo.toml`** at configure time
  rather than holding a copy: `file(READ)` plus a regex scoped to the
  `[workspace.package]` section (`[^[]*` stops at the next section header, so a
  dependency version can never be picked up by mistake). Pre-release suffixes
  survive for display while `project()` gets the numeric part, and
  `CMAKE_CONFIGURE_DEPENDS` on `Cargo.toml` makes a bump re-configure by itself.
- `ui-qt/CMakeLists.txt` defines `SYODEP_VERSION="${SYODEP_VERSION}"` alongside
  the existing `SYODEP_BUILD_TYPE`, and `ui-qt/src/main.cpp` passes it to
  `QApplication::setApplicationVersion` instead of a literal.
- `scripts/check-docs.sh` gained two assertions. Because drift is now impossible
  by construction, they guard the construction itself: CMake must derive the
  version rather than hardcode it, and the shell must not reintroduce a literal.
  These are the script's first content checks about packaging.
- `docs/packaging.md` "Versioning" rewritten to describe what actually happens.

### Why

The version existed in three places and two were wrong:

| Source | Was |
|---|---|
| `Cargo.toml` | 0.4.0 |
| `CMakeLists.txt` | 0.3.0 |
| `ui-qt/src/main.cpp` | `"0.3.0"` hardcoded |

The last one feeds `--version` and `--check`, so **every shipped v0.4.0 binary
told users it was 0.3.0** — and did so incoherently, since the `core:` line on
the same screen reads the Rust crate version and correctly said 0.4.0.

`docs/packaging.md` claimed CMake mirrored `Cargo.toml`. It never has: the bump
commits (`accabc8` for 0.4.0, and the same shape for 0.3.0, 0.2.0, 0.1.1) only
ever touched `Cargo.toml` and `Cargo.lock`. Nothing in CI looked at it, so the
claim and the code drifted from the first release onwards.

Found while planning the Windows installer, which has to assert a version in
its Add/Remove Programs entry and its filename — shipping an installer saying
0.4.0 over an app saying 0.3.0 was not defensible, so this landed first and on
its own.

### Test strategy

No core logic touched, so no new Rust tests. Verified by deleting `build/`,
configuring from scratch and running `syodep --version`, which prints 0.4.0 on
both the shell and core lines.

Single-sourcing was verified the only way that means anything — by editing
**only** `Cargo.toml` to `0.9.1-rc2`, rebuilding without touching any CMake
file, and confirming both lines reported `0.9.1-rc2`. That also exercised the
pre-release path, which bare `project()` would have rejected. Both new guards
were likewise confirmed to fail when deliberately broken.

### Notes / remaining

- `bucket/syodep.json` also carries a version, but CI owns it
  (`release.yml` rewrites it on every tag), so it is deliberately not covered
  by the consistency check.

---

## 2026-07-25 — Fix the AppImage release build (missing `unzip`)

### Implemented

- Added `unzip` to the apt list of the `release-build-linux` container
  (`.github/workflows/release.yml`). Nothing else changed.

### Why

The Release workflow started failing on `main` at the `Build (release)` step,
while the CI workflow stayed green:

```
command failed: unzip -q -d thirdparty/extract/src/template.odt.dir ...
sh: 1: unzip: not found
make: *** [Makethird:358: thirdparty/extract/src/odt_template.c] Error 1
```

MuPDF's `Makethird` runs `thirdparty/extract/src/docx_template_build.py` when
python3 is available, and that script shells out to `unzip` to unpack an ODT
template. The guard is `if python3 -c '...'; then <run it>; else <skip>; fi`,
so the step is skipped only when python3 is **missing** — the `unzip`
dependency is invisible right up until python3 appears.

Verified against the real image rather than guessed. In a bare `ubuntu:22.04`
neither `python3` nor `unzip` is present; simulating an install of exactly the
list this job uses shows `python3` (plus `python3-gi`, `python3-dbus`) being
pulled in transitively through apt *recommends*, while `unzip` is not. So the
container ends up with the one binary that enables the code path and without
the one that path needs.

This is why CI stayed green: `qt-build-linux` runs on a normal `ubuntu-latest`
runner, which ships `unzip` preinstalled. Only the containerized release job
has a minimal userland. The Windows release job was unaffected and passed.

The trigger was an upstream change in that recommends chain, not anything in
this repository — the previous release run (2026-06-26, `d4694a5`) succeeded
with identical workflow and lockfile content.

### Test strategy

CI-only change, no core logic touched, so no new Rust tests. Verified by
dispatching the release workflow on the fix branch and watching
`release-build-linux` reach a packaged, smoke-tested AppImage.

### Notes / remaining

- The job still depends on apt recommends for python3. That is now harmless
  either way: with `unzip` present, both branches of the guard work.
- Pinning the `ubuntu:22.04` container to a digest would make this class of
  drift impossible, at the cost of manual bumps for security updates.
  Deliberately not done here — that is a policy decision, not a bug fix.

---

## 2026-07-25 — Visual selection mode

### Implemented

- **Longest-prefix fallback in the input state machine** (`input.rs`): a
  sequence that was both a complete binding and a prefix of a longer one could
  never fire — the machine waited, and a miss discarded the whole buffer. Now a
  miss walks back to the longest pending prefix that is itself a binding, fires
  it, and queues the leftover chords for replay. Still timer-free. The queue is
  drained by `App::handle_key`, **not** inside `InputState`, because the fired
  command may change mode and the replayed chords must resolve against the new
  mode's keymap (`cw` then `vj` must run `visual_down`, not `word_focus_down`).
  `Effects::merge` keeps one key press resolving several commands from losing an
  effect bit. No-op for the previous defaults: `c`/`g`/`z` were prefixes with no
  command of their own and `o` was nobody's prefix, so every bound sequence was
  a trie leaf.
- **Visual mode** (`caret.rs`, `app.rs`): `Mode::Visual` plus `VisualScope`
  (char/word/line/sentence/paragraph) and `VisualSelection { anchor,
  anchor_scope, head, head_scope, return_mode }`. Entered with `v` — inheriting
  the scope of the mode it was entered from — or `vc`/`vl`/`vw`/`vs`/`vp` to
  name it. `<Esc>` restores the prior mode and carries its mark to the head, so
  the highlight does not snap back.
- **Two independently-scoped ends**: `v` acts on the end that is moving, `o` on
  the other one. `ow` switches ends and makes *that* end word-granular while the
  other stays as it was; `vw` re-scopes the moving end without switching. `oo`
  swaps without waiting for a motion.
- **Motion** reuses the focus modes' steppers wholesale (`step_next_word_start`,
  `line_step_down`, `sentence_step_next`, `paragraph_step_next`, …), so no new
  traversal logic was written; `w`/`b`/`e` stay word-granular in every scope.
- **Config/FFI/Qt**: a `[visual_keys]` overlay table, `syo_app_selection` /
  `syo_selection_free` returning a `SyoRect` array (modelled on the sentence
  path but with no `page` field, since a selection may span pages), and a teal
  fill-only Qt overlay.
- **Docs**: the visual-mode command page, keybinding/config references, and a
  `check-docs.sh` block for the new binding table.

### Tests

- `input.rs`: prefix fires on a miss and the leftover replays; a directly-bound
  longer sequence still wins; `5oj` gives the count to the first command while
  `o5j` replays the digit into the count branch; Escape still cancels pending
  input instead of triggering the fallback; a total miss with no bound prefix
  still resets.
- `caret.rs`: `Caret` ordering is document order; scope inheritance; swapping is
  an involution.
- `app.rs`: entry from normal and from each focus mode; growing at each scope;
  `o` is a render no-op but does move the other end afterwards; `ow` changes one
  end only; crossing the anchor and returning restores the span exactly;
  selections spanning pages; rect count bounded by the visible pages; exit
  restoring mode *and* mark; entering a focus mode dropping the selection; and
  the replay resolving against the new mode's keymap.
- FFI round-trip covers `syo_app_selection` validity, growth and the free.

### Decisions

- **Scope belongs to the endpoint, not to the start/end role.** The alternative
  (the "first" edge owns a granularity) means overshooting and coming back
  silently trades the two granularities and lands on a different selection.
  Binding scope to the endpoint makes cross-and-return the identity.
- **No `active: SelEnd` flag.** `o` swaps the anchor and head records outright,
  so the invariant is just *head moves, anchor stays* and nothing can drift.
  The one subtlety: the swap must recompute the goal column, or the next `j`
  aims at the old head's column.
- **Selections are drawn per visible page**, unlike the page-confined focus
  marks. Overlay getters are `&self` and cannot lazily extract content, so the
  resolved span is cached on `App` and refreshed after each mutation.
- **Scroll and page jumps leave the selection alone**, unlike the focus modes
  where they carry the highlight. A selection is an explicit range; dragging it
  out from under the reader would lose work.
- `v` and `o` now take effect together with the key that follows them, a direct
  consequence of the prefix design the fallback enables.

### Known limitations / next steps

- Mouse selection is still to come; roadmap phase 2 item 2 stays 🚧.
- The selection is not persisted: `Position` stores page/scroll/zoom only, so a
  restart starts in normal mode with nothing selected. Deliberate.
- Nothing consumes the selection yet — highlighting (phase 2 item 3, SQLite
  migration v2) and clipboard yank are the obvious next steps. The span is
  already exposed as `App::visual_span`.

---

## 2026-06-26 — Sentence focus & paragraph focus modes

### Implemented

- **Sentence focus mode** (`syodep-core`): added `Mode::SentenceFocus`, entered
  with `cs` (`sentence_focus_enter`) and left with `<Esc>`. It highlights a whole
  sentence (`SentenceMark { page, start_line, start_cell, end_line, end_cell }`),
  which may span several lines but never crosses a page. Boundaries are detected
  over the cell stream at sentence-terminating punctuation (`.`/`!`/`?`, via
  `is_sentence_terminator`) plus trailing closing quotes/brackets
  (`is_sentence_trailer`), reusing the caret's cross-line `next_cell`/`prev_cell`
  walkers.
- **Paragraph focus mode**: added `Mode::ParagraphFocus`, entered with `cp`
  (`paragraph_focus_enter`). It highlights a block of lines
  (`ParagraphMark { page, start_line, end_line }`). The pure `paragraph_segments`
  splits a page's lines on column changes (reusing `column_ranges`/
  `column_index_of`) and on vertical gaps larger than `PARAGRAPH_GAP_FACTOR`
  times the median line height.
- **Navigation**: both modes are a linear sequence, so all of `hjkl` and the
  arrow keys collapse to previous/next (`*_focus_prev`/`*_focus_next`); counts
  repeat the motion and motion wraps across pages. Scroll and page jumps carry
  the highlight to visible content; zoom leaves it in place.
- **Config/FFI/Qt**: added `[sentence_focus_keys]` and `[paragraph_focus_keys]`
  overlay tables. Paragraph reuses the single-rect `SyoCaret` path
  (`syo_app_paragraph`, purple Qt highlight); sentence renders a text-selection
  shape via a new `syo_app_sentence`/`syo_sentence_free` array FFI
  (`SyoRect`/`SyoSentence`) drawn as one red rectangle per spanned line.
- **Docs**: added the sentence- and paragraph-focus command pages, keybinding/
  config references, and docs-check coverage for the new commands and default
  bindings.

### Tests

- Pure `caret` tests cover the sentence classifiers and `paragraph_segments`
  (tight grouping, large-gap split, column-change split, single/empty).
- App-level tests cover enter/mark/status, next/prev stepping, a sentence
  spanning lines (multi-rect), cross-page motion, within-page paragraph
  stepping, exit behavior and no-document safety.
- FFI round-trip toggles paragraph validity and exercises the sentence rect
  array + `syo_sentence_free`.

### Decisions

- Marks are **page-confined** (every overlay goes through the per-page
  `page_rect_to_screen`); navigation crosses pages while a single mark never
  straddles one, matching `WordMark`/`LineMark`.
- Decimal points and abbreviations (`3.14`, `Mr.`) are treated as sentence
  terminators — a deliberate v1 simplification.

---

## 2026-06-26 — Word focus mode

### Implemented

- **Word focus mode** (`syodep-core`): added `Mode::WordFocus`, entered with
  `cw` (`word_focus_enter`) and left with `<Esc>`. It highlights a whole
  Vim-like word run (`WordMark { page, line, start_cell, end_cell }`), using
  the same word classes as caret word motions: letters/digits/underscore
  together, punctuation/symbols as separate runs, whitespace skipped and each
  image as one stop.
- **Navigation**: `h`/`b` move to the previous run, `l`/`w` move to the next,
  and `j`/`k` move line-wise while keeping a goal column. Counts repeat the
  motion. Scroll and page jumps carry the highlight to visible content; zoom
  leaves it on the same word.
- **Config/FFI/Qt**: added `[word_focus_keys]` with default overlay semantics,
  `syo_app_word` for the overlay rectangle and a green Qt highlight distinct
  from caret and line focus.
- **Docs**: added the word-focus command page, keybinding/config references
  and docs-check coverage for the new commands and default bindings.

### Tests

- Config test covers `[word_focus_keys]` default merging and user overrides.
- App-level tests cover enter/mark/status, horizontal and vertical motion
  across lines/pages, inherited bindings, exit behavior and no-document safety.
- Docs check covers the new command page and word-focus default bindings.

---

## 2026-06-25 — Graphics diagnostics & WSL auto-fallback (0.3.0)

### Implemented

- **Self-diagnosing graphics startup** (`ui-qt/src/diagnostics.{h,cpp}`): a new
  module that detects the host platform (OS, WSL via `WSL_DISTRO_NAME` or
  `/proc/version`, GPU passthrough via `/dev/dxg`, X11/Wayland display env) and,
  **before the `QApplication` is constructed**, applies safe fallbacks: on WSL
  it forces `QT_QPA_PLATFORM=xcb` when a display is present (the WSLg
  wayland-egl client buffer integration is routinely empty) and
  `Qt::AA_UseSoftwareOpenGL` when there is no GPU passthrough. Any user-set
  `QT_QPA_PLATFORM`/`LIBGL_ALWAYS_SOFTWARE`/`QT_OPENGL` is respected and left
  untouched. Silent on a normal launch.
- **`syodep --check`**: prints platform detection, the selected Qt platform
  plugin and the reason, a live OpenGL probe (offscreen context →
  `GL_RENDERER`/`GL_VERSION`, so software `llvmpipe` is visible), the config
  file path with loaded/not-found state plus parse warnings
  (`syo_app_startup_warnings`), and version info; then exits.
- **Extended `syodep --version`**: shell, core, Qt, platform and build-type
  lines instead of the bare name+version. Handled before `QApplication`, so it
  needs no display.
- **FFI**: added `syo_core_version()` (`crates/syodep-ffi/src/lib.rs`) returning
  the core crate version; freed with the existing `syo_string_free`.

### Why

Launching in WSL crashed with `wayland-egl` integration failures and
`QOpenGLWidget: Failed to create context`, because the canvas is a
`QOpenGLWidget` requiring a GL context the WSLg environment could not provide.
The app now degrades automatically instead of failing, and `--check` makes the
active graphics path inspectable.

### Tests

- `QT_QPA_PLATFORM=offscreen ./build/ui-qt/syodep --smoke-test f.pdf` still
  passes; `--version` and `--check` exercised manually (offscreen for the GL
  probe in headless CI). Detection logic is pure and reads only env/filesystem
  signals.

### Decisions

- Fallback selection is **heuristic**, not a live GPU probe: Qt locks the
  platform plugin and GL backend at `QApplication` construction, so there is no
  context to probe at decision time.
- `Qt::AA_UseSoftwareOpenGL` is the cross-platform software switch (Mesa
  llvmpipe on Linux, `opengl32sw` on Windows); the `xcb` override is Linux-only.

---

## 2026-06-25 — Caret word motions

### Implemented

- **Word motions in caret focus mode** (`syodep-core`): added
  `caret_focus_next_word`, `caret_focus_end_word` and
  `caret_focus_prev_word`, bound by default to `w`, `e` and `b` in
  `[caret_focus_keys]`. Motions use Vim-like lowercase word runs:
  letters/digits/underscore together, punctuation/symbols as separate runs,
  whitespace skipped, line/page boundaries splitting runs, and each image as
  one word-like stop. Counts repeat the motion; the caret goal column is
  refreshed after landing and the view scrolls to keep the caret visible.
- **Docs/config**: command docs, default keybindings and the caret-focus
  config example now include the word-motion bindings.

### Tests

- Pure `caret.rs` tests cover word classification, skipped whitespace,
  punctuation runs, line-boundary splitting and image single-stop behavior.
- App-level tests cover `w`, `e`, `b`, repeated counts across lines/pages,
  document-edge clamping and image cells as word-motion stops.

---

## 2026-06-25 — Line focus mode

### Implemented

- **Line focus mode** (`syodep-core`): a third input mode (`Mode::LineFocus`)
  alongside Normal and CaretFocus, entered with `cl` (`line_focus_enter`) and
  left with `<Esc>`. It highlights a whole content line (`ContentLine.bbox`);
  `j`/`k` move the highlight line by line, wrapping across pages, and `h`/`l`
  move between columns on multi-column pages. It mirrors the caret machinery at
  line granularity: a `LineMark { page, line }` position, a `line_focus_keymap`
  (normal keymap overlaid with `[line_focus_keys]`), `enter_line_focus` /
  `line_move` / `line_step_up`/`down` / `line_step_column`, viewport-follow via
  `reposition_line_to_viewport` (reusing `topmost_visible_line`),
  `ensure_line_visible`, and `line_screen_rect`. Scroll/page jumps carry the
  highlight; zoom leaves it in place — same rules as the caret.
- **Column detection** (`caret.rs`, pure + unit-tested): `column_ranges`
  greedily clusters a page's line bboxes into disjoint horizontal bands;
  `column_index_of` maps a line to its column; `nearest_line_in_column` is the
  goal-row analogue of `nearest_cell_in_line` so `h`/`l` keep the vertical
  position. `h`/`l` are a no-op on single-column pages and edge columns.
- **Entry binding `cl`, not `ll`**: `ll` would make a lone `l` ambiguous (both a
  binding and a prefix), breaking `l` scrolling and caret-right. `cl` reuses the
  prefix-only `c` focus family (`cc` caret, `cl` line) with no collisions.
- **FFI + Qt**: `syo_app_line` returns the highlight rect (reusing `SyoCaret`'s
  layout); the Qt canvas paints it as a translucent amber band, distinct from
  the blue caret. Header regenerates via cbindgen.
- **Docs**: new `docs/commands-line-focus-mode.md`; `[line_focus_keys]` in
  `docs/config.md`; line-focus section in `docs/keybindings.md`;
  `scripts/check-docs.sh` extended to cover the new page and bindings.

### Tests

A two-column PDF fixture (`test_support::pdf_two_column_page`) plus core tests:
enter/mark/status, vertical page crossing, exit restores scrolling, inherited
bindings carry the mark, `h`/`l` no-op on single column and jump columns on the
fixture; FFI validity test for `syo_app_line`; pure tests for the three column
helpers.

---

## 2026-06-25 — Navigation commands in caret focus mode

### Implemented

- **View commands carry the caret** (`syodep-core`): the page-scroll
  (`scroll_half_page_down/up`, `scroll_page_down/up`), page-navigation
  (`next_page`, `prev_page`, `goto_first_page`, `goto_last_page`) and zoom
  (`zoom_in/out`, `fit_width`, `zoom_reset`) commands are now first-class in
  caret focus mode. They were already *reachable* there (the caret-focus
  keymap is the normal keymap plus the `[caret_focus_keys]` overlay, which
  only remaps `hjkl`/arrows/`<Esc>`, so no clashes), but the caret stayed
  put. Now scroll and page jumps reposition the caret to the top-most content
  visible in the new viewport, keeping its goal column; zoom leaves the caret
  in place. New `App::reposition_caret_to_viewport` + `topmost_visible_line`
  hook into `App::execute` after the view mutates (`app.rs`); they reuse
  `View::scroll`, `DocumentLayout::page_at_y`/`page`, and `ContentLine::bbox`.
- **Per-mode command docs**: `docs/commands.md` is now an index linking
  `docs/commands-normal-mode.md` and `docs/commands-caret-focus-mode.md`. The
  caret-focus page documents the inherited view commands and the
  reposition/zoom behavior. `scripts/check-docs.sh` greps command names
  against the per-mode pages (and its caret check now follows the renamed
  `default_caret_focus_keybindings`).

### Tests

- `caret_focus_page_jumps_carry_the_caret` (J/K/G/gg move the caret onto the
  destination page), `caret_focus_page_scroll_advances_the_caret` (`<C-f>`
  advances the caret), `caret_focus_zoom_leaves_the_caret_in_place`
  (`+`/`zw` keep the caret), and an extended
  `caret_focus_keeps_non_hjkl_bindings`.

---

## 2026-06-17 — AppImage Qt platform plugin bundling

### Implemented

- **Wayland platform support**: the Linux AppImage release job now installs
  `qt6-wayland` in the Ubuntu 22.04 build container and explicitly asks
  `linuxdeploy-plugin-qt` to bundle `libqwayland-egl.so` and
  `libqwayland-generic.so` alongside the existing offscreen plugin.
  `libqxcb.so` remains the plugin's default platform backend.
- **Packaging verification**: after building the AppImage, CI extracts it
  and asserts the bundled Qt platform directory contains `xcb`,
  `offscreen`, and both Wayland platform plugins before running the
  smoke test.
- **Docs**: `docs/packaging.md` now records the `qt6-wayland` dependency
  and the extracted-AppImage plugin check.

### Test strategy

Workflow/docs change. Local verification: `git diff --check` and
`./scripts/check-docs.sh`; the full AppImage extraction check and packaged
offscreen smoke test run in GitHub Actions on the next release workflow.

---

## 2026-06-17 — Continuous prerelease downloads

### Implemented

- **Rolling release**: `.github/workflows/release.yml` now also runs on
  pushes to `main` and reuses the existing AppImage and Windows zip builders.
  After both packages pass their smoke tests, `publish-continuous` updates
  the `continuous` tag and prerelease with stable asset names for the latest
  main build.
- **Release boundary**: `vMAJOR.MINOR.PATCH` tags still create immutable
  versioned releases and bump the Scoop manifest. The rolling prerelease is
  marked as a prerelease and does not update Scoop metadata.
- **Docs**: `AGENTS.md` and `docs/packaging.md` now document the split
  between branch CI artifacts, the continuous prerelease, and versioned
  releases.

### Test strategy

Workflow/docs change. Local verification: `git diff --check` and
`./scripts/check-docs.sh`; the package smoke tests and continuous release
publish path run in GitHub Actions on the next `main` push.

---

## 2026-06-17 — Push build artifacts policy

### Implemented

- **CI push artifacts**: `.github/workflows/ci.yml` now explicitly runs on
  branch pushes and `v*` tags. A new `build-artifact` job waits for the
  existing lint, Rust test, Qt smoke-test, and docs jobs, then builds a
  release-mode Linux binary, packages it as a tarball with `version.txt`,
  uploads the SHA-256 checksum, and retains the workflow artifact for 14 days.
- **Release boundary**: public releases remain owned by
  `.github/workflows/release.yml` on `vMAJOR.MINOR.PATCH` tags; branch pushes
  produce ephemeral CI artifacts only.
- **Agent guidance**: `AGENTS.md` now records the branch-artifact/tag-release
  policy and forbids CI-driven version bump commits or checked-in binaries.

### Test strategy

Docs/workflow-only change. Local verification: `git diff --check` and
`./scripts/check-docs.sh`; the build artifact path is enforced by the updated
GitHub Actions dependency graph on the next push.

---

## 2026-06-16 — Modal caret navigation (text + images)

### Implemented

- **Content-geometry layer** (`syodep-pdf`): `Document::page_content` returns
  per-page `ContentLine`s of `Cell`s — one cell per character (bbox from the
  glyph quad) and one cell per image — in reading order, in page points.
  Uses `TextPageFlags::PRESERVE_IMAGES` (image blocks are dropped by the
  default stext flags). Image vs text blocks are discriminated via
  `block.image()`/`block.lines()` since `TextBlockType` is not re-exported by
  the bindings.
- **Modal caret** (`syodep-core`): a new `Mode { Normal, CaretFocus }` plus a
  `caret.rs` module (position, direction, goal-column cell picker). `c`
  enters caret focus mode; `h`/`l` move the caret character-wise (wrapping across
  lines/pages), `j`/`k` line-wise keeping a goal column; `<Esc>` exits. Each
  image is a single stop. The view auto-scrolls to keep the caret visible
  (`View::scroll_doc_rect_into_view`), and page content is cached per page in
  the session. The caret keymap is the normal keymap cloned with the
  `[caret_focus_keys]` overrides applied (`Keymap::overlay`), so every other
  binding still works in caret focus mode and normal-binding errors are reported
  once.
- **Config**: new `[caret_focus_keys]` table (`h/j/k/l`/arrows + `<Esc>` defaults)
  and a `cc = caret_focus_enter` default in `[keys]`.
- **FFI/shell**: `SyoCaret` + `syo_app_caret` project the caret rect (canvas
  pixels) across the C ABI; `CanvasWidget::paintGL` draws a translucent
  accent box with a border. The status bar shows `-- CARET FOCUS --  Ln L, Col C`.

### Test strategy

TDD for the pure pieces: `caret.rs` goal-column picker; `View`
`page_rect_to_screen`/`scroll_doc_rect_into_view`; `syodep-pdf` content
extraction including an image cell from a new `pdf_with_image` fixture
(generated, not checked in). App-level integration tests cover enter/exit,
character/line motion, page wrapping, goal-column preservation across pages,
counts, and that non-`hjkl` bindings still work in caret focus mode. The FFI
round-trip test enters caret focus mode, moves, and exits. 104 tests total (was
88); Qt shell covered by compile + offscreen smoke test as before.

### Decisions (details in `docs/architecture.md`, row 11)

- Modal caret (mode-selected keymap) over an always-on caret: keeps `hjkl`
  scrolling intact and matches the existing Vim-like modal design.
- One caret stop per image; goal-column vertical motion like a text editor.
- `page_content` runs only in caret focus mode and is cached, so plain reading is
  unaffected.

### Known limitations / next steps

- Word/sentence/paragraph text objects and selection build on this caret
  (phase 2/3). The caret position is not yet persisted across sessions.
- RTL/vertical scripts rely on MuPDF reading order; not specially handled.

---

## 2026-06-12 — Linux AppImage release

### Implemented

- **`release-build-linux`** (release.yml) now produces
  `syodep-x86_64.AppImage` instead of an unpackaged binary. It builds in
  an `ubuntu:22.04` container (the AppImage inherits the build machine's
  glibc floor — 2.35 covers Ubuntu 22.04+/Debian 12+/Fedora 36+), with
  distro Qt 6.2 and rustup-installed Rust, then packages with
  `linuxdeploy` + `linuxdeploy-plugin-qt` (run via
  `--appimage-extract-and-run`; containers have no FUSE).
- **`packaging/`**: `syodep.desktop` (Office;Viewer, application/pdf
  MIME) and a placeholder `syodep.svg` icon, both required by
  linuxdeploy.
- The `offscreen` Qt platform plugin is bundled
  (`EXTRA_PLATFORM_PLUGINS`) so the AppImage itself is smoke-tested in
  CI (offscreen render of a generated PDF) — same fail-in-CI principle
  as the Windows staged smoke test.
- **`publish-release`** attaches `syodep-vX.Y.Z-x86_64.AppImage` to
  GitHub releases alongside the Windows zip.

### Test strategy

CI-only: the AppImage smoke test exercises open + render through the
real packaged binary. Additionally verified by downloading the artifact
from a `workflow_dispatch` run and running the smoke test on a local
machine with a different userland than the build container.

---

## 2026-06-12 — Scoop distribution

### Implemented

- **`bucket/syodep.json`**: the repo doubles as a Scoop bucket
  (`scoop bucket add syodep https://github.com/nexdep/syodep`). The
  manifest points at the GitHub release zip, sets `extract_dir`
  (`syodep-win64`), `bin`, a Start Menu shortcut, and
  `checkver`/`autoupdate` metadata. No `persist` entries: user data lives
  in `%APPDATA%`, not the install dir.
- **`publish-release`** (release.yml) now bumps the manifest after
  creating each release: recomputes the zip's SHA256, rewrites
  `version`/`url`/`hash` with `jq`, commits to `main` as
  `github-actions[bot]`. The job checks out `main` (not the tag) for this.

### Test strategy

Manifest JSON validated with `jq`; the hash was computed from the actual
published v0.1.0 asset. The CI bump path only executes on the next `v*`
tag — verify it then (winget was considered and dropped for now).

---

## 2026-06-11 — Windows link fixes + GitHub releases on tag push

### Implemented

- Fixed the Windows shell link, found by reading CI logs after four
  distinct failures (all in the top-level `CMakeLists.txt`):
  1. strip `/defaultlib:` linker-flag tokens from rustc's
     `native-static-libs` output (CMake treated them as file paths);
  2. strip ANSI color codes from that output (`CARGO_TERM_COLOR=always`
     in CI poisons tokens) — note `\x` escapes are invalid in CMake
     strings, use `string(ASCII 27 …)`;
  3. resolve `libmupdf.lib`/`libthirdparty.lib` to their cargo `OUT_DIR`
     paths — on Windows rustc does **not** bundle them into the staticlib
     (unlike Linux), leaving 382 unresolved `fz_*` symbols;
  4. configure the Windows CI shell build as Release — a debug config
     links debug Qt + `/MDd` against MuPDF's `/MD` objects (LNK2038).
- **`publish-release`** (release.yml): on `v*` tag pushes, downloads the
  portable zip artifact and publishes it to a GitHub release as
  `syodep-vX.Y.Z-win64.zip` (`gh release create --generate-notes`,
  `contents: write` permission). Manual dispatch runs still stop at
  workflow artifacts.

### Test strategy

CI-only changes, verified by watching runs to green: full CI (all six
jobs including both Windows jobs), a `workflow_dispatch` release run
producing a working zip, and a `v*` tag push producing a GitHub release.

---

## 2026-06-11 — Windows binary in CI/CD

### Implemented

- **`qt-build-windows`** (ci.yml): builds the Qt shell on `windows-2022`
  on every push/PR — Qt 6.7.3 via `jurplel/install-qt-action`
  (`win64_msvc2019_64`), MSVC env via `ilammy/msvc-dev-cmd`,
  `cmake -G Ninja`, then the offscreen smoke test. The exe is a
  GUI-subsystem binary, so the smoke test asserts the exit code (stdout is
  invisible on Windows).
- **`release-build-windows`** (release.yml): release-mode build, portable
  tree staged with `windeployqt --release --no-translations` plus LICENSE/
  README/sample config, smoke test re-run from the staged tree with Qt
  stripped from PATH (an incomplete DLL bundle fails in CI, not on a user
  machine), `syodep-win64.zip` uploaded as a workflow artifact.

### Test strategy

CI-only change: no core logic touched, so no new Rust tests. The Windows
smoke test (build + open + render through the real exe) plus the staged
PATH-stripped smoke test are the appropriate coverage. Verified by pushing
and watching the GitHub Actions runs to green, plus a `workflow_dispatch`
release run producing a working zip.

### Notes / remaining

- Qt version is pinned (6.7.3) in both workflows; bump deliberately.
- Still planned (docs/packaging.md): Linux AppImage, NSIS installer,
  attaching artifacts to GitHub releases on tag push.

---

## 2026-06-11 — Milestone 1: MVP foundation

Everything below landed as one milestone, built bottom-up in small slices
(config → core input/layout → storage → pdf backend → App integration →
FFI → Qt shell → build system → CI/docs).

### Implemented

- **Workspace layout**: Cargo workspace with `syodep-config`,
  `syodep-core`, `syodep-pdf`, `syodep-storage`, `syodep-ffi`; Qt shell in
  `ui-qt/`; top-level CMake driving cargo + Qt.
- **syodep-config**: TOML config (`[view]`, `[keys]`), defaults, overlay
  semantics for user keybindings, descriptive parse errors (unknown field,
  type mismatch, file context), and the key-chord syntax/parser
  (`gg`, `<C-d>`, `<C-A-Left>`, named keys).
- **syodep-core**:
  - `Command` registry (19 commands) with name round-tripping.
  - Input state machine: keymap trie, count prefixes (`5j`, `120G`,
    Vim-style `0` rule), multi-key sequences, deterministic prefix
    disambiguation, Escape-cancels-pending, per-entry error reporting for
    invalid bindings.
  - Layout/View: document-space page stacking with gaps and centering,
    clamped scrolling (small docs centered), current-page = window center,
    page navigation, zoom anchored at the window center with limits,
    fit-width, visible-page computation.
  - Byte-bounded LRU render cache keyed by (page, quantized scale).
  - `App`: ties everything together; `Effects {redraw, quit,
    open_file_dialog}` out; position autosave after navigation + on drop.
- **syodep-pdf**: safe wrapper over the `mupdf` crate exposing only
  syodep types (`Document`, `Size`, `Bitmap` RGBA8, `OutlineItem`);
  open-from-path/bytes, page sizes, render-at-scale with white background,
  plain-text extraction, outline; password-protected files rejected with a
  clear error. Includes a programmatic PDF fixture builder
  (`test_support`, also used by other crates and CI).
- **syodep-storage**: rusqlite (bundled), migration runner over
  `PRAGMA user_version` (refuses newer-schema DBs), schema v1
  (`documents` keyed by SHA-256 content fingerprint, `positions`),
  position save/load, cascade delete.
- **syodep-ffi**: panic-safe C ABI (`syo_app_*`), cbindgen-generated
  header, explicit free functions for strings/bitmaps, default
  config/db path helpers (XDG / %APPDATA%).
- **ui-qt**: `MainWindow` (status bar, file dialog, owns the core handle),
  `CanvasWidget` (QOpenGLWidget; paints core-provided bitmaps, forwards
  keys/wheel/resize), `key_encoder` (QKeyEvent → chord strings),
  `--smoke-test` mode for CI.
- **Build**: top-level CMake builds the Rust staticlib via cargo and links
  the Qt shell against it (Linux: + fontconfig/freetype; Windows libs
  prepared). `SYODEP_RUST_PROFILE` defaults to release.
- **CI**: lint (fmt, clippy -D warnings), tests on Linux + Windows, Qt
  build + offscreen smoke test, docs-consistency script
  (`scripts/check-docs.sh`). Release workflow placeholder with the real
  pipeline specified in `docs/packaging.md`.
- **Docs**: README + architecture/commands/keybindings/config/testing/
  packaging/roadmap/this log.

### Test strategy actually used

TDD for the pure crates (tests written with/before the code, all pure
logic covered without I/O where possible); integration tests at the App
and FFI levels; generated PDF fixtures instead of binary files; offscreen
smoke test for the shell. 88 tests at milestone close. Deviation from
strict TDD: the Qt shell itself is covered by compilation + smoke test
only, by design (it contains no logic).

### Decisions (details in `docs/architecture.md`)

- `mupdf-rs` bindings instead of hand-rolled bindgen (reproducible
  Windows/Linux builds; unsafe stays out of our tree).
- Content-fingerprint document identity (survives moves/renames).
- Scroll state stored in document space → zoom-stable.
- Timer-free key disambiguation (wait on ambiguous prefix; Esc cancels).
- Synchronous rendering for M1; async/tiles deferred to phase 3.

### Known limitations / next steps

- Rendering is synchronous on the UI thread; large pages at high zoom can
  stutter. Planned: phase 3 async tiles (the `App::render_page` seam stays).
- `visible_pages` FFI is capped at 64 entries by the shell's stack buffer
  (fine until extreme zoom-out; the API already reports the real count).
- No text selection yet — phase 2 starts with the char-geometry text layer.
- ~~Windows CI builds the Rust workspace but not yet the Qt shell~~
  (done: see "Windows binary in CI/CD" entry above).

---

*(log started 2026-06-11)*

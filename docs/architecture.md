# Architecture

## Overview

syodep is split into a UI-independent Rust core and a thin Qt 6 shell,
joined by a small C ABI:

```
 ┌────────────────────────── ui-qt (C++, Qt 6) ──────────────────────────┐
 │ MainWindow ── status line, file dialog                                │
 │ CanvasWidget (QOpenGLWidget) ── paints bitmaps, forwards input        │
 │ key_encoder ── QKeyEvent → "j" / "G" / "<C-d>" strings                │
 └──────────────────────────────┬────────────────────────────────────────┘
                                │ C ABI (crates/syodep-ffi, cbindgen header)
 ┌──────────────────────────────┴────────────────────────────────────────┐
 │ syodep-core: App                                                      │
 │   command system ─ input state machine (counts, sequences, keymap)    │
 │   layout/View ─ document space, scroll, zoom, visible pages           │
 │   render cache ─ byte-bounded LRU of page bitmaps                     │
 ├───────────────┬──────────────────────────┬────────────────────────────┤
 │ syodep-config │ syodep-pdf               │ syodep-storage             │
 │ TOML, chords  │ safe MuPDF wrapper       │ SQLite + migrations        │
 └───────────────┴──────────────────────────┴────────────────────────────┘
```

**The rule that everything else follows from:** the core never depends on
Qt types; the shell never contains document logic. The shell forwards
events and paints what the core tells it to paint.

## Crates

### syodep-config

Owns the *shape* of the TOML config and the textual key-chord syntax
(`gg`, `<C-d>`, `<Esc>`), which is shared by the config file and the shell's
key encoder. Semantic validation of command names lives in `syodep-core`
(which owns the command set); this keeps the dependency direction
`core → config` with no cycles.

Error philosophy: configuration errors are *never* fatal. `Config::load`
returns either a config or an error with file/field context; callers fall
back to defaults and surface the message (status bar). User `[keys]`
entries overlay the built-in defaults rather than replacing them.

### syodep-core

Pure Rust, fully unit-testable without a display or a real file (layout and
input take plain data). Key pieces:

- **`Command`** (`command.rs`): every user action is a variant; keybindings
  map key sequences to command *names*. UI never calls behavior directly.
  This is the seed of the later command palette and text-object commands.
- **Input state machine** (`input.rs`): keymap trie + pending state
  (count prefix, partial sequence). Disambiguation: a sequence that is both
  a binding and a prefix of a longer binding waits for more input; `<Esc>`
  cancels. If the wait ends in an unbound sequence, the longest bound prefix
  fires and the leftover chords are queued for replay, so a binding that is
  also a prefix stays reachable. Timer-free throughout, hence deterministic
  and easily tested. The replay queue is drained by the private `App::dispatch`
  (called from both `handle_key` and `handle_timeout`) rather than resolved
  inside `InputState`, because the fired command may switch modes and the
  replayed chords must use the new mode's keymap.
- **Layout** (`layout.rs`): pages stacked vertically in *document space*
  (PDF points), centered on the widest page. Scroll offsets are stored in
  document space so they survive zoom changes. `View` provides clamped
  scrolling, page navigation, anchor-preserving zoom, fit-width and the
  visible-page computation. `scroll_doc_rect_into_view` is the single place
  the view follows the highlight: it scrolls the minimum amount that leaves
  `view.scroll_off` screen pixels of clearance above and below the rectangle,
  and the existing scroll clamp gives that clearance up at the document's
  first and last page.
- **Render cache** (`render_cache.rs`): LRU keyed by (page, quantized
  scale), bounded by bytes. Rendering itself is synchronous on the UI
  thread in milestone 1; asynchronous tile rendering is a later milestone
  (see roadmap) and will live behind the same `App::render_page` seam.
- **`App`** (`app.rs`): glues everything; input events come in, `Effects`
  (redraw / quit / open-file-dialog / pending-input / reload) come out. The
  last two are state rather than one-shot requests: `pending_input` tells the
  shell whether to arm its pause timer, and `reload` tells it a save rewrote
  the document, so cached page bitmaps must be dropped even though nothing
  else about the view changed. Persists the reading position after every
  navigation command and on drop.
- **Focus mode** (`caret.rs` + `app.rs`): one highlighted position over
  page-content geometry. `Mode` has exactly four variants — `Normal`, `Focus`,
  `Visual`, `Highlight` — and *granularity is not a mode*: `Focus` carries a `Scope` (char,
  word, line, sentence or paragraph). The app stores one `Caret` plus that
  scope; what is drawn is derived by `scope_span(caret, scope)` and cached in
  `focus_span`, so changing the scope reinterprets the position you are on
  rather than restoring a separate per-granularity mark. (It used to be five
  `Mode` variants with five marks, which meant `fw` then `fe` teleported you to
  wherever you last were in line focus. The bug is unrepresentable now.)
  The focus keymap is the normal keymap plus the `[focus_keys]` overrides, so
  every other command still works while focused. Extracted page content is
  cached per page in the session; the pure goal-column and word-boundary
  helpers live in `caret.rs`.
- **Visual mode** (`caret.rs` + `app.rs`): a two-ended selection over the same
  content layer. It stores only the anchored end (`VisualAnchor`); the *moving*
  end is the app's `focus`/`focus_scope`. There is exactly one "where you are"
  in the whole app, and visual mode adds a second endpoint rather than a second
  position. `o` exchanges the anchor with the focus position, scopes included,
  so there is no separate "which end is active" flag to drift. Rendering expands
  each end by its own scope and takes the outermost edges, which makes the ends
  crossing a non-case and `o` provably invisible.
  `VisualSelection` still exists, but as a *read-only view* assembled by
  `visual_selection()` — it is what the tests and status line read, never what
  the app stores.
- **Why the moving end is not stored separately**: it was, and that was a bug.
  `enter_focus` (the `f` chords) read `focus` while `hjkl` moved
  `visual.head`, and the two only reconciled inside `exit_visual` — so leaving
  visual mode any other way silently restored the pre-selection position. The
  fix was to delete the duplicate rather than sync it: `exit_visual` now just
  drops the anchor, and `enter_focus` needs no visual-aware branch at all.
- **Why focus and visual share almost everything**: a focus highlight is a
  selection whose two ends coincide. Both resolve to an inclusive `(Caret,
  Caret)` span, so one `scope_span` derives the extent, one `step_scope` table
  maps (scope, direction) onto a motion, and one `span_screen_rects` turns a
  span into overlay rectangles. A scope therefore cannot mean one thing in one
  mode and something else in the other — enforced by construction rather than
  by discipline. The FFI reflects this: a single `SyoOverlay` shape serves all
  three, fetched with `syo_app_focus` / `syo_app_selection` /
  `syo_app_highlights`. Overlays are drawn per *visible* page, so cost does not
  grow with span length. Stored *and* pending highlights come back from one
  getter in one colour, because the shell merges an overlay's rectangles into a
  single fill — two overlays in the same colour would double-blend where they
  overlap.
  The two stay separate modes because their exit semantics differ, focus draws
  one end where visual draws two, and they want distinct colours — but they
  share the position itself.
- **Highlight mode** (`app.rs`): visual mode with a pending highlight attached.
  It stores no extent of its own — the pending highlight's extent *is*
  `visual_span` — and `[highlight_keys]` binds the motion keys to the existing
  `visual_*` **commands**, not to new ones. That works because `visual_move`,
  `swap_visual_ends` and `set_head_scope` guard on `self.visual.is_some()` rather
  than on the mode, so the entire feature added three commands
  (`highlight_enter`/`commit`/`discard`) and no motion code. Entering from focus
  mode synthesises the second end; `PendingHighlight` records the mode, position,
  scope and anchor to restore, so discarding is four assignments with nothing
  partially applied to unwind.
  The exits that *keep* a highlight (`a`, `v`, `f`, saving) all call one
  `store_pending_highlight`, so "which exits keep it" is a fact about the
  keybindings rather than a condition repeated in four places. `enter_visual`
  needs one early branch for it: its ordinary path collapses the selection onto
  the head, which would throw away the extent the user just shaped.
- **Highlights** (`app.rs` + `syodep-storage` + `syodep-pdf`): stored as
  page-space rectangles plus the covered text, not as a caret span — see decision
  15. Committing writes them to SQLite (migration v2) so they survive a reopen;
  `save_document` embeds them in the PDF as real `Highlight` annotations, after
  which syodep stops drawing them because MuPDF renders them itself.

**Traversal audit:** the content pipeline, scope/object matrix, adjacency
policies, cache invalidation, and findings for extending toward the annotation
sidebar are recorded in [`docs/traversal-audit.md`](traversal-audit.md). Read
that before changing motion, extraction order, or object policy.

### syodep-pdf

The only crate that touches MuPDF. Wraps the maintained `mupdf` crate
(bindings + vendored MuPDF C sources) and exposes syodep-owned types only:
`Document`, `Size`, `Rect`, `Bitmap` (tightly packed RGBA8), `OutlineItem`,
and the content-geometry layer `PageContent` — `ContentLine`/`Cell` (per-page
text/image boxes from `page_content`, the foundation the caret — and later
selection and search — navigate) plus `ContentObject`, the runs of lines that
navigation treats as one unit. No MuPDF type or pointer crosses this boundary.

It is also the only crate that *writes* a PDF: `write_highlights` (plus
`HighlightAnnotation` and the read-back helper `page_highlights`, which lets tests
assert against the file rather than the code that produced it).

**Decision — tables are navigable units, found by a second text
pass:** `page_content` extracts text once with `PRESERVE_IMAGES`, then runs a
second pass with `TABLE_HUNT | COLLECT_VECTORS` from which it takes *only* the
bounding boxes of the detected tables. Two passes are needed because the
detection pass rewrites the page — it moves a table's text into a structure
node whose children the Rust bindings cannot walk, and splits lines while
redistributing characters into cells — so its geometry is unusable for text.
`COLLECT_VECTORS` is not optional either: MuPDF hunts for tables among a page's
ruled rectangles, and with no vectors collected it falls back to hunting the
whole page at a loose threshold, which reports ordinary prose as one giant
table. Trade-off: detection is a heuristic and costs a second pass (~2ms per
page, cached), so `view.detect_tables` can switch it off. Boxes that claim
every line on a page, or that map to a non-contiguous set of lines, are
discarded rather than guessed at — degrading to line-by-line navigation is
always safe, whereas a wrong unit is a very visible navigation bug. A
box's *edges* are trimmed rather than trusted, because MuPDF reports the ruled
region and it reaches past the last row: an edge line the box only clips, or one
set apart from the table's own row rhythm, is the prose beside the table and not
part of it. The stored box is then held back from the lines above and below, so
a table's highlight can never tint a line the caret can reach — like a heading's
box (the union of its lines) and an image's, it is bounded by the page's own
content.

**Decision — page furniture is removed from the content layer, not skipped by
motion:** running heads, folios, margin line numbers and text that does not run
in the page's direction never enter `PageContent::lines`, so every motion, span
and overlay ignores them without a single change to the caret. The removed
lines are kept in `PageContent::furniture` rather than discarded. Three
independent rules find them.
*Rotation* flags a line more than 10&deg; off the page's **dominant** direction —
dominant rather than absolute horizontal, which is what lets a page laid out
sideways keep everything, and what makes the rule provably unable to empty a
page, since the dominant cluster is the majority and is never flagged.
*Repetition* flags a margin-band line whose digit-masked text and baseline recur
across sampled pages; position alone is never evidence, so a title that appears
once survives. Sampling takes four anchors of *two consecutive pages*, because
evenly spaced single pages land on one parity and would miss the recto running
head of any book that alternates. Baselines, not bounding boxes, are the
position key — a descender moves a box by points. A line's characters are
further split into `text_segments` wherever a gap exceeds `SEGMENT_GAP_POINTS`
(20pt, comfortably past any word gap): a header and a folio sharing one
baseline — a facing-page layout where the pair swaps sides between recto and
verso is the case this exists for — are then two segments the vote can accept
independently, rather than one string whose order depends on which side either
field is on. Bare page numbers get a slightly deeper bottom band (20% rather
than 15%), because journal layouts often set the folio a few points above the
strict margin and an empty furniture profile leaves every folio in the caret
path — sometimes even flagged as a heading. *Line numbering* flags a run of purely numeric lines (at least
`MIN_LINE_NUMBERS`) forming their own column left of the body text, separated
from it by at least `LINE_NUMBER_GAP` — manuscript line-numbering, which
restarts every page and so has no cross-page profile to learn from; it needs
none, since the pattern (a narrow numeric column beside a wider body column) is
visible on a single page. Its safety net is the same shape as rotation's: the
gap is measured against the body text's own left edge, so a page that is
*entirely* numeric — a table of figures, with no body column to compare
against — is left alone rather than guessed at, and the rule can no more empty
a page than rotation can.
Filtering happens *before* table and heading detection, which also improves
both: a full-width running header inflated the heading rule's widest-line
comparison and an 8pt folio dragged its body-size vote. Consequently
`content_objects` must judge its "claims the whole page" guards against lines
*plus* furniture, or a full-page table starts tripping its own guard once the
header is gone. The profile is document-scoped and owned by `syodep-core`'s
session, built lazily on first content need, so `Document` keeps no interior
mutability and merely reading a document never pays for it.

**Decision — regions form a chain of decreasing coarseness:** every
`ContentObject` bounds a sentence, which is what being a region means; four
predicates on `ObjectKind` say how much further each kind goes, from the finest
scope up. An image `is_atomic()` — one stop from *word* scope up, because there
are no words inside one to walk. An image, a table, an equation and a
footnote `is_block()` — one stop from *line* scope up, and drawn as a single
box; the two categories nest, so atomic implies block. (A footnote's further
invisibility to Sentence/Paragraph auto-search does not fit these four
predicates at all — see the dedicated decision below.) A heading, an
equation, a table and a footnote `splits_paragraphs()`, each being one step
for `s` and `p`; a heading and an
equation are also `is_one_sentence()`, so every terminator inside them is inert,
which is what keeps `2.12.` and `f(x) = 0.` from splitting. A list item is none
of them: one step for `s` only, because a list is a single paragraph made of
many items, and its sentences are worth walking. Adding a kind means answering
those four questions rather than threading a new mechanism through the motion
code — which is exactly what list items did before they became regions, and what
removing that mechanism bought back.

Which category a scope consults is the whole of the per-scope difference, and it
lives in one function (`App::unit_object_at`): char has no units at all, word
asks for atomic kinds, line and coarser ask for blocks. `page_span_rects` asks
for blocks unconditionally and needs no scope, because it already keys the
collapse on whether the span covers the object end to end — which is true at
exactly the scopes where the object is one unit.

**Decision — a heading is a *region*, and neither atomic nor a block:** motion
and highlighting go through the unit accessors, which exclude headings at every
scope, so a heading keeps both its words and its wrapped lines individually
reachable; sentence runs and paragraph splitting go through the region
accessors, which include headings, and that alone makes a heading one step at
sentence and paragraph scope. Headings are found from typography rather than
structure: a line set noticeably larger than the page's body size (the
character-count mode, which body text dominates on every page), or entirely
bold at body size without filling the column. Both signals come free from the
pass that extracts the text. A third, shape-based rule catches multi-level
section numbers at body size — `1.1. Methods`, `2.12. Recommended checking
order` — which typography alone misses; those lines must end with the
section-number's own trailing dot before the title, so a decimal that opens a
sentence (`3.14 is the value`) is not mistaken for one. MuPDF's own heading
detection
(`FZ_STEXT_PARAGRAPH_BREAK`) is unusable here for the same reason as its table
grids — it hides the text inside structure nodes the bindings cannot walk — and
it keys on bold alone, missing size, the stronger signal.

**Decision — display equations are found from fonts *and* characters, and only
when set apart:** a line is an equation when it does not fill the column, reads
as mathematics, carries an operator, and carries almost no words. Two math
signals, because either can be missing: TeX gives its fonts away by name
(`CMMI10`, `MSBM10`, `XITSMath-Regular`, and MuPDF passes the PDF's own font name
through every load path), while a PDF whose fonts are unrecognisable still gives
away its operators and Greek in the characters — but character-based detection
requires a *rich* math mark (Greek or a unicode operator), not ASCII `+`/`=`
alone, or `in C++.` and `count += 1` become equations. A lone signed number
(`−1`) is rejected for the same reason. The set-apart test is what keeps
inline maths out, and inline maths must stay out: a region splits the sentence
around it, so making a formula inside a sentence a unit would break the sentence
carrying it. Cost is nothing extra — both signals come from the extraction pass,
as headings' do.

**Decision — a footnote is a fifth, new shape the four `ObjectKind`
predicates cannot express on their own:** every other kind's four booleans
answer "how far does this go from word scope up," a single axis. A footnote
needs a second, independent axis — "is this reachable by automatic
Sentence/Paragraph search at all" — because unlike every other kind, it
should cost `s`/`p` *zero* stops while reading through ordinary body prose,
not one. `is_block()` alone (which a footnote shares with a table: one stop
from line scope up, still walkable by word and char) only governs line-scope-
and-up single-box behaviour; it does not make Sentence or Paragraph
auto-search skip a region entirely — a table, already `is_block()`, still
costs `s` one stop of its own (`sentence_span_does_not_run_into_a_table`
pins only that a sentence *outside* the table does not extend into it, not
that the table is invisible to the search). So the actual "skip it" behaviour
lives outside `ObjectKind` altogether, in two new `App` predicates
(`in_footnote`, consulted only by `step_next_sentence_start` /
`step_prev_sentence_start` / `first_sentence_start_on_page` /
`paragraph_step_next` / `paragraph_step_prev`) — never by
`sentence_run_start`/`sentence_run_end`, so a caret placed inside a footnote
deliberately (word, char or line motion — a footnote stays in
`content.lines`, unlike furniture, which is removed outright) still expands
and steps through its own sentences normally once there. Decision-log entry
19 below anticipates a kind someday needing a third boundary on the existing
word/line axis, at which point the two booleans there should collapse into
one; a footnote is not that case; it is a genuinely different axis
(*reachability by auto-search*, not *how coarse a stop*), and folding it into
`ObjectKind`'s four predicates would have required `syodep-pdf` to know about
Sentence/Paragraph auto-search, which belongs to `syodep-core`.
Detected the same way headings are, mirrored: a line reads as a footnote when
it sits in the page's bottom margin band and is set noticeably *smaller* than
the page's body size, the inverse of a heading's larger-than-body rule; both
share a factored-out `dominant_body_size` helper so the two detectors' notion
of "the body" cannot drift apart. Found and claimed before list items in
`content_objects`, deliberately: a footnote's own citation text is often
itself enumerator-shaped (`12. Author, Title`) and must not be misread as, or
corrupt the extent of, an unrelated list elsewhere on the page.

**Decision — highlight annotations are written through a second, short-lived
document handle, and their geometry and opacity go in by hand:**
`write_highlights(src, out, …)` is a free function that opens its own
`PdfDocument`, so it never touches the caller's live document (which is busy
rendering, and which `PdfDocument::try_from` would consume by value) and a
failed write cannot leave that document half-annotated. Three things about the
annotation dictionary are not obvious.
`PdfAnnotation::set_rect` *raises* for a highlight — MuPDF lists the subtypes
whose `/Rect` is settable and computes a quad-point annotation's rect from its
`/QuadPoints` instead — and the `mupdf` crate exposes no quad-point setter at all.
So the quads are written straight into the annotation's dictionary, reached
through the page's `/Annots` at the index recorded *before* the annotation was
created (rather than assuming it is the last one). They are transformed by the
inverse page CTM, which is what `pdf_set_annot_quad_points` does internally and
what keeps rotated pages correct, where a bare `height - y` would not.
Opacity has the identical gap: `mupdf` exposes no `/CA` setter either
(`pdf_set_annot_opacity` exists only in C, and a missing `/CA` defaults to
fully opaque), so `HighlightAnnotation::opacity` is written the same way the
quads are — straight into the dictionary — using the *current*
`[view] highlight_opacity` rather than a value captured per highlight the way
colour is: a PDF reader always paints a highlight with Multiply blending, so
without this every saved highlight would be full-strength regardless of what
the overlay previewed, since Multiply alone has no notion of fading a colour.
Finally `page.update()` must come *after* both dictionary edits: it
synthesises the appearance stream, which is what makes the highlight visible
in every reader, including our own renderer (`render_page` runs annotations).

Embedded highlights cannot disturb the content layer: text extraction goes
through `fz_new_stext_page_from_page`, which calls `fz_run_page_contents` and so
skips annotations entirely. That matters twice over — the caret gains no stops it
cannot see, and the `COLLECT_VECTORS` table hunt cannot mistake a highlight's
appearance rectangles for a ruled table. There is a test asserting the extracted
lines and objects are unchanged by a save, so the reasoning is pinned rather than
assumed.

**Decision — use `mupdf-rs` instead of hand-rolled bindgen FFI:** building
MuPDF from vendored source via cargo gives reproducible Linux+Windows
builds with zero system dependencies, and the bindings already encapsulate
the unsafe context/pointer management. If we outgrow them, the swap stays
inside this crate. Trade-off: slightly less control over MuPDF build flags.

Threading: MuPDF contexts are thread-local; `Document` is `!Send` and all
rendering happens on the opening thread (milestone 1 renders synchronously).

### syodep-storage

SQLite via `rusqlite` (bundled SQLite, no system dependency). Decisions:

- **Fingerprint identity:** documents are keyed by SHA-256 of file content,
  not path, so positions/annotations survive file moves and renames. The
  path is stored and refreshed for display purposes.
- **Migrations from day one:** `PRAGMA user_version` counts applied entries
  of an append-only `MIGRATIONS` list; each runs in a transaction.
  Databases from a *newer* build are refused rather than corrupted.
- **No dynamic state in TOML:** TOML is for human-edited settings only.

Schema v1: `documents` (id, fingerprint UNIQUE, path, timestamps) and
`positions` (document_id PK→documents CASCADE, scroll_x, scroll_y, zoom).

Schema v2: `highlights` (id, document_id→documents CASCADE, color, text,
created_at) and `highlight_rects` (highlight_id→highlights CASCADE, ordinal,
page, x0, y0, x1, y1). Geometry lives in a child table because one highlight
covers a rectangle per line and may run across pages — and because PDF
annotations are per-page, so the writer groups by `page` anyway.

Phase 2 adds marks/bookmarks/notes tables as further migrations.

**Consequence of saving:** writing highlights into the PDF changes its bytes and
therefore its fingerprint, which is the document's identity. `rekey_document`
moves the row to the new hash as part of the save, so the reading position
survives; without it every save would silently orphan it. See decision 16.

### syodep-ffi

The C ABI. Owns nothing conceptually; it is a mechanical projection of
`App` plus panic containment (`catch_unwind` on every entry point — a Rust
panic must never unwind into C++). Strings/bitmaps returned to C++ are
heap copies with explicit `syo_*_free` functions. The header is generated
by cbindgen at build time into `crates/syodep-ffi/include/syodep_ffi.h`.

### ui-qt

Five small components; intentionally boring:

- `key_encoder` translates `QKeyEvent` to the chord syntax (the shell's
  only input knowledge).
- `CanvasWidget` (a `QOpenGLWidget`) forwards keys/wheel/resizes, asks the
  core for visible page rects + bitmaps, paints them with `QPainter` on the
  GL-backed surface, and fills the overlay rectangles the core reports —
  `syo_app_focus`, `syo_app_selection`, `syo_app_highlights`, each a set of
  screen rects in canvas pixels, at most one of the first two ever valid at
  once — on top. Focus and visual go into one `QPainterPath` each so
  overlapping rects blend once, then a single plain-alpha fill. Highlights are
  different: each rectangle is composited with Multiply blending onto a copy
  of the page pixels it covers (a `QImage`, so the always-correct raster paint
  engine does the blending) before being drawn, because `QOpenGLWidget`'s own
  GL paint engine cannot be trusted with that composition mode — confirmed by
  testing, where a GPU/driver without the needed blend-equation extension
  painted solid black instead of blending at all. It keeps a tiny per-page
  `QImage` cache only to avoid re-copying bitmaps across the FFI every repaint
  (cleared on a save's `reload` effect, or on opening a different document);
  the real cache is in the core. Tiled GL texture rendering is planned for
  phase 3 (roadmap).
- `MainWindow` owns the `SyoApp*` handle, the status label and the native
  file dialog, and accepts PDFs dropped onto the window. The canvas fills the
  window but leaves `acceptDrops()` false, so Qt delivers drag events to the
  window; only it needs the flag.
- `diagnostics` detects the platform's graphics situation (WSL, software GL,
  missing OpenGL) and produces the `--check`/`--version` reports.
- `main.cpp` parses the CLI and implements `--smoke-test` for CI.

## Data flow example: pressing `5j`

1. Qt delivers two key events; `key_encoder` produces `"5"`, `"j"`.
2. Shell calls `syo_app_key_event` for each; core's `InputState` buffers
   the count, then resolves `j` → `scroll_down` with count 5.
3. `App::execute` scrolls the `View` by 5 × `scroll_step` pixels (converted
   to document space, clamped), saves the position to SQLite.
4. The FFI returns `SYO_EFFECT_REDRAW`; the shell calls `update()` and
   refreshes the status line from `syo_app_status_text`.
5. `paintGL` asks for visible pages, fetches bitmaps (core render cache),
   draws them.

## Decisions log

| # | Decision | Why | Revisit when |
|---|----------|-----|--------------|
| 1 | Rust core + thin Qt shell over C ABI | testability, no Qt types in core, clean ownership | never (foundational) |
| 2 | `mupdf-rs` bindings instead of own bindgen layer | reproducible cross-platform builds, less unsafe to own | MuPDF features we can't reach |
| 3 | Content fingerprint (SHA-256) as document identity | state survives moves/renames | huge files make hashing slow → partial hash |
| 4 | Scroll state in document space (points) | zoom changes don't displace the view | — |
| 5 | Timer-free key disambiguation (prefix waits, then longest-prefix fallback + replay) | predictability, testability; a binding that is also a prefix stays reachable without a timer | users demand Vim `timeoutlen` |
| 12 | Visual-mode scope belongs to each *endpoint*, not to the start/end role | crossing the anchor and coming back is the identity, so the selection never silently changes shape on an overshoot | a use case needs "the first edge is always line-granular" |
| 13 | Selection overlay is computed per visible page, not per selected page | cost is O(visible lines) however long the selection is, and page content is never force-extracted off-screen | ✅ done: storing a highlight needs the whole span, so `page_span_rects` is now the shared per-page geometry and `span_page_rects` walks every covered page |
| 15 | A highlight is stored as page-space rectangles plus its text, not as a caret span | rectangles are what all three consumers need — the overlay, the PDF's `/QuadPoints`, and export — and they draw correctly on reload without re-extracting any page content | highlights need to be re-anchored to text that has moved (a re-flowed or replaced document) |
| 16 | Saving re-keys the document row to the rewritten file's fingerprint, and drops the stored highlights | the fingerprint *is* the identity, so the position must follow the file; and once the highlights are annotations MuPDF renders, keeping rows too would paint them twice | notes/export need the rows after a save (then keep them with an `embedded_at` marker instead of deleting) |
| 17 | Highlight mode binds visual mode's commands rather than having its own | one implementation of reshaping a two-ended range, so a key provably cannot mean different things in the two modes; the feature cost three commands and no motion code | a highlight needs a motion a selection does not have |
| 6 | Synchronous rendering + byte-bounded LRU cache | simplest correct thing for M1 | phase 3 (async tiles) |
| 7 | Counts are runtime input, not part of binding syntax | matches Vim; keeps keymap finite | — |
| 8 | `0` counts only after a nonzero digit (Vim rule) | lets `0`-prefixed bindings exist later | — |
| 9 | Config errors degrade to defaults + warning | app must always start | — |
| 10 | cbindgen-generated header, checked into neither repo nor docs | single source of truth in Rust | ABI freeze for plugins (not planned) |
| 11 | Modal caret over content geometry (mode-selected keymap) | Vim-like `hjkl` caret without losing `hjkl` scrolling; one stop per image; goal-column vertical motion | always-on caret, or richer text objects (phase 3) |
| 14 | Atomicity is a property of the content, layered over the motion table rather than built into it | `step_scope` stays the pure per-scope description of a word/line/sentence/paragraph; one wrapper makes every scope treat a table or image as one unit, so counts and all six call sites keep working unchanged | ✅ done: unit-hood became per-scope in decision 19, and the wrapper now asks `unit_object_at(.., scope)` rather than a single category |
| 18 | Quitting with unsaved highlights asks (Save & Quit / Discard & Quit / Cancel) instead of quitting silently or refusing outright | highlights already outlive the session in the database, so "unsaved" only means "not yet embedded in the PDF bytes"; losing that silently on one careless keystroke (or window-manager Alt+F4) was the bug being fixed, and the existing `save_document`/`quit_discarding_highlights` split made the confirm-then-branch trivial to add without a new `Command` | a command palette or scripting API needs to trigger Save & Quit / Discard & Quit outside of the dialog flow (then promote them to `Command` variants) |
| 19 | Unit-hood is per scope: `is_atomic()` from word scope up, `is_block()` from line scope up | a table's rows and an equation's rows are not reading lines, so both should be one stop for `e`/`s`/`p` and paint as one box — but their contents *are* worth a word at a time, which a single category could not express without losing one or the other. Two nested categories say it in two `matches!` lines, and `page_span_rects` needs no scope at all because covering an object end to end already happens exactly at the scopes where it is one unit | a kind needs a third boundary (say, one unit from sentence scope up but not line), at which point the two booleans should become one "smallest scope at which this is a unit" — which requires moving the mapping into `syodep-core`, since `syodep-pdf` cannot name `Scope` |

## Sioyek: conceptual inspirations (clean-room)

Recorded per the project's license policy — these are *ideas* observed from
using Sioyek and reading its documentation, re-designed and re-implemented
independently:

- keyboard-first reading loop and command abstraction
- multi-key sequences with count prefixes
- persistent per-document reading state
- planned for later phases: marks, jump history, smart jump, overview popup

Deliberately avoided implementation patterns (also from studying Sioyek's
architecture at a high level): god Document/MainWidget classes, Qt types in
core logic, raw pointer ownership spread across the app, `void*` config
values, ad-hoc global state.

# Document-traversal architecture audit

**Date:** 2026-07-31  
**Scope:** Content extraction, caret traversal, scope/object policy, mode state,
and readiness for the annotation sidebar.  
**Non-goals:** No sidebar, comments, chat, highlight-persistence changes, or
reading-order rewrite in this step.

This document is the Step-1 deliverable before the annotation sidebar. It maps
the system as implemented, records invariants, lists findings, proposes
refactors, and notes what was hardened in-repo during the audit.

---

## 1. Module and responsibility map

```text
Qt shell (ui-qt)
    ↓ C ABI (syodep-ffi)
syodep-core::App
    ├── input / Command dispatch
    ├── Session (doc, view, render cache, content cache, furniture)
    ├── mode / focus / visual / pending highlight
    ├── traversal orchestration (private methods on App)
    └── caret.rs  — pure geometry, classifiers, columns, page_span_rects
syodep-pdf
    ├── MuPDF extraction → PageContent
    ├── furniture profile + filtering
    └── ContentObject detection (tables, images, headings, equations,
        footnotes, list items)
syodep-storage / syodep-config
    └── persistence and knobs (orthogonal to traversal math)
```

| Layer | Owns | Must not own |
|-------|------|--------------|
| `syodep-pdf` | Extraction, furniture, structural detection | Motion, modes, screen coords |
| `caret.rs` | Pure predicates, columns, paragraphs, overlay rects | Session, modes, MuPDF |
| `App` | Session, modes, motion orchestration, spans, view follow | Qt types |
| Qt shell | Input encoding, painting | Traversal / annotation logic |

**Verdict on boundaries:** The conceptual split is sound. Complexity has
concentrated in `App` (~8k lines, ~2.5k of which are traversal), not because
the boundaries are wrong, but because motion, object policy, adjacency, and
mode transitions all share one mutable session. That is the main extension
risk before the sidebar — not a need to rewrite extraction.

---

## 2. Content-processing pipeline

```text
PDF page
→ MuPDF TextPage (PRESERVE_IMAGES)
→ ContentLine / Cell (+ LineStyle, synthetic flags)
→ furniture_mask (optional; profile is document-wide)
→ kept lines + furniture lines
→ table_bboxes (optional second MuPDF pass)
→ heading_ranges / equation_ranges / footnote_ranges
→ content_objects (tables → images → headings → equations → footnotes → lists)
→ PageContent
→ Session.content[page]  (lazy, permanent for the session)
→ caret traversal / scope_span / overlays / stored highlight geometry
```

| Stage | In → Out | Pure? | Scope | Failure | Cached? | Downstream assumptions |
|-------|----------|-------|-------|---------|---------|------------------------|
| Pass 1 extract | page → lines/cells/styles | impure | page | `PdfError` | no (caller caches) | Cells in MuPDF block order; `synthetic` set |
| Furniture profile | doc → `FurnitureProfile` | impure sample + pure build | document | empty profile | once/session | Same filter for every page |
| Furniture mask | lines+styles+profile → kept/furniture | pure | page | never errors | with PageContent | Detectors see furniture-free lines |
| Table hunt | page → bboxes | impure | page | empty vec | with PageContent | Contiguous line membership after trim |
| Headings / eqs / footnotes | lines+styles → ranges | pure | page | empty | with PageContent | Indices into filtered lines |
| List items | lines + blocked → ranges | pure | page | empty | with PageContent | Corroborated markers; stops at blocked |
| `content_objects` | ranges → `Vec<ContentObject>` | pure | page | skip bad boxes | with PageContent | Sorted, disjoint, in-bounds (debug_assert) |
| `ensure_content` | page → cache insert | impure | page | **empty content** | yes | Cross-page skip treats empty ≡ failed |

**Reading order:** Taken directly from MuPDF structured-text block order. No
reconstruction pass. Images interleave at their block position. Two-column
pages usually emit left then right (or by block layout), but footnotes and
floats can appear mid-flow in emission order — footnotes are mitigated by
detection + auto-skip, not by reordering. See finding **F-RO-1**.

**Accepted product decisions (page locality):**

- A `SentenceMark` / `ParagraphMark` never spans pages; motion *finds* the
  next mark on another page.
- Furniture is removed from `lines` (not skipped by motion).
- Detector order is load-bearing (footnotes before list items).

---

## 3. Traversal call graph

```text
key → Command → App::execute
  → focus_move | visual_move | focus_scope_motion | visual_scope_motion
  → step_scope_atomic × count
      → step_scope (per-scope table)
      → (Word+) repeat until unit_id_at changes
  → update focus_goal_x | focus_goal_y
  → refresh_focus_span | refresh_visual_span
  → ensure_*_visible → save_position → Effects
```

### Per-scope motion

| Scope | Left / Right | Up / Down | Named next/prev (`w`/`b`/`s`/`p`) | Cross-page | Cross-column |
|-------|--------------|-----------|-------------------------------------|------------|--------------|
| Char | `step_left` / `step_right` | `step_up` / `step_down` + goal_x | N/A (use scope motion) | yes | N/A |
| Word | prev/next word start | `word_step_vertical` | same as L/R via scope motion | yes | N/A |
| Line | `line_step_column` + cell 0 | `line_step_up` / `down` | — | vertical yes; column same page | h/l |
| Sentence | column jump + `snap_to_scope` | `sentence_step_*` (skips footnotes) | `Dir::Down`/`Up` via scope motion | yes | h/l |
| Paragraph | column jump + snap | `paragraph_step_*` (skips footnotes) | same | yes | h/l |

Axis swap for line / sentence / paragraph on multi-column pages is centralized
in `step_scope` and mirrored in goal-row updates inside `focus_move` /
`visual_move`. Named `s`/`p` dispatch through `step_scope_atomic` with
vertical directions — they do **not** column-jump.

Object wrapping: `unit_object_at` → Char never; Word = atomic (Image);
Line+ = block (Image, Table, Equation, Footnote). Headings and list items are
**regions**, not motion units via this gate.

---

## 4. Selection pipeline

```text
Focus:   Caret + Scope → scope_span → (start,end) → focus_span
         → span_screen_rects / page_span_rects → overlay

Visual:  VisualAnchor + focus/focus_scope
         → expand each end by its scope
         → outer document-order span → visual_span
         → same rect pipeline
         o swaps endpoints (scopes included); geometry unchanged

Highlight: pending restore metadata only;
         extent = visual_span;
         commit → store_pending_highlight (page rects + text → SQLite)
         discard → restore pending snapshot
```

`page_span_rects` collapses a fully covered **block** object to `object.bbox`;
partial coverage stays per-line. Stale cell indices skip the line (silent
degrade) — see **F-GEOM-1**.

---

## 5. Mode-state diagram

```text
Normal ──f*──► Focus ──v*──► Visual ──a──► Highlight
   ▲             │  ▲          │  ▲            │
   │             │  │          │  │            │
   └──── Esc ────┘  └──── Esc / v fallthrough ─┘
                         │
                         └── discard (Esc/BS): restore pending
                         └── commit (a) / keep (v,f,save): store then leave
```

**Invariants (intended and largely held):**

1. One moving position: `focus`.
2. Visual adds only `VisualAnchor`.
3. `o` swaps ends; rendered span unchanged.
4. Scope change snaps via `snap_to_scope`; does not restore a per-scope mark.
5. Leaving visual drops the anchor; focus remains.
6. Highlight discard restores exact pre-entry state from `PendingHighlight`.
7. Focus / visual / highlight share `step_scope_atomic` motion.

---

## 6. Cache and invalidation table

| Field | Canonical source | Writers | Readers | Invalidation | Stale-guard tests |
|-------|------------------|---------|---------|--------------|-------------------|
| `Session.content[p]` | `page_content` | `ensure_content`, `set_page_content` (test) | all motion/spans | `open_document` clears map | furniture order; reopen without extract for stored highlights |
| `Session.furniture` | `furniture_profile` | first `ensure_content` | every extract | open clears | learned once/session |
| `focus` / `focus_scope` | user motion / mode entry | enter_*, *_move, swap, discard | spans, status | open resets | many focus tests |
| `focus_span` | `scope_span(focus,…)` | `refresh_focus_span` | overlays, scroll | cleared on open / normal | changing_scope_keeps_the_position |
| `visual` / `visual_span` | anchor + focus | enter_visual, visual_move, refresh | overlays, store | exit / open | visual span / swap tests |
| `focus_goal_x/y` | cell/line geometry | update after motion | vertical / column | open; swap resets | column jump + vertical page tests |
| `pending` | enter_highlight | enter / discard / store paths | discard restore | open | discard restore tests |
| `highlights` | SQLite + commits | load / store / save clear | overlays | save embeds & clears session list | reopen tests |
| `RenderCache` | `render_page` | render | paint | zoom / reload | render_cache unit tests |

**Gap:** Mutations are spread across many methods; most paths refresh spans, but
there is no single transition API. See Candidate F.

---

## 7. Object / scope behavior matrix (canonical)

Derived from `ObjectKind` predicates + `unit_object_at` + sentence/paragraph
search — not from the draft table in the audit brief.

| Object kind | Char | Word | Line | Sentence | Paragraph | Auto-skip (`s`/`p` search) |
|-------------|------|------|------|----------|-----------|----------------------------|
| Image | one cell | **one atomic unit** | **one block unit** | block unit | block; splits paragraphs | no |
| Table | inspect cells | inspect words | **one block** | region-bounded; one block step | splits; block step | no (visible stop) |
| Heading | inspect | inspect words | **inspect lines** (not a unit) | **one sentence** (`is_one_sentence` + region) | **own paragraph** (`splits_paragraphs`) | no |
| Equation | inspect | inspect terms | **one block** | **one sentence** + block | splits; block | no |
| List item | inspect | inspect words | inspect lines | sentences; marker not a boundary | **merged into list paragraph** | no |
| Footnote | inspect deliberately | inspect deliberately | **one block** | expand if inside; **search skips** | segment exists; **search skips** | **yes** |

Policy is split across:

- `ObjectKind::{is_atomic,is_block,is_one_sentence,splits_paragraphs}` in pdf
- `unit_object_at` in App
- `in_footnote` auto-skip in App (explicitly *not* an `ObjectKind` bit —
  architecture decision 19)

That split is intentional for footnotes; it is still easy to mis-extend. See
**F-OBJ-1** and Candidate B.

---

## 8. Adjacency inventory (summary)

Three intentional neighbor policies:

| Policy | Helpers | Crosses synthetic WS? | Crosses authored WS? | Line/page |
|--------|---------|----------------------|----------------------|-----------|
| Immediate | `next_cell_on_line`, tight number checks | no (adjacent only) | no | line |
| Synthetic peek (one cell) | `prev/next_real_cell_on_line`, `is_number_interior`, hyphen | skips **one** synthetic space | no | line |
| Link bridge | `link_at` / `link_span` | yes when shape allows | yes when next token starts with `.`/`/` and link still validates | line |
| Word run | `same_word_run` | only for number/hyphen/suffix bridges | link fragment rules | word motion crosses line/page via `next_cell` |
| Sentence | `sentence_boundary_after` + `*_in_region` | **tight** for numbers (any space ends sentence) | same | region-bounded; marks page-local |
| Token | `token_span` | **no** — any WS ends token | any WS ends token | line |

**Documented intentional disagreement:** word glue across synthetic gaps for
DOI/number fragments vs sentence **tight** neighbors so `reactor. Efficacy`
remains two sentences. Do not unify behind a generic “skip whitespace” helper.

---

## 9. Error-degradation map

| Situation | Behavior | Class |
|-----------|----------|-------|
| Extraction `Err` | empty `PageContent` cached; page skipped | graceful; historically indistinguishable from blank — hardened to set `last_error` |
| Furniture over-match | caps / never last line | graceful |
| Bad table box | discarded | graceful |
| Detector share caps | clear detections | graceful |
| Invalid cell index in span | skip line in `page_span_rects` | defensive; can hide bugs (**F-GEOM-1**) |
| Empty line land (`cell=0`) | possible via `step_*` / `object_landing` | invariant risk (**F-CARET-1**) |
| `store_pending` with no rects | no-op silent | defensive |
| Config detector off | objects absent; motion degrades to text | expected; footnote skip inactive if `detect_footnotes=false` |

---

## 10. Findings register

### Confirmed defects / invariant risks

#### F-CARET-1 — Empty-line landings can yield invalid carets
- **Severity:** Medium *(mitigated this audit)*
- **Where:** `step_right`/`step_left`/`step_up`/`step_down`, `line_step_*`,
  `object_landing`, content-page search
- **Behavior (was):** Landing on a line with `cells.is_empty()` used `cell = 0`.
- **Fix:** Steppers and content-page search skip empty lines; `object_landing`
  prefers a non-empty member line.
- **Tests:** `char_motion_skips_empty_lines_between_content`

#### F-EXTRACT-1 — Extraction failure looked like a blank page
- **Severity:** Medium (diagnostics) *(mitigated this audit)*
- **Where:** `ensure_content`
- **Behavior (was):** Failed extract cached as empty with no warning.
- **Fix:** Still cache empty (non-fatal) but insert into
  `content_extraction_failed` and set `last_error`.
- **Tests:** hand-built content asserts `!content_extraction_failed`; forcing a
  MuPDF failure in-unit remains future work.

#### F-GEOM-1 — `page_span_rects` silently skips disagreeing cell indices
- **Severity:** Low–Medium
- **Where:** `caret::page_span_rects`
- **Behavior:** Safer than wrong paint; can hide stale-span bugs.
- **Action:** Keep silent in release; add `debug_assert` / test helper that span endpoints are in-bounds before calling. Not required before sidebar.
- **Tests:** invariant helper preferred over changing paint.

### Plausible blind spots

#### F-RO-1 — Reading order is MuPDF emission order
- **Severity:** Medium (architectural)
- **Evidence:** Footnote mid-flow was a real bug mitigated by detection, not reordering. Column jumps use geometry; word/sentence sequential motion uses line vector order — these can disagree on pathological layouts.
- **Action:** Prefer option 1 (document limitations) until a second class of failures appears. Do **not** implement ReadingOrder before sidebar without new failing fixtures.
- **Accepted limitation:** RTL / vertical / heavily rotated body text is degraded; rotation furniture rule keeps dominant direction.

#### F-ADJ-1 — Many adjacency helpers, easy to pick the wrong one
- **Severity:** Medium (maintainability)
- **Action:** Candidate C — named neighborhood methods; no generic skip-WS.
- **Tests:** Keep existing DOI / `reactor. Efficacy` / link regression suite as the contract.

#### F-OBJ-1 — Footnote auto-skip lives outside `ObjectKind`
- **Severity:** Low–Medium (accepted design smell)
- **Action:** Documented in architecture.md; Candidate B may add `auto_skip_at(scope)` in **core**, not pdf.
- **Required before sidebar?** No.

#### F-TEST-1 — Sparse metamorphic / equivalence coverage
- **Severity:** Medium (test)
- **Gaps:** count ≡ repeated steps for column jumps; focus ≡ visual for every scope; reverse word metamorphic at page edges; detector-off combinations.
- **Action:** Add targeted metamorphic tests (started this audit); expand before large refactors.

#### F-CFG-1 — Disabling detectors changes motion meaning without warnings
- **Severity:** Low
- **Action:** Document; add one mixed-flag integration test before sidebar if sidebar assumes objects exist.

### Architectural smells

#### F-APP-1 — `App` owns too many traversal concerns
- **Severity:** Medium
- **Action:** Candidate A in two steps (`ContentSession`, then `MotionEngine`). Not required to *start* a read-only sidebar that consumes existing highlight geometry, but required before document-grounded chat that reuses traversal.
- **Risk if deferred:** Sidebar that reimplements span logic in Qt would violate core/UI split.

#### F-STATE-1 — Span/goal refresh duplicated across mode paths
- **Severity:** Low–Medium
- **Action:** Candidate F — named transitions (`set_focus`, `apply_motion`, …).

### Explicitly accepted limitations

- Sentences and paragraphs are **page-local marks**; page breaks always terminate them.
- No perfect reading order for all PDFs.
- Table/heading/equation detection is heuristic with whole-page rejection.
- Furniture false negatives leave headers navigable; false positives are capped.
- Char scope is the escape hatch into every object.

---

## 11. Test coverage matrix

Legend: **D** direct · **I** indirect · **N** not covered · **—** N/A

Rows: scopes. Columns: motions / situations. (Focus mode unless noted.)

| | L | R | U | D | next-unit | prev-unit | page× | col× | obj enter | obj exit |
|-|---|---|---|---|-----------|-----------|-------|------|-----------|----------|
| Char | D | D | D | D | — | — | D | — | D (table) | I |
| Word | D | D | D | D | D (`w`) | D (`b`) | D | — | D (table/image) | D |
| Line | D (col) | D | D | D | — | — | D | D | D (table block) | D |
| Sentence | D (col) | D | D | D | D (`s`) | D | D | D | I (heading/eq) | I |
| Paragraph | D (col) | D | D | D | D (`p`) | D | D | D | I | I |

Additional dimensions:

| Dimension | Status |
|-----------|--------|
| Visual mirrors focus motion | I / some D (column, table); full matrix **N** |
| Highlight reshape ≡ visual | D (`highlight_mode_reshapes_exactly_as_visual_mode_does`) |
| Count ≡ N× single step | I / some D; column-jump counts **N** |
| Highlight discard restore | D |
| Empty content / blank pages | I (cross-page skip) |
| First/last document cell | I |
| Detector flags off | furniture D; others **N** |
| Empty lines between content | **D** (added) |
| Object ranges sorted/disjoint | D (pdf) |
| Reverse then forward identity | I for words; incomplete metamorphic suite |

---

## 12. Refactor proposals

### Candidate A — Extract traversal from `App`
- **Solves:** F-APP-1; testability of motion without mode/scroll/persist.
- **Must preserve:** all motion semantics; goal columns; object atomic steps.
- **Files:** new `navigator.rs` or `content_session.rs`; thin `App` wrappers.
- **Sequence:** (1) ContentSession + ensure_content, (2) MotionEngine taking `&mut ContentSession`, (3) App keeps modes/effects.
- **Tests:** existing app motion suite unchanged at the command level.
- **Risk:** Medium borrow plumbing.
- **Before sidebar?** Not for read-only highlight list. **Yes** before chat that asks “next sentence from caret”.

### Candidate B — Centralize object/scope policy
- **Solves:** F-OBJ-1 readability; prevents contradictory predicates.
- **Must preserve:** footnote auto-skip axis separate from block axis.
- **Form:** `fn movement_unit(self, scope) -> bool` + `fn auto_skip_in_search(self, scope) -> bool` in core using pdf kinds.
- **Before sidebar?** Nice-to-have; document matrix is enough for now.

### Candidate C — TextNeighborhood
- **Solves:** F-ADJ-1.
- **Must preserve:** tight vs peek vs link-bridge distinctions.
- **Before sidebar?** No.

### Candidate D — Validated carets
- **Solves:** F-CARET-1.
- **Form:** `fn caret_is_valid` + skip empties in steppers (partially done).
- **Before sidebar?** Yes for any feature that stores caret-like anchors into annotations.

### Candidate E — Explicit ReadingOrder
- **Solves:** F-RO-1 if new failures appear.
- **Before sidebar?** **No** — needs failing fixtures beyond footnotes.

### Candidate F — Named state transitions
- **Solves:** F-STATE-1.
- **Before sidebar?** Helpful if sidebar triggers mode changes; not blocking for read-only panel.

---

## 13. Performance notes (estimates)

| Operation | Typical cost | Risk |
|-----------|--------------|------|
| one char step | O(1) + ensure_content once/page | low |
| word step | O(run length) same_word_run checks | low |
| sentence step | O(cells in sentence) + optional page scan skipping footnotes | medium on huge pages |
| paragraph step | O(lines) recompute segments each call | medium; cache only if measured |
| column jump | O(lines) column_ranges | low–medium |
| scope_span | O(unit size) | low |
| count 1000 | 1000 × step; may touch many pages | acceptable |
| furniture profile | once/session, samples ~8 pages | OK |
| link_at | O(token + bridge) | OK |

No new caches recommended until a profile shows reuse with simple invalidation.

---

## 14. Sidebar readiness (conclusion)

Safe to start a **read-only annotation sidebar** that:

- lists `highlights` already owned by `App` / storage,
- paints via existing overlay FFI,
- does not reimplement span or motion in Qt.

Defer until after Candidates **D** (caret validity) and preferably **A**/partial **F**:

- comments anchored to live caret spans,
- document-grounded chat that issues traversal commands,
- any UI that assumes extraction failure ≠ blank page (now partially addressed).

Do **not** put traversal logic in the shell.

---

## 15. Changes landed with this audit

1. Documented the pipelines, matrices, and findings in this file.
2. Distinguished extraction failure from blank via `last_error` (+ recorded page set).
3. `PageContent::object_invariants_ok` for tests/debug.
4. Char motion skips empty lines; content-page search requires a non-empty line.
5. Metamorphic / invariant tests for empty lines, visual swap identity, and
   count ≡ repeated word steps.
6. Development-log entry; pointer from `architecture.md`.

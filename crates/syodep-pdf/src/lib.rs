//! Safe PDF backend for syodep.
//!
//! This crate is the only place in the workspace that talks to MuPDF. It
//! wraps the `mupdf` crate (Rust bindings that build MuPDF's C library from
//! vendored sources) and exposes syodep's own value types — [`Document`],
//! [`Size`], [`Rect`], [`Bitmap`], [`OutlineItem`] — so that no MuPDF type
//! or pointer ever leaks to the rest of the application.
//!
//! Architectural decision (see `docs/architecture.md`): we deliberately use
//! the maintained `mupdf` bindings instead of hand-rolling `bindgen` FFI.
//! All `unsafe` stays inside those bindings; this crate and everything above
//! it is 100% safe Rust. If we ever outgrow the bindings we can swap the
//! implementation behind these types without touching callers.
//!
//! Threading: MuPDF contexts are thread-local; [`Document`] is intentionally
//! `!Send` (enforced by the inner type) and all rendering happens on the
//! thread that opened the document. Asynchronous rendering is a later
//! milestone and will use one document handle per worker thread.

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

use std::collections::BTreeMap;
use std::path::Path;

use mupdf::{
    text_page::{TextBlockType, TextCharFlags},
    Colorspace, Matrix, TextPageFlags,
};

/// Errors surfaced by the PDF backend.
#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("cannot open {path}: {message}")]
    Open { path: String, message: String },
    #[error("page {page} out of range (document has {count} pages)")]
    PageOutOfRange { page: usize, count: usize },
    #[error("password-protected documents are not supported yet")]
    PasswordProtected,
    #[error("PDF backend error: {0}")]
    Backend(String),
}

impl From<mupdf::Error> for PdfError {
    fn from(e: mupdf::Error) -> Self {
        PdfError::Backend(e.to_string())
    }
}

/// Page size in PDF points (1/72 inch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// Axis-aligned rectangle in page coordinates (points, origin top-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn area(self) -> f32 {
        (self.x1 - self.x0).max(0.0) * (self.y1 - self.y0).max(0.0)
    }

    pub fn union(self, other: Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    /// Whether the centre of `other` lies within this rectangle.
    ///
    /// Centre containment (rather than overlap or full containment) is the
    /// rule MuPDF itself uses when assigning characters to table cells, and it
    /// tolerates a table box drawn slightly wider than its contents.
    pub fn contains_center_of(self, other: Rect) -> bool {
        let cx = (other.x0 + other.x1) / 2.0;
        let cy = (other.y0 + other.y1) / 2.0;
        cx >= self.x0 && cx <= self.x1 && cy >= self.y0 && cy <= self.y1
    }
}

/// What a single caret stop represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    /// One text character (including spaces).
    Char(char),
    /// A raster or vector image, treated as a single caret stop.
    Image,
}

/// One navigable stop: a character or an image, with its bounding box in page
/// points (origin top-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub kind: CellKind,
    pub bbox: Rect,
    /// Whether MuPDF guessed this character rather than reading it from the
    /// content stream — set from `TextCharFlags::SYNTHETIC`. MuPDF inserts a
    /// guessed space between two glyphs drawn by separate positioning
    /// operations when the gap between them, as a fraction of font size,
    /// looks space-shaped. Common in hyperlinked URLs/DOIs, whose path
    /// segments a PDF producer often draws as separate runs: the gap is real
    /// ink-to-ink space on the page, but it is not an authored word
    /// boundary. Always `false` for [`CellKind::Image`].
    pub synthetic: bool,
}

/// A line of content in reading order — a run of character cells, or a single
/// image cell. `bbox` covers the whole line.
#[derive(Debug, Clone, PartialEq)]
pub struct ContentLine {
    pub bbox: Rect,
    pub cells: Vec<Cell>,
}

/// What kind of thing a [`ContentObject`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Image,
    Table,
    Caption,
    Heading,
    ListItem,
    Equation,
    Code,
    Footnote,
}

impl ObjectKind {
    /// Whether this is a single stop from *word* scope up.
    ///
    /// Only an image is: it has no words to walk through, so `w` inside one
    /// could only ever mean "leave it". Everything else is made of text a
    /// reader may want a part of — a table's cells included, which is why a
    /// table is a [block](Self::is_block) rather than this.
    pub fn is_atomic(self) -> bool {
        matches!(self, Self::Image)
    }

    /// Whether this is a single stop from *line* scope up, and draws as one
    /// box when covered end to end.
    ///
    /// A table's rows and a display equation's rows are not reading lines:
    /// stopping on row two of an aligned system, or tinting only the text
    /// cells of a table and leaving its rules unpainted, is never what the
    /// reader meant. Word and char scope still walk inside both, so a single
    /// coefficient or table cell stays reachable. A footnote is the same
    /// shape for a different reason: it is real prose, not a table, but it
    /// is meant to be reachable only by deliberately walking into it —
    /// `hjkl` at line scope skip over it as one stop rather than reading it
    /// row by row, the same as a table's rows.
    ///
    /// Every atomic kind is also a block — the two nest, coarsest last.
    pub fn is_block(self) -> bool {
        matches!(
            self,
            Self::Image
                | Self::Table
                | Self::Caption
                | Self::Equation
                | Self::Code
                | Self::Footnote
        )
    }

    /// Whether every sentence terminator inside this is inert, making the whole
    /// of it one sentence.
    ///
    /// A heading needs it because `3.1. Methods` is not three sentences, and an
    /// equation because `f(x) = 0.` is not two. Prose kinds do not: a list item
    /// is walked sentence by sentence on purpose, and so are a footnote and a
    /// caption (captions are often multi-sentence) once a reader has
    /// deliberately stepped into one — unlike `is_block`, this predicate is
    /// about what happens *inside* the region, not whether normal reading
    /// order stops there at all (see the sentence/paragraph auto-search logic
    /// in `syodep-core`, which is what actually keeps a footnote or caption
    /// out of ordinary reading).
    pub fn is_one_sentence(self) -> bool {
        matches!(self, Self::Heading | Self::Equation)
    }

    /// Whether this stands alone as a paragraph.
    ///
    /// A list is one paragraph made of many items, so `p` skips the whole list
    /// in a press while `s` walks it item by item. Every other kind is a
    /// paragraph in its own right.
    pub fn splits_paragraphs(self) -> bool {
        !matches!(self, Self::ListItem)
    }
}

/// A run of content lines that navigation treats as a single unit.
///
/// `start_line..=end_line` is inclusive and indexes [`PageContent::lines`].
/// An image is always a one-line object; a table covers every line inside it;
/// a heading covers its (possibly wrapped) lines.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContentObject {
    pub kind: ObjectKind,
    pub bbox: Rect,
    pub start_line: usize,
    pub end_line: usize,
}

/// The navigable content of one page: its lines, plus the runs of lines that
/// behave as a single unit.
///
/// Invariant established by [`Document::page_content`] and relied on by the
/// caret: `objects` is sorted by `start_line`, the ranges are disjoint, every
/// member line exists, and at least one member line is non-empty.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageContent {
    pub lines: Vec<ContentLine>,
    pub objects: Vec<ContentObject>,
    /// Lines removed as page furniture — running heads, folios, rotated
    /// stamps. Never navigable, kept so nothing is destroyed and so tests can
    /// assert *which* lines went rather than merely how many.
    pub furniture: Vec<ContentLine>,
}

impl PageContent {
    /// The object containing `line`, if any — a *region*, heading included.
    /// This is what bounds sentence runs and splits paragraphs.
    pub fn object_at(&self, line: usize) -> Option<&ContentObject> {
        self.objects
            .iter()
            .find(|o| line >= o.start_line && line <= o.end_line)
    }

    /// The *atomic* object containing `line`, if any. This is what motion
    /// treats as one stop from word scope up.
    pub fn atomic_object_at(&self, line: usize) -> Option<&ContentObject> {
        self.object_at(line).filter(|o| o.kind.is_atomic())
    }

    /// The *block* containing `line`, if any. This is what motion treats as
    /// one stop from line scope up, and what draws as a single box.
    pub fn block_object_at(&self, line: usize) -> Option<&ContentObject> {
        self.object_at(line).filter(|o| o.kind.is_block())
    }

    /// Whether [`Self::objects`] matches the invariants [`Document::page_content`]
    /// promises: sorted by `start_line`, pairwise disjoint, in range of
    /// [`Self::lines`], and each range containing at least one non-empty line.
    ///
    /// Used by tests and debug assertions so those rules stay inspectable when
    /// content is hand-built (`set_page_content`) rather than extracted.
    pub fn object_invariants_ok(&self) -> bool {
        object_ranges_ok(&self.lines, &self.objects)
    }
}

/// Shared check for [`PageContent::object_invariants_ok`] and the
/// `content_objects` debug assertion — avoids cloning lines just to validate.
fn object_ranges_ok(lines: &[ContentLine], objects: &[ContentObject]) -> bool {
    let n = lines.len();
    for window in objects.windows(2) {
        if window[0].start_line > window[0].end_line {
            return false;
        }
        if window[0].end_line >= window[1].start_line {
            return false;
        }
    }
    for object in objects {
        if object.start_line > object.end_line || object.end_line >= n {
            return false;
        }
        let has_content = lines[object.start_line..=object.end_line]
            .iter()
            .any(|line| !line.cells.is_empty());
        if !has_content {
            return false;
        }
    }
    true
}

/// Knobs for [`Document::page_content`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentOptions {
    /// Run MuPDF's table detection so a table becomes one navigable unit from
    /// line scope up, drawn as one box, while word and char scope still walk
    /// its cells. Costs a second structured-text pass per page.
    pub detect_tables: bool,
    /// Detect figure/table captions near images and tables. Free: geometry and
    /// type sizes come from the extraction pass.
    pub detect_captions: bool,
    /// Detect headings so each is one sentence and one paragraph. Free: the
    /// type sizes it keys on come from the pass that extracts the text.
    pub detect_headings: bool,
    /// Detect display equations so each is one unit from line scope up — one
    /// sentence, one paragraph, one stop for `e`, and one box — while staying
    /// walkable by word and character. Free, like headings: the fonts and
    /// characters it keys on come from the extraction pass.
    pub detect_equations: bool,
    /// Detect monospace code blocks so each is one stop from line scope up.
    /// Free: font names come from the extraction pass.
    pub detect_code: bool,
    /// Drop running heads, folios and text that does not run in the page's
    /// reading direction, so the caret never traverses them.
    pub skip_page_furniture: bool,
    /// Detect footnote blocks — text set smaller than the body in the bottom
    /// margin — so each is one stop from line scope up, drawn as one box,
    /// while staying walkable by word and character (the same shape as a
    /// table). Sentence and paragraph motion additionally skip a footnote
    /// entirely while auto-searching from ordinary body text, so reading a
    /// page's prose never lands on one; a caret placed there deliberately
    /// still reads normally once inside. Free, like headings and equations.
    pub detect_footnotes: bool,
}

impl Default for ContentOptions {
    fn default() -> Self {
        Self {
            detect_tables: true,
            detect_captions: true,
            detect_headings: true,
            detect_equations: true,
            detect_code: true,
            skip_page_furniture: true,
            detect_footnotes: true,
        }
    }
}

/// The typography of one content line, used only by detection.
///
/// Not part of [`ContentLine`]: nothing outside detection needs it, and
/// keeping it out means the public content types stay about geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LineStyle {
    /// The type size most of the line's characters are set in.
    size: f32,
    /// Whether the line is essentially all bold.
    bold: bool,
    /// Share of the line's inked characters set in a math font, 0.0 to 1.0.
    math: f32,
    /// Share of the line's inked characters set in a monospace font, 0.0 to 1.0.
    mono: f32,
    /// Angle of the line's baseline in degrees, or `None` when its characters
    /// disagree or carry no usable direction.
    angle: Option<f32>,
    /// The line's baseline, as the median character origin. Glyph extents make
    /// a bounding box a poor position key — a descender moves it by points —
    /// whereas the baseline is where the type actually sits.
    baseline: f32,
}

/// Which edge of the page a margin band hugs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

/// One thing that recurs in the margins of a document.
#[derive(Debug, Clone, PartialEq)]
struct FurnitureEntry {
    text: String,
    edge: Edge,
    /// Distance from the nearer page edge, so mixed page sizes still match.
    offset: f32,
    pages: usize,
}

/// What a document repeats in its margins: running heads, folios and the like.
///
/// Document-scoped rather than per-page, because repetition *is* the evidence.
/// A title appears once and so is never mistaken for a running head.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FurnitureProfile {
    entries: Vec<FurnitureEntry>,
    pages_sampled: usize,
}

impl FurnitureProfile {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn pages_sampled(&self) -> usize {
        self.pages_sampled
    }
}

/// A line summarised for the margin-repetition vote.
#[derive(Debug, Clone, PartialEq)]
struct BandLine {
    text: String,
    edge: Edge,
    offset: f32,
}

/// How far off the page's dominant direction a line must sit to be marginal.
/// Ordinary lines report their angle to the exact degree — measured across a
/// real document, every one came back `0.0` — so this is enormously slack.
const ANGLE_TOLERANCE_DEG: f32 = 10.0;

/// The dominant direction must carry this share of a page's characters. Below
/// it the page has no clear reading direction and nothing is marginal.
const ANGLE_DOMINANCE: f32 = 0.55;

/// How much of the page, top and bottom, counts as margin band.
const BAND_SHARE: f32 = 0.15;

/// Deeper bottom band used only for folio-shaped lines (normalised text is
/// exactly `#`). Journal layouts often set the page number a few points above
/// the strict 15% band; without this the profile stays empty and every folio
/// is walked as body text — sometimes even flagged as a heading.
const FOLIO_BAND_SHARE: f32 = 0.20;

/// How far two baselines may differ and still be the same running element.
const BASELINE_TOLERANCE: f32 = 2.5;

/// Fallback tolerance for a line already inside the margin band whose
/// baseline still drifted off the profile by more than
/// [`BASELINE_TOLERANCE`] — own-page content or a MediaBox/rounding quirk
/// can shift a header a few points from its siblings even though it is
/// plainly still in the margin. Only reached once the strict offset match
/// has already failed for a line [`band_of_with_folio`] still accepted, and
/// only ever applied alongside an *exact* normalised-text match against an
/// established profile entry (see [`furniture_mask`]) — text equality is
/// what makes the wider window safe. This never widens the margin band
/// itself: a line [`band_of_with_folio`] rejects outright stays rejected,
/// exactly as strict as before (see
/// `mask_does_not_use_the_deeper_band_for_non_folio_text`, which pins that a
/// text+offset coincidence outside the band must never become furniture).
const RELAXED_BASELINE_TOLERANCE: f32 = 8.0;

/// A margin entry must recur on at least this many sampled pages, and this
/// share of them. Two rather than one is what stops a one-off title being read
/// as a running head; the share tolerates headers that alternate between
/// facing pages.
const MIN_REPEAT_PAGES: usize = 2;
const MIN_REPEAT_SHARE: f32 = 0.30;

/// Caps on the repetition rule (the rotation rule needs none — see
/// [`furniture_mask`]).
const MAX_ENTRIES_PER_EDGE: usize = 4;
const MAX_FURNITURE_PER_EDGE: usize = 3;
const MAX_FURNITURE_SHARE: f32 = 0.25;
const MIN_LINES_FOR_SHARE_CAP: usize = 12;

/// Pages are sampled as this many anchors, each a *pair* of facing pages.
const SAMPLE_ANCHORS: usize = 4;

/// A gap this wide between two characters on the same baseline almost never
/// occurs within one field of text (word spacing is a few points even at
/// large sizes), but comfortably separates two fields sharing a margin line —
/// a running head on one side and a folio on the other, say. Splitting on it
/// is what keeps the repetition rule from caring which side either one is
/// printed on, or which one comes first in reading order.
const SEGMENT_GAP_POINTS: f32 = 20.0;

/// A margin column of line numbers must show at least this many purely
/// numeric lines before it counts as manuscript line-numbering rather than a
/// coincidental stray number (a footnote marker, an equation number, ...).
const MIN_LINE_NUMBERS: usize = 3;

/// How far a numeric column's right edge must sit from the body text's own
/// left edge to count as a separate column rather than part of the body — a
/// real gutter, not ordinary letter-spacing.
const LINE_NUMBER_GAP: f32 = 6.0;

/// An RGBA8 image, tightly packed (`stride == width * 4`).
#[derive(Debug, Clone)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// One entry of the document outline (table of contents).
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    /// Zero-based target page, when the entry points into the document.
    pub page: Option<usize>,
    pub children: Vec<OutlineItem>,
}

/// An open PDF document.
///
/// Pages are loaded lazily and not retained; per-page metadata that the
/// layout needs (sizes) is captured eagerly at open so that layout can be
/// computed without touching MuPDF again.
#[derive(Debug)]
pub struct Document {
    inner: mupdf::Document,
    page_sizes: Vec<Size>,
}

impl Document {
    /// Open a document from a file path.
    pub fn open(path: &Path) -> Result<Self, PdfError> {
        let path_str = path.to_string_lossy();
        let inner = mupdf::Document::open(path_str.as_ref()).map_err(|e| PdfError::Open {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        Self::from_inner(inner, &path_str)
    }

    /// Open a document from in-memory bytes (used by tests).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PdfError> {
        let inner =
            mupdf::Document::from_bytes(bytes, "application/pdf").map_err(|e| PdfError::Open {
                path: "<memory>".to_owned(),
                message: e.to_string(),
            })?;
        Self::from_inner(inner, "<memory>")
    }

    fn from_inner(inner: mupdf::Document, path: &str) -> Result<Self, PdfError> {
        if inner.needs_password().unwrap_or(false) {
            return Err(PdfError::PasswordProtected);
        }
        let count = inner.page_count().map_err(|e| PdfError::Open {
            path: path.to_owned(),
            message: e.to_string(),
        })? as usize;
        let mut page_sizes = Vec::with_capacity(count);
        for i in 0..count {
            let page = inner.load_page(i as i32)?;
            let bounds = page.bounds()?;
            page_sizes.push(Size {
                width: bounds.x1 - bounds.x0,
                height: bounds.y1 - bounds.y0,
            });
        }
        Ok(Self { inner, page_sizes })
    }

    pub fn page_count(&self) -> usize {
        self.page_sizes.len()
    }

    /// Size of every page, in document order.
    pub fn page_sizes(&self) -> &[Size] {
        &self.page_sizes
    }

    pub fn page_size(&self, page: usize) -> Result<Size, PdfError> {
        self.page_sizes
            .get(page)
            .copied()
            .ok_or(PdfError::PageOutOfRange {
                page,
                count: self.page_count(),
            })
    }

    fn check_page(&self, page: usize) -> Result<(), PdfError> {
        if page >= self.page_count() {
            return Err(PdfError::PageOutOfRange {
                page,
                count: self.page_count(),
            });
        }
        Ok(())
    }

    /// Render a page at `scale` (1.0 = 72 dpi) into a tightly packed RGBA8
    /// bitmap with a white background.
    pub fn render_page(&self, page: usize, scale: f32) -> Result<Bitmap, PdfError> {
        self.check_page(page)?;
        let scale = scale.max(0.01);
        let mupdf_page = self.inner.load_page(page as i32)?;
        // alpha = false renders on an opaque white background (paper-like);
        // the RGB samples are then expanded to the RGBA the canvas expects.
        let pixmap = mupdf_page.to_pixmap(
            &Matrix::new_scale(scale, scale),
            &Colorspace::device_rgb(),
            false,
            true,
        )?;
        let width = pixmap.width();
        let height = pixmap.height();
        let samples = pixmap.samples();
        let expected = width as usize * height as usize * 3;
        if samples.len() < expected {
            return Err(PdfError::Backend(format!(
                "pixmap sample buffer too small: {} < {expected}",
                samples.len()
            )));
        }
        let mut data = Vec::with_capacity(width as usize * height as usize * 4);
        for rgb in samples[..expected].as_chunks::<3>().0 {
            data.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 0xff]);
        }
        Ok(Bitmap {
            width,
            height,
            data,
        })
    }

    /// Extract the plain text of a page.
    pub fn page_text(&self, page: usize) -> Result<String, PdfError> {
        self.check_page(page)?;
        let mupdf_page = self.inner.load_page(page as i32)?;
        let text_page = mupdf_page.to_text_page(TextPageFlags::empty())?;
        Ok(text_page.to_text()?)
    }

    /// Per-page navigable content: text lines (each a sequence of character
    /// cells) and images (one cell each), in reading order, with bounding
    /// boxes in page points. This is the geometry layer the caret navigates.
    ///
    /// `PRESERVE_IMAGES` is required for image blocks to appear in the
    /// structured-text output at all; the default flags drop them.
    ///
    /// Tables come from a **second** structured-text pass (see
    /// [`Document::table_bboxes`]), because the pass that finds them cannot
    /// also yield their text.
    pub fn page_content(
        &self,
        page: usize,
        opts: ContentOptions,
        furniture_profile: Option<&FurnitureProfile>,
    ) -> Result<PageContent, PdfError> {
        self.check_page(page)?;
        let mupdf_page = self.inner.load_page(page as i32)?;
        let text_page = mupdf_page.to_text_page(TextPageFlags::PRESERVE_IMAGES)?;
        let mut lines = Vec::new();
        let mut image_lines = Vec::new();
        let mut styles = Vec::new();
        for block in text_page.blocks() {
            if matches!(block.r#type(), TextBlockType::Image) {
                let bbox = rect_from_mupdf(block.bounds());
                image_lines.push(lines.len());
                lines.push(ContentLine {
                    bbox,
                    cells: vec![Cell {
                        kind: CellKind::Image,
                        bbox,
                        synthetic: false,
                    }],
                });
                styles.push(LineStyle {
                    size: 0.0,
                    bold: false,
                    math: 0.0,
                    mono: 0.0,
                    // An image has no direction and no text, so it can never be
                    // furniture: a logo inside a running header survives as an
                    // image stop. Deliberately conservative.
                    angle: None,
                    baseline: (bbox.y0 + bbox.y1) / 2.0,
                });
                continue;
            }
            for line in block.lines() {
                let mut cells = Vec::new();
                let mut sizes: Vec<(i32, usize)> = Vec::new();
                let mut dirs: Vec<(f32, f32)> = Vec::new();
                let mut origins: Vec<f32> = Vec::new();
                let (mut bold, mut inked, mut math, mut mono) = (0usize, 0usize, 0usize, 0usize);
                // Glyphs come in font runs, so remembering the last verdict
                // turns the name test into one string compare per character.
                let mut last_font: Option<(String, bool, bool)> = None;
                for ch in line.chars() {
                    let Some(c) = ch.char() else { continue };
                    let quad = ch.quad();
                    cells.push(Cell {
                        kind: CellKind::Char(c),
                        bbox: rect_from_quad(&quad),
                        synthetic: ch.flags().contains(TextCharFlags::SYNTHETIC),
                    });
                    if c.is_whitespace() {
                        continue;
                    }
                    inked += 1;
                    origins.push(ch.origin().y);
                    // The top and bottom edges of the quad both run along the
                    // baseline, so either gives the direction the line travels
                    // in — even for a single glyph, where there is no second
                    // origin to subtract from.
                    let (mut dx, mut dy) = (quad.ur.x - quad.ul.x, quad.ur.y - quad.ul.y);
                    if dx.hypot(dy) < 1e-3 {
                        dx = quad.lr.x - quad.ll.x;
                        dy = quad.lr.y - quad.ll.y;
                    }
                    let len = dx.hypot(dy);
                    if len >= 1e-3 {
                        dirs.push((dx / len, dy / len));
                    }
                    // Bucket to 0.1pt so one superscript cannot outvote the
                    // body of the line.
                    let bucket = (ch.size() * 10.0).round() as i32;
                    match sizes.iter_mut().find(|(b, _)| *b == bucket) {
                        Some((_, n)) => *n += 1,
                        None => sizes.push((bucket, 1)),
                    }
                    if ch.flags().contains(TextCharFlags::BOLD) {
                        bold += 1;
                    }
                    if let Some(font) = ch.font() {
                        let name = font.name();
                        let (is_math, is_mono) = match &last_font {
                            Some((seen, math_v, mono_v)) if seen == name => (*math_v, *mono_v),
                            _ => {
                                let math_v = is_math_font(name);
                                let mono_v = is_mono_font(name);
                                last_font = Some((name.to_owned(), math_v, mono_v));
                                (math_v, mono_v)
                            }
                        };
                        if is_math {
                            math += 1;
                        }
                        if is_mono {
                            mono += 1;
                        }
                    }
                }
                if cells.is_empty() {
                    continue;
                }
                let size = sizes
                    .iter()
                    .max_by_key(|(bucket, n)| (*n, *bucket))
                    .map_or(0.0, |(bucket, _)| *bucket as f32 / 10.0);
                let bbox = rect_from_mupdf(line.bounds());
                origins.sort_by(f32::total_cmp);
                let baseline = origins
                    .get(origins.len() / 2)
                    .copied()
                    .unwrap_or((bbox.y0 + bbox.y1) / 2.0);
                lines.push(ContentLine { bbox, cells });
                let share = |n: usize| {
                    if inked > 0 {
                        n as f32 / inked as f32
                    } else {
                        0.0
                    }
                };
                styles.push(LineStyle {
                    size,
                    bold: inked > 0 && bold * 5 >= inked * 4,
                    math: share(math),
                    mono: share(mono),
                    angle: line_angle(&dirs),
                    baseline,
                });
            }
        }

        // Furniture goes before anything else looks at the page, so headings
        // and tables are judged against reading matter alone: a full-width
        // running header inflates the "widest line" the heading rule compares
        // against, and an 8pt folio drags its body-size vote.
        let mut furniture = Vec::new();
        if opts.skip_page_furniture {
            let height = self.page_size(page)?.height;
            let mask = furniture_mask(&lines, &styles, height, furniture_profile);
            let mut old_to_new = vec![None; lines.len()];
            let (mut kept_lines, mut kept_styles) = (Vec::new(), Vec::new());
            for (i, (line, style)) in lines.into_iter().zip(styles).enumerate() {
                if mask[i] {
                    furniture.push(line);
                } else {
                    old_to_new[i] = Some(kept_lines.len());
                    kept_lines.push(line);
                    kept_styles.push(style);
                }
            }
            // Image indices point into the unfiltered vector; dropping lines
            // without remapping them compiles perfectly well and silently
            // labels a line of text as an image.
            image_lines = image_lines.iter().filter_map(|&i| old_to_new[i]).collect();
            lines = kept_lines;
            styles = kept_styles;
        }

        // A single line can never form a table, so skip the second pass.
        let mut tables = if opts.detect_tables && lines.len() > 1 {
            self.table_bboxes(&mupdf_page)?
        } else {
            Vec::new()
        };
        // Borderless / lightly-ruled tables often miss MuPDF's vector hunt.
        // An alignment pass recovers grids of short, column-aligned cells
        // without inventing a table from ordinary prose (fail closed).
        if opts.detect_tables && lines.len() > 1 {
            for bbox in alignment_table_bboxes(&lines) {
                let overlaps = tables
                    .iter()
                    .any(|t| bbox.x0 < t.x1 && bbox.x1 > t.x0 && bbox.y0 < t.y1 && bbox.y1 > t.y0);
                if !overlaps {
                    tables.push(bbox);
                }
            }
        }
        let headings = if opts.detect_headings {
            heading_ranges(&lines, &styles)
        } else {
            Vec::new()
        };
        let captions = if opts.detect_captions {
            caption_ranges(&lines, &styles, &image_lines, &tables)
        } else {
            Vec::new()
        };
        let equations = if opts.detect_equations {
            equation_ranges(&lines, &styles)
        } else {
            Vec::new()
        };
        let code = if opts.detect_code {
            code_ranges(&lines, &styles)
        } else {
            Vec::new()
        };
        let footnotes = if opts.detect_footnotes {
            footnote_ranges(&lines, &styles, self.page_size(page)?.height)
        } else {
            Vec::new()
        };

        let objects = content_objects(
            &lines,
            &image_lines,
            &tables,
            &captions,
            &headings,
            &equations,
            &code,
            &footnotes,
            &furniture,
        );
        Ok(PageContent {
            lines,
            objects,
            furniture,
        })
    }

    /// Learn what this document repeats in its margins.
    ///
    /// Sampled as [`SAMPLE_ANCHORS`] anchors of *two consecutive pages* rather
    /// than evenly spaced single pages. Evenly spaced sampling lands on one
    /// parity — 100 pages sampled 8 times steps by 14, hitting only even pages
    /// — and a book that puts its title on versos and the chapter on rectos
    /// would have half its running heads never recur. Pairs cover both at the
    /// same cost.
    pub fn furniture_profile(&self) -> Result<FurnitureProfile, PdfError> {
        let count = self.page_count();
        if count < 2 {
            return Ok(FurnitureProfile::empty());
        }
        let mut pages: Vec<usize> = Vec::new();
        // Always include the opening and closing pairs: publisher mastheads and
        // journal chrome often concentrate on the first leaves and would miss
        // a mid-document-only sample grid.
        for p in [0usize, 1, count.saturating_sub(2), count.saturating_sub(1)] {
            if p < count && !pages.contains(&p) {
                pages.push(p);
            }
        }
        for k in 0..SAMPLE_ANCHORS {
            let anchor = k * (count - 1) / SAMPLE_ANCHORS.max(1);
            for p in [anchor, anchor + 1] {
                if p < count && !pages.contains(&p) {
                    pages.push(p);
                }
            }
        }
        let mut samples = Vec::new();
        for page in pages {
            // A page that will not extract is skipped, not fatal: a profile
            // built from fewer pages is still useful.
            let Ok(summary) = self.band_lines(page) else {
                continue;
            };
            samples.push(summary);
        }
        Ok(build_profile(&samples))
    }

    /// One page's margin-band lines, summarised for the repetition vote.
    ///
    /// A physical line contributes one [`BandLine`] per [`text_segments`]
    /// segment rather than one for its whole concatenated text: a running
    /// head and a folio sharing one baseline (common in facing-page layouts,
    /// where the pair swaps sides between recto and verso) must be votable
    /// independently of which side either one is on, or of which one a
    /// left-to-right character walk happens to read first.
    fn band_lines(&self, page: usize) -> Result<Vec<BandLine>, PdfError> {
        let height = self.page_size(page)?.height;
        let mupdf_page = self.inner.load_page(page as i32)?;
        // No PRESERVE_IMAGES: an image is never furniture, so it is not worth
        // extracting one here.
        let text_page = mupdf_page.to_text_page(TextPageFlags::empty())?;
        let mut out = Vec::new();
        for block in text_page.blocks() {
            if !matches!(block.r#type(), TextBlockType::Text) {
                continue;
            }
            for line in block.lines() {
                let mut origins = Vec::new();
                let mut chars = Vec::new();
                for ch in line.chars() {
                    let Some(c) = ch.char() else { continue };
                    if !c.is_whitespace() {
                        origins.push(ch.origin().y);
                    }
                    chars.push((ch.origin().x, c));
                }
                if origins.is_empty() {
                    continue;
                }
                origins.sort_by(f32::total_cmp);
                let baseline = origins[origins.len() / 2];
                for segment in text_segments(chars.iter().copied()) {
                    let text = normalise_furniture_text(&segment);
                    if text.is_empty() {
                        continue;
                    }
                    let folio = is_folio_text(&text);
                    let Some((edge, offset)) = band_of_with_folio(baseline, height, folio) else {
                        continue;
                    };
                    out.push(BandLine { text, edge, offset });
                }
            }
        }
        Ok(out)
    }

    /// Bounding boxes of the tables MuPDF detects on `page`.
    ///
    /// Only the boxes are taken from this pass, never its lines: `TABLE_HUNT`
    /// rewrites the page, moving a table's text into a structure node and
    /// splitting lines as it redistributes characters into cells. The Rust
    /// bindings expose no accessor for a structure node's children, so that
    /// text is unreachable here — which is exactly why the text comes from a
    /// separate pass and only the geometry comes from this one.
    ///
    /// `COLLECT_VECTORS` is not optional. MuPDF's table hunt looks for ruled
    /// regions among the page's vector rectangles; with no vectors collected
    /// that list is empty and it falls back to hunting the *whole page* at a
    /// loose threshold, which reports ordinary prose pages as one giant table.
    /// `SEGMENT` is deliberately not set: it would wrap the page in region
    /// structure nodes and hide the tables behind them.
    fn table_bboxes(&self, page: &mupdf::Page) -> Result<Vec<Rect>, PdfError> {
        let hunted = page.to_text_page(
            TextPageFlags::TABLE_HUNT
                | TextPageFlags::COLLECT_VECTORS
                | TextPageFlags::PRESERVE_IMAGES,
        )?;
        Ok(hunted
            .blocks()
            .filter(|b| matches!(b.r#type(), TextBlockType::Struct))
            .map(|b| rect_from_mupdf(b.bounds()))
            .filter(|r| r.x1 > r.x0 && r.y1 > r.y0)
            .collect())
    }

    /// The document outline (table of contents), possibly empty.
    pub fn outline(&self) -> Result<Vec<OutlineItem>, PdfError> {
        let outlines = self.inner.outlines()?;
        Ok(outlines.into_iter().map(convert_outline).collect())
    }
}

/// The angle of a line's baseline, in degrees, from its characters' direction
/// vectors (each already normalised).
///
/// A circular mean rather than a bucketed vote: bucketing splits a single
/// physical direction across the +/-180 wraparound, so text at +179 and -179
/// would average to 0 — pointing the opposite way. When the resultant is short
/// the characters genuinely disagree and the answer is `None`, which means the
/// line is never treated as marginal.
fn line_angle(dirs: &[(f32, f32)]) -> Option<f32> {
    if dirs.is_empty() {
        return None;
    }
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    for (x, y) in dirs {
        sx += x;
        sy += y;
    }
    if sx.hypot(sy) < 0.5 * dirs.len() as f32 {
        return None;
    }
    Some(sy.atan2(sx).to_degrees())
}

/// Smallest signed difference between two angles in degrees, across the
/// +/-180 wraparound.
fn angle_difference(a: f32, b: f32) -> f32 {
    let mut d = (a - b) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    d
}

/// The direction most of a page's characters run in.
///
/// Lines are clustered greedily by angle, weighted by how many characters they
/// carry. A tie is broken towards horizontal, which rescues a sparse page whose
/// rotated sidebar out-weighs its body text. `None` when no direction carries
/// [`ANGLE_DOMINANCE`] of the page — better to flag nothing than to guess.
fn dominant_angle(angles: &[(Option<f32>, usize)]) -> Option<f32> {
    let total: usize = angles
        .iter()
        .filter(|(a, _)| a.is_some())
        .map(|(_, w)| *w)
        .sum();
    if total == 0 {
        return None;
    }
    let mut ordered: Vec<(f32, usize)> = angles
        .iter()
        .filter_map(|(a, w)| a.map(|a| (a, *w)))
        .collect();
    ordered.sort_by_key(|(_, weight)| std::cmp::Reverse(*weight));

    let mut clusters: Vec<(f32, usize)> = Vec::new();
    for (angle, weight) in ordered {
        match clusters
            .iter_mut()
            .find(|(rep, _)| angle_difference(angle, *rep).abs() <= ANGLE_TOLERANCE_DEG)
        {
            Some((_, w)) => *w += weight,
            None => clusters.push((angle, weight)),
        }
    }
    let (best_angle, best_weight) = *clusters.iter().max_by_key(|(_, w)| *w)?;
    // A page whose text is split between two directions has no reading
    // direction to speak of, so nothing on it is marginal. This is also what
    // protects a sparse page whose rotated sidebar outweighs its prose: no
    // winner clears the bar, so neither is flagged.
    if (best_weight as f32) < ANGLE_DOMINANCE * total as f32 {
        return None;
    }
    Some(best_angle)
}

/// Reduce a line to the key the margin-repetition vote compares.
///
/// Digits collapse to `#`, so a page number matches itself across pages;
/// everything that is neither alphanumeric nor space is dropped, which is what
/// makes `- 12 -` and `12` the same key and stops the period in `2.1` mattering.
/// An empty result means the line carried no comparable text at all (a rule, a
/// row of dots) and must be excluded, since an empty key collides with every
/// other one.
fn normalise_furniture_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_hash = false;
    for c in text.chars() {
        let mapped = if c.is_ascii_digit() {
            '#'
        } else if c.is_alphanumeric() {
            c.to_lowercase().next().unwrap_or(c)
        } else if c.is_whitespace() {
            ' '
        } else {
            continue;
        };
        if mapped == '#' && last_hash {
            continue;
        }
        last_hash = mapped == '#';
        out.push(mapped);
    }
    let joined = out.split_whitespace().collect::<Vec<_>>().join(" ");
    strip_furniture_page_count_suffix(&joined)
}

/// Drop a trailing page-count tag (`12pp`, `20 pages`) so a journal running
/// head matches itself whether or not a given page also prints the length.
fn strip_furniture_page_count_suffix(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() < 2 {
        return text.to_string();
    }
    let last = tokens[tokens.len() - 1];
    let strip_last = last == "pp"
        || last == "page"
        || last == "pages"
        || (last.ends_with("pp")
            && last.chars().all(|c| c == '#' || c == 'p')
            && last.contains('#'));
    let strip_two = !strip_last
        && tokens.len() >= 2
        && matches!(tokens[tokens.len() - 1], "page" | "pages" | "pp")
        && tokens[tokens.len() - 2].chars().all(|c| c == '#');
    if strip_last {
        tokens[..tokens.len() - 1].join(" ")
    } else if strip_two {
        tokens[..tokens.len() - 2].join(" ")
    } else {
        text.to_string()
    }
}

/// Whether a profile entry and a line's normalised text are the same running
/// head, allowing a longer line to carry an extra trailing field the profile
/// never saw (or the reverse).
fn furniture_text_matches(entry: &str, line: &str) -> bool {
    if entry == line {
        return true;
    }
    let et: Vec<&str> = entry.split_whitespace().collect();
    let lt: Vec<&str> = line.split_whitespace().collect();
    if et.len() >= 3 && lt.len() > et.len() && lt[..et.len()] == et[..] {
        return true;
    }
    if lt.len() >= 3 && et.len() > lt.len() && et[..lt.len()] == lt[..] {
        return true;
    }
    false
}

/// Which band a baseline falls in, and how far it sits from that page edge.
///
/// Folio-shaped lines (normalised text `#`) may use a slightly deeper bottom
/// band — see [`FOLIO_BAND_SHARE`].
fn band_of_with_folio(baseline: f32, page_height: f32, folio_shaped: bool) -> Option<(Edge, f32)> {
    if page_height <= 0.0 {
        return None;
    }
    if baseline <= BAND_SHARE * page_height {
        return Some((Edge::Top, baseline));
    }
    let bottom_share = if folio_shaped {
        FOLIO_BAND_SHARE
    } else {
        BAND_SHARE
    };
    if baseline >= (1.0 - bottom_share) * page_height {
        Some((Edge::Bottom, page_height - baseline))
    } else {
        None
    }
}

/// Whether normalised furniture text is a bare page number.
fn is_folio_text(text: &str) -> bool {
    text == "#"
}

/// Learn which margin lines recur, from one summary per sampled page.
fn build_profile(samples: &[Vec<BandLine>]) -> FurnitureProfile {
    let pages_sampled = samples.len();
    let mut entries: Vec<FurnitureEntry> = Vec::new();
    for page in samples {
        // One page cannot vote twice for the same entry.
        let mut counted: Vec<usize> = Vec::new();
        for line in page {
            let found = entries.iter().position(|e| {
                furniture_text_matches(&e.text, &line.text)
                    && e.edge == line.edge
                    && (e.offset - line.offset).abs() <= BASELINE_TOLERANCE
            });
            match found {
                Some(i) => {
                    if !counted.contains(&i) {
                        entries[i].pages += 1;
                        counted.push(i);
                    }
                }
                None => {
                    entries.push(FurnitureEntry {
                        text: line.text.clone(),
                        edge: line.edge,
                        offset: line.offset,
                        pages: 1,
                    });
                    counted.push(entries.len() - 1);
                }
            }
        }
    }
    let min_pages = MIN_REPEAT_PAGES.max((MIN_REPEAT_SHARE * pages_sampled as f32).ceil() as usize);
    entries.retain(|e| e.pages >= min_pages);
    entries.sort_by_key(|e| std::cmp::Reverse(e.pages));
    for edge in [Edge::Top, Edge::Bottom] {
        let mut kept = 0;
        entries.retain(|e| {
            if e.edge != edge {
                return true;
            }
            kept += 1;
            kept <= MAX_ENTRIES_PER_EDGE
        });
    }
    FurnitureProfile {
        entries,
        pages_sampled,
    }
}

/// Which of a page's lines are furniture rather than reading material.
///
/// Three independent rules. **Rotation**: a line more than
/// [`ANGLE_TOLERANCE_DEG`] off the page's dominant direction. This one needs no
/// cap and provably cannot empty a page — the dominant cluster is by
/// construction the majority of the page's characters and is never flagged, so
/// a wholly sideways page keeps everything. **Margin line numbers**: a run of
/// purely numeric lines forming their own column left of the body text (see
/// [`line_number_mask`]) — also uncapped, and also provably safe, since it
/// never flags the very body-text line whose left edge it measures against.
/// **Repetition**: a margin-band line whose normalised text and baseline
/// recur across the document. That one is capped, because its evidence comes
/// from elsewhere.
fn furniture_mask(
    lines: &[ContentLine],
    styles: &[LineStyle],
    page_height: f32,
    profile: Option<&FurnitureProfile>,
) -> Vec<bool> {
    let mut mask = vec![false; lines.len()];
    if lines.is_empty() {
        return mask;
    }

    let weights: Vec<(Option<f32>, usize)> = lines
        .iter()
        .zip(styles)
        .map(|(l, s)| (s.angle, l.cells.len()))
        .collect();
    if let Some(dominant) = dominant_angle(&weights) {
        for (i, style) in styles.iter().enumerate() {
            if let Some(angle) = style.angle {
                if angle_difference(angle, dominant).abs() > ANGLE_TOLERANCE_DEG {
                    mask[i] = true;
                }
            }
        }
    }

    for (i, numbered) in line_number_mask(lines).into_iter().enumerate() {
        if numbered {
            mask[i] = true;
        }
    }

    let Some(profile) = profile.filter(|p| !p.is_empty()) else {
        return mask;
    };
    // Each match keeps the (edge, text) it matched against, not just its
    // line index: a bare folio digit ("#") is generic enough that a run of
    // ordinary numbered content elsewhere on the page -- a code listing's own
    // line numbers, say -- can coincidentally recur across several *sampled*
    // pages too, at a handful of similar-looking bottom-band offsets, and
    // enter the profile as several more "#" entries alongside the real
    // folio. That flood must not cost the page its genuine, independently
    // corroborated matches (the running head, the byline) sharing the same
    // edge -- see `too_many_for` below.
    let mut repeated: Vec<(usize, Edge, String)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if mask[i] {
            continue;
        }
        let chars = line.cells.iter().filter_map(|cell| match cell.kind {
            CellKind::Char(c) => Some((cell.bbox.x0, c)),
            CellKind::Image => None,
        });
        let matched = text_segments(chars).into_iter().find_map(|segment| {
            let text = normalise_furniture_text(&segment);
            if text.is_empty() {
                return None;
            }
            let folio = is_folio_text(&text);
            let (edge, offset) = band_of_with_folio(styles[i].baseline, page_height, folio)?;
            // Outside the margin band entirely: never furniture, no matter
            // how well the text matches -- the band gate itself never
            // widens (see `mask_does_not_use_the_deeper_band_for_non_folio_text`).
            let strict = profile.entries.iter().any(|e| {
                furniture_text_matches(&e.text, &text)
                    && e.edge == edge
                    && (e.offset - offset).abs() <= BASELINE_TOLERANCE
            });
            // Still in the margin band, but the strict offset match failed --
            // this page's own margin baseline may simply have drifted a few
            // points from its siblings. An exact text match against an
            // already-established profile entry is corroboration enough to
            // accept a looser offset window here; it never accepts on
            // position alone, and a line the band gate above already
            // rejected never reaches this fallback at all.
            let relaxed = strict
                || profile.entries.iter().any(|e| {
                    furniture_text_matches(&e.text, &text)
                        && e.edge == edge
                        && (e.offset - offset).abs() <= RELAXED_BASELINE_TOLERANCE
                });
            relaxed.then_some((edge, text))
        });
        if let Some((edge, text)) = matched {
            repeated.push((i, edge, text));
        }
    }

    // Beyond these bounds the evidence for *this text* is not credible on
    // this edge: leave those lines alone. Scoped per (edge, text) rather
    // than to the edge as a whole, so a generic, over-matching text like a
    // bare folio digit cannot take a differently-texted, individually
    // credible match down with it.
    let too_many_for = |edge: Edge, text: &str| {
        repeated
            .iter()
            .filter(|(_, e, t)| *e == edge && t == text)
            .count()
            > MAX_FURNITURE_PER_EDGE
    };
    // The whole-page guards stay aggregate: text plainly visible on the page
    // that the caret cannot reach at all is the worst failure this feature
    // could produce, and a page that is *mostly* repeated lines, whatever
    // their text, is exactly that failure regardless of how the matches are
    // spread across texts. The share cap only applies once a page has enough
    // lines for a share to mean anything -- a short page is legitimately a
    // third furniture.
    let share_applies = lines.len() >= MIN_LINES_FOR_SHARE_CAP;
    if repeated.len() == lines.len()
        || (share_applies && repeated.len() as f32 > MAX_FURNITURE_SHARE * lines.len() as f32)
    {
        return mask;
    }
    // The repetition rule may never take a page's last remaining inked line.
    // A margin is only a margin if there is something it is in the margin *of*
    // — bottom folios still mask when body lines sit above them.
    for (i, edge, text) in &repeated {
        if !too_many_for(*edge, text) {
            let others_remain = lines
                .iter()
                .enumerate()
                .any(|(j, l)| j != *i && !l.cells.is_empty() && !mask[j]);
            if !others_remain {
                continue;
            }
            mask[*i] = true;
        }
    }
    mask
}

/// The characters of a line, for text comparison.
fn line_text(line: &ContentLine) -> String {
    line.cells
        .iter()
        .filter_map(|c| match c.kind {
            CellKind::Char(ch) => Some(ch),
            CellKind::Image => None,
        })
        .collect()
}

/// Split one baseline's characters, given in reading order as `(x, char)`
/// pairs, into runs separated by a gap wider than [`SEGMENT_GAP_POINTS`].
///
/// Comparing origin-to-origin distance rather than true edge-to-edge gap is a
/// deliberate simplification: at the scale this threshold works at (tens of
/// points), a single character's own width is noise, and the function never
/// has more than an origin to work with for MuPDF's raw text-page characters
/// (see [`Document::band_lines`]) — [`furniture_mask`] uses it identically on
/// [`Cell`] origins so the two call sites can never disagree about where a
/// line splits.
fn text_segments<I: IntoIterator<Item = (f32, char)>>(chars: I) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut last_x: Option<f32> = None;
    for (x, c) in chars {
        if let Some(prev_x) = last_x {
            if x - prev_x > SEGMENT_GAP_POINTS {
                segments.push(std::mem::take(&mut current));
            }
        }
        current.push(c);
        last_x = Some(x);
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

/// Detect manuscript-style line numbering: a run of short, purely numeric
/// lines forming their own column at the page's left margin, clearly
/// separated from the body text that runs beside them. Common in
/// submission/review drafts, where every body line is numbered and the count
/// restarts each page.
///
/// Unlike the repetition rule this needs no cross-document profile — the
/// pattern (a narrow numeric column beside a wider text column) is visible on
/// a single page. The safety net is `body_left`: if every line on the page is
/// purely numeric there is no body column to measure the gap against, so
/// nothing is masked. That is also what makes the rule provably unable to
/// empty a page, the same guarantee the rotation rule has: the line that
/// defines `body_left` is by construction never one of the ones flagged.
fn line_number_mask(lines: &[ContentLine]) -> Vec<bool> {
    let mut mask = vec![false; lines.len()];
    let is_number: Vec<bool> = lines
        .iter()
        .map(|l| normalise_furniture_text(&line_text(l)) == "#")
        .collect();
    if is_number.iter().filter(|&&n| n).count() < MIN_LINE_NUMBERS {
        return mask;
    }
    let body_left = lines
        .iter()
        .zip(&is_number)
        .filter(|(_, &n)| !n)
        .map(|(l, _)| l.bbox.x0)
        .fold(f32::INFINITY, f32::min);
    if !body_left.is_finite() {
        return mask;
    }
    for (i, (line, &n)) in lines.iter().zip(&is_number).enumerate() {
        if n && line.bbox.x1 + LINE_NUMBER_GAP <= body_left {
            mask[i] = true;
        }
    }
    mask
}

/// How close two markers' left edges must be to belong to the same list.
const LIST_MARKER_ALIGN: f32 = 3.0;

/// A list needs at least this many items. One line that looks like a marker is
/// far more often ordinary prose — a sentence opening `1998. That year…`, or a
/// stray dash — so a lone candidate is never a list.
const MIN_LIST_ITEMS: usize = 2;

/// How far past its marker a line must start to be part of that item. A flush
/// list, whose continuations align with the marker, therefore yields items of
/// one line each — deliberate, since under-reaching only shortens a sentence
/// while over-reaching swallows the prose after the list.
const LIST_INDENT_EPS: f32 = 1.0;

/// A gap larger than this many median line heights ends the item: an indented
/// block that far below merely follows the list.
///
/// Same numeric factor paragraph splitting uses in `syodep-core` (`0.75`).
/// List items used to run looser (`1.5×` the previous line's own height),
/// which let a modest inter-paragraph gap — common after a hanging-indent
/// list, where the indent guard cannot fire — slip into the last item.
/// Matching the paragraph threshold means a gap that starts a new paragraph
/// also ends a list item, with no extra signal required.
const LIST_GAP_FACTOR: f32 = 0.75;

/// Multiplier applied to the largest continuation gap actually observed
/// elsewhere in the same list, when calibrating the *last* item's own gap
/// guard (see [`list_items`]). Looser than 1.0 so the last item's own
/// leading, which can run a touch larger than another item's by ordinary
/// typesetting jitter, is not itself mistaken for a paragraph break. Applied
/// *alongside* [`LIST_GAP_FACTOR`]: whichever fires first ends the item.
const LIST_GAP_CALIBRATION_SLACK: f32 = 1.5;

/// No item may claim more than this many lines beyond its marker. Marker
/// corroboration is page-wide, so a stray pair of marker-shaped lines can
/// exist; this bounds what one is able to swallow.
const LIST_ITEM_MAX_LINES: usize = 8;

/// What a line's opening mark makes it, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    /// A bullet glyph, which may sit on a line of its own.
    Bullet,
    /// An enumerator such as `1.`, `2)`, `a.` or `iv)`.
    Enumerated,
}

/// The list marker a line opens with, if any.
///
/// A marker must be followed by a space or be the whole line: PDF extraction
/// frequently puts a bullet on its own line, separated from the item's text by
/// the indent, so both shapes have to count.
fn line_marker(text: &str) -> Option<(Marker, usize)> {
    let indent = text.chars().take_while(|c| c.is_whitespace()).count();
    let trimmed = text.trim_start();
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    let rest = chars.as_str();
    let followed_by_space = |s: &str| s.is_empty() || s.starts_with(char::is_whitespace);

    if matches!(
        first,
        '\u{2022}'
            | '\u{00b7}'
            | '\u{2023}'
            | '\u{25e6}'
            | '\u{25aa}'
            | '\u{25ab}'
            | '\u{2219}'
            | '\u{25cf}'
            | '\u{2043}'
            | '*'
            | '-'
            | '\u{2013}'
            | '\u{2014}'
    ) && followed_by_space(rest)
    {
        return Some((Marker::Bullet, indent + 1));
    }

    // An enumerator: a short run of digits, one letter, or a roman numeral,
    // closed by `.` or `)`. Kept short so `3.14 is the value` cannot qualify —
    // there the character after the stop is a digit, not a space.
    let label: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    if label.is_empty() || label.len() > 3 {
        return None;
    }
    let after_label = &trimmed[label.len()..];
    let mut closers = after_label.chars();
    if !matches!(closers.next(), Some('.') | Some(')')) {
        return None;
    }
    if !followed_by_space(closers.as_str()) {
        return None;
    }
    let all_digits = label.chars().all(|c| c.is_ascii_digit());
    let roman = label.chars().all(|c| {
        matches!(
            c.to_ascii_lowercase(),
            'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm'
        )
    });
    // Lowercase single letters only (`a.`, `b.`). An uppercase `T. Author`
    // is an initial on a citation line, and a page of those would otherwise
    // corroborate each other into a false enumerated list beside a real
    // bullet list. Uppercase `A.`/`B.` legal-style lists are uncommon next
    // to the damage that false positive does; digits and roman still cover
    // ordinary enumerated prose.
    let single_letter = label.len() == 1 && label.chars().all(|c| c.is_ascii_lowercase());
    // The label plus its closing `.` or `)`, after any indent.
    (all_digits || roman || single_letter)
        .then_some((Marker::Enumerated, indent + label.chars().count() + 1))
}

/// The lines that begin a list item.
///
/// Corroboration rather than shape alone: a marker counts only when at least
/// one other line of the same kind starts at the same left edge, which is what
/// separates a real list from a sentence that happens to open with a numeral.
/// Lines inside a heading are excluded — a numbered section heading such as
/// `2.1. Directory layout` is indistinguishable from an enumerated item by
/// shape, and is already known to be a heading.
fn list_items(lines: &[ContentLine], blocked: &[ContentObject]) -> Vec<(usize, usize)> {
    let blocked_at = |i: usize| blocked.iter().any(|o| i >= o.start_line && i <= o.end_line);
    let candidates: Vec<(usize, Marker, f32)> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !blocked_at(*i))
        .filter_map(|(i, line)| line_marker(&line_text(line)).map(|(m, _)| (i, m, line.bbox.x0)))
        .collect();

    let starts: Vec<(usize, f32)> = candidates
        .iter()
        .filter(|&&(i, marker, x0)| {
            let peers = candidates
                .iter()
                .filter(|&&(j, m, x)| j != i && m == marker && (x - x0).abs() <= LIST_MARKER_ALIGN)
                .count();
            peers + 1 >= MIN_LIST_ITEMS
        })
        .map(|&(i, _, x0)| (i, x0))
        .collect();

    // Median height of non-empty lines — same basis paragraph splitting uses
    // for its gap threshold, so a gap that opens a new paragraph also ends an
    // item here.
    let mut heights: Vec<f32> = lines
        .iter()
        .filter(|l| !l.cells.is_empty())
        .map(|l| (l.bbox.y1 - l.bbox.y0).max(1.0))
        .collect();
    heights.sort_by(f32::total_cmp);
    let median_height = heights.get(heights.len() / 2).copied().unwrap_or(1.0);

    // An item runs from its marker through the lines indented past that
    // marker: its own text where extraction split the bullet off, and any
    // wrapped continuation. The prose after a list returns to the marker's own
    // margin, which is precisely where the last item has to stop.
    let is_marker = |i: usize| starts.iter().any(|&(s, _)| s == i);
    let mut items: Vec<(usize, usize)> = starts
        .iter()
        .map(|&(marker_idx, marker_x)| {
            let forward_end = extend_item(
                lines,
                &is_marker,
                &blocked_at,
                marker_idx,
                marker_x,
                median_height,
                None,
            );
            // MuPDF sometimes emits a bullet *after* its text in reading
            // order (and even above it in y). Pair a lone bullet with the
            // nearest indented neighbour so the item covers marker + text.
            let partner = pair_lone_bullet(
                lines,
                &is_marker,
                &blocked_at,
                marker_idx,
                marker_x,
                median_height,
                &starts,
            );
            let start = partner.filter(|&p| p < marker_idx).unwrap_or(marker_idx);
            let end = forward_end.max(partner.unwrap_or(marker_idx));
            (start, end)
        })
        .collect();

    // Every item but the last stops at a hard boundary: the next marker. The
    // last has none, so it falls back to the gap and indent guards above —
    // and on a list whose marker sits left of the body column, ordinary
    // prose (including a new paragraph's own first line) can sit to the
    // right of the marker exactly like a genuine continuation would,
    // leaving the gap guard as the only real defence.
    //
    // `LIST_GAP_FACTOR` already matches the paragraph threshold, which is
    // what stops the common case. Other items in the same list can still
    // tighten that further: a non-last item is always ultimately bounded by
    // the next marker regardless of the gap guard, so an imprecise guard can
    // only ever under-extend it, never swallow a real paragraph the way it
    // can for the last item. Recompute only the last item, with its gap
    // guard tightened to a multiple of the largest continuation gap actually
    // observed among the others — the max, not a median, biased toward not
    // over-tightening a genuine continuation. A list with nothing to
    // calibrate against (no other item wraps) is left exactly as computed
    // above: zero behaviour change when there is no evidence to act on.
    if let Some(last_idx) = items.len().checked_sub(1) {
        let max_observed_gap = items[..last_idx]
            .iter()
            .filter(|&&(s, e)| e > s)
            .flat_map(|&(s, e)| (s..e).map(move |k| lines[k + 1].bbox.y0 - lines[k].bbox.y1))
            .fold(None::<f32>, |acc, gap| {
                Some(acc.map_or(gap, |a| a.max(gap)))
            });
        if let Some(max_observed_gap) = max_observed_gap {
            let (marker_idx, marker_x) = starts[last_idx];
            let cap = LIST_GAP_CALIBRATION_SLACK * max_observed_gap;
            let forward_end = extend_item(
                lines,
                &is_marker,
                &blocked_at,
                marker_idx,
                marker_x,
                median_height,
                Some(cap),
            );
            let partner = pair_lone_bullet(
                lines,
                &is_marker,
                &blocked_at,
                marker_idx,
                marker_x,
                median_height,
                &starts,
            );
            let start = partner.filter(|&p| p < marker_idx).unwrap_or(marker_idx);
            let end = forward_end.max(partner.unwrap_or(marker_idx));
            items[last_idx] = (start, end);
        }
    }
    items
}

/// Find text that belongs to a bullet sitting alone on its line.
///
/// Extraction often splits `-` / `•` onto its own line; sometimes that line
/// appears *after* the item text in the line vector (and occasionally above
/// it on the page). Forward-only extension then leaves a bare marker. The
/// partner must be indented past the marker, within a small vertical window,
/// and not claimed by another marker.
fn pair_lone_bullet(
    lines: &[ContentLine],
    is_marker: &impl Fn(usize) -> bool,
    blocked_at: &impl Fn(usize) -> bool,
    marker_idx: usize,
    marker_x: f32,
    median_height: f32,
    starts: &[(usize, f32)],
) -> Option<usize> {
    let marker_text = line_text(&lines[marker_idx]);
    let Some((Marker::Bullet, _)) = line_marker(&marker_text) else {
        return None;
    };
    // Marker + body on the same line already form a complete item.
    if marker_text.chars().filter(|c| !c.is_whitespace()).count() > 2 {
        return None;
    }
    let claimed: Vec<usize> = starts.iter().map(|&(i, _)| i).collect();
    let my_y = lines[marker_idx].bbox.y0;
    let max_dy = 2.5 * median_height.max(1.0);
    let mut best: Option<(usize, f32)> = None;
    for (j, line) in lines.iter().enumerate() {
        if j == marker_idx || is_marker(j) || blocked_at(j) || line.cells.is_empty() {
            continue;
        }
        if claimed.contains(&j) {
            continue;
        }
        if line.bbox.x0 <= marker_x + LIST_INDENT_EPS {
            continue;
        }
        let dy = (line.bbox.y0 - my_y).abs();
        if dy > max_dy {
            continue;
        }
        // Prefer text below the bullet (larger y in MuPDF); accept above too.
        let score = dy + if line.bbox.y0 >= my_y { 0.0 } else { 1.0 };
        if best.map(|(_, s)| score < s).unwrap_or(true) {
            best = Some((j, score));
        }
    }
    best.map(|(j, _)| j)
}

/// One list item's line extent, from its marker through however many
/// following lines are indented past it — see [`list_items`].
///
/// `gap_cap`, when given, is an additional break threshold checked alongside
/// [`LIST_GAP_FACTOR`]: whichever guard fires first ends the item. `None`
/// reproduces the plain, uncalibrated extent every item starts from.
fn extend_item(
    lines: &[ContentLine],
    is_marker: &impl Fn(usize) -> bool,
    blocked_at: &impl Fn(usize) -> bool,
    start: usize,
    marker_x: f32,
    median_height: f32,
    gap_cap: Option<f32>,
) -> usize {
    let mut end = start;
    for j in start + 1..lines.len() {
        // Corroboration is page-wide, so a stray pair of marker-shaped lines
        // can exist. Cap how much one is allowed to claim.
        if j - start > LIST_ITEM_MAX_LINES {
            break;
        }
        if is_marker(j) || blocked_at(j) || lines[j].cells.is_empty() {
            break;
        }
        let previous = lines[end].bbox;
        let current = lines[j].bbox;
        let height = (previous.y1 - previous.y0).max(1.0);
        // Moving back up the page by more than a line is a new column or
        // region, and the top of the next column is trivially "indented
        // past" a marker in the left one — without this an item swallows
        // it. The tolerance matters: a bullet's own box starts a point or
        // two below its text's, because the glyph is small and the text has
        // ascenders, so an exact test would cut every item off at its
        // marker.
        if previous.y0 - current.y0 > height {
            break;
        }
        // A wide gap means the block below merely follows the list rather
        // than belonging to its last item. Uses the page median height at
        // the same factor paragraph splitting does, so the two agree on
        // where a new block of prose begins.
        if current.y0 - previous.y1 > LIST_GAP_FACTOR * median_height {
            break;
        }
        if let Some(cap) = gap_cap {
            if current.y0 - previous.y1 > cap {
                break;
            }
        }
        if current.x0 <= marker_x + LIST_INDENT_EPS {
            break;
        }
        end = j;
    }
    end
}

/// The type size most of a page's characters are set in, weighted by
/// character count over the given `inked` (non-blank, sized) lines.
///
/// Body text dominates by character count on essentially every page,
/// including title pages, which makes this far steadier than an average or a
/// median. Shared by [`heading_ranges`] (a heading reads noticeably *larger*
/// than this) and [`footnote_ranges`] (a footnote reads noticeably
/// *smaller*) so the two detectors' notion of "the body" cannot drift apart.
fn dominant_body_size(lines: &[ContentLine], styles: &[LineStyle], inked: &[usize]) -> f32 {
    let mut weights: Vec<(i32, usize)> = Vec::new();
    for &i in inked {
        let bucket = (styles[i].size * 10.0).round() as i32;
        let weight = lines[i].cells.len();
        match weights.iter_mut().find(|(b, _)| *b == bucket) {
            Some((_, w)) => *w += weight,
            None => weights.push((bucket, weight)),
        }
    }
    weights
        .iter()
        .max_by_key(|(bucket, w)| (*w, *bucket))
        .map_or(0.0, |(bucket, _)| *bucket as f32 / 10.0)
}

/// How much larger than the body text a line must be set to read as a heading.
///
/// 1.15 rather than something tighter because of a real failure: on a page
/// dominated by 9pt code listings, ordinary 10pt prose is 1.11x the computed
/// body size and would otherwise be flagged wholesale.
const HEADING_SIZE_FACTOR: f32 = 1.15;

/// A heading may not be more than this many lines. A heading that wraps three
/// times is almost certainly a misdetection.
const HEADING_MAX_LINES: usize = 4;

/// If more than this share of a page's lines look like headings, none of them
/// do. Set loosely because a title page legitimately is mostly large type.
const HEADING_MAX_SHARE: f32 = 0.5;

/// Find the headings on a page, as inclusive `(start_line, end_line)` ranges.
///
/// Pure so that every threshold below is testable without MuPDF, which matters
/// because these are heuristics whose failure mode is silent.
///
/// A line is a heading when it is set noticeably larger than the page's body
/// text, or when it is entirely bold at roughly body size *and* does not run
/// the full width of its column, **or** when it opens with a multi-level
/// section number such as `1.1.` / `2.12.` followed by a title. That last
/// clause is shape rather than typography: subsection headings are often set
/// at body size, so size/weight alone miss them, and missing them glues the
/// title into the paragraph below while splitting `1.1.` as its own sentence.
///
/// Typography-flagged lines still face the share and length caps; numbered
/// headings are high-precision and are added afterwards, so a page of false
/// bold flags cannot erase a real `1.1. Methods`. A numbered opener that
/// fills its measure may still wrap onto a short continuation that is not
/// itself numbered and is invisible to the size/weight vote — those
/// continuations are claimed with the opener, so a two-line title stays one
/// heading.
fn heading_ranges(lines: &[ContentLine], styles: &[LineStyle]) -> Vec<(usize, usize)> {
    let inked: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].cells.is_empty() && styles.get(i).is_some_and(|s| s.size > 0.0))
        .collect();
    if inked.is_empty() {
        return Vec::new();
    }

    let body = dominant_body_size(lines, styles, &inked);
    let widest = inked
        .iter()
        .map(|&i| lines[i].bbox.x1 - lines[i].bbox.x0)
        .fold(0.0_f32, f32::max);

    let is_typography_heading = |i: usize| {
        if body <= 0.0 {
            return false;
        }
        // A lone oversized glyph is a drop cap, not a heading: treating it as
        // one severs the chapter's first letter from its own sentence.
        let alphanum = line_text(&lines[i])
            .chars()
            .filter(|c| c.is_alphanumeric())
            .count();
        if alphanum <= 1 {
            return false;
        }
        let style = styles[i];
        let width = lines[i].bbox.x1 - lines[i].bbox.x0;
        style.size >= body * HEADING_SIZE_FACTOR
            || (style.bold && style.size >= body * 0.95 && width < 0.9 * widest)
    };

    // Merge adjacent heading lines of the same size and weight, so a title
    // that wraps stays one heading.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut flagged = 0usize;
    for &i in &inked {
        if !is_typography_heading(i) {
            continue;
        }
        flagged += 1;
        let joins_previous = ranges.last().is_some_and(|&(_, end)| {
            end + 1 == i
                && (styles[end].size - styles[i].size).abs() < 0.05
                && styles[end].bold == styles[i].bold
        });
        match ranges.last_mut() {
            Some(last) if joins_previous => last.1 = i,
            _ => ranges.push((i, i)),
        }
    }

    if flagged as f32 > HEADING_MAX_SHARE * inked.len() as f32 {
        ranges.clear();
    } else {
        ranges.retain(|&(start, end)| {
            let lines_covered = end - start + 1;
            lines_covered <= HEADING_MAX_LINES
        });
    }

    // Numbered subsection headings, independent of the typography vote.
    // Claim short wrap continuations too: a line that fills the column is
    // body prose, not title leftover — that is what keeps
    // `2.3.1. Interface overview` from swallowing the paragraph under it
    // while still joining `… in the` / `evaluated file`.
    let claimed = |ranges: &[(usize, usize)], i: usize| {
        ranges.iter().any(|&(start, end)| i >= start && i <= end)
    };
    for &i in &inked {
        if !is_numbered_heading_text(&line_text(&lines[i])) || claimed(&ranges, i) {
            continue;
        }
        let mut end = i;
        while end + 1 < lines.len()
            && end - i + 1 < HEADING_MAX_LINES
            && !lines[end + 1].cells.is_empty()
            && !claimed(&ranges, end + 1)
            && styles
                .get(end + 1)
                .is_some_and(|s| (s.size - styles[i].size).abs() < 0.05 && s.bold == styles[i].bold)
            && !is_numbered_heading_text(&line_text(&lines[end + 1]))
            && !is_typography_heading(end + 1)
            && (lines[end + 1].bbox.x1 - lines[end + 1].bbox.x0) < 0.9 * widest
        {
            end += 1;
        }
        ranges.push((i, end));
    }
    ranges.sort_by_key(|&(start, _)| start);
    ranges
}

/// Whether `text` opens with a multi-level section number and a title:
/// `1.1. Methods`, `2.12. Recommended checking order`, `1.2.3 Overview`.
///
/// At least one internal dot in the number is required, so a plain enumerated
/// item (`1. First point`) stays a list candidate rather than a heading. The
/// optional trailing dot after the last component matches both `1.1 Title`
/// and `1.1. Title`.
fn is_numbered_heading_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    let mut chars = trimmed.chars().peekable();
    let mut saw_internal_dot = false;
    // First component: one or more digits.
    if !chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
        chars.next();
    }
    // Further `.digits` components — at least one.
    loop {
        if chars.peek() != Some(&'.') {
            break;
        }
        let mut look = chars.clone();
        look.next(); // '.'
        if !look.peek().is_some_and(|c| c.is_ascii_digit()) {
            break;
        }
        chars.next();
        saw_internal_dot = true;
        while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
            chars.next();
        }
    }
    if !saw_internal_dot {
        return false;
    }
    // Require the trailing section-number dot before the title: `1.1. Title`,
    // not a decimal that opens a sentence (`3.14 is the value`).
    if chars.peek() != Some(&'.') {
        return false;
    }
    chars.next();
    let mut saw_space = false;
    while chars.peek().is_some_and(|c| c.is_whitespace()) {
        saw_space = true;
        chars.next();
    }
    saw_space && chars.next().is_some_and(|c| c.is_alphanumeric())
}

/// How much smaller than the body text a line must be set to read as a
/// footnote — the mirror of [`HEADING_SIZE_FACTOR`].
const FOOTNOTE_SIZE_FACTOR: f32 = 0.92;

/// How much of the bottom of the page counts as footnote territory. Deeper
/// than a folio's margin band: a footnote block is often several lines tall
/// and can start well above the strict margin a running head or page number
/// sits in.
const FOOTNOTE_BAND_SHARE: f32 = 0.30;

/// If more than this share of a page's lines look like footnotes, none of
/// them do — the same escape hatch [`HEADING_MAX_SHARE`] and
/// `EQUATION_MAX_SHARE` give their own detectors.
const FOOTNOTE_MAX_SHARE: f32 = 0.5;

/// Find footnote blocks at the foot of a page, as inclusive
/// `(start_line, end_line)` ranges.
///
/// Pure, like [`heading_ranges`]/`equation_ranges`, so the thresholds below
/// are testable without MuPDF.
///
/// A line reads as a footnote when it sits in the page's bottom margin band
/// *and* is set noticeably smaller than the page's body size — the mirror of
/// `heading_ranges`' larger-than-body rule. Contiguous flagged lines merge
/// into one block, the way an aligned equation system does.
fn footnote_ranges(
    lines: &[ContentLine],
    styles: &[LineStyle],
    page_height: f32,
) -> Vec<(usize, usize)> {
    if page_height <= 0.0 {
        return Vec::new();
    }
    let inked: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].cells.is_empty() && styles.get(i).is_some_and(|s| s.size > 0.0))
        .collect();
    if inked.is_empty() {
        return Vec::new();
    }
    let body = dominant_body_size(lines, styles, &inked);
    if body <= 0.0 {
        return Vec::new();
    }
    let bottom_threshold = (1.0 - FOOTNOTE_BAND_SHARE) * page_height;

    let is_footnote_line = |i: usize| {
        styles[i].baseline >= bottom_threshold
            && styles[i].size <= body * FOOTNOTE_SIZE_FACTOR
            // Display-math fragments often sit in the foot at a smaller size;
            // claiming them as footnotes makes `s`/`p` skip real equations and
            // splits a formula across kinds. Math-shaped lines stay out.
            && !line_is_mathish(&lines[i], &styles[i])
    };

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut flagged = 0usize;
    for &i in &inked {
        if !is_footnote_line(i) {
            continue;
        }
        flagged += 1;
        let joins_previous = ranges.last().is_some_and(|&(_, end)| end + 1 == i);
        match ranges.last_mut() {
            Some(last) if joins_previous => last.1 = i,
            _ => ranges.push((i, i)),
        }
    }

    if flagged == 0 || flagged as f32 > FOOTNOTE_MAX_SHARE * inked.len() as f32 {
        return Vec::new();
    }
    ranges
}

/// Fonts whose names say "this is mathematics". Matched as lower-case
/// substrings, which covers TeX's families (`CMMI10`, `CMSY7`, `CMEX10`,
/// `MSBM10`), the Unicode math fonts (`XITSMath-Regular`, `CambriaMath`,
/// `LMMath-Italic10`, `Asana-Math`), base-14 `Symbol`, and the subset prefixes
/// PDFs carry (`ABCDEF+CMMI10`) all in one rule.
const MATH_FONT_MARKS: &[&str] = &[
    "cmmi", "cmsy", "cmex", "cmmib", "msam", "msbm", "rsfs", "eufm", "stix", "xits", "symbol",
    "math",
];

/// Whether a font name reads as a math font.
fn is_math_font(name: &str) -> bool {
    let lower = name.to_lowercase();
    MATH_FONT_MARKS.iter().any(|mark| lower.contains(mark))
}

/// Fonts whose names say "this is monospace". Matched as lower-case
/// substrings so subset prefixes (`ABCDEF+Courier`) and family variants
/// (`Courier-Bold`, `DejaVuSansMono`) all fire.
const MONO_FONT_MARKS: &[&str] = &[
    "courier",
    "mono",
    "consolas",
    "menlo",
    "monaco",
    "inconsolata",
    "firacode",
    "sourcecode",
    "anonymouspro",
    "liberationmono",
    "nimbusmono",
    "notomono",
];

/// Whether a font name reads as a monospace / code font.
fn is_mono_font(name: &str) -> bool {
    let lower = name.to_lowercase();
    MONO_FONT_MARKS.iter().any(|mark| lower.contains(mark))
}

/// Whether a line reads as mathematics for the purpose of keeping it out of
/// the footnote detector.
///
/// Softer than a full [`equation_ranges`] hit: a fragment like `Ic,t = Ic,0`
/// may fail the set-apart / word-count gates of display-equation detection
/// while still being plainly not a footnote. Font share, rich math symbols,
/// or an ASCII operator with almost no prose words is enough.
fn line_is_mathish(line: &ContentLine, style: &LineStyle) -> bool {
    let text: String = line
        .cells
        .iter()
        .filter_map(|cell| match cell.kind {
            CellKind::Char(c) => Some(c),
            CellKind::Image => None,
        })
        .collect();
    if text.chars().filter(|c| !c.is_whitespace()).count() == 0 {
        return false;
    }
    if is_equation_number(&text) {
        return true;
    }
    if style.math >= EQUATION_MATH_FONT_SHARE {
        return true;
    }
    let has_operator = text.chars().any(is_math_operator);
    if !has_operator {
        return false;
    }
    let has_rich = text
        .chars()
        .any(|c| is_math_symbol(c) && !matches!(c, '=' | '+' | '<' | '>'));
    if has_rich {
        return true;
    }
    // ASCII operator with almost no prose words: `Ic,t = Ic,0`, `n = 2`.
    let mut words = 0usize;
    let mut run = 0usize;
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_alphabetic() && !is_math_symbol(c) {
            run += 1;
        } else {
            if run >= 3 {
                words += 1;
            }
            run = 0;
        }
    }
    words <= EQUATION_MAX_WORDS
}

/// Whether `c` is a mathematical operator or relation — the mark that makes a
/// line a *statement* rather than a label.
fn is_math_operator(c: char) -> bool {
    matches!(
        c,
        '=' | '+'
            | '<'
            | '>'
            | '±'
            | '∓'
            | '×'
            | '÷'
            | '−'
            | '≠'
            | '≈'
            | '≃'
            | '≅'
            | '≡'
            | '≤'
            | '≥'
            | '≪'
            | '≫'
            | '∝'
            | '∑'
            | '∏'
            | '∫'
            | '∮'
            | '√'
            | '∂'
            | '∇'
            | '∈'
            | '∉'
            | '⊂'
            | '⊆'
            | '⊃'
            | '⊇'
            | '∪'
            | '∩'
            | '∀'
            | '∃'
            | '∧'
            | '∨'
            | '¬'
            | '→'
            | '←'
            | '↔'
            | '↦'
            | '⇒'
            | '⇔'
            | '∞'
    )
}

/// Whether `c` is a character that belongs to mathematics rather than to prose:
/// an operator, or a Greek letter.
///
/// Greek counts but does not stand alone — see [`is_math_operator`] — so a
/// centred Greek word is not mistaken for an equation.
fn is_math_symbol(c: char) -> bool {
    is_math_operator(c)
        || matches!(c, '\u{0370}'..='\u{03ff}' | '\u{1d400}'..='\u{1d7ff}' | '·' | '⋅' | '∼')
}

/// Share of a line's inked characters that must be math symbols for the line to
/// read as mathematics on its characters alone. Low, because an equation is
/// mostly variables and digits with a sprinkling of operators between them.
const EQUATION_SYMBOL_SHARE: f32 = 0.15;

/// Share of a line's inked characters that must be set in a math font for the
/// line to read as mathematics on its fonts alone.
const EQUATION_MATH_FONT_SHARE: f32 = 0.4;

/// How many ordinary words a display equation may carry. Display math routinely
/// includes `where` or `for all`; a sentence carries many more.
const EQUATION_MAX_WORDS: usize = 2;

/// A line must be narrower than this share of the widest line on the page to
/// count as set apart from the prose. This is what keeps a sentence containing
/// inline math out: a prose line fills its measure, a display equation does not.
const EQUATION_MAX_WIDTH_SHARE: f32 = 0.9;

/// An equation may not run to more than this many lines. An aligned system is
/// several lines; a dozen is a misdetection.
const EQUATION_MAX_LINES: usize = 12;

/// If more than this share of a page's lines read as equations, none of them do.
/// Loose, because an appendix page legitimately is mostly display math — and the
/// cost of the guard firing is only that the page navigates line by line.
const EQUATION_MAX_SHARE: f32 = 0.6;

/// Whether `text` is nothing but an equation number: `(12)`, `(3.4)`, `(A.1)`.
///
/// Journals set these against the right margin, where MuPDF sometimes reports
/// them as a line of their own; absorbing one keeps it from becoming a stop
/// between an equation and the prose after it.
fn is_equation_number(text: &str) -> bool {
    let body = text.trim();
    let Some(inner) = body.strip_prefix('(').and_then(|b| b.strip_suffix(')')) else {
        return false;
    };
    !inner.is_empty()
        && inner
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
        && inner.chars().any(|c| c.is_numeric())
}

/// Find the display equations on a page, as inclusive `(start_line, end_line)`
/// ranges.
///
/// Pure, like [`heading_ranges`], so every threshold is testable without MuPDF.
///
/// A line is a display equation when it is **set apart** from the prose (it does
/// not fill the column), reads as mathematics — either by its fonts or by its
/// characters — carries an operator or relation, and carries almost no ordinary
/// words. All four together, because each alone has a common counter-example:
/// prose sentences contain inline math, a centred label is set apart, and a
/// citation line is full of punctuation.
///
/// Inline math is deliberately out of reach here: a formula inside a sentence
/// would have to become a region to be found, and a region splits the sentence
/// around it.
fn equation_ranges(lines: &[ContentLine], styles: &[LineStyle]) -> Vec<(usize, usize)> {
    let inked: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].cells.is_empty() && styles.get(i).is_some_and(|s| s.size > 0.0))
        .collect();
    if inked.is_empty() {
        return Vec::new();
    }
    let widest = inked
        .iter()
        .map(|&i| lines[i].bbox.x1 - lines[i].bbox.x0)
        .fold(0.0_f32, f32::max);

    let text_of = |i: usize| -> String {
        lines[i]
            .cells
            .iter()
            .filter_map(|cell| match cell.kind {
                CellKind::Char(c) => Some(c),
                CellKind::Image => None,
            })
            .collect()
    };
    let is_equation = |i: usize| {
        let width = lines[i].bbox.x1 - lines[i].bbox.x0;
        if width >= EQUATION_MAX_WIDTH_SHARE * widest {
            return false;
        }
        let text = text_of(i);
        let counted = text.chars().filter(|c| !c.is_whitespace()).count();
        if counted == 0 {
            return false;
        }
        // A lone signed number (`−1`, `+2`) is not a display equation — it is
        // usually a table cell, an axis tick or a code fragment.
        if text
            .chars()
            .filter(|c| !c.is_whitespace())
            .all(|c| c.is_numeric() || matches!(c, '+' | '-' | '\u{2212}' | '.' | ','))
        {
            return false;
        }
        let symbols = text.chars().filter(|&c| is_math_symbol(c)).count();
        let math_by_font = styles[i].math >= EQUATION_MATH_FONT_SHARE;
        // ASCII `+`/`=`/`<>` alone are not enough: they fire on `C++`,
        // `count += 1`, `-> None` and other code/prose that is not display
        // mathematics. Character-based detection needs a richer math mark
        // (Greek, a unicode operator, …); TeX and Unicode math fonts still
        // qualify through `math_by_font`.
        let has_rich_math = text
            .chars()
            .any(|c| is_math_symbol(c) && !matches!(c, '=' | '+' | '<' | '>'));
        let math_by_chars =
            has_rich_math && symbols as f32 >= EQUATION_SYMBOL_SHARE * counted as f32;
        if !(math_by_font || math_by_chars) {
            return false;
        }
        if !text.chars().any(is_math_operator) {
            return false;
        }
        // Words, as runs of three or more letters. Two letters would count `if`
        // and every pair of adjacent variables.
        let mut words = 0usize;
        let mut run = 0usize;
        for c in text.chars().chain(std::iter::once(' ')) {
            if c.is_alphabetic() && !is_math_symbol(c) {
                run += 1;
            } else {
                if run >= 3 {
                    words += 1;
                }
                run = 0;
            }
        }
        words <= EQUATION_MAX_WORDS
    };

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut flagged = 0usize;
    for &i in &inked {
        if !is_equation(i) {
            continue;
        }
        flagged += 1;
        match ranges.last_mut() {
            // An aligned system is several lines of one equation.
            Some(last) if last.1 + 1 == i => last.1 = i,
            _ => ranges.push((i, i)),
        }
    }
    if flagged == 0 {
        return Vec::new();
    }
    // A number set on its own line belongs to the equation it sits against.
    for range in &mut ranges {
        if range.0 > 0 && is_equation_number(&text_of(range.0 - 1)) {
            range.0 -= 1;
        }
        if range.1 + 1 < lines.len() && is_equation_number(&text_of(range.1 + 1)) {
            range.1 += 1;
        }
    }

    if flagged as f32 > EQUATION_MAX_SHARE * inked.len() as f32 {
        return Vec::new();
    }
    ranges.retain(|&(start, end)| {
        let lines_covered = end - start + 1;
        lines_covered <= EQUATION_MAX_LINES
    });
    ranges
}

/// How close (in points) a caption line's bbox may sit to an image or table.
const CAPTION_PROXIMITY_PT: f32 = 24.0;

/// A set-apart caption must be narrower than this share of the widest body line.
const CAPTION_MAX_WIDTH_SHARE: f32 = 0.85;

/// Share of a line's inked glyphs that must be monospace for a code line.
const CODE_MONO_SHARE: f32 = 0.6;

/// Vertical gap between two bboxes, or 0 when they overlap in y.
fn vertical_gap(a: Rect, b: Rect) -> f32 {
    if a.y1 < b.y0 {
        b.y0 - a.y1
    } else if b.y1 < a.y0 {
        a.y0 - b.y1
    } else {
        0.0
    }
}

/// Whether `text` opens like a figure/table caption: `Fig. 1`, `Figure 2:`,
/// `Table 3.`, `Tab. 4`.
fn has_caption_prefix(text: &str) -> bool {
    let trimmed = text.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    for prefix in ["fig.", "figure", "table", "tab."] {
        let Some(rest) = lower.strip_prefix(prefix) else {
            continue;
        };
        let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_whitespace());
        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// Find figure/table captions as inclusive `(start_line, end_line)` ranges.
///
/// A line is a caption when it sits within [`CAPTION_PROXIMITY_PT`] of an image
/// or table bbox (above or below) **and** either opens with a caption prefix
/// (`Fig.` / `Figure` / `Table` / `Tab.` + number) or is set apart from the
/// body (narrower and smaller). Pure so the thresholds stay unit-testable.
fn caption_ranges(
    lines: &[ContentLine],
    styles: &[LineStyle],
    image_line_indices: &[usize],
    table_bboxes: &[Rect],
) -> Vec<(usize, usize)> {
    let inked: Vec<usize> = (0..lines.len())
        .filter(|&i| {
            !lines[i].cells.is_empty()
                && styles.get(i).is_some_and(|s| s.size > 0.0)
                && !lines[i]
                    .cells
                    .iter()
                    .any(|c| matches!(c.kind, CellKind::Image))
        })
        .collect();
    if inked.is_empty() {
        return Vec::new();
    }

    let body = dominant_body_size(lines, styles, &inked);
    let widest = inked
        .iter()
        .map(|&i| lines[i].bbox.x1 - lines[i].bbox.x0)
        .fold(0.0_f32, f32::max);

    let mut anchors: Vec<Rect> = table_bboxes.to_vec();
    for &i in image_line_indices {
        if i < lines.len() {
            anchors.push(lines[i].bbox);
        }
    }
    if anchors.is_empty() {
        return Vec::new();
    }

    let near_anchor = |bbox: Rect| {
        anchors
            .iter()
            .any(|&a| vertical_gap(bbox, a) <= CAPTION_PROXIMITY_PT)
    };
    let set_apart = |i: usize| {
        if body <= 0.0 || widest <= 0.0 {
            return false;
        }
        let width = lines[i].bbox.x1 - lines[i].bbox.x0;
        styles[i].size < body * 0.95 && width < CAPTION_MAX_WIDTH_SHARE * widest
    };

    let mut ranges = Vec::new();
    for &i in &inked {
        let text = line_text(&lines[i]);
        if !near_anchor(lines[i].bbox) {
            continue;
        }
        if !(has_caption_prefix(&text) || set_apart(i)) {
            continue;
        }
        ranges.push((i, i));
    }
    ranges
}

/// Find monospace code blocks as inclusive `(start_line, end_line)` ranges.
///
/// Consecutive lines with a high monospace-font share (≥ [`CODE_MONO_SHARE`])
/// that do not already read as mathematics. Pure, like the other detectors.
fn code_ranges(lines: &[ContentLine], styles: &[LineStyle]) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(style) = styles.get(i) else {
            continue;
        };
        if style.size <= 0.0 || style.mono < CODE_MONO_SHARE {
            continue;
        }
        if line_is_mathish(line, style) {
            continue;
        }
        if line.cells.iter().any(|c| matches!(c.kind, CellKind::Image)) {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if last.1 + 1 == i => last.1 = i,
            _ => ranges.push((i, i)),
        }
    }
    ranges
}

/// Minimum rows / columns for the alignment-based (borderless) table detector.
const ALIGN_TABLE_MIN_ROWS: usize = 3;
const ALIGN_TABLE_MIN_COLS: usize = 2;
/// A cell-like line is narrower than this share of the page's content width.
const ALIGN_TABLE_MAX_WIDTH_SHARE: f32 = 0.48;
/// How close two cell left-edges must be to share a column.
const ALIGN_TABLE_COL_ALIGN: f32 = 4.0;
/// How close two baselines must be to share a row.
const ALIGN_TABLE_ROW_ALIGN: f32 = 3.0;

/// How much of a line's own height must fall inside a table box for the line to
/// be one of that table's rows. A row's ascenders and descenders can poke a
/// point or two past the outer rule; a line the box merely cuts through
/// half-way is the prose beside the table.
const TABLE_MEMBER_OVERLAP: f32 = 0.7;

/// How much wider than the table's own median row gap a gap must be for the
/// line beyond it to be set apart from the grid — a caption, or the prose after
/// it. Measured against the table's own rhythm rather than against line height,
/// so a table with generously padded rows keeps all of them.
const TABLE_GAP_FACTOR: f32 = 1.8;

/// Bounding boxes of borderless tables found by cell alignment.
///
/// MuPDF's hunt needs ruled vectors; many journal parameter tables have none.
/// A grid of short lines whose left edges form ≥2 stable columns across ≥3
/// rows is recovered here. Fail closed: one column, sparse rows, or a span
/// that claims the whole page yields nothing.
fn alignment_table_bboxes(lines: &[ContentLine]) -> Vec<Rect> {
    let content_width = {
        let x0 = lines
            .iter()
            .filter(|l| !l.cells.is_empty())
            .map(|l| l.bbox.x0)
            .fold(f32::INFINITY, f32::min);
        let x1 = lines
            .iter()
            .filter(|l| !l.cells.is_empty())
            .map(|l| l.bbox.x1)
            .fold(f32::NEG_INFINITY, f32::max);
        if !x0.is_finite() || x1 <= x0 {
            return Vec::new();
        }
        x1 - x0
    };
    let cell_idxs: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            !l.cells.is_empty()
                && !l.cells.iter().any(|c| matches!(c.kind, CellKind::Image))
                && (l.bbox.x1 - l.bbox.x0) <= ALIGN_TABLE_MAX_WIDTH_SHARE * content_width
        })
        .map(|(i, _)| i)
        .collect();
    if cell_idxs.len() < ALIGN_TABLE_MIN_ROWS * ALIGN_TABLE_MIN_COLS {
        return Vec::new();
    }

    // Cluster into rows by y0.
    let mut order = cell_idxs;
    order.sort_by(|&a, &b| lines[a].bbox.y0.total_cmp(&lines[b].bbox.y0));
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for i in order {
        let y = lines[i].bbox.y0;
        match rows.last_mut() {
            Some(row)
                if (y - lines[*row.last().unwrap()].bbox.y0).abs() <= ALIGN_TABLE_ROW_ALIGN =>
            {
                row.push(i);
            }
            _ => rows.push(vec![i]),
        }
    }
    rows.retain(|r| r.len() >= ALIGN_TABLE_MIN_COLS);
    if rows.len() < ALIGN_TABLE_MIN_ROWS {
        return Vec::new();
    }

    // Walk contiguous row runs; each run needs ≥2 x-columns that recur.
    let mut out = Vec::new();
    let mut run_start = 0usize;
    while run_start < rows.len() {
        let mut run_end = run_start + 1;
        while run_end < rows.len() {
            let prev_y = lines[*rows[run_end - 1].last().unwrap()].bbox.y1;
            let next_y = lines[rows[run_end][0]].bbox.y0;
            // A large vertical gap ends the grid.
            if next_y - prev_y > 3.0 * ALIGN_TABLE_ROW_ALIGN + 12.0 {
                break;
            }
            run_end += 1;
        }
        if run_end - run_start >= ALIGN_TABLE_MIN_ROWS {
            let run = &rows[run_start..run_end];
            if let Some(bbox) = alignment_run_bbox(lines, run, content_width) {
                out.push(bbox);
            }
        }
        run_start = run_end;
    }
    out
}

fn alignment_run_bbox(
    lines: &[ContentLine],
    rows: &[Vec<usize>],
    content_width: f32,
) -> Option<Rect> {
    // Prefer left-edge columns (left-aligned label/value grids). When headers
    // are left-aligned but numeric cells are right- or centre-aligned, x0
    // clusters fall apart — fall back to centre clustering, which still lines
    // those columns up. Two-column prose stays rejected by the cell-width gate
    // above and the page-width fail-closed check below.
    let min_hits = rows.len().div_ceil(2).max(2);
    let x0s: Vec<f32> = rows
        .iter()
        .flat_map(|r| r.iter().map(|&i| lines[i].bbox.x0))
        .collect();
    let centres: Vec<f32> = rows
        .iter()
        .flat_map(|r| {
            r.iter().map(|&i| {
                let b = lines[i].bbox;
                (b.x0 + b.x1) / 2.0
            })
        })
        .collect();
    let cols_x0 = alignment_stable_columns(&x0s, min_hits);
    let (cols, use_centre) = if cols_x0.len() >= ALIGN_TABLE_MIN_COLS {
        (cols_x0, false)
    } else {
        let cols_centre = alignment_stable_columns(&centres, min_hits);
        if cols_centre.len() < ALIGN_TABLE_MIN_COLS {
            return None;
        }
        (cols_centre, true)
    };
    // Every row should land in ≥2 of those columns.
    let rows_ok = rows
        .iter()
        .filter(|r| {
            let hits = cols
                .iter()
                .filter(|&&cx| {
                    r.iter().any(|&i| {
                        let b = lines[i].bbox;
                        let key = if use_centre {
                            (b.x0 + b.x1) / 2.0
                        } else {
                            b.x0
                        };
                        (key - cx).abs() <= ALIGN_TABLE_COL_ALIGN
                    })
                })
                .count();
            hits >= ALIGN_TABLE_MIN_COLS
        })
        .count();
    if rows_ok < ALIGN_TABLE_MIN_ROWS {
        return None;
    }
    let members: Vec<usize> = rows.iter().flatten().copied().collect();
    let bbox = members
        .iter()
        .map(|&i| lines[i].bbox)
        .reduce(|a, b| a.union(b))?;
    // Fail closed: a page-wide two-column band is almost certainly prose
    // columns. Result tables with three or more stable columns may span the
    // full measure (GLUE-style benchmark grids).
    if bbox.x1 - bbox.x0 >= 0.9 * content_width && cols.len() < 3 {
        return None;
    }
    Some(bbox)
}

/// Cluster sorted-ish scalar positions into stable column representatives that
/// appear at least `min_hits` times within [`ALIGN_TABLE_COL_ALIGN`].
fn alignment_stable_columns(values: &[f32], min_hits: usize) -> Vec<f32> {
    let mut values = values.to_vec();
    values.sort_by(f32::total_cmp);
    let mut clusters: Vec<(f32, usize)> = Vec::new();
    for x in values {
        match clusters
            .last_mut()
            .filter(|(rep, _)| (x - *rep).abs() <= ALIGN_TABLE_COL_ALIGN)
        {
            Some((rep, n)) => {
                *rep = (*rep * *n as f32 + x) / (*n as f32 + 1.0);
                *n += 1;
            }
            None => clusters.push((x, 1)),
        }
    }
    clusters
        .into_iter()
        .filter(|&(_, n)| n >= min_hits)
        .map(|(x, _)| x)
        .collect()
}

/// Drop the lines at the edges of `members` that belong to the prose around the
/// table rather than to the table itself.
///
/// MuPDF's box is the *ruled region* it found, which reaches past the last row
/// of text; when it reaches past the centre of the next line, centre
/// containment hands that line to the table. Both tests below are applied at
/// the edges only: a box cannot clip an interior line, and an interior gap is a
/// real part of the grid.
///
/// Trimming can only shrink a table, and one shrunk below two lines falls to
/// the caller's existing guard — degrading to line-by-line navigation, which is
/// always the safe direction.
fn trim_table_edges(lines: &[ContentLine], table: Rect, mut members: Vec<usize>) -> Vec<usize> {
    let inside_fraction = |i: usize| {
        let b = lines[i].bbox;
        let height = b.y1 - b.y0;
        if height <= 0.0 {
            return 1.0;
        }
        (b.y1.min(table.y1) - b.y0.max(table.y0)).max(0.0) / height
    };
    while members
        .first()
        .is_some_and(|&i| inside_fraction(i) < TABLE_MEMBER_OVERLAP)
    {
        members.remove(0);
    }
    while members
        .last()
        .is_some_and(|&i| inside_fraction(i) < TABLE_MEMBER_OVERLAP)
    {
        members.pop();
    }
    // A line the box covers entirely can still be a caption: what marks it as
    // separate is the gap above it, wider than the table's own rhythm. Needs
    // three members to have a rhythm to compare against.
    while members.len() >= 3 {
        let gaps: Vec<f32> = members
            .windows(2)
            .map(|w| (lines[w[1]].bbox.y0 - lines[w[0]].bbox.y1).max(0.0))
            .collect();
        let mut sorted = gaps.clone();
        sorted.sort_by(f32::total_cmp);
        let median = sorted[sorted.len() / 2];
        if median <= 0.0 {
            break;
        }
        let threshold = TABLE_GAP_FACTOR * median;
        if gaps[gaps.len() - 1] > threshold {
            members.pop();
        } else if gaps[0] > threshold {
            members.remove(0);
        } else {
            break;
        }
    }
    members
}

/// The box to paint for a table: MuPDF's rectangle, extended to the rows it
/// contains and then held back so it never reaches a line outside the table.
///
/// The rectangle is worth keeping — painting a table row by row leaves its
/// rules and empty cells unpainted, which reads as a broken highlight — but on
/// its own it is the one object geometry not derived from the page's own lines,
/// and an overhang tints the prose next to the table. Clamping is vertical
/// because that is where the neighbours are; a box too wide is already covered
/// by the caller's "claims the page" guard.
fn table_bbox(lines: &[ContentLine], table: Rect, first: usize, last: usize) -> Rect {
    let text = (first..=last)
        .map(|i| lines[i].bbox)
        .reduce(|a, b| a.union(b))
        .unwrap_or(table);
    let above = lines[..first]
        .iter()
        .rev()
        .find(|l| !l.cells.is_empty())
        .map(|l| l.bbox.y1);
    let below = lines
        .iter()
        .skip(last + 1)
        .find(|l| !l.cells.is_empty())
        .map(|l| l.bbox.y0);
    // Cover the rectangle and the rows' text, then let the neighbours cut it
    // back. The neighbour wins on purpose: line boxes include ascenders and
    // descenders and so can overlap each other by a point, and stopping a
    // point short of a descender is invisible where tinting the line below is
    // exactly the reported bug.
    let mut y0 = table.y0.min(text.y0);
    let mut y1 = table.y1.max(text.y1);
    if let Some(above) = above {
        y0 = y0.max(above);
    }
    if let Some(below) = below {
        y1 = y1.min(below);
    }
    // A page whose lines overlap their neighbours outright can invert the pair,
    // and no box satisfies both edges there; fall back to the rows' own extent.
    if y0 >= y1 {
        y0 = text.y0;
        y1 = text.y1;
    }
    Rect {
        x0: table.x0.min(text.x0),
        y0,
        x1: table.x1.max(text.x1),
        y1,
    }
}

/// Turn detected table boxes and image lines into the page's atomic objects.
///
/// Pure so that every heuristic below is unit-testable without MuPDF — which
/// matters, because MuPDF's table detection is a heuristic itself and these
/// guards are what keep its mistakes from reaching the caret.
///
/// Claim order (coarsest first): tables → images → captions → headings →
/// equations → code → footnotes → list items. Later kinds yield to earlier ones
/// on overlap.
#[allow(clippy::too_many_arguments)] // claim-chain inputs stay explicit and ordered
fn content_objects(
    lines: &[ContentLine],
    image_lines: &[usize],
    tables: &[Rect],
    captions: &[(usize, usize)],
    headings: &[(usize, usize)],
    equations: &[(usize, usize)],
    code: &[(usize, usize)],
    footnotes: &[(usize, usize)],
    furniture: &[ContentLine],
) -> Vec<ContentObject> {
    // The "claims the whole page" guards below must be judged against the
    // whole page, furniture included. Otherwise removing a running head and a
    // folio makes a full-page table claim every remaining line, the guard
    // fires, and table detection quietly dies on exactly the pages that have
    // tables. Membership and indices stay in filtered space.
    let whole_page = || lines.iter().chain(furniture);
    let non_empty = whole_page().filter(|l| !l.cells.is_empty()).count();
    let content_area = whole_page()
        .map(|l| l.bbox)
        .reduce(|a, b| a.union(b))
        .map_or(0.0, |r| r.area());

    let mut objects: Vec<ContentObject> = Vec::new();
    for table in tables {
        let members: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| table.contains_center_of(l.bbox))
            .map(|(i, _)| i)
            .collect();
        let members = trim_table_edges(lines, *table, members);
        let (Some(&first), Some(&last)) = (members.first(), members.last()) else {
            continue;
        };
        // A one-line table is no better than a line, and a table nobody can
        // see through is almost always MuPDF's whole-page fallback firing on
        // ordinary prose — the one false positive observed in practice.
        let contiguous = last - first + 1 == members.len();
        let claims_everything = members.len() >= non_empty || table.area() >= 0.9 * content_area;
        let has_content = members.iter().any(|&i| !lines[i].cells.is_empty());
        if members.len() < 2 || !contiguous || claims_everything || !has_content {
            continue;
        }
        objects.push(ContentObject {
            kind: ObjectKind::Table,
            bbox: table_bbox(lines, *table, first, last),
            start_line: first,
            end_line: last,
        });
    }

    // Two boxes for the same table, or a nested one: keep the wider span.
    objects.sort_by_key(|o| (o.start_line, std::cmp::Reverse(o.end_line)));
    let mut kept: Vec<ContentObject> = Vec::new();
    for object in objects {
        match kept.last() {
            Some(prev) if object.start_line <= prev.end_line => continue,
            _ => kept.push(object),
        }
    }

    // An image inside a table is part of that table, not a unit of its own.
    for &line in image_lines {
        if kept
            .iter()
            .any(|o| line >= o.start_line && line <= o.end_line)
        {
            continue;
        }
        kept.push(ContentObject {
            kind: ObjectKind::Image,
            bbox: lines[line].bbox,
            start_line: line,
            end_line: line,
        });
    }

    // Captions sit against an image or table; they yield to those, and headings
    // yield to captions so a short "Figure 1." is not mistaken for a heading.
    for &(start, end) in captions {
        if end >= lines.len()
            || kept
                .iter()
                .any(|o| start <= o.end_line && end >= o.start_line)
        {
            continue;
        }
        let bbox = lines[start..=end]
            .iter()
            .map(|l| l.bbox)
            .reduce(|a, b| a.union(b))
            .unwrap_or(lines[start].bbox);
        kept.push(ContentObject {
            kind: ObjectKind::Caption,
            bbox,
            start_line: start,
            end_line: end,
        });
    }

    // A bold, short line inside a table is a column header, not a heading, so
    // headings yield to any object already claimed.
    for &(start, end) in headings {
        if end >= lines.len()
            || kept
                .iter()
                .any(|o| start <= o.end_line && end >= o.start_line)
        {
            continue;
        }
        let bbox = lines[start..=end]
            .iter()
            .map(|l| l.bbox)
            .reduce(|a, b| a.union(b))
            .unwrap_or(lines[start].bbox);
        kept.push(ContentObject {
            kind: ObjectKind::Heading,
            bbox,
            start_line: start,
            end_line: end,
        });
    }

    // Equations yield in turn: a formula inside a table cell is a table row, and
    // a line both detectors like stays whichever came first — harmless, because
    // a heading and an equation navigate identically.
    for &(start, end) in equations {
        if end >= lines.len()
            || kept
                .iter()
                .any(|o| start <= o.end_line && end >= o.start_line)
        {
            continue;
        }
        let bbox = lines[start..=end]
            .iter()
            .map(|l| l.bbox)
            .reduce(|a, b| a.union(b))
            .unwrap_or(lines[start].bbox);
        kept.push(ContentObject {
            kind: ObjectKind::Equation,
            bbox,
            start_line: start,
            end_line: end,
        });
    }

    // Code blocks yield to equations (a formula set in a mono font is still
    // math) and claim their lines before footnotes, which look for small type
    // at the foot of the page.
    for &(start, end) in code {
        if end >= lines.len()
            || kept
                .iter()
                .any(|o| start <= o.end_line && end >= o.start_line)
        {
            continue;
        }
        let bbox = lines[start..=end]
            .iter()
            .map(|l| l.bbox)
            .reduce(|a, b| a.union(b))
            .unwrap_or(lines[start].bbox);
        kept.push(ContentObject {
            kind: ObjectKind::Code,
            bbox,
            start_line: start,
            end_line: end,
        });
    }

    // Footnotes are found before list items, deliberately: a footnote's own
    // citation-style text (`12. Author, Title`) is often marker-shaped, and
    // must not be misread as, or corrupt the extent of, an ordinary list —
    // claiming its lines here keeps `list_items` from ever seeing them.
    for &(start, end) in footnotes {
        if end >= lines.len()
            || kept
                .iter()
                .any(|o| start <= o.end_line && end >= o.start_line)
        {
            continue;
        }
        let bbox = lines[start..=end]
            .iter()
            .map(|l| l.bbox)
            .reduce(|a, b| a.union(b))
            .unwrap_or(lines[start].bbox);
        kept.push(ContentObject {
            kind: ObjectKind::Footnote,
            bbox,
            start_line: start,
            end_line: end,
        });
    }

    // List items are found last, against everything already claimed rather
    // than against raw heading ranges: a bulleted line inside a table is a
    // table row, and an item's extent stops at the figure below it rather than
    // being thrown away for touching one. Because the scan breaks on a blocked
    // line, the ranges are disjoint from `kept` by construction.
    for (start, end) in list_items(lines, &kept) {
        if end >= lines.len() {
            continue;
        }
        let bbox = lines[start..=end]
            .iter()
            .map(|l| l.bbox)
            .reduce(|a, b| a.union(b))
            .unwrap_or(lines[start].bbox);
        kept.push(ContentObject {
            kind: ObjectKind::ListItem,
            bbox,
            start_line: start,
            end_line: end,
        });
    }

    kept.sort_by_key(|o| o.start_line);
    debug_assert!(
        object_ranges_ok(lines, &kept),
        "objects must stay sorted, disjoint, in-bounds, and non-empty: {kept:?}"
    );
    kept
}

fn rect_from_mupdf(r: mupdf::Rect) -> Rect {
    Rect {
        x0: r.x0,
        y0: r.y0,
        x1: r.x1,
        y1: r.y1,
    }
}

/// Bounding box of a glyph quad (the four corners may be rotated/skewed, so
/// take the min/max over all of them).
fn rect_from_quad(q: &mupdf::Quad) -> Rect {
    let xs = [q.ul.x, q.ur.x, q.ll.x, q.lr.x];
    let ys = [q.ul.y, q.ur.y, q.ll.y, q.lr.y];
    Rect {
        x0: xs.iter().copied().fold(f32::INFINITY, f32::min),
        y0: ys.iter().copied().fold(f32::INFINITY, f32::min),
        x1: xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        y1: ys.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    }
}

fn convert_outline(item: mupdf::Outline) -> OutlineItem {
    OutlineItem {
        title: item.title,
        page: item.dest.as_ref().map(|d| d.loc.page_number as usize),
        children: item.down.into_iter().map(convert_outline).collect(),
    }
}

/// A highlight to embed in a PDF: the rectangles it covers on one page, in page
/// points with the origin at the top left (the same space [`Cell::bbox`] uses),
/// its colour, and its opacity.
///
/// One value per page: PDF highlight annotations belong to a page, so a
/// selection spanning several pages becomes several of these.
///
/// `opacity` matters more here than it would look: a PDF reader always paints
/// a highlight with Multiply blending, so leaving `/CA` at its default (fully
/// opaque) makes a saved highlight noticeably more saturated than the same
/// colour previewed at less than full opacity before saving. Writing the same
/// opacity the app previewed with is what keeps the two in agreement.
#[derive(Debug, Clone, PartialEq)]
pub struct HighlightAnnotation {
    pub page: usize,
    pub rects: Vec<Rect>,
    /// Colour as 8-bit RGB.
    pub color: (u8, u8, u8),
    /// Constant alpha (`/CA`), 0.0 (invisible) to 1.0 (opaque).
    pub opacity: f32,
    /// Optional annotation name, written as `/NM`.
    ///
    /// This is the only durable handle on an annotation once it is in the
    /// file: page indices shift and geometry repeats, so removing exactly one
    /// highlight later ([`remove_highlight_annotation`]) means matching this
    /// string. A multi-page highlight gives every one of its annotations the
    /// same name deliberately — they are one highlight, and they are deleted
    /// together. `None` writes no `/NM`, leaving the annotation anonymous and
    /// therefore not individually removable.
    pub name: Option<String>,
}

/// Copy the PDF at `src` to `out`, adding `highlights` as PDF `Highlight`
/// annotations. `src` is never modified.
///
/// Deliberately a free function that opens its own handle rather than a method
/// on [`Document`]: `PdfDocument::try_from` consumes the document by value, and
/// the caller's live document is busy rendering. Writing through a second,
/// short-lived handle keeps the two from interfering, and means a failed write
/// cannot leave the open document in a half-annotated state.
pub fn write_highlights(
    src: &Path,
    out: &Path,
    highlights: &[HighlightAnnotation],
) -> Result<(), PdfError> {
    let src_str = src.to_string_lossy();
    let doc = mupdf::Document::open(src_str.as_ref()).map_err(|e| PdfError::Open {
        path: src.display().to_string(),
        message: e.to_string(),
    })?;
    let pdf = mupdf::pdf::PdfDocument::try_from(doc).map_err(|e| PdfError::Open {
        path: src.display().to_string(),
        message: format!("not a PDF that can be annotated: {e}"),
    })?;
    let page_count = pdf.page_count()? as usize;

    // Group by page so a page is loaded and its appearance streams regenerated
    // once however many highlights land on it.
    let mut by_page: BTreeMap<usize, Vec<&HighlightAnnotation>> = BTreeMap::new();
    for highlight in highlights {
        if highlight.rects.is_empty() {
            continue;
        }
        if highlight.page >= page_count {
            return Err(PdfError::PageOutOfRange {
                page: highlight.page,
                count: page_count,
            });
        }
        by_page.entry(highlight.page).or_default().push(highlight);
    }

    for (page_number, page_highlights) in by_page {
        let page = pdf.load_page(page_number as i32)?;
        let mut page = mupdf::pdf::PdfPage::try_from(page)?;
        // MuPDF stores /QuadPoints in the page's *default user space* (origin
        // bottom left), while our rectangles are in the transformed space text
        // extraction reports (origin top left). The page CTM is exactly that
        // transform, so its inverse is the conversion — and using it rather
        // than an ad-hoc `height - y` is what keeps rotated pages correct.
        let inverse = page.ctm()?.invert().ok_or_else(|| {
            PdfError::Backend(format!("page {page_number} has no invertible CTM"))
        })?;
        for highlight in page_highlights {
            add_highlight_annotation(&mut page, &inverse, highlight)?;
        }
        // Generates the appearance streams for the annotations just created, so
        // every viewer — including our own renderer, which runs annotations —
        // shows them. Must come after the /QuadPoints are in place.
        page.update()?;
    }

    let out_str = out.to_string_lossy();
    pdf.save_with_options(out_str.as_ref(), mupdf::pdf::PdfWriteOptions::default())
        .map_err(|e| PdfError::Backend(format!("cannot write {}: {e}", out.display())))?;
    Ok(())
}

/// Create one `Highlight` annotation on `page` covering `highlight.rects`.
///
/// `mupdf` 0.7 exposes no quad-point setter, and `PdfAnnotation::set_rect`
/// *raises* for a highlight (MuPDF computes a quad-point annotation's `/Rect`
/// from its `/QuadPoints`, so the rect is not settable by design). So the
/// geometry is written straight into the annotation's dictionary, reached
/// through the page's `/Annots` array at the index recorded before the
/// annotation was created.
fn add_highlight_annotation(
    page: &mut mupdf::pdf::PdfPage,
    inverse: &Matrix,
    highlight: &HighlightAnnotation,
) -> Result<(), PdfError> {
    let index = annots_len(page)?;
    let (r, g, b) = highlight.color;
    let mut annot = page.create_annotation(mupdf::pdf::PdfAnnotationType::Highlight)?;
    annot.set_color(mupdf::color::AnnotationColor::Rgb {
        red: f32::from(r) / 255.0,
        green: f32::from(g) / 255.0,
        blue: f32::from(b) / 255.0,
    })?;
    // Dropped before the dictionary is edited: the annotation borrows the page,
    // and the edit needs the page's own object.
    drop(annot);

    let mut dict = annot_dict(page, index)?;
    let doc = dict
        .document()
        .ok_or_else(|| PdfError::Backend("annotation has no owning document".to_owned()))?;

    let mut quads = doc.new_array()?;
    let mut bounds: Option<Rect> = None;
    for rect in &highlight.rects {
        // Order is the PDF one: upper-left, upper-right, lower-left,
        // lower-right, each as an x/y pair.
        let (ulx, uly) = inverse.transform_xy(rect.x0, rect.y0);
        let (urx, ury) = inverse.transform_xy(rect.x1, rect.y0);
        let (llx, lly) = inverse.transform_xy(rect.x0, rect.y1);
        let (lrx, lry) = inverse.transform_xy(rect.x1, rect.y1);
        for value in [ulx, uly, urx, ury, llx, lly, lrx, lry] {
            quads.array_push(mupdf::pdf::PdfObject::new_real(value)?)?;
        }
        let quad = Rect {
            x0: ulx.min(urx).min(llx).min(lrx),
            y0: uly.min(ury).min(lly).min(lry),
            x1: ulx.max(urx).max(llx).max(lrx),
            y1: uly.max(ury).max(lly).max(lry),
        };
        bounds = Some(match bounds {
            Some(b) => b.union(quad),
            None => quad,
        });
    }
    dict.dict_put("QuadPoints", quads)?;

    // `/NM` is a PDF text string, and the only stable way to find this exact
    // annotation again after the file has been closed and reopened.
    if let Some(name) = &highlight.name {
        dict.dict_put("NM", mupdf::pdf::PdfObject::new_string(name)?)?;
    }

    // `mupdf` 0.7 exposes no opacity setter either (`pdf_set_annot_opacity`
    // exists only in C), and the default `/CA` — fully opaque — is exactly
    // what would make a saved highlight look stronger than the same colour
    // previewed at less than full opacity, since every reader paints a
    // highlight with Multiply blending regardless of what wrote it.
    dict.dict_put("CA", doc.new_real(highlight.opacity.clamp(0.0, 1.0))?)?;

    // MuPDF derives a highlight's /Rect from its quads when it synthesises the
    // appearance, but a reader that does not synthesise still needs a /Rect that
    // contains the quads, or it clips the highlight away.
    if let Some(bounds) = bounds {
        let mut rect = doc.new_array()?;
        for value in [bounds.x0, bounds.y0, bounds.x1, bounds.y1] {
            rect.array_push(mupdf::pdf::PdfObject::new_real(value)?)?;
        }
        dict.dict_put("Rect", rect)?;
    }
    Ok(())
}

/// How many entries the page's `/Annots` array has, treating a missing or
/// malformed array as empty.
fn annots_len(page: &mupdf::pdf::PdfPage) -> Result<usize, PdfError> {
    let Some(annots) = resolved_annots(page)? else {
        return Ok(0);
    };
    Ok(annots.len().unwrap_or(0))
}

/// The page's `/Annots` array with any indirect reference resolved.
fn resolved_annots(page: &mupdf::pdf::PdfPage) -> Result<Option<mupdf::pdf::PdfObject>, PdfError> {
    let Some(annots) = page.object().get_dict("Annots")? else {
        return Ok(None);
    };
    let annots = match annots.resolve()? {
        Some(resolved) => resolved,
        None => annots,
    };
    if !annots.is_array()? {
        return Ok(None);
    }
    Ok(Some(annots))
}

/// The dictionary of the annotation at `index` in the page's `/Annots`.
fn annot_dict(page: &mupdf::pdf::PdfPage, index: usize) -> Result<mupdf::pdf::PdfObject, PdfError> {
    let annots = resolved_annots(page)?
        .ok_or_else(|| PdfError::Backend("page has no /Annots array".to_owned()))?;
    let entry = annots
        .get_array(index as i32)?
        .ok_or_else(|| PdfError::Backend(format!("no annotation at /Annots[{index}]")))?;
    let dict = match entry.resolve()? {
        Some(resolved) => resolved,
        None => entry,
    };
    if !dict.is_dict()? {
        return Err(PdfError::Backend(format!(
            "/Annots[{index}] is not a dictionary"
        )));
    }
    Ok(dict)
}

/// The `/NM` of the annotation at `index`, when it has one that is a text
/// string.
fn annot_name(page: &mupdf::pdf::PdfPage, index: usize) -> Result<Option<String>, PdfError> {
    let dict = annot_dict(page, index)?;
    let Some(name) = dict.get_dict("NM")? else {
        return Ok(None);
    };
    Ok(name.as_string().ok().map(|s| s.to_owned()))
}

/// Copy the PDF at `src` to `out` without the annotations named
/// `annotation_name` (their `/NM`), returning how many were removed. `src` is
/// never modified.
///
/// Every page is searched, and every match is removed: one syodep highlight
/// that runs across a page break is several annotations sharing one name, and
/// they are one thing to the user. Matching is on `/NM` alone — an exact
/// string the caller wrote — rather than on geometry or subtype, because
/// "which annotation is this row?" must never be a guess: the wrong guess
/// deletes an annotation somebody else made.
///
/// When nothing matches, `out` is **not** written and `Ok(0)` is returned; the
/// caller decides whether that is an error, and there is no point copying a
/// file that would be identical to its source.
pub fn remove_highlight_annotation(
    src: &Path,
    out: &Path,
    annotation_name: &str,
) -> Result<usize, PdfError> {
    let src_str = src.to_string_lossy();
    let doc = mupdf::Document::open(src_str.as_ref()).map_err(|e| PdfError::Open {
        path: src.display().to_string(),
        message: e.to_string(),
    })?;
    let pdf = mupdf::pdf::PdfDocument::try_from(doc).map_err(|e| PdfError::Open {
        path: src.display().to_string(),
        message: format!("not a PDF that can be annotated: {e}"),
    })?;
    let page_count = pdf.page_count()? as usize;

    let mut removed = 0usize;
    for page_number in 0..page_count {
        let page = pdf.load_page(page_number as i32)?;
        let page = mupdf::pdf::PdfPage::try_from(page)?;
        let Some(mut annots) = resolved_annots(&page)? else {
            continue;
        };
        let len = annots.len().unwrap_or(0);
        let mut matches = Vec::new();
        for index in 0..len {
            if annot_name(&page, index)?.as_deref() == Some(annotation_name) {
                matches.push(index);
            }
        }
        // Highest index first: deleting shifts everything after it down.
        for index in matches.into_iter().rev() {
            annots.array_delete(index as i32)?;
            removed += 1;
        }
    }
    if removed == 0 {
        return Ok(0);
    }

    // Garbage collection so the annotation's own object leaves with it, rather
    // than staying in the file as unreferenced data that still carries the
    // highlighted region.
    let mut options = mupdf::pdf::PdfWriteOptions::default();
    options.set_garbage(true);
    let out_str = out.to_string_lossy();
    pdf.save_with_options(out_str.as_ref(), options)
        .map_err(|e| PdfError::Backend(format!("cannot write {}: {e}", out.display())))?;
    Ok(removed)
}

/// The names (`/NM`) of the `Highlight` annotations on `page`, in `/Annots`
/// order. `None` for an annotation that carries no name.
///
/// Reads back what [`write_highlights`] wrote, so a test can assert against the
/// file rather than the code that produced it.
pub fn page_highlight_names(path: &Path, page: usize) -> Result<Vec<Option<String>>, PdfError> {
    let path_str = path.to_string_lossy();
    let doc = mupdf::Document::open(path_str.as_ref()).map_err(|e| PdfError::Open {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let pdf = mupdf::pdf::PdfDocument::try_from(doc).map_err(|e| PdfError::Open {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let count = pdf.page_count()? as usize;
    if page >= count {
        return Err(PdfError::PageOutOfRange { page, count });
    }
    let loaded = pdf.load_page(page as i32)?;
    let pdf_page = mupdf::pdf::PdfPage::try_from(loaded)?;
    let Some(annots) = resolved_annots(&pdf_page)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for index in 0..annots.len().unwrap_or(0) {
        let dict = annot_dict(&pdf_page, index)?;
        let is_highlight = dict
            .get_dict("Subtype")?
            .and_then(|s| s.as_name().ok().map(|n| n == b"Highlight"))
            .unwrap_or(false);
        if !is_highlight {
            continue;
        }
        out.push(annot_name(&pdf_page, index)?);
    }
    Ok(out)
}

/// The `Highlight` annotations on `page`, as the page-space rectangles each one
/// covers (origin top left, matching [`HighlightAnnotation::rects`]).
///
/// Reads back what [`write_highlights`] wrote, which is what lets a test assert
/// against the file rather than against the code that produced it.
pub fn page_highlights(path: &Path, page: usize) -> Result<Vec<Vec<Rect>>, PdfError> {
    let path_str = path.to_string_lossy();
    let doc = mupdf::Document::open(path_str.as_ref()).map_err(|e| PdfError::Open {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let pdf = mupdf::pdf::PdfDocument::try_from(doc).map_err(|e| PdfError::Open {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let count = pdf.page_count()? as usize;
    if page >= count {
        return Err(PdfError::PageOutOfRange { page, count });
    }
    let loaded = pdf.load_page(page as i32)?;
    let pdf_page = mupdf::pdf::PdfPage::try_from(loaded)?;
    let ctm = pdf_page.ctm()?;
    let Some(annots) = resolved_annots(&pdf_page)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    // Walking `/Annots` rather than the `PdfAnnotation` iterator: the quads live
    // in the dictionary, and `PdfAnnotation` exposes no accessor for its own
    // object, so the iterator would only have to be matched back to a dictionary
    // anyway.
    for index in 0..annots.len().unwrap_or(0) {
        let dict = annot_dict(&pdf_page, index)?;
        let is_highlight = dict
            .get_dict("Subtype")?
            .and_then(|s| s.as_name().ok().map(|n| n == b"Highlight"))
            .unwrap_or(false);
        if !is_highlight {
            continue;
        }
        let Some(quads) = dict.get_dict("QuadPoints")? else {
            continue;
        };
        let len = quads.len().unwrap_or(0);
        let mut rects = Vec::new();
        for quad in 0..len / 8 {
            let mut xs = [0.0f32; 4];
            let mut ys = [0.0f32; 4];
            for corner in 0..4 {
                let base = (quad * 8 + corner * 2) as i32;
                xs[corner] = array_real(&quads, base)?;
                ys[corner] = array_real(&quads, base + 1)?;
            }
            let corners: Vec<(f32, f32)> = (0..4).map(|i| ctm.transform_xy(xs[i], ys[i])).collect();
            rects.push(Rect {
                x0: corners.iter().map(|c| c.0).fold(f32::INFINITY, f32::min),
                y0: corners.iter().map(|c| c.1).fold(f32::INFINITY, f32::min),
                x1: corners
                    .iter()
                    .map(|c| c.0)
                    .fold(f32::NEG_INFINITY, f32::max),
                y1: corners
                    .iter()
                    .map(|c| c.1)
                    .fold(f32::NEG_INFINITY, f32::max),
            });
        }
        out.push(rects);
    }
    Ok(out)
}

/// One number from a PDF array, as `f32`.
fn array_real(array: &mupdf::pdf::PdfObject, index: i32) -> Result<f32, PdfError> {
    let entry = array
        .get_array(index)?
        .ok_or_else(|| PdfError::Backend(format!("missing array entry {index}")))?;
    Ok(entry.as_float()?)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::test_support::pdf_with_pages;

    fn three_page_doc() -> Document {
        Document::from_bytes(&pdf_with_pages(&[
            "Hello syodep page one",
            "Second page text",
            "Third page text",
        ]))
        .unwrap()
    }

    #[test]
    fn opens_pdf_and_counts_pages() {
        let doc = three_page_doc();
        assert_eq!(doc.page_count(), 3);
    }

    #[test]
    fn opens_pdf_from_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pdf");
        std::fs::write(&path, pdf_with_pages(&["From disk"])).unwrap();
        let doc = Document::open(&path).unwrap();
        assert_eq!(doc.page_count(), 1);
    }

    #[test]
    fn open_missing_file_is_a_clean_error() {
        let err = Document::open(Path::new("/nonexistent/x.pdf")).unwrap_err();
        assert!(matches!(err, PdfError::Open { .. }), "{err}");
        assert!(err.to_string().contains("/nonexistent/x.pdf"));
    }

    #[test]
    fn open_garbage_bytes_is_a_clean_error() {
        let err = Document::from_bytes(b"this is not a pdf").unwrap_err();
        assert!(matches!(err, PdfError::Open { .. }), "{err}");
    }

    #[test]
    fn page_sizes_match_media_box() {
        let doc = three_page_doc();
        for size in doc.page_sizes() {
            assert_eq!(size.width, 595.0);
            assert_eq!(size.height, 842.0);
        }
        assert!(matches!(
            doc.page_size(99),
            Err(PdfError::PageOutOfRange { page: 99, count: 3 })
        ));
    }

    #[test]
    fn renders_page_to_rgba_bitmap() {
        let doc = three_page_doc();
        let bitmap = doc.render_page(0, 1.0).unwrap();
        assert_eq!(bitmap.width, 595);
        assert_eq!(bitmap.height, 842);
        assert_eq!(
            bitmap.data.len(),
            bitmap.width as usize * bitmap.height as usize * 4
        );
        // Mostly white page: the first pixel is blank paper, opaque.
        assert_eq!(&bitmap.data[..4], &[0xff, 0xff, 0xff, 0xff]);
        // Some ink exists somewhere (the text).
        assert!(bitmap.data.as_chunks::<4>().0.iter().any(|px| px[0] < 0x80));
    }

    #[test]
    fn render_scale_scales_pixels() {
        let doc = three_page_doc();
        let bitmap = doc.render_page(0, 2.0).unwrap();
        assert_eq!(bitmap.width, 1190);
        assert_eq!(bitmap.height, 1684);
    }

    #[test]
    fn render_out_of_range_page_fails() {
        let doc = three_page_doc();
        assert!(matches!(
            doc.render_page(3, 1.0),
            Err(PdfError::PageOutOfRange { .. })
        ));
    }

    #[test]
    fn extracts_page_text() {
        let doc = three_page_doc();
        assert!(doc.page_text(0).unwrap().contains("Hello syodep page one"));
        assert!(doc.page_text(2).unwrap().contains("Third page text"));
    }

    #[test]
    fn outline_of_plain_document_is_empty() {
        let doc = three_page_doc();
        assert_eq!(doc.outline().unwrap(), vec![]);
    }

    fn cell_text(lines: &[ContentLine]) -> String {
        lines
            .iter()
            .flat_map(|l| l.cells.iter())
            .filter_map(|c| match c.kind {
                CellKind::Char(ch) => Some(ch),
                CellKind::Image => None,
            })
            .collect()
    }

    #[test]
    fn page_content_extracts_chars_in_reading_order() {
        let doc = three_page_doc();
        let lines = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap()
            .lines;
        assert!(!lines.is_empty());
        assert!(cell_text(&lines).contains("Hello syodep page one"));
        for line in &lines {
            // Line stays within the page.
            assert!(
                line.bbox.x0 >= 0.0 && line.bbox.x1 <= 595.0,
                "{:?}",
                line.bbox
            );
            assert!(
                line.bbox.y0 >= 0.0 && line.bbox.y1 <= 842.0,
                "{:?}",
                line.bbox
            );
            // Character cells run left to right.
            let mut prev = f32::NEG_INFINITY;
            for cell in &line.cells {
                assert!(
                    cell.bbox.x0 >= prev - 0.5,
                    "cells out of order: {:?}",
                    line.cells
                );
                prev = cell.bbox.x0;
            }
        }
    }

    #[test]
    fn page_content_out_of_range_fails() {
        let doc = three_page_doc();
        assert!(matches!(
            doc.page_content(3, ContentOptions::default(), None),
            Err(PdfError::PageOutOfRange { .. })
        ));
    }

    #[test]
    fn page_content_includes_one_cell_per_image() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_image()).unwrap();
        let lines = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap()
            .lines;
        let images: Vec<Cell> = lines
            .iter()
            .flat_map(|l| l.cells.iter())
            .copied()
            .filter(|c| c.kind == CellKind::Image)
            .collect();
        assert_eq!(images.len(), 1, "expected exactly one image cell");
        let b = images[0].bbox;
        // Drawn as a 120x90 pt box; allow generous tolerance.
        assert!((b.x1 - b.x0 - 120.0).abs() < 5.0, "image width: {b:?}");
        assert!((b.y1 - b.y0 - 90.0).abs() < 5.0, "image height: {b:?}");
        // The caption text coexists with the image.
        assert!(cell_text(&lines).contains("Caption"));
    }

    // ---- Atomic objects -------------------------------------------------

    /// A page of `count` stacked lines, each 10pt tall at x 100..200, with the
    /// given text length so `cells` is non-empty.
    fn stacked_lines(count: usize) -> Vec<ContentLine> {
        (0..count)
            .map(|i| {
                let y = 100.0 + i as f32 * 12.0;
                let bbox = Rect {
                    x0: 100.0,
                    y0: y,
                    x1: 200.0,
                    y1: y + 10.0,
                };
                ContentLine {
                    bbox,
                    cells: vec![Cell {
                        kind: CellKind::Char('x'),
                        bbox,
                        synthetic: false,
                    }],
                }
            })
            .collect()
    }

    /// A box covering `lines[range]` exactly.
    fn box_over(lines: &[ContentLine], range: std::ops::RangeInclusive<usize>) -> Rect {
        lines[*range.start()..=*range.end()]
            .iter()
            .map(|l| l.bbox)
            .reduce(|a, b| a.union(b))
            .unwrap()
    }

    #[test]
    fn object_ranges_cover_the_lines_inside_a_table_box() {
        let lines = stacked_lines(10);
        let objects = content_objects(
            &lines,
            &[],
            &[box_over(&lines, 3..=6)],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, ObjectKind::Table);
        assert_eq!((objects[0].start_line, objects[0].end_line), (3, 6));
    }

    #[test]
    fn object_ranges_stop_at_a_line_the_box_only_clips() {
        // MuPDF's box is the ruled region, which reaches past the last row. A
        // box overhanging the next line's centre must not claim that line: it
        // is the prose under the table, and claiming it makes the caret skip
        // over it.
        let lines = stacked_lines(10);
        let mut table = box_over(&lines, 3..=6);
        // Line 7 is 10pt tall; reach just past its centre, which is what makes
        // centre containment hand it over.
        table.y1 = lines[7].bbox.y0 + 6.0;
        let objects = content_objects(&lines, &[], &[table], &[], &[], &[], &[], &[], &[]);
        assert_eq!(objects.len(), 1, "objects: {objects:?}");
        assert_eq!((objects[0].start_line, objects[0].end_line), (3, 6));
        assert!(
            objects[0].bbox.y1 <= lines[7].bbox.y0,
            "the box still reaches line 7: {:?}",
            objects[0].bbox
        );
    }

    #[test]
    fn object_ranges_stop_at_a_line_set_apart_from_the_grid() {
        // Here the box covers the caption completely, so the overlap test says
        // nothing: what marks it as separate is the gap, wider than the
        // table's own rhythm.
        let mut lines = stacked_lines(8);
        // Push line 7 down, as a caption set below the last rule.
        let shift = 30.0;
        lines[7].bbox.y0 += shift;
        lines[7].bbox.y1 += shift;
        lines[7].cells[0].bbox = lines[7].bbox;
        let table = box_over(&lines, 3..=7);
        let objects = content_objects(&lines, &[], &[table], &[], &[], &[], &[], &[], &[]);
        assert_eq!(objects.len(), 1, "objects: {objects:?}");
        assert_eq!((objects[0].start_line, objects[0].end_line), (3, 6));
        assert!(
            objects[0].bbox.y1 <= lines[7].bbox.y0,
            "the box still reaches the caption: {:?}",
            objects[0].bbox
        );
    }

    #[test]
    fn object_ranges_keep_every_row_of_a_generously_spaced_table() {
        // The rhythm test compares against the table's *own* median gap, not
        // against line height: `pdf_with_table` sets 10pt text on 28pt rows, and
        // a line-height comparison would eat the first and last row of it.
        let lines: Vec<ContentLine> = (0..5)
            .map(|i| {
                let y = 100.0 + i as f32 * 28.0;
                let bbox = Rect {
                    x0: 100.0,
                    y0: y,
                    x1: 200.0,
                    y1: y + 10.0,
                };
                ContentLine {
                    bbox,
                    cells: vec![Cell {
                        kind: CellKind::Char('x'),
                        bbox,
                        synthetic: false,
                    }],
                }
            })
            .collect();
        let objects = content_objects(
            &lines,
            &[],
            &[box_over(&lines, 0..=4)],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        // The page is only the table here, so the "claims everything" guard
        // would fire; give it a sixth line of prose well clear of the box.
        assert_eq!(objects, vec![], "sanity: the page-claiming guard fires");

        let mut with_prose = lines.clone();
        let y = 100.0 + 6.0 * 28.0;
        let bbox = Rect {
            x0: 100.0,
            y0: y,
            x1: 200.0,
            y1: y + 10.0,
        };
        with_prose.push(ContentLine {
            bbox,
            cells: vec![Cell {
                kind: CellKind::Char('x'),
                bbox,
                synthetic: false,
            }],
        });
        let objects = content_objects(
            &with_prose,
            &[],
            &[box_over(&lines, 0..=4)],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(objects.len(), 1, "objects: {objects:?}");
        assert_eq!((objects[0].start_line, objects[0].end_line), (0, 4));
    }

    #[test]
    fn object_ranges_trim_a_clipped_first_line_too() {
        let lines = stacked_lines(10);
        let mut table = box_over(&lines, 3..=6);
        // Reach back over line 2, just past its centre.
        table.y0 = lines[2].bbox.y1 - 6.0;
        let objects = content_objects(&lines, &[], &[table], &[], &[], &[], &[], &[], &[]);
        assert_eq!(objects.len(), 1, "objects: {objects:?}");
        assert_eq!((objects[0].start_line, objects[0].end_line), (3, 6));
        assert!(
            objects[0].bbox.y0 >= lines[2].bbox.y1,
            "the box still reaches line 2: {:?}",
            objects[0].bbox
        );
    }

    #[test]
    fn object_ranges_discard_a_table_trimmed_below_two_lines() {
        // Trimming can only shrink a table, and a shrunken one falls to the
        // existing guard rather than being reported as a one-line table.
        let lines = stacked_lines(10);
        let mut table = box_over(&lines, 4..=5);
        table.y0 = lines[4].bbox.y0 + 7.0;
        let objects = content_objects(&lines, &[], &[table], &[], &[], &[], &[], &[], &[]);
        assert_eq!(objects, vec![]);
    }

    #[test]
    fn object_ranges_reject_a_single_line_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(
            &lines,
            &[],
            &[box_over(&lines, 4..=4)],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(objects, vec![], "a one-line table is just a line");
    }

    #[test]
    fn object_ranges_reject_a_table_claiming_every_line() {
        // MuPDF's whole-page fallback fires on ordinary prose; this guard is
        // the only thing standing between it and unnavigable pages.
        let lines = stacked_lines(10);
        let objects = content_objects(
            &lines,
            &[],
            &[box_over(&lines, 0..=9)],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(objects, vec![]);
    }

    #[test]
    fn object_ranges_reject_a_non_contiguous_member_set() {
        let mut lines = stacked_lines(6);
        // Push line 3 far to the right so a tall, narrow box skips it.
        lines[3].bbox.x0 = 400.0;
        lines[3].bbox.x1 = 500.0;
        let table = Rect {
            x0: 90.0,
            y0: 100.0,
            x1: 210.0,
            y1: 172.0,
        };
        let objects = content_objects(&lines, &[], &[table], &[], &[], &[], &[], &[], &[]);
        assert_eq!(objects, vec![], "a gapped table must degrade, not guess");
    }

    #[test]
    fn object_ranges_keep_the_wider_of_two_overlapping_tables() {
        let lines = stacked_lines(12);
        let objects = content_objects(
            &lines,
            &[],
            &[box_over(&lines, 2..=8), box_over(&lines, 4..=6)],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(objects.len(), 1);
        assert_eq!((objects[0].start_line, objects[0].end_line), (2, 8));
    }

    #[test]
    fn object_ranges_make_each_image_its_own_object() {
        let lines = stacked_lines(6);
        let objects = content_objects(&lines, &[1, 4], &[], &[], &[], &[], &[], &[], &[]);
        assert_eq!(objects.len(), 2);
        assert!(objects.iter().all(|o| o.kind == ObjectKind::Image));
        assert_eq!((objects[0].start_line, objects[0].end_line), (1, 1));
        assert_eq!((objects[1].start_line, objects[1].end_line), (4, 4));
    }

    #[test]
    fn object_ranges_absorb_an_image_inside_a_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(
            &lines,
            &[5],
            &[box_over(&lines, 3..=6)],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(objects.len(), 1, "the table is the enclosing unit");
        assert_eq!(objects[0].kind, ObjectKind::Table);
    }

    #[test]
    fn object_ranges_are_sorted_and_disjoint() {
        let lines = stacked_lines(20);
        let objects = content_objects(
            &lines,
            &[0, 15],
            &[box_over(&lines, 8..=11), box_over(&lines, 3..=5)],
            &[],
            &[(17, 18)],
            &[],
            &[],
            &[],
            &[],
        );
        let page = PageContent {
            lines,
            objects: objects.clone(),
            furniture: Vec::new(),
        };
        assert!(
            page.object_invariants_ok(),
            "objects violate invariants: {objects:?}"
        );
        for pair in objects.windows(2) {
            assert!(
                pair[0].end_line < pair[1].start_line,
                "objects overlap or are unsorted: {objects:?}"
            );
        }
    }

    // ---- Headings --------------------------------------------------------

    /// `count` body lines of `width` points at 10pt, non-bold.
    fn body_lines(count: usize, width: f32) -> (Vec<ContentLine>, Vec<LineStyle>) {
        let mut lines = Vec::new();
        let mut styles = Vec::new();
        for i in 0..count {
            let y = 100.0 + i as f32 * 12.0;
            lines.push(ContentLine {
                bbox: Rect {
                    x0: 100.0,
                    y0: y,
                    x1: 100.0 + width,
                    y1: y + 10.0,
                },
                // Character count is what weights the body-size vote.
                cells: vec![
                    Cell {
                        kind: CellKind::Char('x'),
                        bbox: Rect {
                            x0: 100.0,
                            y0: y,
                            x1: 106.0,
                            y1: y + 10.0,
                        },
                        synthetic: false,
                    };
                    60
                ],
            });
            styles.push(LineStyle {
                size: 10.0,
                bold: false,
                math: 0.0,
                mono: 0.0,
                angle: Some(0.0),
                baseline: y + 8.0,
            });
        }
        (lines, styles)
    }

    fn set_style(
        lines: &mut [ContentLine],
        styles: &mut [LineStyle],
        i: usize,
        size: f32,
        bold: bool,
        width: f32,
    ) {
        styles[i] = LineStyle {
            size,
            bold,
            ..styles[i]
        };
        lines[i].bbox.x1 = lines[i].bbox.x0 + width;
    }

    /// Give line `i` real text at `width` points, with `math` of its glyphs set
    /// in a math font. `body_lines` alone cannot express either.
    fn set_text(
        lines: &mut [ContentLine],
        styles: &mut [LineStyle],
        i: usize,
        text: &str,
        width: f32,
        math: f32,
    ) {
        let y = lines[i].bbox.y0;
        let step = width / text.chars().count().max(1) as f32;
        let mut x = lines[i].bbox.x0;
        lines[i].cells = text
            .chars()
            .map(|c| {
                let cell = Cell {
                    kind: CellKind::Char(c),
                    bbox: Rect {
                        x0: x,
                        y0: y,
                        x1: x + step,
                        y1: y + 10.0,
                    },
                    synthetic: false,
                };
                x += step;
                cell
            })
            .collect();
        lines[i].bbox.x1 = lines[i].bbox.x0 + width;
        styles[i] = LineStyle { math, ..styles[i] };
    }

    #[test]
    fn equation_ranges_ignore_ascii_code_fragments() {
        // Programming idioms that look like maths to a naive operator check.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(&mut lines, &mut styles, 4, "in C++.", 80.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![]);
        set_text(&mut lines, &mut styles, 4, "count += 1", 90.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![]);
        set_text(&mut lines, &mut styles, 4, "-> None", 70.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![]);
        set_text(&mut lines, &mut styles, 4, "== 1)", 50.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![]);
        set_text(&mut lines, &mut styles, 4, "−1", 30.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn equation_ranges_still_find_unicode_maths_without_a_math_font() {
        // Character-based detection must keep working for real formulae once
        // ASCII-only fragments are filtered out.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(&mut lines, &mut styles, 4, "α + β = γ", 120.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![(4, 4)]);
        set_text(&mut lines, &mut styles, 5, "E = mc²", 80.0, 0.0);
        // `²` is not in the math-symbol set and there is no Greek / unicode
        // operator besides ASCII `=`, so fonts must carry this one.
        assert_eq!(equation_ranges(&lines, &styles), vec![(4, 4)]);
        set_text(&mut lines, &mut styles, 5, "E = mc²", 80.0, 1.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![(4, 5)]);
    }

    #[test]
    fn equation_ranges_flags_a_set_apart_formula() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(&mut lines, &mut styles, 4, "α + β = γ", 120.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![(4, 4)]);
    }

    #[test]
    fn equation_ranges_ignore_prose_carrying_inline_maths() {
        // The guard that matters most: a sentence with a formula in it fills its
        // measure, and making it a region would split the sentence around it.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(
            &mut lines,
            &mut styles,
            4,
            "we set x = 1 and obtain the bound α + β for every sample in the set",
            400.0,
            0.2,
        );
        assert_eq!(equation_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn equation_ranges_ignore_a_short_last_line_of_a_paragraph() {
        // Short, so the width test passes — but it is prose, with words and no
        // operator.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(&mut lines, &mut styles, 4, "installed there.", 110.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn equation_ranges_ignore_a_centred_label() {
        // Greek is a math symbol, but with no operator this is a caption.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(&mut lines, &mut styles, 4, "Table 3: σ values", 130.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn equation_ranges_read_the_fonts_when_the_characters_are_plain() {
        // "x1 + a2b3c4d5e6" is barely 1/15 operators, under the character
        // threshold, so only the font share can carry it.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(&mut lines, &mut styles, 4, "x1 + a2b3c4d5e6", 140.0, 1.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![(4, 4)]);

        set_text(&mut lines, &mut styles, 4, "x1 + a2b3c4d5e6", 140.0, 0.0);
        assert_eq!(
            equation_ranges(&lines, &styles),
            vec![],
            "without the fonts there is nothing to go on"
        );
    }

    #[test]
    fn equation_ranges_merge_an_aligned_system() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(&mut lines, &mut styles, 3, "α + β = γ", 120.0, 0.0);
        set_text(&mut lines, &mut styles, 4, "γ − δ ≤ ε", 120.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![(3, 4)]);
    }

    #[test]
    fn equation_ranges_absorb_an_equation_number_set_on_its_own_line() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(&mut lines, &mut styles, 4, "α + β = γ", 120.0, 0.0);
        set_text(&mut lines, &mut styles, 5, "(3.4)", 40.0, 0.0);
        assert_eq!(equation_ranges(&lines, &styles), vec![(4, 5)]);
    }

    #[test]
    fn an_equation_number_is_a_bracketed_figure_and_nothing_else() {
        assert!(is_equation_number("(12)"));
        assert!(is_equation_number("(3.4)"));
        assert!(is_equation_number(" (A.1) "));
        assert!(!is_equation_number("(see below)"));
        assert!(!is_equation_number("(a)"), "a list marker, not a number");
        assert!(!is_equation_number("12"));
        assert!(!is_equation_number("()"));
    }

    #[test]
    fn equation_ranges_reject_a_page_that_is_mostly_equations() {
        // Most lines reading as maths means the signal is wrong, not that the
        // page is one long formula. Three full-width prose lines stay, so the
        // width test still has a column to compare against and it is the share
        // guard that fires.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        for i in 0..7 {
            set_text(&mut lines, &mut styles, i, "α + β = γ", 120.0, 0.0);
        }
        assert_eq!(equation_ranges(&lines, &styles), vec![]);

        // One fewer, and the same page detects them.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        for i in 0..6 {
            set_text(&mut lines, &mut styles, i, "α + β = γ", 120.0, 0.0);
        }
        assert_eq!(equation_ranges(&lines, &styles), vec![(0, 5)]);
    }

    #[test]
    fn equation_ranges_reject_a_run_too_long_to_be_a_system() {
        let (mut lines, mut styles) = body_lines(30, 400.0);
        for i in 5..=17 {
            set_text(&mut lines, &mut styles, i, "α + β = γ", 120.0, 0.0);
        }
        assert_eq!(equation_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn a_math_font_is_recognised_by_its_name() {
        // The names real PDFs carry, subset prefixes included.
        for name in [
            "CMMI10",
            "ABCDEF+CMSY7",
            "CMEX10",
            "MSBM10",
            "XITSMath-Regular",
            "CambriaMath",
            "Symbol",
        ] {
            assert!(is_math_font(name), "{name}");
        }
        for name in ["Helvetica", "NimbusRomNo9L-Regu", "CMR10", "Times-Italic"] {
            assert!(!is_math_font(name), "{name}");
        }
    }

    #[test]
    fn a_mono_font_is_recognised_by_its_name() {
        for name in [
            "Courier",
            "Courier-Bold",
            "ABCDEF+Menlo-Regular",
            "Consolas",
            "DejaVuSansMono",
            "SourceCodePro-Regular",
        ] {
            assert!(is_mono_font(name), "{name}");
        }
        for name in ["Helvetica", "Times-Roman", "Symbol", "CMMI10"] {
            assert!(!is_mono_font(name), "{name}");
        }
    }

    #[test]
    fn caption_ranges_flags_a_prefixed_line_near_an_image() {
        let (mut lines, mut styles) = body_lines(8, 400.0);
        // Image at line 3; caption just below within proximity.
        lines[3].cells = vec![Cell {
            kind: CellKind::Image,
            bbox: lines[3].bbox,
            synthetic: false,
        }];
        styles[3] = LineStyle {
            size: 0.0,
            bold: false,
            math: 0.0,
            mono: 0.0,
            angle: None,
            baseline: lines[3].bbox.y0 + 5.0,
        };
        // Place caption line 4 immediately under the image.
        let gap = 10.0;
        lines[4].bbox.y0 = lines[3].bbox.y1 + gap;
        lines[4].bbox.y1 = lines[4].bbox.y0 + 10.0;
        styles[4].baseline = lines[4].bbox.y0 + 8.0;
        set_text(&mut lines, &mut styles, 4, "Fig. 1 An overview", 160.0, 0.0);
        let ranges = caption_ranges(&lines, &styles, &[3], &[]);
        assert_eq!(ranges, vec![(4, 4)]);
    }

    #[test]
    fn caption_ranges_ignore_a_distant_prefixed_line() {
        let (mut lines, mut styles) = body_lines(8, 400.0);
        lines[3].cells = vec![Cell {
            kind: CellKind::Image,
            bbox: lines[3].bbox,
            synthetic: false,
        }];
        styles[3].size = 0.0;
        set_text(&mut lines, &mut styles, 6, "Fig. 2 Far away", 160.0, 0.0);
        // body_lines spaces lines 12pt apart; line 6 is ~36pt below line 3.
        assert!(lines[6].bbox.y0 - lines[3].bbox.y1 > CAPTION_PROXIMITY_PT);
        assert_eq!(caption_ranges(&lines, &styles, &[3], &[]), vec![]);
    }

    #[test]
    fn code_ranges_flags_a_run_of_mono_lines() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        for (i, text) in [(3, "fn main() {"), (4, "    println!(hi);"), (5, "}")].iter() {
            set_text(&mut lines, &mut styles, *i, text, 120.0, 0.0);
            styles[*i].mono = 1.0;
        }
        assert_eq!(code_ranges(&lines, &styles), vec![(3, 5)]);
    }

    #[test]
    fn code_ranges_skip_mathish_mono_lines() {
        let (mut lines, mut styles) = body_lines(8, 400.0);
        set_text(&mut lines, &mut styles, 3, "α + β = γ", 100.0, 0.8);
        styles[3].mono = 1.0;
        assert_eq!(code_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn heading_ranges_flags_a_numbered_subsection_at_body_size() {
        // Shape, not typography: `1.1. Title` at body size was previously
        // invisible to the size/weight vote and split as two sentences.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(
            &mut lines,
            &mut styles,
            3,
            "1.1. The ENDF format and nuclear data libraries",
            380.0,
            0.0,
        );
        assert_eq!(heading_ranges(&lines, &styles), vec![(3, 3)]);
    }

    #[test]
    fn heading_ranges_extends_a_numbered_heading_that_wraps() {
        // The ENDFtk 2.3.2 shape: opener fills the measure mid-phrase, then a
        // short leftover on the next line. Without claiming the leftover,
        // sentence scope splits the title in half.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(
            &mut lines,
            &mut styles,
            3,
            "2.3.2. Application: inserting the reconstructed cross section data in the ",
            395.0,
            0.0,
        );
        set_text(&mut lines, &mut styles, 4, "evaluated file", 80.0, 0.0);
        set_text(
            &mut lines,
            &mut styles,
            5,
            "With the functionality presented above, we can now develop a sim-",
            400.0,
            0.0,
        );
        assert_eq!(heading_ranges(&lines, &styles), vec![(3, 4)]);
    }

    #[test]
    fn heading_ranges_does_not_extend_a_numbered_heading_into_body() {
        // A short opener followed by a full-width paragraph must stay one
        // line: the body fills the column, so it is not a title wrap.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(
            &mut lines,
            &mut styles,
            3,
            "2.3.1. Interface overview",
            180.0,
            0.0,
        );
        set_text(
            &mut lines,
            &mut styles,
            4,
            "In addition to functions used to navigate and traverse an ENDF tree,",
            400.0,
            0.0,
        );
        assert_eq!(heading_ranges(&lines, &styles), vec![(3, 3)]);
    }

    #[test]
    fn heading_ranges_flags_a_deeper_numbered_heading() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(
            &mut lines,
            &mut styles,
            2,
            "2.12. Recommended checking order",
            300.0,
            0.0,
        );
        assert_eq!(heading_ranges(&lines, &styles), vec![(2, 2)]);
    }

    #[test]
    fn heading_ranges_ignores_a_single_level_enumerator() {
        // `1. First point` is a list candidate, not a section heading.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(
            &mut lines,
            &mut styles,
            4,
            "1. First point of the list",
            280.0,
            0.0,
        );
        assert_eq!(heading_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn a_numbered_heading_text_needs_an_internal_dot_and_a_title() {
        assert!(is_numbered_heading_text("1.1. Methods"));
        assert!(is_numbered_heading_text("2.12. Recommended checking order"));
        assert!(is_numbered_heading_text("1.2.3. Overview"));
        assert!(
            !is_numbered_heading_text("1.2.3 Overview"),
            "needs the trailing section dot"
        );
        assert!(!is_numbered_heading_text("1. Introduction"));
        assert!(!is_numbered_heading_text("1. First point"));
        assert!(!is_numbered_heading_text("1.1."));
        assert!(!is_numbered_heading_text("The 1.1. Methods"));
        assert!(!is_numbered_heading_text("3.14 is the value"));
    }

    #[test]
    fn heading_ranges_keeps_a_numbered_heading_when_typography_share_trips() {
        // A page of short bold lines trips the share cap and would erase every
        // typography heading — the shape rule must still report `1.1. Title`.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        for i in 0..10 {
            set_style(&mut lines, &mut styles, i, 10.0, true, 80.0);
        }
        set_text(&mut lines, &mut styles, 4, "1.1. Methods", 200.0, 0.0);
        styles[4].bold = false;
        assert_eq!(heading_ranges(&lines, &styles), vec![(4, 4)]);
    }

    #[test]
    fn content_objects_promote_a_numbered_heading_range() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_text(
            &mut lines,
            &mut styles,
            3,
            "1.1. The ENDF format and nuclear data libraries",
            380.0,
            0.0,
        );
        let headings = heading_ranges(&lines, &styles);
        assert_eq!(headings, vec![(3, 3)]);
        let objects = content_objects(&lines, &[], &[], &[], &headings, &[], &[], &[], &[]);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, ObjectKind::Heading);
        assert_eq!((objects[0].start_line, objects[0].end_line), (3, 3));
    }

    #[test]
    fn heading_ranges_flags_a_line_set_larger_than_the_body() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_style(&mut lines, &mut styles, 3, 18.0, true, 200.0);
        assert_eq!(heading_ranges(&lines, &styles), vec![(3, 3)]);
    }

    #[test]
    fn heading_ranges_rejects_a_single_glyph_drop_cap() {
        // An oversized one-letter line is a drop cap, not a heading: claiming
        // it as one severs the chapter opener from its own sentence.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_style(&mut lines, &mut styles, 0, 36.0, true, 30.0);
        set_text(&mut lines, &mut styles, 0, "T", 30.0, 0.0);
        styles[0].size = 36.0;
        styles[0].bold = true;
        assert_eq!(heading_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn heading_ranges_ignores_prose_only_slightly_larger_than_the_body() {
        // The real failure this guards: on a page of 9pt code listings,
        // ordinary 10pt prose is 1.11x the body size.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        for s in styles.iter_mut() {
            s.size = 9.0;
        }
        set_style(&mut lines, &mut styles, 4, 10.0, false, 400.0);
        assert_eq!(heading_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn heading_ranges_merges_a_heading_that_wraps() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_style(&mut lines, &mut styles, 2, 18.0, true, 380.0);
        set_style(&mut lines, &mut styles, 3, 18.0, true, 150.0);
        assert_eq!(heading_ranges(&lines, &styles), vec![(2, 3)]);
    }

    #[test]
    fn heading_ranges_flags_a_short_bold_line_at_body_size() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_style(&mut lines, &mut styles, 5, 10.0, true, 120.0);
        assert_eq!(heading_ranges(&lines, &styles), vec![(5, 5)]);
    }

    #[test]
    fn heading_ranges_ignores_a_bold_line_that_fills_the_column() {
        // A bold lead-in sentence inside a paragraph, not a heading.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_style(&mut lines, &mut styles, 5, 10.0, true, 400.0);
        assert_eq!(heading_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn heading_ranges_finds_nothing_in_uniform_prose() {
        let (lines, styles) = body_lines(12, 400.0);
        assert_eq!(heading_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn heading_ranges_reject_a_page_that_is_mostly_headings() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        for i in 0..6 {
            set_style(&mut lines, &mut styles, i, 18.0, true, 120.0);
        }
        assert_eq!(
            heading_ranges(&lines, &styles),
            vec![],
            "over half the page cannot be heading"
        );
    }

    #[test]
    fn heading_ranges_reject_a_run_too_long_to_be_a_heading() {
        let (mut lines, mut styles) = body_lines(20, 400.0);
        for i in 2..=6 {
            set_style(&mut lines, &mut styles, i, 14.0, true, 380.0);
        }
        assert_eq!(heading_ranges(&lines, &styles), vec![]);
    }

    #[test]
    fn a_heading_is_neither_atomic_nor_a_block() {
        assert!(!ObjectKind::Heading.is_atomic());
        assert!(!ObjectKind::Heading.is_block());
        // A table is walkable at word scope but one unit from line scope up.
        assert!(!ObjectKind::Table.is_atomic());
        assert!(ObjectKind::Table.is_block());
        assert!(ObjectKind::Image.is_atomic());
    }

    #[test]
    fn object_ranges_drop_a_heading_that_overlaps_a_table() {
        // A bold, short line inside a table is a column header.
        let lines = stacked_lines(10);
        let objects = content_objects(
            &lines,
            &[],
            &[box_over(&lines, 3..=6)],
            &[],
            &[(4, 4)],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, ObjectKind::Table);
    }

    #[test]
    fn object_ranges_keep_a_heading_outside_every_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(
            &lines,
            &[],
            &[box_over(&lines, 5..=8)],
            &[],
            &[(1, 2)],
            &[],
            &[],
            &[],
            &[],
        );
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].kind, ObjectKind::Heading);
        assert_eq!((objects[0].start_line, objects[0].end_line), (1, 2));
        assert_eq!(objects[1].kind, ObjectKind::Table);
    }

    #[test]
    fn page_content_detects_a_heading_and_a_bold_subheading() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_heading()).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let headings: Vec<_> = content
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Heading)
            .collect();
        let text_of = |i: usize| -> String {
            content.lines[i]
                .cells
                .iter()
                .filter_map(|c| match c.kind {
                    CellKind::Char(ch) => Some(ch),
                    CellKind::Image => None,
                })
                .collect()
        };
        assert_eq!(headings.len(), 2, "objects: {:?}", content.objects);
        assert!(
            text_of(headings[0].start_line).contains("Directory layout"),
            "first heading is {:?}",
            text_of(headings[0].start_line)
        );
        assert!(
            text_of(headings[1].start_line).contains("Ordering"),
            "second heading is {:?}",
            text_of(headings[1].start_line)
        );
        // Neither may swallow the prose beneath it.
        for h in &headings {
            assert_eq!(h.start_line, h.end_line, "heading covers body text");
        }
    }

    #[test]
    fn page_content_reports_no_heading_on_uniform_prose() {
        let doc = three_page_doc();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        assert!(
            !content
                .objects
                .iter()
                .any(|o| o.kind == ObjectKind::Heading),
            "prose reported as a heading: {:?}",
            content.objects
        );
    }

    #[test]
    fn page_content_detects_a_display_equation() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_equation()).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let text_of = |i: usize| -> String {
            content.lines[i]
                .cells
                .iter()
                .filter_map(|c| match c.kind {
                    CellKind::Char(ch) => Some(ch),
                    CellKind::Image => None,
                })
                .collect()
        };
        let equations: Vec<_> = content
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Equation)
            .collect();
        assert_eq!(equations.len(), 1, "objects: {:?}", content.objects);
        let equation = equations[0];
        // The formula, and only the formula. MuPDF may decode Symbol as Greek
        // (`α + β = γ`) or leave the Latin source (`a + b = g`); either is fine
        // — detection here is driven by the font name.
        assert_eq!(equation.start_line, equation.end_line, "prose swallowed");
        let text = text_of(equation.start_line);
        let looks_like_formula = text.contains('=')
            && (text.contains('\u{3b1}') || (text.contains('a') && text.contains('+')));
        assert!(looks_like_formula, "equation line reads {text:?}");
        // The prose around it stays outside.
        for i in 0..content.lines.len() {
            if i == equation.start_line {
                continue;
            }
            let other = text_of(i);
            assert!(
                !other.contains('=') && !other.contains('\u{3b1}'),
                "line {i} reads {other:?}"
            );
        }
    }

    #[test]
    fn an_aligned_system_is_one_equation_spanning_its_rows() {
        let doc =
            Document::from_bytes(&crate::test_support::pdf_with_multiline_equation()).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let equations: Vec<_> = content
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Equation)
            .collect();
        assert_eq!(equations.len(), 1, "objects: {:?}", content.objects);
        let equation = equations[0];
        assert_eq!(
            equation.end_line - equation.start_line + 1,
            3,
            "the three rows are not one object: {equation:?}"
        );
        // The box spans every row, so it is wider than the widest single row —
        // which is exactly what makes it worth drawing instead of per-row
        // strips.
        let widest = (equation.start_line..=equation.end_line)
            .map(|i| content.lines[i].bbox.x1 - content.lines[i].bbox.x0)
            .fold(0.0f32, f32::max);
        assert!(
            equation.bbox.x1 - equation.bbox.x0 >= widest,
            "box narrower than its widest row"
        );
    }

    #[test]
    fn page_content_reports_no_equation_on_uniform_prose() {
        let doc = three_page_doc();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        assert!(
            !content
                .objects
                .iter()
                .any(|o| o.kind == ObjectKind::Equation),
            "prose reported as an equation: {:?}",
            content.objects
        );
    }

    #[test]
    fn page_content_without_equation_detection_reports_none() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_equation()).unwrap();
        let content = doc
            .page_content(
                0,
                ContentOptions {
                    detect_equations: false,
                    ..ContentOptions::default()
                },
                None,
            )
            .unwrap();
        assert!(content
            .objects
            .iter()
            .all(|o| o.kind != ObjectKind::Equation));
    }

    #[test]
    fn page_content_without_heading_detection_reports_none() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_heading()).unwrap();
        let content = doc
            .page_content(
                0,
                ContentOptions {
                    detect_headings: false,
                    ..ContentOptions::default()
                },
                None,
            )
            .unwrap();
        assert!(content
            .objects
            .iter()
            .all(|o| o.kind != ObjectKind::Heading));
    }

    // ---- Footnotes ---------------------------------------------------------

    #[test]
    fn footnote_ranges_flags_undersized_text_in_the_bottom_band() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        let y = 900.0;
        lines.push(ContentLine {
            bbox: Rect {
                x0: 100.0,
                y0: y,
                x1: 180.0,
                y1: y + 8.0,
            },
            cells: vec![
                Cell {
                    kind: CellKind::Char('x'),
                    bbox: Rect {
                        x0: 100.0,
                        y0: y,
                        x1: 106.0,
                        y1: y + 8.0,
                    },
                    synthetic: false,
                };
                20
            ],
        });
        styles.push(LineStyle {
            size: 8.0,
            bold: false,
            math: 0.0,
            mono: 0.0,
            angle: Some(0.0),
            baseline: y + 6.0,
        });
        assert_eq!(footnote_ranges(&lines, &styles, 1000.0), vec![(10, 10)]);
    }

    #[test]
    fn footnote_ranges_ignore_math_fragments_in_the_bottom_band() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        let y = 900.0;
        // `Ic,t = Ic,0` shape: small, at the foot, but plainly mathematics.
        lines.push(text_line_at_x(100.0, y, "Ic,t = Ic,0"));
        styles.push(LineStyle {
            size: 8.0,
            bold: false,
            math: 0.0,
            mono: 0.0,
            angle: Some(0.0),
            baseline: y + 6.0,
        });
        assert!(
            footnote_ranges(&lines, &styles, 1000.0).is_empty(),
            "math fragment must not become a footnote"
        );
    }

    #[test]
    fn footnote_ranges_ignore_ordinary_body_text_near_the_foot() {
        // Same bottom-band position as the line above, but set at the body's
        // own size: a page that legitimately ends with body prose near the
        // foot must not lose it to this heuristic.
        let (mut lines, mut styles) = body_lines(10, 400.0);
        let y = 900.0;
        lines.push(ContentLine {
            bbox: Rect {
                x0: 100.0,
                y0: y,
                x1: 180.0,
                y1: y + 10.0,
            },
            cells: vec![
                Cell {
                    kind: CellKind::Char('x'),
                    bbox: Rect {
                        x0: 100.0,
                        y0: y,
                        x1: 106.0,
                        y1: y + 10.0,
                    },
                    synthetic: false,
                };
                20
            ],
        });
        styles.push(LineStyle {
            size: 10.0,
            bold: false,
            math: 0.0,
            mono: 0.0,
            angle: Some(0.0),
            baseline: y + 8.0,
        });
        assert_eq!(
            footnote_ranges(&lines, &styles, 1000.0),
            Vec::<(usize, usize)>::new()
        );
    }

    #[test]
    fn footnote_ranges_reject_a_page_that_is_mostly_small_type() {
        // Two real body lines carry most of the page's character weight, so
        // the body size is still correctly computed as 10pt even though
        // three short, small-type lines crowding the bottom band outnumber
        // them by *line* count -- the same share guard `heading_ranges` and
        // `equation_ranges` give their own detectors.
        let mut lines = Vec::new();
        let mut styles = Vec::new();
        for i in 0..2 {
            let y = 100.0 + i as f32 * 12.0;
            lines.push(ContentLine {
                bbox: Rect {
                    x0: 100.0,
                    y0: y,
                    x1: 460.0,
                    y1: y + 10.0,
                },
                cells: vec![
                    Cell {
                        kind: CellKind::Char('x'),
                        bbox: Rect {
                            x0: 100.0,
                            y0: y,
                            x1: 106.0,
                            y1: y + 10.0,
                        },
                        synthetic: false,
                    };
                    60
                ],
            });
            styles.push(LineStyle {
                size: 10.0,
                bold: false,
                math: 0.0,
                mono: 0.0,
                angle: Some(0.0),
                baseline: y + 8.0,
            });
        }
        for i in 0..3 {
            let y = 900.0 + i as f32 * 10.0;
            lines.push(ContentLine {
                bbox: Rect {
                    x0: 100.0,
                    y0: y,
                    x1: 140.0,
                    y1: y + 8.0,
                },
                cells: vec![
                    Cell {
                        kind: CellKind::Char('x'),
                        bbox: Rect {
                            x0: 100.0,
                            y0: y,
                            x1: 106.0,
                            y1: y + 8.0,
                        },
                        synthetic: false,
                    };
                    5
                ],
            });
            styles.push(LineStyle {
                size: 8.0,
                bold: false,
                math: 0.0,
                mono: 0.0,
                angle: Some(0.0),
                baseline: y + 6.0,
            });
        }
        assert_eq!(
            footnote_ranges(&lines, &styles, 1000.0),
            Vec::<(usize, usize)>::new()
        );
    }

    #[test]
    fn content_objects_promote_a_footnote_range() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        let y = 900.0;
        lines.push(ContentLine {
            bbox: Rect {
                x0: 100.0,
                y0: y,
                x1: 180.0,
                y1: y + 8.0,
            },
            cells: vec![
                Cell {
                    kind: CellKind::Char('x'),
                    bbox: Rect {
                        x0: 100.0,
                        y0: y,
                        x1: 106.0,
                        y1: y + 8.0,
                    },
                    synthetic: false,
                };
                20
            ],
        });
        styles.push(LineStyle {
            size: 8.0,
            bold: false,
            math: 0.0,
            mono: 0.0,
            angle: Some(0.0),
            baseline: y + 6.0,
        });
        let footnotes = footnote_ranges(&lines, &styles, 1000.0);
        assert_eq!(footnotes, vec![(10, 10)]);
        let objects = content_objects(&lines, &[], &[], &[], &[], &[], &[], &footnotes, &[]);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, ObjectKind::Footnote);
        assert_eq!((objects[0].start_line, objects[0].end_line), (10, 10));
    }

    #[test]
    fn a_footnote_line_is_never_read_as_a_list_marker() {
        // A footnote's own citation text is often enumerator-shaped ("12.
        // Author, Title") and can land aligned with a real list elsewhere on
        // the page. Footnotes are claimed before list detection runs
        // precisely so this can never be misread as a third item of that
        // list -- see the ordering note in `content_objects`.
        let (mut lines, mut styles) = body_lines(6, 400.0);
        set_text(&mut lines, &mut styles, 2, "1. first list item", 180.0, 0.0);
        set_text(
            &mut lines,
            &mut styles,
            3,
            "2. second list item",
            180.0,
            0.0,
        );
        let y = 900.0;
        lines.push(text_line_at(y, "12. Author, Title"));
        styles.push(LineStyle {
            size: 8.0,
            bold: false,
            math: 0.0,
            mono: 0.0,
            angle: Some(0.0),
            baseline: y,
        });

        let footnotes = footnote_ranges(&lines, &styles, 1000.0);
        assert_eq!(
            footnotes,
            vec![(6, 6)],
            "the footnote line itself must be detected"
        );
        let objects = content_objects(&lines, &[], &[], &[], &[], &[], &[], &footnotes, &[]);
        assert!(
            objects
                .iter()
                .any(|o| o.kind == ObjectKind::Footnote && o.start_line == 6),
            "objects: {objects:?}"
        );
        assert!(
            !objects
                .iter()
                .any(|o| o.kind == ObjectKind::ListItem && o.start_line == 6),
            "the footnote's own text must not become a third list item: {objects:?}"
        );
    }

    // ---- Lists -----------------------------------------------------------

    #[test]
    fn a_bullet_is_a_marker_alone_or_before_text() {
        // Extraction often puts the bullet on a line of its own.
        assert_eq!(line_marker("\u{2022}"), Some((Marker::Bullet, 1)));
        assert_eq!(
            line_marker("\u{2022} A standard way to install"),
            Some((Marker::Bullet, 1))
        );
        assert_eq!(line_marker("- a dashed item"), Some((Marker::Bullet, 1)));
    }

    #[test]
    fn an_enumerator_is_a_marker() {
        // The reported length covers the label and its closer, so a sentence
        // can be stopped from ending inside the marker.
        assert_eq!(line_marker("1. First item"), Some((Marker::Enumerated, 2)));
        assert_eq!(line_marker("2) Second item"), Some((Marker::Enumerated, 2)));
        assert_eq!(
            line_marker("iv. Fourth item"),
            Some((Marker::Enumerated, 3))
        );
        assert_eq!(
            line_marker("a) Lettered item"),
            Some((Marker::Enumerated, 2))
        );
    }

    #[test]
    fn a_marker_length_counts_the_indent_before_it() {
        assert_eq!(line_marker("   1. Indented"), Some((Marker::Enumerated, 5)));
        assert_eq!(
            line_marker("  \u{2022} Indented"),
            Some((Marker::Bullet, 3))
        );
    }

    #[test]
    fn ordinary_prose_is_not_a_marker() {
        assert_eq!(line_marker("The database is a set"), None);
        // A decimal is not an enumerator: a digit follows the stop, not a space.
        assert_eq!(line_marker("3.14 is the value"), None);
        // A hyphenated word is not a bullet: no space after the dash.
        assert_eq!(line_marker("-ish results"), None);
        // Too long to be an enumerator label.
        assert_eq!(line_marker("1998. That year saw"), None);
    }

    #[test]
    fn a_list_needs_more_than_one_item() {
        // A lone marker-looking line is prose, not a list.
        let lines = vec![
            text_line_at(20.0, "Introductory prose here"),
            text_line_at(40.0, "1. Something that looks like an item"),
            text_line_at(60.0, "More prose follows on"),
        ];
        assert_eq!(list_items(&lines, &[]), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn aligned_markers_of_the_same_kind_form_a_list() {
        let lines = vec![
            text_line_at(20.0, "The files created are:"),
            text_line_at(40.0, "\u{2022} the first file"),
            text_line_at(60.0, "\u{2022} the second file"),
            text_line_at(80.0, "\u{2022} the third file"),
        ];
        assert_eq!(list_items(&lines, &[]), vec![(1, 1), (2, 2), (3, 3)]);
    }

    #[test]
    fn uppercase_initials_are_not_enumerated_list_markers() {
        // Sidebar citations: `T. Author, …` must not corroborate into a list
        // beside real `-` bullets.
        let lines = vec![
            text_line_at(20.0, "-"),
            text_line_at(40.0, "T. Puetterich, F. Albrecht et al."),
            text_line_at(60.0, "-"),
            text_line_at(80.0, "K. Tsuchiya, H. Murakami et al."),
        ];
        // Indent the citation lines past the bullets.
        let mut lines = lines;
        indent(&mut lines[1], 10.0);
        indent(&mut lines[3], 10.0);
        let items = list_items(&lines, &[]);
        assert!(
            items.iter().all(|&(s, _)| {
                let t = line_text(&lines[s]);
                line_marker(&t).is_some_and(|(m, _)| m == Marker::Bullet)
            }),
            "enumerated initials must not become list starts: {items:?}"
        );
    }

    #[test]
    fn a_lone_bullet_pairs_with_text_emitted_before_it() {
        // Emission order: citation text, then the `-` above it on the page.
        let mut lines = vec![
            text_line_at(40.0, "A.D. Xu, Y.H. Li, T. Yu et al."),
            text_line_at(20.0, "-"),
            text_line_at(80.0, "K. Tsuchiya, H. Murakami et al."),
            text_line_at(60.0, "-"),
        ];
        indent(&mut lines[0], 10.0);
        indent(&mut lines[2], 10.0);
        assert_eq!(list_items(&lines, &[]), vec![(0, 1), (2, 3)]);
    }

    /// Indent a line past the list's markers, as a wrapped continuation sits.
    fn indent(line: &mut ContentLine, by: f32) {
        line.bbox.x0 += by;
    }

    /// An object standing in the way of list detection.
    fn blocking(
        lines: &[ContentLine],
        kind: ObjectKind,
        start: usize,
        end: usize,
    ) -> ContentObject {
        ContentObject {
            kind,
            bbox: lines[start].bbox.union(lines[end].bbox),
            start_line: start,
            end_line: end,
        }
    }

    #[test]
    fn an_item_covers_its_indented_continuation() {
        // Continuation leading must sit under LIST_GAP_FACTOR * median height
        // (0.75 * 8 = 6 here). A 12pt step gives a 4pt gap — real wrapped
        // items run tighter still.
        let mut lines = vec![
            text_line_at(20.0, "\u{2022} the first file"),
            text_line_at(32.0, "wrapped onto a second line"),
            text_line_at(44.0, "\u{2022} the second file"),
        ];
        indent(&mut lines[1], 10.0);
        assert_eq!(list_items(&lines, &[]), vec![(0, 1), (2, 2)]);
    }

    #[test]
    fn the_last_item_stops_where_the_list_ends() {
        // The whole point: prose returning to the markers' own margin is not
        // part of the final item.
        let mut lines = vec![
            text_line_at(20.0, "\u{2022} the first file"),
            text_line_at(32.0, "\u{2022} the second file"),
            text_line_at(44.0, "wrapped onto a second line"),
            text_line_at(56.0, "Each of them is regenerated in turn."),
        ];
        indent(&mut lines[2], 10.0);
        assert_eq!(list_items(&lines, &[]), vec![(0, 0), (1, 2)]);
    }

    #[test]
    fn the_last_item_stops_at_a_gap_the_height_guard_would_have_missed() {
        // A hanging-indent list, as in a real repro: body text (including a
        // new paragraph's own first line) sits to the right of the markers,
        // so the indent guard alone can never end the last item there -- see
        // "A list item always starts a sentence" in the dev log for the
        // same shape. Both items wrap with a tight ~4pt continuation gap;
        // the prose that follows the list sits at that same indent, with an
        // 8pt gap. Calibration against the observed ~4pt continuation
        // (× LIST_GAP_CALIBRATION_SLACK → 6pt) stops it; so does the
        // paragraph-matched LIST_GAP_FACTOR * median height (also 6pt here).
        let mut lines = vec![
            text_line_at(20.0, "\u{2022} the first file"),
            text_line_at(32.0, "wrapped onto a second line"),
            text_line_at(44.0, "\u{2022} the second file"),
            text_line_at(56.0, "wrapped onto a second line too"),
            text_line_at(72.0, "Each of them is regenerated in turn."),
        ];
        indent(&mut lines[1], 10.0);
        indent(&mut lines[3], 10.0);
        indent(&mut lines[4], 10.0);
        assert_eq!(list_items(&lines, &[]), vec![(0, 1), (2, 3)]);
    }

    #[test]
    fn the_last_item_stops_at_a_paragraph_gap_without_wrap_peers() {
        // The ENDFtk shape: a hanging-indent list of single-line items (no
        // wrap peers to calibrate against), then prose whose first line sits
        // to the right of the markers with a modest inter-paragraph gap.
        // Height is 8pt; gap is 10pt — under the old 1.5× previous-line
        // threshold (12pt) that used to swallow the opener, over the
        // paragraph-matched 0.75× median (6pt).
        let mut lines = vec![
            text_line_at(20.0, "\u{2022} git"),
            text_line_at(32.0, "\u{2022} CMake 3.15 or higher"),
            text_line_at(44.0, "\u{2022} Python 3.5 or higher"),
            text_line_at(62.0, "The interface is available as a header-only library."),
            text_line_at(74.0, "It does not require compilation."),
        ];
        indent(&mut lines[3], 10.0);
        // Second line of the following paragraph returns to the body margin,
        // mirroring the real layout where only the opener hangs.
        assert_eq!(list_items(&lines, &[]), vec![(0, 0), (1, 1), (2, 2)]);
    }

    #[test]
    fn a_later_list_is_not_poisoned_by_an_earlier_lists_gap() {
        // Two hanging-indent lists on one page, same marker column. The first
        // list's items do not wrap; under the old loose gap guard its last
        // item would absorb the prose opener between the lists, and that
        // wrong ~10pt gap would then calibrate the *second* list's last item
        // so loosely it absorbed *its* following prose too. With the
        // paragraph-matched threshold neither absorption happens, and each
        // list ends where its paragraph does.
        let mut lines = vec![
            text_line_at(20.0, "\u{2022} git"),
            text_line_at(32.0, "\u{2022} CMake"),
            text_line_at(50.0, "Prose between the two lists starts here."),
            text_line_at(62.0, "And continues on the body margin."),
            text_line_at(80.0, "\u{2022} -Dpython=OFF turns the binding off"),
            text_line_at(92.0, "and wraps onto a second line"),
            text_line_at(104.0, "\u{2022} -Dtests=ON turns the tests on"),
            text_line_at(122.0, "Prose after the second list starts here."),
            text_line_at(134.0, "And continues on the body margin."),
        ];
        indent(&mut lines[2], 10.0);
        indent(&mut lines[5], 10.0);
        indent(&mut lines[7], 10.0);
        assert_eq!(
            list_items(&lines, &[]),
            vec![(0, 0), (1, 1), (4, 5), (6, 6)]
        );
    }

    #[test]
    fn a_flush_list_yields_one_line_items() {
        // Nothing is indented past the marker, so nothing can be shown to
        // belong to the item. Under-reaching is the safe direction.
        let lines = vec![
            text_line_at(20.0, "\u{2022} the first file"),
            text_line_at(40.0, "wrapped onto a second line"),
            text_line_at(60.0, "\u{2022} the second file"),
        ];
        assert_eq!(list_items(&lines, &[]), vec![(0, 0), (2, 2)]);
    }

    #[test]
    fn a_wide_gap_ends_an_item() {
        let mut lines = vec![
            text_line_at(20.0, "\u{2022} the first file"),
            text_line_at(200.0, "a distant indented block"),
            text_line_at(220.0, "\u{2022} the second file"),
        ];
        indent(&mut lines[1], 10.0);
        assert_eq!(list_items(&lines, &[]), vec![(0, 0), (2, 2)]);
    }

    #[test]
    fn an_item_never_crosses_a_column_break() {
        // The top of the next column is trivially "indented past" a marker in
        // the left one. Without the y-reset guard the last item of column one
        // swallows the head of column two.
        let mut lines = vec![
            text_line_at(700.0, "\u{2022} the first file"),
            text_line_at(720.0, "\u{2022} the second file"),
            text_line_at(60.0, "the next column starts up here"),
        ];
        indent(&mut lines[2], 200.0);
        assert_eq!(list_items(&lines, &[]), vec![(0, 0), (1, 1)]);
    }

    #[test]
    fn an_item_stops_at_a_table_or_an_image() {
        let mut lines = vec![
            text_line_at(20.0, "\u{2022} the first file"),
            text_line_at(32.0, "a figure caption below it"),
            text_line_at(44.0, "\u{2022} the second file"),
        ];
        indent(&mut lines[1], 10.0);
        let table = blocking(&lines, ObjectKind::Table, 1, 1);
        // Without the table the caption would be part of the item.
        assert_eq!(list_items(&lines, &[]), vec![(0, 1), (2, 2)]);
        assert_eq!(list_items(&lines, &[table]), vec![(0, 0), (2, 2)]);
    }

    #[test]
    fn a_marker_inside_a_table_is_not_an_item() {
        let lines = vec![
            text_line_at(20.0, "\u{2022} a bulleted table cell"),
            text_line_at(40.0, "\u{2022} another table cell"),
            text_line_at(60.0, "ordinary prose"),
        ];
        let table = blocking(&lines, ObjectKind::Table, 0, 1);
        assert_eq!(list_items(&lines, &[table]), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn an_item_never_swallows_more_than_the_line_cap() {
        let mut lines = vec![text_line_at(20.0, "\u{2022} the first file")];
        for i in 1..14 {
            let mut line = text_line_at(20.0 + i as f32 * 12.0, "an indented line");
            indent(&mut line, 10.0);
            lines.push(line);
        }
        lines.push(text_line_at(200.0, "\u{2022} the second file"));
        let items = list_items(&lines, &[]);
        assert_eq!(items[0], (0, LIST_ITEM_MAX_LINES));
    }

    #[test]
    fn a_nested_marker_ends_the_outer_item() {
        let mut lines = vec![
            text_line_at(20.0, "\u{2022} outer item"),
            text_line_at(40.0, "\u{2022} nested one"),
            text_line_at(60.0, "\u{2022} nested two"),
            text_line_at(80.0, "\u{2022} outer again"),
        ];
        indent(&mut lines[1], 40.0);
        indent(&mut lines[2], 40.0);
        // Two lists: the outer pair and the nested pair. The outer item stops
        // at the nested marker rather than swallowing the sub-list.
        assert_eq!(
            list_items(&lines, &[]),
            vec![(0, 0), (1, 1), (2, 2), (3, 3)]
        );
    }

    #[test]
    fn a_numbered_heading_is_not_a_list_item() {
        // `2.1. Directory layout` is indistinguishable from an enumerated item
        // by shape, so known headings are excluded outright.
        let lines = vec![
            text_line_at(20.0, "1. Introduction"),
            text_line_at(40.0, "Body prose here"),
            text_line_at(60.0, "2. Unified system"),
        ];
        assert_eq!(
            list_items(
                &lines,
                &[
                    blocking(&lines, ObjectKind::Heading, 0, 0),
                    blocking(&lines, ObjectKind::Heading, 2, 2),
                ]
            ),
            Vec::<(usize, usize)>::new()
        );
        // Without the heading knowledge they would both look like items.
        assert_eq!(list_items(&lines, &[]), vec![(0, 0), (2, 2)]);
    }

    #[test]
    fn the_object_kind_matrix_is_what_it_claims() {
        // (kind, atomic, block, one sentence, splits paragraphs)
        let matrix = [
            (ObjectKind::Image, true, true, false, true),
            (ObjectKind::Table, false, true, false, true),
            (ObjectKind::Caption, false, true, false, true),
            (ObjectKind::Equation, false, true, true, true),
            (ObjectKind::Code, false, true, false, true),
            (ObjectKind::Heading, false, false, true, true),
            (ObjectKind::ListItem, false, false, false, false),
            (ObjectKind::Footnote, false, true, false, true),
        ];
        for (kind, atomic, block, one_sentence, splits) in matrix {
            assert_eq!(kind.is_atomic(), atomic, "{kind:?}: is_atomic");
            assert_eq!(kind.is_block(), block, "{kind:?}: is_block");
            assert_eq!(
                kind.is_one_sentence(),
                one_sentence,
                "{kind:?}: is_one_sentence"
            );
            assert_eq!(
                kind.splits_paragraphs(),
                splits,
                "{kind:?}: splits_paragraphs"
            );
        }
        // The categories nest, coarsest last: anything that is one unit from
        // word scope up is necessarily one from line scope up too.
        for (kind, ..) in matrix {
            assert!(
                !kind.is_atomic() || kind.is_block(),
                "{kind:?} breaks nesting"
            );
        }
    }

    #[test]
    fn page_content_reports_a_list_item_per_bullet() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_list()).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let items: Vec<&ContentObject> = content
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::ListItem)
            .collect();
        assert_eq!(items.len(), 3, "objects: {:?}", content.objects);
        for item in &items {
            assert!(line_text(&content.lines[item.start_line])
                .trim_start()
                .starts_with('\u{2022}'));
        }
        // The closing prose returns to the markers' margin, so the final item
        // must stop before it.
        let last = items.last().unwrap();
        let closing = content.lines.len() - 1;
        assert!(
            last.end_line < closing,
            "the last item swallowed the closing prose"
        );
        assert!(line_text(&content.lines[closing]).contains("regenerated"));
    }

    // ---- Page furniture --------------------------------------------------

    fn dirs_at(degrees: f32, n: usize) -> Vec<(f32, f32)> {
        let r = degrees.to_radians();
        vec![(r.cos(), r.sin()); n]
    }

    #[test]
    fn line_angle_of_horizontal_text_is_zero() {
        assert_eq!(line_angle(&dirs_at(0.0, 5)), Some(0.0));
    }

    #[test]
    fn line_angle_of_a_rotated_run() {
        let a = line_angle(&dirs_at(-90.0, 4)).unwrap();
        assert!((a + 90.0).abs() < 0.01, "got {a}");
    }

    #[test]
    fn line_angle_survives_the_wraparound() {
        // +179 and -179 are the same physical direction; a bucketed vote would
        // average them to 0, which points the opposite way.
        let mut dirs = dirs_at(179.0, 3);
        dirs.extend(dirs_at(-179.0, 3));
        let a = line_angle(&dirs).unwrap();
        assert!(a.abs() > 170.0, "got {a}");
    }

    #[test]
    fn line_angle_is_none_without_usable_directions() {
        assert_eq!(line_angle(&[]), None);
        // Characters pointing opposite ways cancel: no direction to report.
        let mut dirs = dirs_at(0.0, 3);
        dirs.extend(dirs_at(180.0, 3));
        assert_eq!(line_angle(&dirs), None);
    }

    #[test]
    fn dominant_angle_is_the_character_weighted_majority() {
        let angles = [(Some(0.0), 300), (Some(-90.0), 20), (Some(-45.0), 10)];
        assert_eq!(dominant_angle(&angles), Some(0.0));
    }

    #[test]
    fn dominant_angle_of_a_sideways_page_is_the_rotation() {
        let angles = [(Some(-90.0), 200), (Some(-90.0), 180)];
        assert_eq!(dominant_angle(&angles), Some(-90.0));
    }

    #[test]
    fn dominant_angle_flags_nothing_on_a_near_tie() {
        // A sparse page whose rotated stamp just out-weighs its prose: no
        // direction clears the floor, so neither is treated as marginal.
        let angles = [(Some(-90.0), 105), (Some(0.0), 100)];
        assert_eq!(dominant_angle(&angles), None);
    }

    #[test]
    fn dominant_angle_is_none_when_no_direction_carries_the_page() {
        let angles = [(Some(0.0), 100), (Some(-90.0), 100)];
        assert_eq!(dominant_angle(&angles), None);
    }

    #[test]
    fn normalise_masks_digit_runs_and_folds_case() {
        assert_eq!(normalise_furniture_text("Page 12 of 340"), "page # of #");
        assert_eq!(
            normalise_furniture_text("Nucl. Fusion 66 (2026) 086003 (20pp)"),
            normalise_furniture_text("Nucl. Fusion 66 (2026) 086003"),
            "trailing page-count tags must not split a running head key"
        );
        assert!(furniture_text_matches(
            &normalise_furniture_text("Nucl. Fusion 66 (2026) 086003"),
            &normalise_furniture_text("Nucl. Fusion 66 (2026) 086003 Extra"),
        ));
        assert_eq!(
            normalise_furniture_text("Shared MIME-info Database"),
            "shared mimeinfo database"
        );
        // Decoration around a folio must not stop it matching a bare one.
        assert_eq!(
            normalise_furniture_text("— 12 —"),
            normalise_furniture_text("12")
        );
    }

    #[test]
    fn normalise_of_a_decorative_rule_is_empty() {
        // An empty key would collide with every other decorative line, so
        // such lines are excluded from the vote entirely.
        assert_eq!(normalise_furniture_text("......."), "");
        assert_eq!(normalise_furniture_text("---"), "");
    }

    #[test]
    fn text_segments_keeps_ordinary_word_spacing_together() {
        let chars = [(100.0, 'a'), (106.0, 'b'), (115.0, 'c')];
        assert_eq!(text_segments(chars), vec!["abc".to_owned()]);
    }

    #[test]
    fn text_segments_splits_on_a_wide_gap() {
        // A running head and a folio sharing one baseline, far enough apart
        // that no word gap could explain the distance.
        let chars = [
            (72.0, 'A'),
            (78.0, 'B'),
            (84.0, 'C'),
            (400.0, 'X'),
            (406.0, 'Y'),
        ];
        assert_eq!(
            text_segments(chars),
            vec!["ABC".to_owned(), "XY".to_owned()]
        );
    }

    #[test]
    fn text_segments_of_nothing_is_empty() {
        assert!(text_segments(std::iter::empty::<(f32, char)>()).is_empty());
    }

    fn band(text: &str, edge: Edge, offset: f32) -> BandLine {
        BandLine {
            text: text.to_owned(),
            edge,
            offset,
        }
    }

    #[test]
    fn profile_learns_a_line_that_repeats_across_pages() {
        let samples: Vec<Vec<BandLine>> = (0..6)
            .map(|_| vec![band("shared mimeinfo database", Edge::Top, 42.0)])
            .collect();
        let profile = build_profile(&samples);
        assert_eq!(profile.entries.len(), 1);
        assert_eq!(profile.pages_sampled(), 6);
    }

    #[test]
    fn profile_ignores_a_line_seen_once() {
        let mut samples: Vec<Vec<BandLine>> = (0..6).map(|_| Vec::new()).collect();
        samples[0].push(band("a paper title", Edge::Top, 90.0));
        assert!(build_profile(&samples).is_empty());
    }

    #[test]
    fn profile_ignores_a_line_below_the_share_threshold() {
        // Two of eight is 25%: under the floor, so a coincidence rather than
        // a running element.
        let mut samples: Vec<Vec<BandLine>> = (0..8).map(|_| Vec::new()).collect();
        samples[0].push(band("#", Edge::Bottom, 104.0));
        samples[1].push(band("#", Edge::Bottom, 104.0));
        assert!(build_profile(&samples).is_empty());
    }

    #[test]
    fn profile_tolerates_baseline_jitter_but_not_a_different_height() {
        let jitter: Vec<Vec<BandLine>> = (0..4)
            .map(|i| vec![band("running head", Edge::Top, 42.0 + i as f32 * 0.5)])
            .collect();
        assert_eq!(build_profile(&jitter).entries.len(), 1);

        let moved: Vec<Vec<BandLine>> = (0..4)
            .map(|i| vec![band("running head", Edge::Top, 42.0 + i as f32 * 20.0)])
            .collect();
        assert!(build_profile(&moved).is_empty());
    }

    #[test]
    fn profile_keeps_the_two_edges_apart() {
        let samples: Vec<Vec<BandLine>> = (0..4)
            .map(|_| vec![band("#", Edge::Top, 42.0), band("#", Edge::Bottom, 42.0)])
            .collect();
        let profile = build_profile(&samples);
        assert_eq!(profile.entries.len(), 2);
    }

    /// A page of `count` lines, evenly spaced down `height`, all upright.
    fn page_lines(count: usize, height: f32) -> (Vec<ContentLine>, Vec<LineStyle>) {
        let mut lines = Vec::new();
        let mut styles = Vec::new();
        for i in 0..count {
            let y = height * (i as f32 + 0.5) / count as f32;
            lines.push(text_line_at(y, "body text here"));
            styles.push(LineStyle {
                size: 10.0,
                bold: false,
                math: 0.0,
                mono: 0.0,
                angle: Some(0.0),
                baseline: y,
            });
        }
        (lines, styles)
    }

    fn text_line_at(y: f32, text: &str) -> ContentLine {
        let cells: Vec<Cell> = text
            .chars()
            .enumerate()
            .map(|(i, ch)| Cell {
                kind: CellKind::Char(ch),
                bbox: Rect {
                    x0: 100.0 + i as f32 * 6.0,
                    y0: y - 8.0,
                    x1: 106.0 + i as f32 * 6.0,
                    y1: y,
                },
                synthetic: false,
            })
            .collect();
        ContentLine {
            bbox: Rect {
                x0: 100.0,
                y0: y - 8.0,
                x1: 100.0 + text.chars().count() as f32 * 6.0,
                y1: y,
            },
            cells,
        }
    }

    fn profile_of(entries: &[(&str, Edge, f32)]) -> FurnitureProfile {
        FurnitureProfile {
            entries: entries
                .iter()
                .map(|(t, e, o)| FurnitureEntry {
                    text: (*t).to_owned(),
                    edge: *e,
                    offset: *o,
                    pages: 4,
                })
                .collect(),
            pages_sampled: 8,
        }
    }

    #[test]
    fn mask_removes_a_repeated_running_head() {
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(10, height);
        lines[0] = text_line_at(42.0, "shared mime info database");
        styles[0].baseline = 42.0;
        let profile = profile_of(&[("shared mime info database", Edge::Top, 42.0)]);
        let mask = furniture_mask(&lines, &styles, height, Some(&profile));
        assert!(mask[0]);
        assert!(mask[1..].iter().all(|m| !m));
    }

    #[test]
    fn mask_removes_a_running_head_whose_baseline_drifted_off_profile() {
        // The profile learned this header at baseline 42.0 from other pages,
        // but this page's own content nudged it to 47.0 -- a 5pt drift, past
        // BASELINE_TOLERANCE (2.5) but inside RELAXED_BASELINE_TOLERANCE
        // (8.0), and nowhere near leaving the margin band itself.
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(10, height);
        lines[0] = text_line_at(47.0, "shared mime info database");
        styles[0].baseline = 47.0;
        let profile = profile_of(&[("shared mime info database", Edge::Top, 42.0)]);
        let mask = furniture_mask(&lines, &styles, height, Some(&profile));
        assert!(
            mask[0],
            "a few points of baseline drift must not defeat the match"
        );
        assert!(mask[1..].iter().all(|m| !m));
    }

    #[test]
    fn mask_still_ignores_text_whose_offset_drifted_past_the_relaxed_tolerance() {
        // Same profile, but this page's line drifted 20pt from it -- still
        // comfortably inside the margin band, but past even the relaxed
        // window. The relaxed fallback must stay bounded, not accept any
        // in-band drift.
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(10, height);
        lines[0] = text_line_at(62.0, "shared mime info database");
        styles[0].baseline = 62.0;
        let profile = profile_of(&[("shared mime info database", Edge::Top, 42.0)]);
        let mask = furniture_mask(&lines, &styles, height, Some(&profile));
        assert!(
            mask.iter().all(|m| !m),
            "drift beyond the relaxed tolerance must not match: {mask:?}"
        );
    }

    #[test]
    fn a_folio_just_above_the_strict_band_is_still_in_the_folio_band() {
        // ENDFtk's folio sits near 706pt on an 842pt page — outside the 15%
        // band (threshold ~716) but inside the 20% folio band (threshold ~674).
        let height = 842.0;
        let y = 706.0;
        assert!(
            band_of_with_folio(y, height, false).is_none(),
            "ordinary text at this height is not margin furniture"
        );
        assert_eq!(
            band_of_with_folio(y, height, true).map(|(edge, _)| edge),
            Some(Edge::Bottom)
        );
    }

    #[test]
    fn mask_removes_a_folio_just_above_the_strict_bottom_band() {
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(10, height);
        let y = 706.0;
        lines[9] = text_line_at(y, "12");
        styles[9].baseline = y;
        let profile = profile_of(&[("#", Edge::Bottom, height - y)]);
        let mask = furniture_mask(&lines, &styles, height, Some(&profile));
        assert!(
            mask[9],
            "folio in the deeper band must be furniture: {mask:?}"
        );
        assert!(
            mask[..9].iter().all(|m| !m),
            "body lines must stay: {mask:?}"
        );
    }

    #[test]
    fn mask_does_not_use_the_deeper_band_for_non_folio_text() {
        // The deeper band is folio-only: body text at the same height that
        // happens to match a profile entry must not vanish.
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(10, height);
        let y = 706.0;
        lines[9] = text_line_at(y, "closing remark near the foot");
        styles[9].baseline = y;
        let profile = profile_of(&[("closing remark near the foot", Edge::Bottom, height - y)]);
        assert!(
            furniture_mask(&lines, &styles, height, Some(&profile))
                .iter()
                .all(|m| !m),
            "non-folio text outside the strict band stays navigable"
        );
    }

    #[test]
    fn build_profile_learns_folios_from_the_deeper_band() {
        // Sampling must see the same deeper-band folios the mask will match,
        // or the profile stays empty and nothing is removed.
        let height = 842.0;
        let y = 706.0;
        let folio = BandLine {
            text: "#".into(),
            edge: Edge::Bottom,
            offset: height - y,
        };
        let samples: Vec<Vec<BandLine>> = (0..8).map(|_| vec![folio.clone()]).collect();
        let profile = build_profile(&samples);
        assert!(
            !profile.is_empty(),
            "a folio recurring in the deeper band must enter the profile"
        );
        assert!(profile
            .entries
            .iter()
            .any(|e| e.text == "#" && e.edge == Edge::Bottom));
    }

    #[test]
    fn mask_keeps_a_band_line_that_does_not_repeat() {
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(10, height);
        lines[0] = text_line_at(42.0, "a one off paper title");
        styles[0].baseline = 42.0;
        let profile = profile_of(&[("shared mime info database", Edge::Top, 42.0)]);
        assert!(furniture_mask(&lines, &styles, height, Some(&profile))
            .iter()
            .all(|m| !m));
    }

    #[test]
    fn mask_keeps_a_repeated_line_outside_the_bands() {
        // The bands are what stop a repeated phrase in body text vanishing.
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(10, height);
        let middle = height / 2.0;
        lines[5] = text_line_at(middle, "shared mime info database");
        styles[5].baseline = middle;
        let profile = profile_of(&[("shared mime info database", Edge::Top, middle)]);
        assert!(furniture_mask(&lines, &styles, height, Some(&profile))
            .iter()
            .all(|m| !m));
    }

    #[test]
    fn mask_removes_a_rotated_stamp_without_any_profile() {
        let height = 842.0;
        let (lines, mut styles) = page_lines(10, height);
        styles[3].angle = Some(-90.0);
        let mask = furniture_mask(&lines, &styles, height, None);
        assert!(mask[3]);
        assert_eq!(mask.iter().filter(|m| **m).count(), 1);
    }

    #[test]
    fn mask_keeps_everything_on_a_sideways_page() {
        // The rotated direction is the reading direction here, so it is the
        // dominant one and nothing is marginal.
        let height = 842.0;
        let (lines, mut styles) = page_lines(10, height);
        for s in styles.iter_mut() {
            s.angle = Some(-90.0);
        }
        assert!(furniture_mask(&lines, &styles, height, None)
            .iter()
            .all(|m| !m));
    }

    #[test]
    fn mask_caps_the_repetition_rule_per_edge() {
        // Four "running heads" stacked in one band is not a running head.
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(20, height);
        let mut entries = Vec::new();
        for (i, line) in lines.iter_mut().enumerate().take(4) {
            let y = 20.0 + i as f32 * 8.0;
            *line = text_line_at(y, "running head");
            styles[i].baseline = y;
            entries.push(("running head", Edge::Top, y));
        }
        let profile = profile_of(&entries);
        assert!(furniture_mask(&lines, &styles, height, Some(&profile))
            .iter()
            .all(|m| !m));
    }

    #[test]
    fn mask_caps_the_repetition_rule_per_text_not_per_edge() {
        // A bare folio digit ("#" once normalised) is generic enough that an
        // unrelated numbered code listing elsewhere in the document can
        // coincidentally recur across sampled pages too, entering the
        // profile as several more "#" entries at various bottom-band
        // offsets alongside the real folio. On a page whose own numbered
        // content matches several of those, the flood must not take down a
        // completely different, individually credible match sharing the
        // bottom -- or, as here, the top -- edge: the cap is scoped to the
        // specific text that flooded, not the edge as a whole.
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(24, height);
        lines[0] = text_line_at(20.0, "shared mime info database");
        styles[0].baseline = 20.0;
        let mut entries = vec![("shared mime info database", Edge::Top, 20.0)];
        for (i, y) in [800.0, 808.0, 816.0, 824.0].into_iter().enumerate() {
            lines[i + 1] = text_line_at(y, "1");
            styles[i + 1].baseline = y;
            entries.push(("#", Edge::Bottom, height - y));
        }
        let profile = profile_of(&entries);
        let mask = furniture_mask(&lines, &styles, height, Some(&profile));
        assert!(
            mask[0],
            "the running head must survive an unrelated flood on another edge: {mask:?}"
        );
        assert!(
            mask[1..5].iter().all(|m| !m),
            "the flooding text itself must still be capped: {mask:?}"
        );
    }

    #[test]
    fn mask_caps_the_repetition_rule_by_share() {
        // On a page with enough lines to judge, a quarter of them being
        // furniture means the evidence is not credible.
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(12, height);
        let mut entries = Vec::new();
        for (i, offset) in [20.0f32, 30.0, 812.0, 822.0].into_iter().enumerate() {
            lines[i] = text_line_at(offset, "running head");
            styles[i].baseline = offset;
            let (edge, off) = band_of_with_folio(offset, height, false).unwrap();
            entries.push(("running head", edge, off));
        }
        let profile = profile_of(&entries);
        assert!(furniture_mask(&lines, &styles, height, Some(&profile))
            .iter()
            .all(|m| !m));
    }

    #[test]
    fn repetition_never_takes_a_pages_last_reachable_line() {
        // Body is rotated off the dominant angle (and short, so it loses the
        // character-count vote) and is masked first. Both margin lines match
        // the profile; the per-line guard must leave exactly one of them
        // reachable so the page is not emptied.
        let height = 842.0;
        let lines = vec![
            text_line_at(42.0, "running head text"),
            text_line_at(100.0, "x"),
            text_line_at(800.0, "12"),
        ];
        let style_at = |baseline: f32, angle: Option<f32>| LineStyle {
            size: 9.0,
            bold: false,
            math: 0.0,
            mono: 0.0,
            angle,
            baseline,
        };
        let styles = vec![
            style_at(42.0, Some(0.0)),
            style_at(100.0, Some(90.0)),
            style_at(800.0, Some(0.0)),
        ];
        let profile = profile_of(&[
            ("running head text", Edge::Top, 42.0),
            ("#", Edge::Bottom, 42.0),
        ]);
        let mask = furniture_mask(&lines, &styles, height, Some(&profile));
        assert!(mask[1], "rotated body is furniture: {mask:?}");
        let reachable = mask.iter().filter(|m| !*m).count();
        assert_eq!(
            reachable, 1,
            "exactly one margin match must stay reachable: {mask:?}"
        );
    }

    #[test]
    fn repetition_still_masks_a_folio_under_body_content() {
        // The last-reachable guard must not spare a bottom folio when body
        // lines above it keep the page non-empty.
        let height = 842.0;
        let lines = vec![
            text_line_at(100.0, "body one of this page"),
            text_line_at(112.0, "body two of this page"),
            text_line_at(124.0, "body three of this page"),
            text_line_at(800.0, "12"),
        ];
        let style_at = |baseline: f32| LineStyle {
            size: 9.0,
            bold: false,
            math: 0.0,
            mono: 0.0,
            angle: Some(0.0),
            baseline,
        };
        let styles = vec![
            style_at(100.0),
            style_at(112.0),
            style_at(124.0),
            style_at(800.0),
        ];
        let profile = profile_of(&[("#", Edge::Bottom, 42.0)]);
        let mask = furniture_mask(&lines, &styles, height, Some(&profile));
        assert!(
            mask[3],
            "folio under body must still be furniture: {mask:?}"
        );
        assert!(mask.iter().take(3).all(|m| !m), "body stays free: {mask:?}");
    }

    /// A line at a given left edge, otherwise like [`text_line_at`].
    fn text_line_at_x(x0: f32, y: f32, text: &str) -> ContentLine {
        let cells: Vec<Cell> = text
            .chars()
            .enumerate()
            .map(|(i, ch)| Cell {
                kind: CellKind::Char(ch),
                bbox: Rect {
                    x0: x0 + i as f32 * 6.0,
                    y0: y - 8.0,
                    x1: x0 + 6.0 + i as f32 * 6.0,
                    y1: y,
                },
                synthetic: false,
            })
            .collect();
        ContentLine {
            bbox: Rect {
                x0,
                y0: y - 8.0,
                x1: x0 + text.chars().count() as f32 * 6.0,
                y1: y,
            },
            cells,
        }
    }

    #[test]
    fn line_number_mask_flags_a_narrow_numeric_margin_column() {
        let mut lines = Vec::new();
        for i in 1..=5 {
            let y = 100.0 + i as f32 * 20.0;
            lines.push(text_line_at_x(40.0, y, &i.to_string()));
            lines.push(text_line_at_x(100.0, y, "a body line right here"));
        }
        let mask = line_number_mask(&lines);
        for (i, line) in lines.iter().enumerate() {
            let is_number_line = i % 2 == 0;
            assert_eq!(mask[i], is_number_line, "line {i}: {:?}", line_text(line));
        }
    }

    #[test]
    fn line_number_mask_ignores_a_lone_stray_number() {
        // Below MIN_LINE_NUMBERS: a single number could be a footnote marker
        // or an equation number, not a margin column.
        let mut lines = vec![text_line_at_x(40.0, 120.0, "1")];
        for i in 0..5 {
            lines.push(text_line_at_x(100.0, 140.0 + i as f32 * 16.0, "body text"));
        }
        assert!(line_number_mask(&lines).iter().all(|&m| !m));
    }

    #[test]
    fn line_number_mask_leaves_an_all_numeric_page_alone() {
        // No body line means no left edge to measure the gap against, so a
        // genuinely numeric page (a table of figures) is left alone rather
        // than guessed at.
        let lines: Vec<ContentLine> = (1..=6)
            .map(|i| text_line_at_x(40.0, 100.0 + i as f32 * 20.0, &i.to_string()))
            .collect();
        assert!(line_number_mask(&lines).iter().all(|&m| !m));
    }

    #[test]
    fn line_number_mask_requires_a_real_gap_from_the_body() {
        // A number sitting inside the body's own left margin (no gutter) is
        // not a separate column -- it could be a numbered list.
        let mut lines = Vec::new();
        for i in 1..=5 {
            let y = 100.0 + i as f32 * 20.0;
            lines.push(text_line_at_x(100.0, y, &i.to_string()));
            lines.push(text_line_at_x(100.0, y + 8.0, "body text here"));
        }
        assert!(line_number_mask(&lines).iter().all(|&m| !m));
    }

    #[test]
    fn image_line_indices_survive_furniture_removal() {
        // Regression: image indices point into the unfiltered vector. Dropping
        // a line without remapping them compiles fine and silently labels a
        // line of text as an image.
        let lines = [
            text_line_at(20.0, "body one"),
            text_line_at(40.0, "body two"),
            text_line_at(60.0, "body three"),
        ];
        // Pretend line 0 was furniture: the image at old index 2 must land at 1.
        let kept: Vec<ContentLine> = lines[1..].to_vec();
        let objects = content_objects(&kept, &[1], &[], &[], &[], &[], &[], &[], &lines[..1]);
        let image = objects
            .iter()
            .find(|o| o.kind == ObjectKind::Image)
            .expect("image object");
        assert_eq!((image.start_line, image.end_line), (1, 1));
    }

    #[test]
    fn a_table_covering_every_body_line_survives_a_page_that_had_furniture() {
        // Regression: the "claims the whole page" guard must count furniture
        // too, or removing a running head makes a full-page table trip it.
        let all = stacked_lines(12);
        let body: Vec<ContentLine> = all[..10].to_vec();
        let furniture: Vec<ContentLine> = all[10..].to_vec();
        let table = box_over(&body, 0..=9);
        let objects = content_objects(&body, &[], &[table], &[], &[], &[], &[], &[], &furniture);
        assert_eq!(
            objects
                .iter()
                .filter(|o| o.kind == ObjectKind::Table)
                .count(),
            1,
            "the table was discarded: {objects:?}"
        );
    }

    #[test]
    fn page_content_skips_a_running_header_and_a_folio() {
        let doc =
            Document::from_bytes(&crate::test_support::pdf_with_running_header(6, true)).unwrap();
        let profile = doc.furniture_profile().unwrap();
        assert!(!profile.is_empty(), "nothing repeated: {profile:?}");
        let content = doc
            .page_content(2, ContentOptions::default(), Some(&profile))
            .unwrap();
        let body: String = content.lines.iter().map(line_text).collect();
        assert!(
            !body.contains("Shared MIME-info Database"),
            "header survived"
        );
        assert!(body.contains("The database is a set"), "body was removed");
        let removed: String = content.furniture.iter().map(line_text).collect();
        assert!(removed.contains("Shared MIME-info Database"));
        assert!(removed.contains('3'), "folio not removed: {removed:?}");
    }

    #[test]
    fn page_content_keeps_a_top_line_that_differs_on_every_page() {
        let doc =
            Document::from_bytes(&crate::test_support::pdf_with_running_header(6, false)).unwrap();
        let profile = doc.furniture_profile().unwrap();
        let content = doc
            .page_content(2, ContentOptions::default(), Some(&profile))
            .unwrap();
        let body: String = content.lines.iter().map(line_text).collect();
        assert!(
            body.contains("Writing a glob pattern"),
            "a one-off heading was removed: {body:?}"
        );
    }

    #[test]
    fn a_running_head_with_a_counter_in_it_is_still_caught() {
        // Digits are masked before comparison, so "Chapter 1 of 9" and
        // "Chapter 2 of 9" are one running head, not two headings.
        assert_eq!(
            normalise_furniture_text("Chapter 1 of 9"),
            normalise_furniture_text("Chapter 7 of 9")
        );
    }

    #[test]
    fn page_content_keeps_the_furniture_when_the_option_is_off() {
        let doc =
            Document::from_bytes(&crate::test_support::pdf_with_running_header(6, true)).unwrap();
        let profile = doc.furniture_profile().unwrap();
        let content = doc
            .page_content(
                2,
                ContentOptions {
                    skip_page_furniture: false,
                    ..ContentOptions::default()
                },
                Some(&profile),
            )
            .unwrap();
        let body: String = content.lines.iter().map(line_text).collect();
        assert!(body.contains("Shared MIME-info Database"));
        assert!(content.furniture.is_empty());
    }

    #[test]
    fn page_content_skips_rotated_marginal_text() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_rotated_text(false)).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let body: String = content.lines.iter().map(line_text).collect();
        assert!(!body.contains("CONFIDENTIAL"), "side stamp survived");
        assert!(!body.contains("PREPRINT"), "watermark survived");
        assert!(body.contains("The database is a set"), "body was removed");
        assert_eq!(content.furniture.len(), 2);
    }

    #[test]
    fn page_content_keeps_everything_on_a_sideways_page() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_rotated_text(true)).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        assert!(content.furniture.is_empty(), "a sideways page was emptied");
        assert_eq!(content.lines.len(), 6);
    }

    #[test]
    fn furniture_profile_of_a_single_page_document_is_empty() {
        let doc =
            Document::from_bytes(&crate::test_support::pdf_with_pages(&["only page"])).unwrap();
        let profile = doc.furniture_profile().unwrap();
        assert!(profile.is_empty());
        assert_eq!(profile.pages_sampled(), 0);
    }

    #[test]
    fn page_content_skips_a_manuscript_line_number_column() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_line_numbers(3, 6)).unwrap();
        for page in 0..3 {
            let content = doc
                .page_content(page, ContentOptions::default(), None)
                .unwrap();
            let body: String = content.lines.iter().map(line_text).collect();
            assert!(
                !body.chars().any(|c| c.is_ascii_digit()),
                "a line number reached the body text: {body:?}"
            );
            assert!(body.contains("continues here"), "body was removed");
            assert_eq!(
                content.furniture.len(),
                6,
                "all six line numbers should be furniture on page {page}"
            );
        }
    }

    #[test]
    fn page_content_skips_a_running_head_and_folio_that_swap_sides() {
        // A facing-page layout where the two fields on one footer line trade
        // places between recto and verso -- the repetition rule must not
        // care which side either one is on, or which one a page happens to
        // put first.
        let doc = Document::from_bytes(&crate::test_support::pdf_with_alternating_margin_fields(8))
            .unwrap();
        let profile = doc.furniture_profile().unwrap();
        assert!(!profile.is_empty(), "nothing repeated: {profile:?}");
        for page in 0..8 {
            let content = doc
                .page_content(page, ContentOptions::default(), Some(&profile))
                .unwrap();
            let body: String = content.lines.iter().map(line_text).collect();
            assert!(
                !body.contains("AUTHOR SUBMITTED"),
                "header survived on page {page}: {body:?}"
            );
            assert!(
                body.contains("The database is a set"),
                "real body text was removed on page {page}"
            );
            let removed: String = content.furniture.iter().map(line_text).collect();
            assert!(
                removed.contains("AUTHOR SUBMITTED"),
                "header not caught on page {page}"
            );
            assert!(
                removed.contains("Page") && removed.contains("of 8"),
                "folio not caught on page {page}: {removed:?}"
            );
        }
    }

    #[test]
    fn furniture_removal_leaves_the_table_and_heading_fixtures_alone() {
        // Collateral-damage guard for the two shipped features.
        let table = Document::from_bytes(&crate::test_support::pdf_with_table(4, 5)).unwrap();
        let content = table
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        assert_eq!(
            content
                .objects
                .iter()
                .filter(|o| o.kind == ObjectKind::Table)
                .count(),
            1
        );
        let heading = Document::from_bytes(&crate::test_support::pdf_with_heading()).unwrap();
        let content = heading
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        assert_eq!(
            content
                .objects
                .iter()
                .filter(|o| o.kind == ObjectKind::Heading)
                .count(),
            2
        );
    }

    #[test]
    fn page_content_reports_no_table_on_plain_prose() {
        // The failure that hurts users is a false positive, so this is a
        // load-bearing test rather than a nicety.
        let doc = three_page_doc();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        assert!(
            !content.objects.iter().any(|o| o.kind == ObjectKind::Table),
            "prose reported as a table: {:?}",
            content.objects
        );
    }

    #[test]
    fn alignment_table_bboxes_finds_a_borderless_parameter_grid() {
        // Two short columns, three rows — the shape MuPDF's vector hunt misses.
        let lines = vec![
            text_line_at_x(50.0, 100.0, "Parameter"),
            text_line_at_x(250.0, 100.0, "Value"),
            text_line_at_x(50.0, 120.0, "Major radius"),
            text_line_at_x(250.0, 120.0, "5.00 m"),
            text_line_at_x(50.0, 140.0, "Minor radius"),
            text_line_at_x(250.0, 140.0, "2.17 m"),
            text_line_at_x(50.0, 160.0, "Aspect ratio"),
            text_line_at_x(250.0, 160.0, "2.30"),
            // Wide prose below must not join the grid.
            text_line_at_x(
                50.0,
                220.0,
                "These dimensions define the envelope for the core.",
            ),
        ];
        // Stretch the prose line so it fails the cell-width gate.
        let mut lines = lines;
        let last = lines.len() - 1;
        lines[last].bbox.x1 = 500.0;
        let boxes = alignment_table_bboxes(&lines);
        assert_eq!(
            boxes.len(),
            1,
            "expected one borderless table, got {boxes:?}"
        );
        assert!(boxes[0].y1 < 200.0, "prose must stay outside the table");
    }

    #[test]
    fn alignment_table_bboxes_ignore_ordinary_two_column_prose() {
        // Full-width-ish column lines are not cell-like.
        let lines = vec![
            text_line_at_x(
                40.0,
                100.0,
                "Left column prose that fills most of its measure here.",
            ),
            text_line_at_x(
                320.0,
                100.0,
                "Right column prose that fills most of its measure here.",
            ),
            text_line_at_x(
                40.0,
                120.0,
                "More left column prose continuing the paragraph along.",
            ),
            text_line_at_x(
                320.0,
                120.0,
                "More right column prose continuing the paragraph along.",
            ),
            text_line_at_x(
                40.0,
                140.0,
                "Still more left column text for a third aligned row.",
            ),
            text_line_at_x(
                320.0,
                140.0,
                "Still more right column text for a third aligned row.",
            ),
        ];
        let mut lines = lines;
        for line in &mut lines {
            // ~half page each — above ALIGN_TABLE_MAX_WIDTH_SHARE of ~500pt width.
            if line.bbox.x0 < 200.0 {
                line.bbox.x1 = 290.0;
            } else {
                line.bbox.x1 = 550.0;
            }
        }
        assert!(
            alignment_table_bboxes(&lines).is_empty(),
            "two-column prose must not become a table"
        );
    }

    #[test]
    fn alignment_table_bboxes_finds_right_aligned_numeric_columns() {
        // Header left-aligned, values right-aligned under the same centre —
        // common in benchmark tables. Left-edge clustering alone misses this;
        // centre clustering recovers it.
        // Score centre = 200 + 5*6/2 = 215; "80.1" at x0=203 has the same mid.
        let mut lines = vec![
            text_line_at_x(50.0, 100.0, "System"),
            text_line_at_x(200.0, 100.0, "Score"),
            text_line_at_x(350.0, 100.0, "Avg"),
            text_line_at_x(50.0, 120.0, "Baseline"),
            text_line_at_x(203.0, 120.0, "80.1"),
            text_line_at_x(359.0, 120.0, "74"),
            text_line_at_x(50.0, 140.0, "Ours"),
            text_line_at_x(203.0, 140.0, "91.2"),
            text_line_at_x(359.0, 140.0, "88"),
            text_line_at_x(50.0, 160.0, "Prior"),
            text_line_at_x(203.0, 160.0, "85.0"),
            text_line_at_x(359.0, 160.0, "81"),
        ];
        lines.push(text_line_at_x(
            50.0,
            220.0,
            "Prose under the table that should stay outside the detected box.",
        ));
        let last = lines.len() - 1;
        lines[last].bbox.x1 = 520.0;
        let boxes = alignment_table_bboxes(&lines);
        assert_eq!(
            boxes.len(),
            1,
            "expected right-aligned numeric grid, got {boxes:?}"
        );
        assert!(boxes[0].y1 < 200.0, "prose must stay outside the table");
    }

    #[test]
    fn page_content_reports_no_table_on_a_two_column_page() {
        let doc = Document::from_bytes(&crate::test_support::pdf_two_column_page(6)).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        assert!(
            !content.objects.iter().any(|o| o.kind == ObjectKind::Table),
            "columns reported as a table: {:?}",
            content.objects
        );
    }

    #[test]
    fn page_content_reports_an_image_as_a_one_line_object() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_image()).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let images: Vec<_> = content
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Image)
            .collect();
        assert_eq!(images.len(), 1, "objects: {:?}", content.objects);
        assert_eq!(images[0].start_line, images[0].end_line);
        assert!(matches!(
            content.lines[images[0].start_line].cells[0].kind,
            CellKind::Image
        ));
    }

    #[test]
    fn page_content_reports_a_caption_under_an_image() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_image()).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let captions: Vec<_> = content
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Caption)
            .collect();
        assert_eq!(captions.len(), 1, "objects: {:?}", content.objects);
        assert!(line_text(&content.lines[captions[0].start_line]).contains("Fig."));
    }

    #[test]
    fn page_content_reports_a_caption_under_a_table() {
        let doc =
            Document::from_bytes(&crate::test_support::pdf_with_table_gap(4, 5, 8.0)).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let captions: Vec<_> = content
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Caption)
            .collect();
        assert_eq!(captions.len(), 1, "objects: {:?}", content.objects);
        assert!(line_text(&content.lines[captions[0].start_line]).contains("Table 1"));
        // Caption stays outside the table object.
        let table = content
            .objects
            .iter()
            .find(|o| o.kind == ObjectKind::Table)
            .expect("table");
        assert!(captions[0].start_line > table.end_line);
    }

    #[test]
    fn page_content_reports_a_courier_listing_as_code() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_code_block()).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let codes: Vec<_> = content
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Code)
            .collect();
        assert_eq!(codes.len(), 1, "objects: {:?}", content.objects);
        assert!(codes[0].end_line > codes[0].start_line);
        assert!(line_text(&content.lines[codes[0].start_line]).contains("fn main"));
    }

    #[test]
    fn interleaved_two_column_page_keeps_stream_order_but_two_bands() {
        // Characterisation, not a fix: row-major content streams make MuPDF
        // report L1,R1,L2,R2,… so sequential line motion interleaves columns
        // even though the x-bands remain two columns.
        let doc =
            Document::from_bytes(&crate::test_support::pdf_interleaved_two_column_page(3)).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let texts: Vec<String> = content.lines.iter().map(line_text).collect();
        assert_eq!(
            texts,
            vec![
                "C1L1.".to_string(),
                "C2L1.".to_string(),
                "C1L2.".to_string(),
                "C2L2.".to_string(),
                "C1L3.".to_string(),
                "C2L3.".to_string(),
            ],
            "stream order changed: {texts:?}"
        );
        // Left centres near 72+…, right near 340+… — two x-bands.
        let centres: Vec<f32> = content
            .lines
            .iter()
            .map(|l| (l.bbox.x0 + l.bbox.x1) / 2.0)
            .collect();
        let left = centres.iter().filter(|&&c| c < 200.0).count();
        let right = centres.iter().filter(|&&c| c >= 200.0).count();
        assert_eq!((left, right), (3, 3), "centres: {centres:?}");
    }

    #[test]
    fn page_content_detects_a_ruled_grid_as_one_table() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_table(4, 5)).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let tables: Vec<_> = content
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Table)
            .collect();
        assert_eq!(tables.len(), 1, "objects: {:?}", content.objects);
        let table = tables[0];
        // The heading above and the caption below stay outside the table.
        let text_of = |i: usize| -> String {
            content.lines[i]
                .cells
                .iter()
                .filter_map(|c| match c.kind {
                    CellKind::Char(ch) => Some(ch),
                    CellKind::Image => None,
                })
                .collect()
        };
        assert!(table.start_line > 0, "heading swallowed by the table");
        assert!(
            text_of(table.start_line).contains("R1C1"),
            "table starts at {:?}",
            text_of(table.start_line)
        );
        assert!(
            (0..content.lines.len()).any(|i| text_of(i).contains("Caption") && i > table.end_line),
            "caption swallowed by the table"
        );
    }

    #[test]
    fn page_content_leaves_a_caption_set_tight_under_a_table_outside_it() {
        // The reported bug, reproduced against real MuPDF: at this gap its box
        // reaches past the caption's centre, so centre containment handed the
        // caption to the table — `s` skipped it and the highlight covered it.
        // Measured before the fix: box y 141.65..282.35, caption y
        // 275.25..288.99, and the table's range ran to the caption's line.
        let doc =
            Document::from_bytes(&crate::test_support::pdf_with_table_gap(4, 5, 4.0)).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let text_of = |i: usize| -> String {
            content.lines[i]
                .cells
                .iter()
                .filter_map(|c| match c.kind {
                    CellKind::Char(ch) => Some(ch),
                    CellKind::Image => None,
                })
                .collect()
        };
        let caption = (0..content.lines.len())
            .find(|&i| text_of(i).contains("Caption"))
            .expect("caption line");
        let tables: Vec<_> = content
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Table)
            .collect();
        assert_eq!(tables.len(), 1, "objects: {:?}", content.objects);
        let table = tables[0];
        assert!(
            caption > table.end_line,
            "caption (line {caption}) swallowed by the table {table:?}"
        );
        assert!(
            table.bbox.y1 <= content.lines[caption].bbox.y0,
            "the table's box reaches the caption: {:?} vs {:?}",
            table.bbox,
            content.lines[caption].bbox
        );
    }

    #[test]
    fn page_content_without_table_detection_reports_no_tables() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_table(4, 5)).unwrap();
        let content = doc
            .page_content(
                0,
                ContentOptions {
                    detect_tables: false,
                    ..ContentOptions::default()
                },
                None,
            )
            .unwrap();
        assert!(content.objects.iter().all(|o| o.kind != ObjectKind::Table));
    }

    // ---- Highlight annotations ------------------------------------------

    /// The classic highlighter yellow, as the core's default.
    const YELLOW: (u8, u8, u8) = (0xff, 0xe0, 0x66);

    /// A two-page fixture on disk, plus the box of the first page's first word.
    fn highlight_fixture(dir: &Path) -> (PathBuf, Rect) {
        let path = dir.join("src.pdf");
        std::fs::write(
            &path,
            pdf_with_pages(&["Highlight this line", "And this second page"]),
        )
        .unwrap();
        let doc = Document::open(&path).unwrap();
        let content = doc
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let cells = &content.lines[0].cells;
        let first_word: Vec<&Cell> = cells
            .iter()
            .take_while(|c| c.kind != CellKind::Char(' '))
            .collect();
        let bbox = first_word
            .iter()
            .map(|c| c.bbox)
            .reduce(Rect::union)
            .unwrap();
        (path, bbox)
    }

    #[test]
    fn write_highlights_leaves_the_source_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (src, bbox) = highlight_fixture(dir.path());
        let before = std::fs::read(&src).unwrap();
        let out = dir.path().join("out.pdf");
        write_highlights(
            &src,
            &out,
            &[HighlightAnnotation {
                page: 0,
                rects: vec![bbox],
                color: YELLOW,
                opacity: 1.0,
                name: None,
            }],
        )
        .unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), before);
        assert!(std::fs::metadata(&out).unwrap().len() > 0);
    }

    #[test]
    fn written_highlights_read_back_with_their_page_geometry() {
        let dir = tempfile::tempdir().unwrap();
        let (src, bbox) = highlight_fixture(dir.path());
        let out = dir.path().join("out.pdf");
        let second = Rect {
            x0: bbox.x0,
            y0: bbox.y1 + 4.0,
            x1: bbox.x1 + 20.0,
            y1: bbox.y1 + 16.0,
        };
        write_highlights(
            &src,
            &out,
            &[
                HighlightAnnotation {
                    page: 0,
                    rects: vec![bbox, second],
                    color: YELLOW,
                    opacity: 1.0,
                    name: None,
                },
                HighlightAnnotation {
                    page: 1,
                    rects: vec![bbox],
                    color: YELLOW,
                    opacity: 1.0,
                    name: None,
                },
            ],
        )
        .unwrap();

        // One annotation per page, and the round trip through /QuadPoints (which
        // are in bottom-left-origin user space) returns the top-left-origin
        // rectangles that went in.
        let first = page_highlights(&out, 0).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].len(), 2);
        for (got, want) in first[0].iter().zip([bbox, second]) {
            for (got, want) in [
                (got.x0, want.x0),
                (got.y0, want.y0),
                (got.x1, want.x1),
                (got.y1, want.y1),
            ] {
                assert!((got - want).abs() < 0.01, "{got} != {want}");
            }
        }
        assert_eq!(page_highlights(&out, 1).unwrap().len(), 1);
    }

    #[test]
    fn a_named_highlight_is_written_with_its_name_on_every_page() {
        let dir = tempfile::tempdir().unwrap();
        let (src, bbox) = highlight_fixture(dir.path());
        let out = dir.path().join("out.pdf");
        // One highlight running across a page break: two annotations, one name.
        write_highlights(
            &src,
            &out,
            &[
                HighlightAnnotation {
                    page: 0,
                    rects: vec![bbox],
                    color: YELLOW,
                    opacity: 1.0,
                    name: Some("syodep-highlight-7".to_owned()),
                },
                HighlightAnnotation {
                    page: 1,
                    rects: vec![bbox],
                    color: YELLOW,
                    opacity: 1.0,
                    name: Some("syodep-highlight-7".to_owned()),
                },
            ],
        )
        .unwrap();

        assert_eq!(
            page_highlight_names(&out, 0).unwrap(),
            vec![Some("syodep-highlight-7".to_owned())]
        );
        assert_eq!(
            page_highlight_names(&out, 1).unwrap(),
            vec![Some("syodep-highlight-7".to_owned())]
        );
    }

    #[test]
    fn an_unnamed_highlight_stays_anonymous() {
        let dir = tempfile::tempdir().unwrap();
        let (src, bbox) = highlight_fixture(dir.path());
        let out = dir.path().join("out.pdf");
        write_highlights(
            &src,
            &out,
            &[HighlightAnnotation {
                page: 0,
                rects: vec![bbox],
                color: YELLOW,
                opacity: 1.0,
                name: None,
            }],
        )
        .unwrap();
        assert_eq!(page_highlight_names(&out, 0).unwrap(), vec![None]);
    }

    #[test]
    fn removing_a_named_highlight_takes_every_page_of_it_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let (src, bbox) = highlight_fixture(dir.path());
        let annotated = dir.path().join("annotated.pdf");
        let neighbour = Rect {
            x0: bbox.x0,
            y0: bbox.y1 + 4.0,
            x1: bbox.x1,
            y1: bbox.y1 + 16.0,
        };
        write_highlights(
            &src,
            &annotated,
            &[
                HighlightAnnotation {
                    page: 0,
                    rects: vec![bbox],
                    color: YELLOW,
                    opacity: 1.0,
                    name: Some("syodep-highlight-1".to_owned()),
                },
                HighlightAnnotation {
                    page: 0,
                    rects: vec![neighbour],
                    color: YELLOW,
                    opacity: 1.0,
                    name: Some("syodep-highlight-2".to_owned()),
                },
                HighlightAnnotation {
                    page: 1,
                    rects: vec![bbox],
                    color: YELLOW,
                    opacity: 1.0,
                    name: Some("syodep-highlight-1".to_owned()),
                },
            ],
        )
        .unwrap();

        let out = dir.path().join("out.pdf");
        let before = std::fs::read(&annotated).unwrap();
        let removed = remove_highlight_annotation(&annotated, &out, "syodep-highlight-1").unwrap();

        assert_eq!(removed, 2, "both pages of the one highlight");
        assert_eq!(
            std::fs::read(&annotated).unwrap(),
            before,
            "the source is never modified"
        );
        assert_eq!(
            page_highlight_names(&out, 0).unwrap(),
            vec![Some("syodep-highlight-2".to_owned())],
            "the other highlight on the same page survives"
        );
        assert!(page_highlight_names(&out, 1).unwrap().is_empty());
    }

    #[test]
    fn removing_an_unknown_name_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (src, bbox) = highlight_fixture(dir.path());
        let annotated = dir.path().join("annotated.pdf");
        write_highlights(
            &src,
            &annotated,
            &[HighlightAnnotation {
                page: 0,
                rects: vec![bbox],
                color: YELLOW,
                opacity: 1.0,
                name: None,
            }],
        )
        .unwrap();

        let out = dir.path().join("out.pdf");
        assert_eq!(
            remove_highlight_annotation(&annotated, &out, "syodep-highlight-1").unwrap(),
            0
        );
        assert!(!out.exists(), "no match means no output file");
        assert_eq!(page_highlight_names(&annotated, 0).unwrap(), vec![None]);
    }

    /// The `/CA` (constant alpha) of the first `Highlight` annotation on
    /// `page`, read the same way [`page_highlights`] reads `/QuadPoints`.
    ///
    /// Test-only: opacity isn't part of `page_highlights`'s public return
    /// value because nothing outside this module needs it yet.
    fn saved_highlight_opacity(path: &Path, page: usize) -> f32 {
        let path_str = path.to_string_lossy();
        let doc = mupdf::Document::open(path_str.as_ref()).unwrap();
        let pdf = mupdf::pdf::PdfDocument::try_from(doc).unwrap();
        let loaded = pdf.load_page(page as i32).unwrap();
        let pdf_page = mupdf::pdf::PdfPage::try_from(loaded).unwrap();
        let annots = resolved_annots(&pdf_page).unwrap().unwrap();
        for index in 0..annots.len().unwrap_or(0) {
            let dict = annot_dict(&pdf_page, index).unwrap();
            let is_highlight = dict
                .get_dict("Subtype")
                .unwrap()
                .and_then(|s| s.as_name().ok().map(|n| n == b"Highlight"))
                .unwrap_or(false);
            if is_highlight {
                return dict.get_dict("CA").unwrap().unwrap().as_float().unwrap();
            }
        }
        panic!("no highlight annotation on page {page}");
    }

    #[test]
    fn a_highlights_opacity_is_written_as_constant_alpha() {
        // A reader always paints a highlight with Multiply blending, so this
        // is what keeps a saved highlight from looking stronger than the same
        // colour previewed at less than full opacity: the app's opacity has
        // to reach the PDF, not just the colour.
        let dir = tempfile::tempdir().unwrap();
        let (src, bbox) = highlight_fixture(dir.path());
        let out = dir.path().join("out.pdf");
        write_highlights(
            &src,
            &out,
            &[HighlightAnnotation {
                page: 0,
                rects: vec![bbox],
                color: YELLOW,
                opacity: 0.55,
                name: None,
            }],
        )
        .unwrap();
        assert!((saved_highlight_opacity(&out, 0) - 0.55).abs() < 0.01);
    }

    #[test]
    fn a_document_with_no_highlights_still_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let (src, _) = highlight_fixture(dir.path());
        let out = dir.path().join("out.pdf");
        write_highlights(&src, &out, &[]).unwrap();
        assert_eq!(Document::open(&out).unwrap().page_count(), 2);
        assert!(page_highlights(&out, 0).unwrap().is_empty());
    }

    #[test]
    fn embedded_highlights_do_not_reach_the_content_layer() {
        // Text extraction runs `fz_run_page_contents`, which skips annotations.
        // This matters twice over: the caret must not gain stops it cannot see,
        // and the `COLLECT_VECTORS` table hunt must not mistake a highlight's
        // appearance rectangles for a ruled table.
        let dir = tempfile::tempdir().unwrap();
        let (src, bbox) = highlight_fixture(dir.path());
        let out = dir.path().join("out.pdf");
        write_highlights(
            &src,
            &out,
            &[HighlightAnnotation {
                page: 0,
                rects: vec![bbox],
                color: YELLOW,
                opacity: 1.0,
                name: None,
            }],
        )
        .unwrap();

        let before = Document::open(&src).unwrap();
        let after = Document::open(&out).unwrap();
        assert_eq!(
            after.page_text(0).unwrap(),
            before.page_text(0).unwrap(),
            "annotations must not change extracted text"
        );
        let before_content = before
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        let after_content = after
            .page_content(0, ContentOptions::default(), None)
            .unwrap();
        assert_eq!(after_content.lines, before_content.lines);
        assert_eq!(after_content.objects, before_content.objects);
    }

    #[test]
    fn a_saved_highlight_renders_as_colour_on_the_page() {
        let dir = tempfile::tempdir().unwrap();
        let (src, bbox) = highlight_fixture(dir.path());
        let out = dir.path().join("out.pdf");
        write_highlights(
            &src,
            &out,
            &[HighlightAnnotation {
                page: 0,
                rects: vec![bbox],
                color: YELLOW,
                opacity: 1.0,
                name: None,
            }],
        )
        .unwrap();

        // `render_page` runs annotations, so the appearance stream `page.update`
        // generated must show up in the bitmap. The test asserts on the colour of
        // the paper rather than diffing two renders: a highlight is drawn with
        // Multiply blending, so the pixels under a glyph stroke stay black and
        // any single sample point is a coin toss.
        let bitmap = Document::open(&out).unwrap().render_page(0, 1.0).unwrap();
        let width = bitmap.width as usize;
        // Yellow-ish means the blue channel is clearly the darkest, which no
        // shade of the fixture's black-on-white text can be.
        let yellow_in_band = |top: f32, bottom: f32| {
            let mut count = 0usize;
            for y in top.max(0.0) as usize..(bottom as usize).min(bitmap.height as usize) {
                for x in 0..width {
                    let i = (y * width + x) * 4;
                    let (r, g, b) = (bitmap.data[i], bitmap.data[i + 1], bitmap.data[i + 2]);
                    if r.saturating_sub(b) > 0x30 && g.saturating_sub(b) > 0x30 {
                        count += 1;
                    }
                }
            }
            count
        };
        assert!(
            yellow_in_band(bbox.y0, bbox.y1) > 100,
            "expected the highlighted line to be painted yellow"
        );
        // A band well below the highlight is untouched paper and text.
        assert_eq!(
            yellow_in_band(bbox.y1 + 40.0, bbox.y1 + 140.0),
            0,
            "the highlight must not paint outside its own line"
        );
    }

    #[test]
    fn a_highlight_past_the_last_page_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (src, bbox) = highlight_fixture(dir.path());
        let out = dir.path().join("out.pdf");
        let err = write_highlights(
            &src,
            &out,
            &[HighlightAnnotation {
                page: 9,
                rects: vec![bbox],
                color: YELLOW,
                opacity: 1.0,
                name: None,
            }],
        )
        .unwrap_err();
        assert!(
            matches!(err, PdfError::PageOutOfRange { page: 9, count: 2 }),
            "{err}"
        );
    }
}

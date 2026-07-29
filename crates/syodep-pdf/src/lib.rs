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
    Heading,
}

impl ObjectKind {
    /// Whether this is a single stop at *every* scope above char.
    ///
    /// Tables and images are: there is nothing useful inside them to move
    /// through word by word. A heading is not — it is ordinary prose that you
    /// may well want to select a word of, so it is a unit only for sentence
    /// and paragraph scope, which it gets by bounding runs rather than by
    /// being atomic.
    pub fn is_atomic(self) -> bool {
        !matches!(self, Self::Heading)
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

    /// The *atomic* object containing `line`, if any — headings excluded.
    /// This is what motion treats as one stop.
    pub fn atomic_object_at(&self, line: usize) -> Option<&ContentObject> {
        self.object_at(line).filter(|o| o.kind.is_atomic())
    }
}

/// Knobs for [`Document::page_content`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentOptions {
    /// Run MuPDF's table detection so tables become single navigable units.
    /// Costs a second structured-text pass per page.
    pub detect_tables: bool,
    /// Detect headings so each is one sentence and one paragraph. Free: the
    /// type sizes it keys on come from the pass that extracts the text.
    pub detect_headings: bool,
    /// Drop running heads, folios and text that does not run in the page's
    /// reading direction, so the caret never traverses them.
    pub skip_page_furniture: bool,
}

impl Default for ContentOptions {
    fn default() -> Self {
        Self {
            detect_tables: true,
            detect_headings: true,
            skip_page_furniture: true,
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

/// How far two baselines may differ and still be the same running element.
const BASELINE_TOLERANCE: f32 = 2.5;

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
        for rgb in samples[..expected].chunks_exact(3) {
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
                    }],
                });
                styles.push(LineStyle {
                    size: 0.0,
                    bold: false,
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
                let (mut bold, mut inked) = (0usize, 0usize);
                for ch in line.chars() {
                    let Some(c) = ch.char() else { continue };
                    let quad = ch.quad();
                    cells.push(Cell {
                        kind: CellKind::Char(c),
                        bbox: rect_from_quad(&quad),
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
                styles.push(LineStyle {
                    size,
                    bold: inked > 0 && bold * 5 >= inked * 4,
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
        let tables = if opts.detect_tables && lines.len() > 1 {
            self.table_bboxes(&mupdf_page)?
        } else {
            Vec::new()
        };
        let headings = if opts.detect_headings {
            heading_ranges(&lines, &styles)
        } else {
            Vec::new()
        };

        let objects = content_objects(&lines, &image_lines, &tables, &headings, &furniture);
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
                let mut text = String::new();
                for ch in line.chars() {
                    let Some(c) = ch.char() else { continue };
                    text.push(c);
                    if !c.is_whitespace() {
                        origins.push(ch.origin().y);
                    }
                }
                if origins.is_empty() {
                    continue;
                }
                origins.sort_by(f32::total_cmp);
                let baseline = origins[origins.len() / 2];
                let Some((edge, offset)) = band_of(baseline, height) else {
                    continue;
                };
                let text = normalise_furniture_text(&text);
                if text.is_empty() {
                    continue;
                }
                out.push(BandLine { text, edge, offset });
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
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Which band a baseline falls in, and how far it sits from that page edge.
fn band_of(baseline: f32, page_height: f32) -> Option<(Edge, f32)> {
    if page_height <= 0.0 {
        return None;
    }
    if baseline <= BAND_SHARE * page_height {
        Some((Edge::Top, baseline))
    } else if baseline >= (1.0 - BAND_SHARE) * page_height {
        Some((Edge::Bottom, page_height - baseline))
    } else {
        None
    }
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
                e.text == line.text
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
/// Two independent rules. **Rotation**: a line more than
/// [`ANGLE_TOLERANCE_DEG`] off the page's dominant direction. This one needs no
/// cap and provably cannot empty a page — the dominant cluster is by
/// construction the majority of the page's characters and is never flagged, so
/// a wholly sideways page keeps everything. **Repetition**: a margin-band line
/// whose normalised text and baseline recur across the document. That one is
/// capped, because its evidence comes from elsewhere.
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

    let Some(profile) = profile.filter(|p| !p.is_empty()) else {
        return mask;
    };
    let mut repeated: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if mask[i] {
            continue;
        }
        let Some((edge, offset)) = band_of(styles[i].baseline, page_height) else {
            continue;
        };
        let text = normalise_furniture_text(&line_text(line));
        if text.is_empty() {
            continue;
        }
        let matched = profile.entries.iter().any(|e| {
            e.text == text && e.edge == edge && (e.offset - offset).abs() <= BASELINE_TOLERANCE
        });
        if matched {
            repeated.push(i);
        }
    }

    // Beyond these bounds the evidence is not credible: leave the page alone.
    let per_edge = |edge: Edge| {
        repeated
            .iter()
            .filter(|&&i| band_of(styles[i].baseline, page_height).map(|b| b.0) == Some(edge))
            .count()
    };
    // The per-edge cap is the real bound and applies always. The share cap is
    // only meaningful once a page has enough lines for a share to mean
    // anything — a short page is legitimately a third furniture.
    //
    // The repetition rule may never take a page's last line. A margin is only
    // a margin if there is something it is in the margin *of*, and the failure
    // this prevents is the worst one available: text plainly visible on the
    // page that the caret cannot reach at all.
    let share_applies = lines.len() >= MIN_LINES_FOR_SHARE_CAP;
    let too_many = repeated.len() == lines.len()
        || per_edge(Edge::Top) > MAX_FURNITURE_PER_EDGE
        || per_edge(Edge::Bottom) > MAX_FURNITURE_PER_EDGE
        || (share_applies && repeated.len() as f32 > MAX_FURNITURE_SHARE * lines.len() as f32);
    if !too_many {
        for i in repeated {
            mask[i] = true;
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
/// the full width of its column. That last clause is what separates a bold
/// subsection heading from a bold lead-in sentence inside a paragraph; the
/// widest line on the page stands in for the column width, which avoids
/// needing column detection here.
fn heading_ranges(lines: &[ContentLine], styles: &[LineStyle]) -> Vec<(usize, usize)> {
    let inked: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].cells.is_empty() && styles.get(i).is_some_and(|s| s.size > 0.0))
        .collect();
    if inked.len() < 2 {
        return Vec::new();
    }

    // Body size: the size most of the page's characters are set in. Body text
    // dominates by character count on essentially every page, including title
    // pages, which makes this far steadier than an average or a median.
    let mut weights: Vec<(i32, usize)> = Vec::new();
    for &i in &inked {
        let bucket = (styles[i].size * 10.0).round() as i32;
        let weight = lines[i].cells.len();
        match weights.iter_mut().find(|(b, _)| *b == bucket) {
            Some((_, w)) => *w += weight,
            None => weights.push((bucket, weight)),
        }
    }
    let body = weights
        .iter()
        .max_by_key(|(bucket, w)| (*w, *bucket))
        .map_or(0.0, |(bucket, _)| *bucket as f32 / 10.0);
    if body <= 0.0 {
        return Vec::new();
    }
    let widest = inked
        .iter()
        .map(|&i| lines[i].bbox.x1 - lines[i].bbox.x0)
        .fold(0.0_f32, f32::max);

    let is_heading = |i: usize| {
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
        if !is_heading(i) {
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
        return Vec::new();
    }
    ranges.retain(|&(start, end)| {
        let lines_covered = end - start + 1;
        lines_covered <= HEADING_MAX_LINES
    });
    ranges
}

/// Turn detected table boxes and image lines into the page's atomic objects.
///
/// Pure so that every heuristic below is unit-testable without MuPDF — which
/// matters, because MuPDF's table detection is a heuristic itself and these
/// guards are what keep its mistakes from reaching the caret.
fn content_objects(
    lines: &[ContentLine],
    image_lines: &[usize],
    tables: &[Rect],
    headings: &[(usize, usize)],
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
            bbox: *table,
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

    kept.sort_by_key(|o| o.start_line);
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

#[cfg(test)]
mod tests {
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
        assert!(bitmap.data.chunks_exact(4).any(|px| px[0] < 0x80));
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
        let objects = content_objects(&lines, &[], &[box_over(&lines, 3..=6)], &[], &[]);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, ObjectKind::Table);
        assert_eq!((objects[0].start_line, objects[0].end_line), (3, 6));
    }

    #[test]
    fn object_ranges_reject_a_single_line_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[], &[box_over(&lines, 4..=4)], &[], &[]);
        assert_eq!(objects, vec![], "a one-line table is just a line");
    }

    #[test]
    fn object_ranges_reject_a_table_claiming_every_line() {
        // MuPDF's whole-page fallback fires on ordinary prose; this guard is
        // the only thing standing between it and unnavigable pages.
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[], &[box_over(&lines, 0..=9)], &[], &[]);
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
        let objects = content_objects(&lines, &[], &[table], &[], &[]);
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
        );
        assert_eq!(objects.len(), 1);
        assert_eq!((objects[0].start_line, objects[0].end_line), (2, 8));
    }

    #[test]
    fn object_ranges_make_each_image_its_own_object() {
        let lines = stacked_lines(6);
        let objects = content_objects(&lines, &[1, 4], &[], &[], &[]);
        assert_eq!(objects.len(), 2);
        assert!(objects.iter().all(|o| o.kind == ObjectKind::Image));
        assert_eq!((objects[0].start_line, objects[0].end_line), (1, 1));
        assert_eq!((objects[1].start_line, objects[1].end_line), (4, 4));
    }

    #[test]
    fn object_ranges_absorb_an_image_inside_a_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[5], &[box_over(&lines, 3..=6)], &[], &[]);
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
            &[(17, 18)],
            &[],
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
                    };
                    60
                ],
            });
            styles.push(LineStyle {
                size: 10.0,
                bold: false,
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

    #[test]
    fn heading_ranges_flags_a_line_set_larger_than_the_body() {
        let (mut lines, mut styles) = body_lines(10, 400.0);
        set_style(&mut lines, &mut styles, 3, 18.0, true, 200.0);
        assert_eq!(heading_ranges(&lines, &styles), vec![(3, 3)]);
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
    fn a_heading_is_not_an_atomic_object() {
        assert!(!ObjectKind::Heading.is_atomic());
        assert!(ObjectKind::Table.is_atomic());
        assert!(ObjectKind::Image.is_atomic());
    }

    #[test]
    fn object_ranges_drop_a_heading_that_overlaps_a_table() {
        // A bold, short line inside a table is a column header.
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[], &[box_over(&lines, 3..=6)], &[(4, 4)], &[]);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, ObjectKind::Table);
    }

    #[test]
    fn object_ranges_keep_a_heading_outside_every_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[], &[box_over(&lines, 5..=8)], &[(1, 2)], &[]);
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
    fn mask_caps_the_repetition_rule_by_share() {
        // On a page with enough lines to judge, a quarter of them being
        // furniture means the evidence is not credible.
        let height = 842.0;
        let (mut lines, mut styles) = page_lines(12, height);
        let mut entries = Vec::new();
        for (i, offset) in [20.0f32, 30.0, 812.0, 822.0].into_iter().enumerate() {
            lines[i] = text_line_at(offset, "running head");
            styles[i].baseline = offset;
            let (edge, off) = band_of(offset, height).unwrap();
            entries.push(("running head", edge, off));
        }
        let profile = profile_of(&entries);
        assert!(furniture_mask(&lines, &styles, height, Some(&profile))
            .iter()
            .all(|m| !m));
    }

    #[test]
    fn repetition_never_takes_a_pages_last_line() {
        // The worst failure this feature could produce is text visible on the
        // page that the caret cannot reach, so the repetition rule always
        // leaves something behind.
        let height = 842.0;
        let lines = vec![
            text_line_at(42.0, "running head"),
            text_line_at(800.0, "12"),
        ];
        let styles = vec![
            LineStyle {
                size: 9.0,
                bold: false,
                angle: Some(0.0),
                baseline: 42.0,
            },
            LineStyle {
                size: 9.0,
                bold: false,
                angle: Some(0.0),
                baseline: 800.0,
            },
        ];
        let profile = profile_of(&[("running head", Edge::Top, 42.0), ("#", Edge::Bottom, 42.0)]);
        let mask = furniture_mask(&lines, &styles, height, Some(&profile));
        assert!(mask.iter().all(|m| !m), "mask: {mask:?}");
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
        let objects = content_objects(&kept, &[1], &[], &[], &lines[..1]);
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
        let objects = content_objects(&body, &[], &[table], &[], &furniture);
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
}

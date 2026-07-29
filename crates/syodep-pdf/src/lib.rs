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
    ListItem,
    Equation,
}

impl ObjectKind {
    /// Whether this is a single stop at *every* scope above char.
    ///
    /// Tables and images are: there is nothing useful inside them to move
    /// through word by word. A heading, a list item or an equation is not —
    /// each is text you may well want to select a part of, so they are units
    /// only for the scopes that group text into runs.
    pub fn is_atomic(self) -> bool {
        !matches!(self, Self::Heading | Self::ListItem | Self::Equation)
    }

    /// Whether every sentence terminator inside this is inert, making the whole
    /// of it one sentence.
    ///
    /// A heading needs it because `3.1. Methods` is not three sentences, and an
    /// equation because `f(x) = 0.` is not two. Prose kinds do not: a list item
    /// is walked sentence by sentence on purpose.
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

    /// The *atomic* object containing `line`, if any — headings and list items
    /// excluded. This is what motion treats as one stop.
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
    /// Detect display equations so each is one sentence and one paragraph, while
    /// staying walkable by word and character. Free, like headings: the fonts
    /// and characters it keys on come from the extraction pass.
    pub detect_equations: bool,
    /// Drop running heads, folios and text that does not run in the page's
    /// reading direction, so the caret never traverses them.
    pub skip_page_furniture: bool,
}

impl Default for ContentOptions {
    fn default() -> Self {
        Self {
            detect_tables: true,
            detect_headings: true,
            detect_equations: true,
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
    /// Share of the line's inked characters set in a math font, 0.0 to 1.0.
    math: f32,
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
                    math: 0.0,
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
                let (mut bold, mut inked, mut math) = (0usize, 0usize, 0usize);
                // Glyphs come in font runs, so remembering the last verdict
                // turns the name test into one string compare per character.
                let mut last_font: Option<(String, bool)> = None;
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
                    if let Some(font) = ch.font() {
                        let name = font.name();
                        let is_math = match &last_font {
                            Some((seen, verdict)) if seen == name => *verdict,
                            _ => {
                                let verdict = is_math_font(name);
                                last_font = Some((name.to_owned(), verdict));
                                verdict
                            }
                        };
                        if is_math {
                            math += 1;
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
                styles.push(LineStyle {
                    size,
                    bold: inked > 0 && bold * 5 >= inked * 4,
                    math: if inked > 0 {
                        math as f32 / inked as f32
                    } else {
                        0.0
                    },
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
        let equations = if opts.detect_equations {
            equation_ranges(&lines, &styles)
        } else {
            Vec::new()
        };

        let objects = content_objects(
            &lines,
            &image_lines,
            &tables,
            &headings,
            &equations,
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

/// A gap larger than this many line heights ends the item: an indented block
/// that far below merely follows the list.
const LIST_GAP_FACTOR: f32 = 1.5;

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
    let single_letter = label.len() == 1 && label.chars().all(|c| c.is_ascii_alphabetic());
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

    // An item runs from its marker through the lines indented past that
    // marker: its own text where extraction split the bullet off, and any
    // wrapped continuation. The prose after a list returns to the marker's own
    // margin, which is precisely where the last item has to stop.
    let is_marker = |i: usize| starts.iter().any(|&(s, _)| s == i);
    starts
        .iter()
        .map(|&(start, marker_x)| {
            let mut end = start;
            for j in start + 1..lines.len() {
                // Corroboration is page-wide, so a stray pair of marker-shaped
                // lines can exist. Cap how much one is allowed to claim.
                if j - start > LIST_ITEM_MAX_LINES {
                    break;
                }
                if is_marker(j) || blocked_at(j) || lines[j].cells.is_empty() {
                    break;
                }
                let previous = lines[end].bbox;
                let current = lines[j].bbox;
                let height = (previous.y1 - previous.y0).max(1.0);
                // Moving back up the page by more than a line is a new column
                // or region, and the top of the next column is trivially
                // "indented past" a marker in the left one — without this an
                // item swallows it. The tolerance matters: a bullet's own box
                // starts a point or two below its text's, because the glyph is
                // small and the text has ascenders, so an exact test would cut
                // every item off at its marker.
                if previous.y0 - current.y0 > height {
                    break;
                }
                // A wide gap means the block below merely follows the list
                // rather than belonging to its last item.
                if current.y0 - previous.y1 > LIST_GAP_FACTOR * height {
                    break;
                }
                if current.x0 <= marker_x + LIST_INDENT_EPS {
                    break;
                }
                end = j;
            }
            (start, end)
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
        let symbols = text.chars().filter(|&c| is_math_symbol(c)).count();
        let math_by_font = styles[i].math >= EQUATION_MATH_FONT_SHARE;
        let math_by_chars = symbols as f32 >= EQUATION_SYMBOL_SHARE * counted as f32;
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
fn content_objects(
    lines: &[ContentLine],
    image_lines: &[usize],
    tables: &[Rect],
    headings: &[(usize, usize)],
    equations: &[(usize, usize)],
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
        kept.windows(2).all(|w| w[0].end_line < w[1].start_line),
        "objects must stay sorted and disjoint: {kept:?}"
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
        let objects = content_objects(&lines, &[], &[box_over(&lines, 3..=6)], &[], &[], &[]);
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
        let objects = content_objects(&lines, &[], &[table], &[], &[], &[]);
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
        let objects = content_objects(&lines, &[], &[table], &[], &[], &[]);
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
                    }],
                }
            })
            .collect();
        let objects = content_objects(&lines, &[], &[box_over(&lines, 0..=4)], &[], &[], &[]);
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
            }],
        });
        let objects = content_objects(&with_prose, &[], &[box_over(&lines, 0..=4)], &[], &[], &[]);
        assert_eq!(objects.len(), 1, "objects: {objects:?}");
        assert_eq!((objects[0].start_line, objects[0].end_line), (0, 4));
    }

    #[test]
    fn object_ranges_trim_a_clipped_first_line_too() {
        let lines = stacked_lines(10);
        let mut table = box_over(&lines, 3..=6);
        // Reach back over line 2, just past its centre.
        table.y0 = lines[2].bbox.y1 - 6.0;
        let objects = content_objects(&lines, &[], &[table], &[], &[], &[]);
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
        let objects = content_objects(&lines, &[], &[table], &[], &[], &[]);
        assert_eq!(objects, vec![]);
    }

    #[test]
    fn object_ranges_reject_a_single_line_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[], &[box_over(&lines, 4..=4)], &[], &[], &[]);
        assert_eq!(objects, vec![], "a one-line table is just a line");
    }

    #[test]
    fn object_ranges_reject_a_table_claiming_every_line() {
        // MuPDF's whole-page fallback fires on ordinary prose; this guard is
        // the only thing standing between it and unnavigable pages.
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[], &[box_over(&lines, 0..=9)], &[], &[], &[]);
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
        let objects = content_objects(&lines, &[], &[table], &[], &[], &[]);
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
        );
        assert_eq!(objects.len(), 1);
        assert_eq!((objects[0].start_line, objects[0].end_line), (2, 8));
    }

    #[test]
    fn object_ranges_make_each_image_its_own_object() {
        let lines = stacked_lines(6);
        let objects = content_objects(&lines, &[1, 4], &[], &[], &[], &[]);
        assert_eq!(objects.len(), 2);
        assert!(objects.iter().all(|o| o.kind == ObjectKind::Image));
        assert_eq!((objects[0].start_line, objects[0].end_line), (1, 1));
        assert_eq!((objects[1].start_line, objects[1].end_line), (4, 4));
    }

    #[test]
    fn object_ranges_absorb_an_image_inside_a_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[5], &[box_over(&lines, 3..=6)], &[], &[], &[]);
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
                math: 0.0,
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
                };
                x += step;
                cell
            })
            .collect();
        lines[i].bbox.x1 = lines[i].bbox.x0 + width;
        styles[i] = LineStyle { math, ..styles[i] };
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
        let objects = content_objects(&lines, &[], &[box_over(&lines, 3..=6)], &[(4, 4)], &[], &[]);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, ObjectKind::Table);
    }

    #[test]
    fn object_ranges_keep_a_heading_outside_every_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[], &[box_over(&lines, 5..=8)], &[(1, 2)], &[], &[]);
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
        // The formula, and only the formula: `Symbol` encodes `a + b = g` as
        // Greek, so this is what the caret sees.
        assert_eq!(equation.start_line, equation.end_line, "prose swallowed");
        let text = text_of(equation.start_line);
        assert!(
            text.contains('\u{3b1}') && text.contains('='),
            "equation line reads {text:?}"
        );
        // The prose around it stays outside.
        for i in 0..content.lines.len() {
            if i == equation.start_line {
                continue;
            }
            assert!(
                !text_of(i).contains('\u{3b1}'),
                "line {i} reads {:?}",
                text_of(i)
            );
        }
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
        let mut lines = vec![
            text_line_at(20.0, "\u{2022} the first file"),
            text_line_at(40.0, "wrapped onto a second line"),
            text_line_at(60.0, "\u{2022} the second file"),
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
            text_line_at(40.0, "\u{2022} the second file"),
            text_line_at(60.0, "wrapped onto a second line"),
            text_line_at(80.0, "Each of them is regenerated in turn."),
        ];
        indent(&mut lines[2], 10.0);
        assert_eq!(list_items(&lines, &[]), vec![(0, 0), (1, 2)]);
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
            text_line_at(40.0, "a figure caption below it"),
            text_line_at(60.0, "\u{2022} the second file"),
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
        for kind in [ObjectKind::Image, ObjectKind::Table] {
            assert!(kind.is_atomic() && kind.splits_paragraphs(), "{kind:?}");
        }
        assert!(!ObjectKind::Heading.is_atomic());
        assert!(ObjectKind::Heading.splits_paragraphs());
        // A list item is the only kind that is neither: prose you can walk
        // word by word, and part of the one paragraph its list makes.
        assert!(!ObjectKind::ListItem.is_atomic());
        assert!(!ObjectKind::ListItem.splits_paragraphs());
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
                math: 0.0,
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
                math: 0.0,
                angle: Some(0.0),
                baseline: 42.0,
            },
            LineStyle {
                size: 9.0,
                bold: false,
                math: 0.0,
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
        let objects = content_objects(&kept, &[1], &[], &[], &[], &lines[..1]);
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
        let objects = content_objects(&body, &[], &[table], &[], &[], &furniture);
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
}

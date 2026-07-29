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
}

impl Default for ContentOptions {
    fn default() -> Self {
        Self {
            detect_tables: true,
            detect_headings: true,
        }
    }
}

/// The typography of one content line, used only to spot headings.
///
/// Not part of [`ContentLine`]: nothing outside detection needs it, and
/// keeping it out means the public content types stay about geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LineStyle {
    /// The type size most of the line's characters are set in.
    size: f32,
    /// Whether the line is essentially all bold.
    bold: bool,
}

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
    pub fn page_content(&self, page: usize, opts: ContentOptions) -> Result<PageContent, PdfError> {
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
                });
                continue;
            }
            for line in block.lines() {
                let mut cells = Vec::new();
                let mut sizes: Vec<(i32, usize)> = Vec::new();
                let (mut bold, mut inked) = (0usize, 0usize);
                for ch in line.chars() {
                    let Some(c) = ch.char() else { continue };
                    cells.push(Cell {
                        kind: CellKind::Char(c),
                        bbox: rect_from_quad(&ch.quad()),
                    });
                    if c.is_whitespace() {
                        continue;
                    }
                    inked += 1;
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
                lines.push(ContentLine {
                    bbox: rect_from_mupdf(line.bounds()),
                    cells,
                });
                styles.push(LineStyle {
                    size,
                    bold: inked > 0 && bold * 5 >= inked * 4,
                });
            }
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

        let objects = content_objects(&lines, &image_lines, &tables, &headings);
        Ok(PageContent { lines, objects })
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
) -> Vec<ContentObject> {
    let non_empty = lines.iter().filter(|l| !l.cells.is_empty()).count();
    let content_area = lines
        .iter()
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
            .page_content(0, ContentOptions::default())
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
            doc.page_content(3, ContentOptions::default()),
            Err(PdfError::PageOutOfRange { .. })
        ));
    }

    #[test]
    fn page_content_includes_one_cell_per_image() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_image()).unwrap();
        let lines = doc
            .page_content(0, ContentOptions::default())
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
        let objects = content_objects(&lines, &[], &[box_over(&lines, 3..=6)], &[]);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, ObjectKind::Table);
        assert_eq!((objects[0].start_line, objects[0].end_line), (3, 6));
    }

    #[test]
    fn object_ranges_reject_a_single_line_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[], &[box_over(&lines, 4..=4)], &[]);
        assert_eq!(objects, vec![], "a one-line table is just a line");
    }

    #[test]
    fn object_ranges_reject_a_table_claiming_every_line() {
        // MuPDF's whole-page fallback fires on ordinary prose; this guard is
        // the only thing standing between it and unnavigable pages.
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[], &[box_over(&lines, 0..=9)], &[]);
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
        let objects = content_objects(&lines, &[], &[table], &[]);
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
        );
        assert_eq!(objects.len(), 1);
        assert_eq!((objects[0].start_line, objects[0].end_line), (2, 8));
    }

    #[test]
    fn object_ranges_make_each_image_its_own_object() {
        let lines = stacked_lines(6);
        let objects = content_objects(&lines, &[1, 4], &[], &[]);
        assert_eq!(objects.len(), 2);
        assert!(objects.iter().all(|o| o.kind == ObjectKind::Image));
        assert_eq!((objects[0].start_line, objects[0].end_line), (1, 1));
        assert_eq!((objects[1].start_line, objects[1].end_line), (4, 4));
    }

    #[test]
    fn object_ranges_absorb_an_image_inside_a_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[5], &[box_over(&lines, 3..=6)], &[]);
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
        styles[i] = LineStyle { size, bold };
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
        let objects = content_objects(&lines, &[], &[box_over(&lines, 3..=6)], &[(4, 4)]);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, ObjectKind::Table);
    }

    #[test]
    fn object_ranges_keep_a_heading_outside_every_table() {
        let lines = stacked_lines(10);
        let objects = content_objects(&lines, &[], &[box_over(&lines, 5..=8)], &[(1, 2)]);
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].kind, ObjectKind::Heading);
        assert_eq!((objects[0].start_line, objects[0].end_line), (1, 2));
        assert_eq!(objects[1].kind, ObjectKind::Table);
    }

    #[test]
    fn page_content_detects_a_heading_and_a_bold_subheading() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_heading()).unwrap();
        let content = doc.page_content(0, ContentOptions::default()).unwrap();
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
        let content = doc.page_content(0, ContentOptions::default()).unwrap();
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
            )
            .unwrap();
        assert!(content
            .objects
            .iter()
            .all(|o| o.kind != ObjectKind::Heading));
    }

    #[test]
    fn page_content_reports_no_table_on_plain_prose() {
        // The failure that hurts users is a false positive, so this is a
        // load-bearing test rather than a nicety.
        let doc = three_page_doc();
        let content = doc.page_content(0, ContentOptions::default()).unwrap();
        assert!(
            !content.objects.iter().any(|o| o.kind == ObjectKind::Table),
            "prose reported as a table: {:?}",
            content.objects
        );
    }

    #[test]
    fn page_content_reports_no_table_on_a_two_column_page() {
        let doc = Document::from_bytes(&crate::test_support::pdf_two_column_page(6)).unwrap();
        let content = doc.page_content(0, ContentOptions::default()).unwrap();
        assert!(
            !content.objects.iter().any(|o| o.kind == ObjectKind::Table),
            "columns reported as a table: {:?}",
            content.objects
        );
    }

    #[test]
    fn page_content_reports_an_image_as_a_one_line_object() {
        let doc = Document::from_bytes(&crate::test_support::pdf_with_image()).unwrap();
        let content = doc.page_content(0, ContentOptions::default()).unwrap();
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
        let content = doc.page_content(0, ContentOptions::default()).unwrap();
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
            )
            .unwrap();
        assert!(content.objects.iter().all(|o| o.kind != ObjectKind::Table));
    }
}

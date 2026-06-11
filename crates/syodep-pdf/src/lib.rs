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

use mupdf::{Colorspace, Matrix, TextPageFlags};

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

    /// The document outline (table of contents), possibly empty.
    pub fn outline(&self) -> Result<Vec<OutlineItem>, PdfError> {
        let outlines = self.inner.outlines()?;
        Ok(outlines.into_iter().map(convert_outline).collect())
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
}

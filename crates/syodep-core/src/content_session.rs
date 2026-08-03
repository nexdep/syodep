//! Lazily extracted page content and derived navigation caches for one open document.

use std::collections::{HashMap, HashSet};

use syodep_pdf::{
    ContentLine, ContentObject, ContentOptions, Document, FurnitureProfile, PageContent,
};

use crate::caret::{column_ranges, paragraph_segments, split_segments_at_objects};

/// Per-page data derived from extracted content (paragraphs, columns).
///
/// `PageContent` is session-immutable, so this cache needs no invalidation
/// beyond document open / [`ContentSession::set_page_content`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DerivedPageData {
    /// Paragraph segments after object splits (what `p` motion walks).
    pub paragraphs: Vec<(usize, usize)>,
    /// Column x-ranges for `h`/`l` jumps.
    pub columns: Vec<(f32, f32)>,
}

/// Extracted navigable content for the life of a session.
///
/// Text is cheap to keep, so every visited page stays cached. Furniture is
/// learned once (when first needed) so every page is filtered against the same
/// evidence.
#[derive(Debug, Default)]
pub struct ContentSession {
    content: HashMap<usize, PageContent>,
    /// Pages whose extraction returned an error. Still cached as empty content
    /// so motion skips them, but distinct from a genuinely blank page for
    /// diagnostics.
    content_extraction_failed: HashSet<usize>,
    /// What this document repeats in its margins. Learned once, before the
    /// first page is extracted.
    furniture: Option<FurnitureProfile>,
    derived: HashMap<usize, DerivedPageData>,
}

impl ContentSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ensure page `page`'s navigable content is extracted and cached.
    ///
    /// On extraction failure still caches empty content (so motion skips the
    /// page) and returns `Err` with a status-bar message; the caller stores it
    /// as the app's `last_error`.
    pub fn ensure(
        &mut self,
        doc: &Document,
        page: usize,
        opts: ContentOptions,
    ) -> Result<(), String> {
        if self.content.contains_key(&page) {
            return Ok(());
        }
        // Learn the margins before extracting anything, so every page in this
        // session is filtered against the same evidence. Done here rather than
        // at open so that merely reading a document never pays for it — page
        // content is only ever extracted once the caret is used.
        if self.furniture.is_none() && opts.skip_page_furniture {
            self.furniture = Some(doc.furniture_profile().unwrap_or_default());
        }
        match doc.page_content(page, opts, self.furniture.as_ref()) {
            Ok(content) => {
                self.content.insert(page, content);
                Ok(())
            }
            Err(e) => {
                self.content_extraction_failed.insert(page);
                self.content.insert(page, PageContent::default());
                // 1-based page number matches what the status line shows.
                Err(format!(
                    "could not extract content on page {}: {e}",
                    page + 1
                ))
            }
        }
    }

    /// Full cached page content, if extracted.
    pub fn get(&self, page: usize) -> Option<&PageContent> {
        self.content.get(&page)
    }

    /// Cached content lines for `page` (empty if absent/uncached).
    pub fn content(&self, page: usize) -> &[ContentLine] {
        self.content
            .get(&page)
            .map(|c| c.lines.as_slice())
            .unwrap_or(&[])
    }

    /// Cached structural objects for `page` — every kind, atomic or not.
    pub fn objects(&self, page: usize) -> &[ContentObject] {
        self.content
            .get(&page)
            .map(|c| c.objects.as_slice())
            .unwrap_or(&[])
    }

    /// Ensure derived paragraph/column data for `page` is memoized.
    ///
    /// Caller must have already extracted the page via [`Self::ensure`].
    pub fn ensure_derived(&mut self, page: usize) {
        if self.derived.contains_key(&page) {
            return;
        }
        let lines = self.content(page);
        let columns = column_ranges(lines);
        let segs = paragraph_segments(lines);
        let splitting: Vec<ContentObject> = self
            .objects(page)
            .iter()
            .filter(|o| o.kind.splits_paragraphs())
            .copied()
            .collect();
        let paragraphs = split_segments_at_objects(&segs, &splitting);
        self.derived.insert(
            page,
            DerivedPageData {
                paragraphs,
                columns,
            },
        );
    }

    /// Memoized paragraph segments for `page` (after object splits).
    pub fn paragraphs(&mut self, page: usize) -> &[(usize, usize)] {
        self.ensure_derived(page);
        &self.derived[&page].paragraphs
    }

    /// Memoized column ranges for `page`.
    pub fn columns(&mut self, page: usize) -> &[(f32, f32)] {
        self.ensure_derived(page);
        &self.derived[&page].columns
    }

    /// Whether derived data for `page` is already memoized (tests).
    pub fn has_derived(&self, page: usize) -> bool {
        self.derived.contains_key(&page)
    }

    /// Replace a page's extracted content (tests / injected layouts).
    pub fn set_page_content(&mut self, page: usize, content: PageContent) {
        self.derived.remove(&page);
        self.content_extraction_failed.remove(&page);
        self.content.insert(page, content);
    }

    /// Whether extraction previously failed for `page`.
    pub fn extraction_failed(&self, page: usize) -> bool {
        self.content_extraction_failed.contains(&page)
    }
}

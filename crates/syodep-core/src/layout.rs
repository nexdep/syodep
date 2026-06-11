//! Continuous document layout and viewport math.
//!
//! Coordinate systems:
//!
//! - *Document space*: PDF points (1/72 inch) at zoom 1.0. Pages are stacked
//!   vertically, horizontally centered on the widest page, separated by a
//!   configurable gap. `y` grows downwards.
//! - *Screen space*: physical pixels of the canvas. `screen = (doc - scroll) * zoom`.
//!
//! Scroll offsets are kept in document space so they stay stable across zoom
//! changes. All math here is pure and fully unit-tested; nothing in this
//! module touches PDF data or the UI.

/// Size of a single page in document points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

/// A page placed in document space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedPage {
    pub size: PageSize,
    /// Top edge of the page in document space.
    pub y: f32,
    /// Left edge of the page in document space (pages are centered).
    pub x: f32,
}

/// Immutable vertical stack layout of all pages of a document.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentLayout {
    pages: Vec<PlacedPage>,
    gap: f32,
    max_width: f32,
    total_height: f32,
}

impl DocumentLayout {
    pub fn new(sizes: &[PageSize], gap: f32) -> Self {
        let max_width = sizes.iter().map(|s| s.width).fold(0.0, f32::max);
        let mut pages = Vec::with_capacity(sizes.len());
        let mut y = 0.0;
        for size in sizes {
            pages.push(PlacedPage {
                size: *size,
                y,
                x: (max_width - size.width) / 2.0,
            });
            y += size.height + gap;
        }
        let total_height = if sizes.is_empty() { 0.0 } else { y - gap };
        Self {
            pages,
            gap,
            max_width,
            total_height,
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn page(&self, index: usize) -> Option<&PlacedPage> {
        self.pages.get(index)
    }

    pub fn max_width(&self) -> f32 {
        self.max_width
    }

    pub fn total_height(&self) -> f32 {
        self.total_height
    }

    /// Page whose vertical slot (page plus trailing gap) contains `doc_y`.
    /// Out-of-range coordinates clamp to the first/last page.
    pub fn page_at_y(&self, doc_y: f32) -> usize {
        if self.pages.is_empty() {
            return 0;
        }
        let i = self.pages.partition_point(|p| p.y <= doc_y);
        i.saturating_sub(1)
    }
}

/// A rectangle in screen pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 16.0;

/// Layout plus viewport: scroll position, zoom and canvas size.
#[derive(Debug, Clone)]
pub struct View {
    layout: DocumentLayout,
    viewport_width: f32,
    viewport_height: f32,
    zoom: f32,
    /// Document-space coordinate of the viewport's top-left corner.
    scroll_x: f32,
    scroll_y: f32,
}

impl View {
    pub fn new(layout: DocumentLayout, viewport_width: f32, viewport_height: f32) -> Self {
        let mut view = Self {
            layout,
            viewport_width: viewport_width.max(1.0),
            viewport_height: viewport_height.max(1.0),
            zoom: 1.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
        };
        view.clamp_scroll();
        view
    }

    pub fn layout(&self) -> &DocumentLayout {
        &self.layout
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn scroll(&self) -> (f32, f32) {
        (self.scroll_x, self.scroll_y)
    }

    pub fn set_viewport_size(&mut self, width: f32, height: f32) {
        self.viewport_width = width.max(1.0);
        self.viewport_height = height.max(1.0);
        self.clamp_scroll();
    }

    /// Restore a persisted position. Values are clamped to the document.
    pub fn restore(&mut self, scroll_x: f32, scroll_y: f32, zoom: f32) {
        self.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        self.scroll_x = scroll_x;
        self.scroll_y = scroll_y;
        self.clamp_scroll();
    }

    /// Viewport height expressed in document points.
    fn viewport_doc_height(&self) -> f32 {
        self.viewport_height / self.zoom
    }

    fn clamp_scroll(&mut self) {
        let max_y = self.layout.total_height() - self.viewport_doc_height();
        // A document shorter/narrower than the viewport is centered.
        self.scroll_y = if max_y <= 0.0 {
            max_y / 2.0
        } else {
            self.scroll_y.clamp(0.0, max_y)
        };
        let max_x = self.layout.max_width() - self.viewport_width / self.zoom;
        self.scroll_x = if max_x <= 0.0 {
            max_x / 2.0
        } else {
            self.scroll_x.clamp(0.0, max_x)
        };
    }

    /// Scroll by a pixel delta (positive y scrolls the content up, i.e.
    /// moves the viewport down the document).
    pub fn scroll_by_px(&mut self, dx: f32, dy: f32) {
        self.scroll_x += dx / self.zoom;
        self.scroll_y += dy / self.zoom;
        self.clamp_scroll();
    }

    /// The page considered "current": the one under the viewport's center.
    pub fn current_page(&self) -> usize {
        self.layout
            .page_at_y(self.scroll_y + self.viewport_doc_height() / 2.0)
    }

    /// Scroll so the top of `index` (clamped) aligns with the viewport top.
    pub fn goto_page(&mut self, index: usize) {
        if self.layout.page_count() == 0 {
            return;
        }
        let index = index.min(self.layout.page_count() - 1);
        self.scroll_y = self.layout.page(index).expect("clamped index").y;
        self.clamp_scroll();
    }

    pub fn next_page(&mut self, count: usize) {
        self.goto_page(self.current_page().saturating_add(count));
    }

    pub fn prev_page(&mut self, count: usize) {
        self.goto_page(self.current_page().saturating_sub(count));
    }

    /// Set zoom, keeping the document point at the viewport center fixed.
    pub fn set_zoom(&mut self, zoom: f32) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let center_x = self.scroll_x + self.viewport_width / self.zoom / 2.0;
        let center_y = self.scroll_y + self.viewport_height / self.zoom / 2.0;
        self.zoom = zoom;
        self.scroll_x = center_x - self.viewport_width / self.zoom / 2.0;
        self.scroll_y = center_y - self.viewport_height / self.zoom / 2.0;
        self.clamp_scroll();
    }

    pub fn zoom_by(&mut self, factor: f32) {
        self.set_zoom(self.zoom * factor);
    }

    /// Zoom so the widest page exactly fills the viewport width.
    pub fn fit_width(&mut self) {
        if self.layout.max_width() > 0.0 {
            self.set_zoom(self.viewport_width / self.layout.max_width());
        }
    }

    /// Pages intersecting the viewport, with their screen-space rectangles.
    pub fn visible_pages(&self) -> Vec<(usize, ScreenRect)> {
        let mut out = Vec::new();
        for index in 0..self.layout.page_count() {
            let page = self.layout.page(index).expect("index in range");
            let rect = ScreenRect {
                x: (page.x - self.scroll_x) * self.zoom,
                y: (page.y - self.scroll_y) * self.zoom,
                width: page.size.width * self.zoom,
                height: page.size.height * self.zoom,
            };
            if rect.y < self.viewport_height && rect.y + rect.height > 0.0 {
                out.push((index, rect));
            } else if !out.is_empty() {
                // Pages are sorted by y; once we leave the viewport we are done.
                break;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three A4-ish portrait pages (595x842 points) with a 10pt gap.
    fn three_pages() -> DocumentLayout {
        DocumentLayout::new(
            &[
                PageSize {
                    width: 595.0,
                    height: 842.0,
                },
                PageSize {
                    width: 595.0,
                    height: 842.0,
                },
                PageSize {
                    width: 595.0,
                    height: 842.0,
                },
            ],
            10.0,
        )
    }

    #[test]
    fn layout_stacks_pages_with_gaps() {
        let layout = three_pages();
        assert_eq!(layout.page_count(), 3);
        assert_eq!(layout.page(0).unwrap().y, 0.0);
        assert_eq!(layout.page(1).unwrap().y, 852.0);
        assert_eq!(layout.page(2).unwrap().y, 1704.0);
        assert_eq!(layout.total_height(), 1704.0 + 842.0);
        assert_eq!(layout.max_width(), 595.0);
    }

    #[test]
    fn narrow_pages_are_centered() {
        let layout = DocumentLayout::new(
            &[
                PageSize {
                    width: 400.0,
                    height: 600.0,
                },
                PageSize {
                    width: 600.0,
                    height: 600.0,
                },
            ],
            10.0,
        );
        assert_eq!(layout.page(0).unwrap().x, 100.0);
        assert_eq!(layout.page(1).unwrap().x, 0.0);
    }

    #[test]
    fn page_at_y_picks_slot() {
        let layout = three_pages();
        assert_eq!(layout.page_at_y(-50.0), 0);
        assert_eq!(layout.page_at_y(0.0), 0);
        assert_eq!(layout.page_at_y(841.0), 0);
        assert_eq!(layout.page_at_y(852.0), 1);
        assert_eq!(layout.page_at_y(9999.0), 2);
    }

    #[test]
    fn empty_layout_is_safe() {
        let layout = DocumentLayout::new(&[], 10.0);
        assert_eq!(layout.page_count(), 0);
        assert_eq!(layout.total_height(), 0.0);
        let mut view = View::new(layout, 800.0, 600.0);
        view.scroll_by_px(0.0, 100.0);
        view.goto_page(5);
        assert_eq!(view.visible_pages(), vec![]);
        assert_eq!(view.current_page(), 0);
    }

    #[test]
    fn scrolling_is_clamped_to_document() {
        let mut view = View::new(three_pages(), 595.0, 600.0);
        view.scroll_by_px(0.0, -500.0);
        assert_eq!(view.scroll().1, 0.0);
        view.scroll_by_px(0.0, 1.0e9);
        let max_y = view.layout().total_height() - 600.0 / view.zoom();
        assert!((view.scroll().1 - max_y).abs() < 0.01);
    }

    #[test]
    fn scroll_delta_respects_zoom() {
        let mut view = View::new(three_pages(), 595.0, 600.0);
        view.set_zoom(2.0);
        let before = view.scroll().1;
        view.scroll_by_px(0.0, 100.0);
        // 100 px at zoom 2 is 50 document points.
        assert!((view.scroll().1 - before - 50.0).abs() < 0.01);
    }

    #[test]
    fn page_navigation() {
        let mut view = View::new(three_pages(), 595.0, 600.0);
        assert_eq!(view.current_page(), 0);
        view.next_page(1);
        assert_eq!(view.current_page(), 1);
        assert_eq!(view.scroll().1, 852.0);
        view.next_page(5);
        assert_eq!(view.current_page(), 2);
        view.prev_page(2);
        assert_eq!(view.current_page(), 0);
    }

    #[test]
    fn goto_page_clamps() {
        let mut view = View::new(three_pages(), 595.0, 600.0);
        view.goto_page(999);
        assert_eq!(view.current_page(), 2);
    }

    #[test]
    fn fit_width_fills_viewport() {
        let mut view = View::new(three_pages(), 1190.0, 600.0);
        view.fit_width();
        assert!((view.zoom() - 2.0).abs() < 1.0e-6);
        let pages = view.visible_pages();
        assert_eq!(pages[0].1.x, 0.0);
        assert!((pages[0].1.width - 1190.0).abs() < 0.01);
    }

    #[test]
    fn zoom_keeps_viewport_center() {
        let mut view = View::new(three_pages(), 595.0, 600.0);
        view.scroll_by_px(0.0, 800.0);
        let center_before = view.scroll().1 + 600.0 / view.zoom() / 2.0;
        view.zoom_by(1.5);
        let center_after = view.scroll().1 + 600.0 / view.zoom() / 2.0;
        assert!((center_before - center_after).abs() < 0.5);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut view = View::new(three_pages(), 595.0, 600.0);
        view.set_zoom(1000.0);
        assert_eq!(view.zoom(), MAX_ZOOM);
        view.set_zoom(0.0001);
        assert_eq!(view.zoom(), MIN_ZOOM);
    }

    #[test]
    fn document_narrower_than_viewport_is_centered() {
        let mut view = View::new(three_pages(), 1200.0, 600.0);
        view.set_zoom(1.0);
        let pages = view.visible_pages();
        // (1200 - 595) / 2 = 302.5 px left margin.
        assert!((pages[0].1.x - 302.5).abs() < 0.01);
    }

    #[test]
    fn visible_pages_at_boundary() {
        let mut view = View::new(three_pages(), 595.0, 600.0);
        let visible: Vec<usize> = view.visible_pages().iter().map(|(i, _)| *i).collect();
        assert_eq!(visible, vec![0]);
        // Scroll to straddle pages 0 and 1.
        view.scroll_by_px(0.0, 700.0);
        let visible: Vec<usize> = view.visible_pages().iter().map(|(i, _)| *i).collect();
        assert_eq!(visible, vec![0, 1]);
    }

    #[test]
    fn restore_clamps_persisted_values() {
        let mut view = View::new(three_pages(), 595.0, 600.0);
        view.restore(0.0, 1.0e9, 3.0);
        assert_eq!(view.zoom(), 3.0);
        let max_y = view.layout().total_height() - 600.0 / 3.0;
        assert!((view.scroll().1 - max_y).abs() < 0.01);
    }
}

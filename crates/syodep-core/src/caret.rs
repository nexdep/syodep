//! Caret navigation: a Vim-like cursor that moves through a document's
//! content (text characters and images) independently of scrolling.
//!
//! The caret is *modal*: the app is either in [`Mode::Normal`] (where `hjkl`
//! scroll the page) or [`Mode::CaretFocus`] (where `hjkl` move the caret — `h`/`l`
//! by character, `j`/`k` by line — and the view auto-scrolls to follow it).
//! Each image is a single caret stop, so the caret traverses text and images
//! uniformly.
//!
//! This module owns the small pieces that are pure and unit-testable in
//! isolation: the position type, the movement direction, and the
//! goal-column cell picker. Orchestration (loading page content, crossing
//! page boundaries, scrolling the caret into view) lives in [`crate::app`],
//! which holds the document and the layout.

use syodep_pdf::{Cell, CellKind, ContentLine};

/// Whether `hjkl` scroll the page or move the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// `hjkl` scroll the page (the original behavior).
    #[default]
    Normal,
    /// Caret focus mode: `hjkl` move the caret; the view follows it.
    CaretFocus,
    /// Line focus mode: a whole line is highlighted; `j`/`k` move it line-wise
    /// and `H`/`L` move between columns (multi-column pages only).
    LineFocus,
    /// Word focus mode: a whole word is highlighted; `h`/`l` (and `w`/`b`) step
    /// word-wise and `j`/`k` move by line, keeping a goal column like the caret.
    WordFocus,
}

/// A word-focus position: the run of cells `start_cell..=end_cell` within a line
/// within a page. All indices are zero-based and only meaningful against the
/// document the mark belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WordMark {
    pub page: usize,
    pub line: usize,
    pub start_cell: usize,
    pub end_cell: usize,
}

/// A line-focus position: a line within a page. Both indices are zero-based and
/// only meaningful against the document the mark belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineMark {
    pub page: usize,
    pub line: usize,
}

/// A caret position: a cell within a line within a page. All indices are
/// zero-based and only meaningful against the document the caret belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub page: usize,
    pub line: usize,
    pub cell: usize,
}

/// A movement direction for the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// A cell's class for Vim-like lowercase word motions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordClass {
    /// Letters, digits and `_`.
    Word,
    /// Non-whitespace punctuation and symbols.
    Punctuation,
    /// Whitespace is skipped by word motions.
    Whitespace,
    /// Images are single word-like stops.
    Image,
}

/// Classify a cell for `w`/`e`/`b` caret motion.
pub fn word_class(cell: &Cell) -> WordClass {
    match cell.kind {
        CellKind::Char(c) if c.is_alphanumeric() || c == '_' => WordClass::Word,
        CellKind::Char(c) if c.is_whitespace() => WordClass::Whitespace,
        CellKind::Char(_) => WordClass::Punctuation,
        CellKind::Image => WordClass::Image,
    }
}

/// Whether this class is a place word motions can land.
pub fn is_word_target(class: WordClass) -> bool {
    !matches!(class, WordClass::Whitespace)
}

/// Whether two adjacent cells are part of the same word-motion run.
///
/// Runs never continue across line/page boundaries, whitespace is skipped, and
/// each image is its own stop even when images are adjacent.
pub fn continues_word_run(left: WordClass, right: WordClass, same_line: bool) -> bool {
    same_line
        && matches!(
            (left, right),
            (WordClass::Word, WordClass::Word) | (WordClass::Punctuation, WordClass::Punctuation)
        )
}

/// Index of the cell whose horizontal extent is nearest `goal_x` (the
/// remembered "goal column"). A cell that contains `goal_x` wins with
/// distance zero; otherwise the closest edge wins. Empty lines yield 0.
///
/// This is what makes repeated `j`/`k` track a column instead of drifting,
/// exactly like a text editor's vertical motion.
pub fn nearest_cell_in_line(cells: &[Cell], goal_x: f32) -> usize {
    let mut best = 0;
    let mut best_dist = f32::INFINITY;
    for (i, cell) in cells.iter().enumerate() {
        let dist = if goal_x < cell.bbox.x0 {
            cell.bbox.x0 - goal_x
        } else if goal_x > cell.bbox.x1 {
            goal_x - cell.bbox.x1
        } else {
            0.0
        };
        if dist < best_dist {
            best_dist = dist;
            best = i;
        }
    }
    best
}

/// Index of the line whose vertical extent is nearest `goal_y` (the remembered
/// "goal row") among `lines` restricted to those in `candidates`. A line that
/// contains `goal_y` wins with distance zero; otherwise the closest edge wins.
///
/// This is the line-focus analogue of [`nearest_cell_in_line`]: it makes `H`/`L`
/// land on the column line nearest the current vertical position instead of
/// drifting, exactly like the caret's goal column for `j`/`k`.
pub fn nearest_line_in_column(lines: &[ContentLine], candidates: &[usize], goal_y: f32) -> usize {
    let mut best = *candidates.first().unwrap_or(&0);
    let mut best_dist = f32::INFINITY;
    for &i in candidates {
        let Some(line) = lines.get(i) else { continue };
        let dist = if goal_y < line.bbox.y0 {
            line.bbox.y0 - goal_y
        } else if goal_y > line.bbox.y1 {
            goal_y - line.bbox.y1
        } else {
            0.0
        };
        if dist < best_dist {
            best_dist = dist;
            best = i;
        }
    }
    best
}

/// Detect the horizontal columns of a page from its lines' bounding boxes,
/// returned left-to-right as `(x0, x1)` x-ranges. Fewer than two ranges means
/// the page is single-column.
///
/// Lines are grouped greedily by horizontal overlap: a line joins an existing
/// column when its x-span overlaps that column's accumulated span, otherwise it
/// starts a new one. This is intentionally simple — it recognizes the common
/// multi-column article layout where columns occupy disjoint x-bands — and is
/// pure so it can be unit-tested in isolation.
pub fn column_ranges(lines: &[ContentLine]) -> Vec<(f32, f32)> {
    let mut cols: Vec<(f32, f32)> = Vec::new();
    for line in lines {
        if line.cells.is_empty() {
            continue;
        }
        let (lx0, lx1) = (line.bbox.x0, line.bbox.x1);
        match cols
            .iter_mut()
            .find(|(cx0, cx1)| lx0 <= *cx1 && lx1 >= *cx0)
        {
            Some((cx0, cx1)) => {
                *cx0 = cx0.min(lx0);
                *cx1 = cx1.max(lx1);
            }
            None => cols.push((lx0, lx1)),
        }
    }
    cols.sort_by(|a, b| a.0.total_cmp(&b.0));
    cols
}

/// The index of the column in `cols` (from [`column_ranges`]) that contains
/// x-range `[x0, x1]` — the column whose span overlaps it most. `None` when
/// `cols` is empty.
pub fn column_index_of(cols: &[(f32, f32)], x0: f32, x1: f32) -> Option<usize> {
    cols.iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| {
            let oa = (x1.min(a.1) - x0.max(a.0)).max(0.0);
            let ob = (x1.min(b.1) - x0.max(b.0)).max(0.0);
            oa.total_cmp(&ob)
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use syodep_pdf::{CellKind, Rect};

    fn char_cell_at(c: char, x0: f32, x1: f32) -> Cell {
        Cell {
            kind: CellKind::Char(c),
            bbox: Rect {
                x0,
                y0: 0.0,
                x1,
                y1: 10.0,
            },
        }
    }

    fn char_cell(x0: f32, x1: f32) -> Cell {
        char_cell_at('x', x0, x1)
    }

    fn line(x0: f32, y0: f32, x1: f32, y1: f32) -> ContentLine {
        ContentLine {
            bbox: Rect { x0, y0, x1, y1 },
            cells: vec![char_cell(x0, x1)],
        }
    }

    fn image_cell() -> Cell {
        Cell {
            kind: CellKind::Image,
            bbox: Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 10.0,
                y1: 10.0,
            },
        }
    }

    #[test]
    fn word_class_identifies_word_cells() {
        assert_eq!(word_class(&char_cell_at('a', 0.0, 1.0)), WordClass::Word);
        assert_eq!(word_class(&char_cell_at('9', 0.0, 1.0)), WordClass::Word);
        assert_eq!(word_class(&char_cell_at('_', 0.0, 1.0)), WordClass::Word);
    }

    #[test]
    fn word_class_identifies_skips_and_single_stops() {
        assert_eq!(
            word_class(&char_cell_at(' ', 0.0, 1.0)),
            WordClass::Whitespace
        );
        assert_eq!(
            word_class(&char_cell_at('-', 0.0, 1.0)),
            WordClass::Punctuation
        );
        assert_eq!(word_class(&image_cell()), WordClass::Image);

        assert!(!is_word_target(WordClass::Whitespace));
        assert!(is_word_target(WordClass::Word));
        assert!(is_word_target(WordClass::Punctuation));
        assert!(is_word_target(WordClass::Image));
    }

    #[test]
    fn word_runs_respect_class_and_boundaries() {
        assert!(continues_word_run(WordClass::Word, WordClass::Word, true));
        assert!(continues_word_run(
            WordClass::Punctuation,
            WordClass::Punctuation,
            true
        ));
        assert!(!continues_word_run(
            WordClass::Word,
            WordClass::Punctuation,
            true
        ));
        assert!(!continues_word_run(WordClass::Word, WordClass::Word, false));
        assert!(!continues_word_run(
            WordClass::Image,
            WordClass::Image,
            true
        ));
        assert!(!continues_word_run(
            WordClass::Whitespace,
            WordClass::Whitespace,
            true
        ));
    }

    #[test]
    fn nearest_cell_picks_containing_cell() {
        let cells = [
            char_cell(0.0, 10.0),
            char_cell(10.0, 20.0),
            char_cell(20.0, 30.0),
        ];
        assert_eq!(nearest_cell_in_line(&cells, 15.0), 1);
        assert_eq!(nearest_cell_in_line(&cells, 25.0), 2);
    }

    #[test]
    fn nearest_cell_clamps_to_edges() {
        let cells = [char_cell(10.0, 20.0), char_cell(20.0, 30.0)];
        // Left of everything -> first cell.
        assert_eq!(nearest_cell_in_line(&cells, -5.0), 0);
        // Right of everything -> last cell.
        assert_eq!(nearest_cell_in_line(&cells, 99.0), 1);
    }

    #[test]
    fn nearest_cell_on_empty_line_is_zero() {
        assert_eq!(nearest_cell_in_line(&[], 12.0), 0);
    }

    #[test]
    fn column_ranges_detects_two_columns() {
        // Two disjoint x-bands (left 0..100, right 200..300), interleaved.
        let lines = [
            line(0.0, 0.0, 100.0, 10.0),
            line(200.0, 0.0, 300.0, 10.0),
            line(0.0, 20.0, 100.0, 30.0),
            line(200.0, 20.0, 300.0, 30.0),
        ];
        let cols = column_ranges(&lines);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0], (0.0, 100.0));
        assert_eq!(cols[1], (200.0, 300.0));
    }

    #[test]
    fn column_ranges_single_column_when_lines_overlap() {
        let lines = [line(0.0, 0.0, 300.0, 10.0), line(10.0, 20.0, 290.0, 30.0)];
        assert_eq!(column_ranges(&lines).len(), 1);
    }

    #[test]
    fn column_index_of_picks_best_overlap() {
        let cols = [(0.0, 100.0), (200.0, 300.0)];
        assert_eq!(column_index_of(&cols, 10.0, 90.0), Some(0));
        assert_eq!(column_index_of(&cols, 210.0, 290.0), Some(1));
        assert_eq!(column_index_of(&[], 0.0, 1.0), None);
    }

    #[test]
    fn nearest_line_in_column_tracks_goal_row() {
        let lines = [
            line(0.0, 0.0, 100.0, 10.0),
            line(200.0, 0.0, 300.0, 10.0),
            line(200.0, 20.0, 300.0, 30.0),
        ];
        // Right column lines are indices 1 and 2; goal_y 25 -> index 2.
        assert_eq!(nearest_line_in_column(&lines, &[1, 2], 25.0), 2);
        assert_eq!(nearest_line_in_column(&lines, &[1, 2], 3.0), 1);
    }
}

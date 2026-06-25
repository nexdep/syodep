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

use syodep_pdf::{Cell, CellKind};

/// Whether `hjkl` scroll the page or move the caret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// `hjkl` scroll the page (the original behavior).
    #[default]
    Normal,
    /// Caret focus mode: `hjkl` move the caret; the view follows it.
    CaretFocus,
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
}

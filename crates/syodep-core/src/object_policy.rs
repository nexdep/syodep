//! Central object/scope policy for motion and auto-search.
//!
//! `syodep-pdf` exposes raw kind bits (`is_atomic`, `is_block`, …). Whether a
//! kind is a movement unit at a given [`Scope`], and whether Sentence/Paragraph
//! auto-search should skip it, are core concerns — they name scopes and are
//! the single place a new kind (Caption, Code, …) must be wired.

use syodep_pdf::ObjectKind;

use crate::caret::Scope;

/// Whether `kind` is one atomic step at `scope`.
///
/// Char scope has no units. Word scope uses [`ObjectKind::is_atomic`]. Line and
/// Paragraph use [`ObjectKind::is_block`]. Sentence uses blocks *except*
/// footnotes, which stay walkable sentence-by-sentence once the caret is
/// inside (auto-search still skips them via [`auto_skip_in_search`]). Captions
/// are blocks at Sentence too — one stop when landed on via line motion — while
/// still being auto-skipped by body `s`/`p` search.
pub fn movement_unit(kind: ObjectKind, scope: Scope) -> bool {
    match scope {
        Scope::Char => false,
        Scope::Word => kind.is_atomic(),
        Scope::Sentence => kind.is_block() && kind != ObjectKind::Footnote,
        Scope::Line | Scope::Paragraph => kind.is_block(),
    }
}

/// Whether Sentence/Paragraph *auto-search* should treat `kind` as invisible.
///
/// Independent of [`movement_unit`]: a footnote is a line-scope block but costs
/// body `s`/`p` zero stops; a caption is the same for auto-search, while still
/// being a Sentence movement unit when the caret is already on it. Char/word/
/// line search never auto-skips.
pub fn auto_skip_in_search(kind: ObjectKind, scope: Scope) -> bool {
    match scope {
        Scope::Sentence | Scope::Paragraph => {
            matches!(kind, ObjectKind::Footnote | ObjectKind::Caption)
        }
        Scope::Char | Scope::Word | Scope::Line => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footnote_is_a_block_except_at_sentence_scope() {
        assert!(!movement_unit(ObjectKind::Footnote, Scope::Char));
        assert!(!movement_unit(ObjectKind::Footnote, Scope::Word));
        assert!(!movement_unit(ObjectKind::Footnote, Scope::Sentence));
        assert!(movement_unit(ObjectKind::Footnote, Scope::Line));
        assert!(movement_unit(ObjectKind::Footnote, Scope::Paragraph));
    }

    #[test]
    fn footnote_is_auto_skipped_only_by_sentence_and_paragraph_search() {
        assert!(auto_skip_in_search(ObjectKind::Footnote, Scope::Sentence));
        assert!(auto_skip_in_search(ObjectKind::Footnote, Scope::Paragraph));
        assert!(!auto_skip_in_search(ObjectKind::Footnote, Scope::Line));
        assert!(!auto_skip_in_search(ObjectKind::Table, Scope::Sentence));
    }

    #[test]
    fn caption_is_a_block_including_at_sentence_scope() {
        assert!(!movement_unit(ObjectKind::Caption, Scope::Char));
        assert!(!movement_unit(ObjectKind::Caption, Scope::Word));
        assert!(movement_unit(ObjectKind::Caption, Scope::Sentence));
        assert!(movement_unit(ObjectKind::Caption, Scope::Line));
        assert!(movement_unit(ObjectKind::Caption, Scope::Paragraph));
    }

    #[test]
    fn caption_is_auto_skipped_like_a_footnote_in_search() {
        assert!(auto_skip_in_search(ObjectKind::Caption, Scope::Sentence));
        assert!(auto_skip_in_search(ObjectKind::Caption, Scope::Paragraph));
        assert!(!auto_skip_in_search(ObjectKind::Caption, Scope::Line));
    }

    #[test]
    fn code_is_a_block_and_never_auto_skipped() {
        assert!(movement_unit(ObjectKind::Code, Scope::Sentence));
        assert!(movement_unit(ObjectKind::Code, Scope::Line));
        assert!(movement_unit(ObjectKind::Code, Scope::Paragraph));
        assert!(!auto_skip_in_search(ObjectKind::Code, Scope::Sentence));
        assert!(!auto_skip_in_search(ObjectKind::Code, Scope::Paragraph));
    }

    #[test]
    fn images_are_atomic_and_blocks() {
        assert!(movement_unit(ObjectKind::Image, Scope::Word));
        assert!(movement_unit(ObjectKind::Image, Scope::Line));
        assert!(movement_unit(ObjectKind::Image, Scope::Sentence));
    }
}

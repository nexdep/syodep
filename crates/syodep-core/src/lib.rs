//! UI-independent application core for syodep.
//!
//! Architectural rule: this crate must never depend on Qt or any UI toolkit.
//! The UI shell forwards input events here and renders whatever this crate
//! tells it to render. All document, navigation, command and persistence
//! logic lives in this crate (or below it).

pub mod app;
pub mod caret;
pub mod command;
pub mod content_session;
pub mod input;
pub mod layout;
pub mod object_policy;
pub mod render_cache;

pub use app::{
    App, DocumentAnchor, Effects, HelpNavigation, Highlight, HighlightSummary, KeybindingHelp,
    KeybindingHelpEntry, KeybindingHelpGroup, TextAnnotation, TextAnnotationSummary, VisiblePage,
};
pub use caret::{Caret, Mode};
pub use command::Command;
pub use input::{InputState, KeyOutcome, Keymap, KeymapError};
pub use layout::{DocumentLayout, View};
pub use syodep_storage::{HighlightId, HighlightPdfState, TextAnnotationId};

//! Caret navigation: a Vim-like cursor that moves through a document's
//! content (text characters and images) independently of scrolling.
//!
//! The caret is *modal* — see [`Mode`] for the four modes and what `hjkl`
//! does in each. What a step of `hjkl` covers within focus, visual or
//! highlight mode is the [`Scope`] — a character, word, line, sentence or
//! paragraph — not a separate mode. Each image is a single caret stop, so the
//! caret traverses text and images uniformly.
//!
//! This module owns the small pieces that are pure and unit-testable in
//! isolation: the position type, the movement direction, and the
//! goal-column cell picker. Orchestration (loading page content, crossing
//! page boundaries, scrolling the caret into view) lives in [`crate::app`],
//! which holds the document and the layout.

use syodep_pdf::{Cell, CellKind, ContentLine, ContentObject, PageContent};

/// Whether `hjkl` scroll the page, move a focus highlight, grow a selection, or
/// shape a highlight.
///
/// There are exactly four modes. Granularity is *not* a mode: [`Focus`],
/// [`Visual`] and [`Highlight`] all carry a [`Scope`], so "word focus" and "line
/// focus" are the same mode holding a different scope.
///
/// [`Focus`]: Mode::Focus
/// [`Visual`]: Mode::Visual
/// [`Highlight`]: Mode::Highlight
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// `hjkl` scroll the page (the original behavior).
    #[default]
    Normal,
    /// Focus mode: one position is highlighted at the active [`Scope`] and
    /// `hjkl`/arrows move it by one unit of that scope; the view follows it.
    Focus,
    /// Visual mode: a two-ended selection. `hjkl`/arrows grow it by one unit of
    /// the active end's [`Scope`]; `o` swaps which end moves.
    Visual,
    /// Highlight mode: a selection being turned into a highlight. Every motion
    /// is visual mode's — a pending highlight *is* a selection, drawn in the
    /// highlight colour — plus keys to keep it or throw it away.
    Highlight,
}

/// A granularity that a position moves and snaps by.
///
/// Focus mode carries one; visual mode carries one *per end*, so a selection
/// can be line-granular at one edge and word-granular at the other (`ve` then
/// `ow`). `App::step_scope` is the single table mapping a scope and a direction
/// onto a motion, shared by focus and visual so a scope cannot mean two
/// different things.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    Char,
    Word,
    Line,
    Sentence,
    Paragraph,
}

impl Scope {
    /// Lower-case name for the status line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Char => "char",
            Self::Word => "word",
            Self::Line => "line",
            Self::Sentence => "sentence",
            Self::Paragraph => "paragraph",
        }
    }
}

/// The anchored end of a visual selection — the half that stays put.
///
/// The *moving* end is not stored here: it is the app's focus position and
/// focus scope. There is exactly one "where you are" in the whole app, and
/// visual mode adds a second endpoint rather than a second position. That is
/// why leaving visual mode by any route keeps your place — there is nothing to
/// carry across, and nothing that can go stale.
///
/// `o` exchanges this end with the focus position, scopes included, so the
/// invariant is simply *the focus position is the end that moves*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualAnchor {
    pub anchor: Caret,
    pub anchor_scope: Scope,
    /// The mode to restore when visual mode is left.
    pub return_mode: Mode,
}

/// A visual-mode selection: two positions, each with its own granularity.
///
/// This is a **read-only view**, assembled by `App::visual_selection` from the
/// focus position (the head) and the stored [`VisualAnchor`]. It is not what
/// the app stores, so mutating a copy changes nothing.
///
/// The rendered selection runs from the outer edge of the earlier snapped end
/// to the outer edge of the later one, so the ends crossing needs no special
/// case and `o` provably never changes what is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualSelection {
    pub anchor: Caret,
    pub anchor_scope: Scope,
    pub head: Caret,
    pub head_scope: Scope,
    /// The mode to restore when visual mode is left with `<Esc>`.
    pub return_mode: Mode,
}

/// A highlight being placed, holding everything needed to undo entering
/// [`Mode::Highlight`].
///
/// Discarding restores all four fields, which is the whole operation — there is
/// no partially-applied state to unwind, because entering highlight mode only
/// ever *adds* an anchor (when coming from focus mode) and never moves the
/// position.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingHighlight {
    /// [`Mode::Focus`] or [`Mode::Visual`]: the mode `a` was pressed in.
    pub return_mode: Mode,
    pub return_focus: Caret,
    pub return_scope: Scope,
    /// The anchor as it was, `None` when entering from focus mode (where the
    /// second end is synthesised on entry and must disappear again).
    pub return_visual: Option<VisualAnchor>,
    /// `#rrggbb`, captured on entry so a config reload mid-highlight cannot
    /// change the colour of a highlight already on screen.
    pub color: String,
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

/// A sentence-focus position: the run of cells from `[start_line, start_cell]`
/// to `[end_line, end_cell]` (inclusive) within a single page. Unlike a
/// [`WordMark`] a sentence may span several lines, but never crosses a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentenceMark {
    pub page: usize,
    pub start_line: usize,
    pub start_cell: usize,
    pub end_line: usize,
    pub end_cell: usize,
}

/// A paragraph-focus position: the inclusive run of lines `start_line..=end_line`
/// within a single page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParagraphMark {
    pub page: usize,
    pub start_line: usize,
    pub end_line: usize,
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
///
/// The field order makes the derived ordering *document order*, which visual
/// mode relies on to find the earlier and later end of a selection without
/// caring which one the user is currently moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Caret {
    pub page: usize,
    pub line: usize,
    pub cell: usize,
}

/// Identity of an atomic object (a table or an image), used to tell "am I
/// still inside the object I started in" without holding a borrow on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectId {
    pub page: usize,
    pub start_line: usize,
}

/// Which end of an atomic object a motion should land on.
///
/// Every scope stepper lands on unit *starts*, so [`Landing::Start`] is the
/// rule everywhere a motion moves the caret. [`Landing::End`] is used only to
/// resolve the *other* edge of a span (`scope_span`, for an atomic object
/// covered at a scope above char): a highlight needs both edges, even though
/// no motion ever lands on the second one directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    Start,
    End,
}

/// Index of the object containing `line`, if any.
///
/// `objects` is sorted by `start_line` with disjoint ranges (an invariant of
/// `syodep_pdf::PageContent`), so a linear scan is both correct and, at the
/// handful of objects a page has, faster than a search.
pub fn object_index_at(objects: &[ContentObject], line: usize) -> Option<usize> {
    objects
        .iter()
        .position(|o| line >= o.start_line && line <= o.end_line)
}

/// Cut paragraph segments so no segment straddles an atomic object, and every
/// object is a segment of its own.
///
/// [`paragraph_segments`] groups lines by vertical gaps, which can merge a
/// figure or a table into the prose next to it. Splitting afterwards keeps
/// that heuristic (and its tests) untouched while making paragraph scope
/// respect object boundaries.
pub fn split_segments_at_objects(
    segments: &[(usize, usize)],
    objects: &[ContentObject],
) -> Vec<(usize, usize)> {
    if objects.is_empty() {
        return segments.to_vec();
    }
    let mut out = Vec::with_capacity(segments.len());
    for &(start, end) in segments {
        let mut cur = start;
        while cur <= end {
            match object_index_at(objects, cur) {
                Some(i) => {
                    let stop = objects[i].end_line.min(end);
                    out.push((cur, stop));
                    cur = stop + 1;
                }
                None => {
                    // Run to just before the next object that starts inside
                    // this segment, or to the segment's end.
                    let next = objects
                        .iter()
                        .filter(|o| o.start_line > cur && o.start_line <= end)
                        .map(|o| o.start_line)
                        .min();
                    let stop = next.map_or(end, |n| n - 1);
                    out.push((cur, stop));
                    cur = stop + 1;
                }
            }
        }
    }
    out
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

/// Whether `c` ends a sentence.
///
/// This is the character's shape alone. A full stop inside a number is not a
/// terminator — see [`is_inside_number`] — but an abbreviation's full stop
/// (`Mr.`) still is, which remains a simplification.
pub fn is_sentence_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

/// Whether `c` can sit *inside* a number, holding its digits together: a
/// decimal point or a digit-grouping separator.
pub fn is_numeric_separator(c: char) -> bool {
    matches!(c, '.' | ',')
}

/// Whether `c` is a letter, digit or underscore — the sides of a dotted token
/// such as `VII.0`, `file.txt` or `3.14`.
fn is_dotted_token_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `c` is a hyphen — a character that *joins* the words it sits
/// between rather than separating them.
///
/// Hyphens only. The en and em dashes (`–`, `—`) punctuate a sentence, so they
/// are deliberately absent: `one—two` is two words. The soft hyphen is here
/// because a PDF can carry one where a word was set to break and did not.
pub fn is_word_hyphen(c: char) -> bool {
    matches!(c, '-' | '\u{2010}' | '\u{2011}' | '\u{00ad}')
}

/// Whether `hyphen`, with `before` and `after` beside it, joins a compound
/// word rather than punctuating the text.
///
/// Same shape as [`is_inside_number`]: word characters on *both* sides. That
/// makes `well-known`, `state-of-the-art` and `COVID-19` each one word, while a
/// dash used as punctuation (`one - two`, `well- known`, `one--two`) keeps the
/// stop of its own it has always had. It composes, so a chain of hyphens holds
/// together throughout.
pub fn is_inside_hyphenated_word(before: Option<char>, hyphen: char, after: Option<char>) -> bool {
    let joinable = |c: char| c.is_alphanumeric() || c == '_';
    is_word_hyphen(hyphen) && before.is_some_and(joinable) && after.is_some_and(joinable)
}

/// Abbreviations whose closing full stop is usually not the end of a sentence.
///
/// Lower-case and without the stop. Dotted initials (`e.g.`, `U.S.`, `Ph.D.`)
/// are *not* here — they are recognised by shape, so they need no list and no
/// maintenance. This is only for the ones a rule cannot infer.
///
/// Several entries are also ordinary words that can genuinely close a sentence
/// (`no.`, `min.`, `co.`). That is safe because the closing stop still ends a
/// sentence when a capital follows it — the list only says "suspect an
/// abbreviation here", never "this is never an ending".
const ABBREVIATIONS: &[&str] = &[
    // Latin and citation
    "al", "approx", "ca", "cf", "et", "etc", "ibid", "op", "resp", "seq", "viz", "vs",
    // References into a document
    "app", "ch", "chap", "eq", "eqs", "fig", "figs", "no", "nos", "p", "para", "pp", "pt", "ref",
    "refs", "sec", "secs", "tab", "tabs", "vol", "vols", // Bibliographic
    "ed", "eds", "orig", "repr", "rev", "suppl", "trans", "transl", // Titles
    "capt", "col", "dr", "gen", "hon", "jr", "lt", "mr", "mrs", "ms", "prof", "sgt", "sr", "st",
    // Months
    "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "sept", "oct", "nov", "dec",
    // Days
    "mon", "tue", "tues", "wed", "thu", "thur", "thurs", "fri", "sat", "sun",
    // Organisations and measurement
    "co", "corp", "dept", "est", "excl", "incl", "inc", "ltd", "max", "min", "univ",
];

/// Whether `token` is a run of short letter groups joined by stops, as in
/// `e.g.`, `i.e.`, `U.S.`, `Ph.D.` or `a.k.a.`.
///
/// Recognised by shape rather than by a list: no sentence ever ends in the
/// middle of one, and no list has to be maintained or translated.
pub fn is_dotted_initials(token: &str) -> bool {
    let body = token.strip_suffix('.').unwrap_or(token);
    let segments: Vec<&str> = body.split('.').collect();
    segments.len() >= 2
        && segments
            .iter()
            .all(|s| (1..=2).contains(&s.chars().count()) && s.chars().all(char::is_alphabetic))
}

/// Whether `token` — with or without its closing stop — is a known
/// abbreviation.
pub fn is_known_abbreviation(token: &str) -> bool {
    let body = token.strip_suffix('.').unwrap_or(token).to_lowercase();
    !body.is_empty() && ABBREVIATIONS.contains(&body.as_str())
}

/// Whether `token` is an abbreviation of either kind.
pub fn is_abbreviation(token: &str) -> bool {
    is_dotted_initials(token) || is_known_abbreviation(token)
}

/// Punctuation that continues the sentence it is in and so can never stand
/// between one sentence and the next.
pub fn is_sentence_continuation(c: char) -> bool {
    matches!(c, ',' | ';' | ':')
}

/// Whether text starting with `next` reads as a new sentence — the first
/// letter or digit after any opening punctuation is a capital, and no
/// continuing punctuation comes first.
///
/// The second half is what carries `(e.g., Smith 2020)`: the capital would
/// otherwise say "new sentence" while the comma in front of it says the text
/// runs on, and the comma is the stronger signal.
///
/// Only consulted where an abbreviation is already suspected. Applied to prose
/// at large it merges real sentences: a technical document routinely starts one
/// with a lower-case identifier.
pub fn opens_a_sentence(next: &str) -> bool {
    next.chars()
        .find(|c| c.is_alphanumeric() || is_sentence_continuation(*c))
        .is_some_and(|c| c.is_uppercase())
}

/// Whether `separator`, with `before` and `after` beside it, is punctuation
/// inside a number rather than between words or sentences.
///
/// This is what makes `3.14` one word and one sentence while the full stop in
/// `costs 3.` still ends both: the test is that digits flank the separator on
/// *both* sides. It composes, so `1,234.56` holds together throughout.
pub fn is_inside_number(before: Option<char>, separator: char, after: Option<char>) -> bool {
    is_numeric_separator(separator)
        && before.is_some_and(|c| c.is_numeric())
        && after.is_some_and(|c| c.is_numeric())
}

/// Whether a full stop joins two alphanumeric (or `_`) sides with no space —
/// a dotted identifier such as `VII.0`, `file.txt`, `a.b.c` or `3.14`.
///
/// Same asymmetry as [`is_inside_number`]: `word.` with nothing after still
/// ends the word and the sentence. Digits alone are covered too, so callers
/// may use this for every `.` case and keep [`is_inside_number`] for grouping
/// commas.
pub fn is_inside_dotted_token(before: Option<char>, separator: char, after: Option<char>) -> bool {
    separator == '.'
        && before.is_some_and(is_dotted_token_char)
        && after.is_some_and(is_dotted_token_char)
}

/// Whether `c` can sign the exponent of a number in scientific notation.
///
/// The Unicode minus is included: a typesetter may well have used it where the
/// author wrote a hyphen.
pub fn is_exponent_sign(c: char) -> bool {
    matches!(c, '+' | '-' | '\u{2212}')
}

/// Whether `sign` is the sign of an exponent in scientific notation — the
/// `+` in `2.3E+5` — given the two characters before it and the one after.
///
/// `before` is the exponent marker (`e`/`E`) and `before2` the digit in front
/// of it. That digit is what separates a number from an identifier: without it
/// `cache+1` would join into one word.
pub fn is_inside_scientific_exponent(
    before2: Option<char>,
    before: Option<char>,
    sign: char,
    after: Option<char>,
) -> bool {
    is_exponent_sign(sign)
        && before.is_some_and(|c| c == 'e' || c == 'E')
        && before2.is_some_and(|c| c.is_numeric())
        && after.is_some_and(|c| c.is_numeric())
}

/// Whether `c` is a sign that belongs to the figure it *follows*, as the `%` in
/// `45.5%` does.
///
/// Only the proportion signs, which are always written tight against the
/// figure and carry nothing after them. A unit (`°C`, `kg`) is deliberately not
/// here: the letters following it raise a question of their own, and `45.5 %`
/// or a bare `%` must stay untouched — hence the digit required immediately
/// before.
pub fn is_number_suffix(c: char) -> bool {
    matches!(c, '%' | '‰' | '‱')
}

/// Whether `c`, with `before` beside it, is a suffix attached to a number
/// rather than a symbol standing on its own.
pub fn is_attached_number_suffix(before: Option<char>, c: char) -> bool {
    is_number_suffix(c) && before.is_some_and(|b| b.is_numeric())
}

/// Brackets and quotes a link may be wrapped in.
fn is_link_opener(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | '<' | '"' | '\'' | '«' | '“' | '‘')
}

/// Punctuation that follows a link in prose without belonging to it. Closing
/// brackets are handled separately, since a link may contain a matched pair.
fn is_link_trailer(c: char) -> bool {
    matches!(
        c,
        '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '>' | '»' | '”' | '’'
    )
}

/// The opening bracket `c` closes, if it is a closing bracket.
fn matching_opener(c: char) -> Option<char> {
    match c {
        ')' => Some('('),
        ']' => Some('['),
        '}' => Some('{'),
        _ => None,
    }
}

/// Schemes that are followed by `:` alone rather than `://`.
const SCHEMES_WITHOUT_AUTHORITY: &[&str] = &["mailto", "doi", "tel", "urn", "arxiv"];

/// Whether `s` is shaped like a URL scheme (`https`, `ftp`, `git+ssh`).
fn is_url_scheme(s: &str) -> bool {
    (1..=16).contains(&s.chars().count())
        && s.starts_with(char::is_alphabetic)
        && s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Whether `host` is a dotted host name: labels of word characters, with a
/// plausible alphabetic top-level domain last.
fn is_dotted_host(host: &str) -> bool {
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        })
        && labels.last().is_some_and(|tld| {
            (2..=24).contains(&tld.chars().count()) && tld.chars().all(char::is_alphabetic)
        })
}

/// Whether `token` is a link: a URL, or an email address.
///
/// Deliberately conservative about the form with no scheme. A bare
/// `example.com` is *not* recognised, because extraction that drops a space
/// leaves `sentence.Next` looking exactly like it, and treating that as a link
/// would both glue two words together and swallow a sentence boundary. A path
/// (`doi.org/10.1000/182`) or a `www.` prefix is required instead — the forms
/// that cannot be an accident of a missing space.
fn is_link(token: &str) -> bool {
    let lower = token.to_lowercase();
    if let Some(i) = lower.find("://") {
        return is_url_scheme(&lower[..i]) && i + 3 < lower.len();
    }
    if let Some((scheme, rest)) = lower.split_once(':') {
        if SCHEMES_WITHOUT_AUTHORITY.contains(&scheme) {
            return !rest.is_empty();
        }
    }
    if let Some((local, host)) = lower.split_once('@') {
        if !local.is_empty() && !local.contains('/') && is_dotted_host(host) {
            return true;
        }
    }
    match lower.split_once('/') {
        Some((host, _)) => is_dotted_host(host),
        None => lower.starts_with("www.") && is_dotted_host(&lower),
    }
}

/// The inclusive char-index range of the link inside `token`, if it holds one.
///
/// The punctuation prose wraps a link in is trimmed off first, so
/// `(https://example.com/a),` yields just the address. A closing bracket the
/// link itself opened is kept: `…/Glob_(pattern)` ends with its own `)`.
pub fn link_span(token: &str) -> Option<(usize, usize)> {
    let chars: Vec<char> = token.chars().collect();
    let mut lo = 0;
    let mut hi = chars.len().checked_sub(1)?;
    while lo < hi && is_link_opener(chars[lo]) {
        lo += 1;
    }
    while hi > lo {
        let here = chars[hi];
        if is_link_trailer(here) {
            hi -= 1;
            continue;
        }
        match matching_opener(here) {
            Some(open) if !chars[lo..hi].contains(&open) => hi -= 1,
            _ => break,
        }
    }
    let core: String = chars[lo..=hi].iter().collect();
    is_link(&core).then_some((lo, hi))
}

/// Whether `c` is a closing character that stays attached to the end of a
/// sentence after its terminator (so `."` / `.)` close together).
pub fn is_sentence_trailer(c: char) -> bool {
    matches!(c, '"' | '\'' | ')' | ']' | '}' | '»' | '”' | '’')
}

/// Whether cell `idx` is a colon whose only followers on the line are
/// whitespace characters — the line ends at that colon.
///
/// Image cells count as non-whitespace, so `colon` + image does not qualify.
/// Used by sentence-boundary detection and by [`line_ends_with_colon`].
pub fn is_line_final_colon(cells: &[Cell], idx: usize) -> bool {
    matches!(cells.get(idx).map(|c| &c.kind), Some(CellKind::Char(':')))
        && cells[idx + 1..].iter().all(|c| match c.kind {
            CellKind::Char(ch) => ch.is_whitespace(),
            CellKind::Image => false,
        })
}

/// Whether the last non-whitespace cell of `cells` is a colon.
///
/// The paragraph-break form of [`is_line_final_colon`]: a line that ends in
/// `:` (optionally followed by spaces) starts a new paragraph at the next
/// line.
pub fn line_ends_with_colon(cells: &[Cell]) -> bool {
    cells
        .iter()
        .rposition(|c| match c.kind {
            CellKind::Char(ch) => !ch.is_whitespace(),
            CellKind::Image => true,
        })
        .is_some_and(|i| is_line_final_colon(cells, i))
}

/// A new paragraph starts when the vertical gap between consecutive lines
/// exceeds this fraction of the median line height.
pub const PARAGRAPH_GAP_FACTOR: f32 = 0.75;

/// Split a page's lines into paragraphs, returned as inclusive
/// `(start_line, end_line)` index ranges over `lines`.
///
/// Consecutive non-empty lines belong to the same paragraph until a break is
/// detected: a column change (reusing [`column_ranges`]/[`column_index_of`]), a
/// vertical gap larger than [`PARAGRAPH_GAP_FACTOR`] times the median line
/// height, the y-coordinate jumping back upward (a column/region reset), or
/// the previous line ending in a colon (optionally followed by spaces).
/// Empty lines are skipped, mirroring the rest of the navigation code.
///
/// Pure so it can be unit-tested in isolation, like [`column_ranges`].
pub fn paragraph_segments(lines: &[ContentLine]) -> Vec<(usize, usize)> {
    let cols = column_ranges(lines);
    let non_empty: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].cells.is_empty())
        .collect();
    if non_empty.is_empty() {
        return Vec::new();
    }
    // Median height of non-empty lines, for the gap threshold.
    let mut heights: Vec<f32> = non_empty
        .iter()
        .map(|&i| lines[i].bbox.y1 - lines[i].bbox.y0)
        .collect();
    heights.sort_by(f32::total_cmp);
    let median_height = heights[heights.len() / 2];
    let threshold = PARAGRAPH_GAP_FACTOR * median_height;

    let col_of = |i: usize| {
        let b = lines[i].bbox;
        column_index_of(&cols, b.x0, b.x1)
    };

    let mut segments = Vec::new();
    let mut start = non_empty[0];
    for w in non_empty.windows(2) {
        let (prev, cur) = (w[0], w[1]);
        let pb = lines[prev].bbox;
        let cb = lines[cur].bbox;
        let breaks = col_of(prev) != col_of(cur)
            || cb.y0 - pb.y1 > threshold
            || cb.y0 < pb.y0
            || line_ends_with_colon(&lines[prev].cells);
        if breaks {
            segments.push((start, prev));
            start = cur;
        }
    }
    segments.push((start, *non_empty.last().unwrap()));
    segments
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
/// Lines are grouped greedily by horizontal overlap. A line that overlaps
/// several columns merges them: keeping only the first match left nested
/// fragments that confused [`column_index_of`].
///
/// Spanning lines — figures, captions, keywords, table titles that cross the
/// page midpoint — are ignored when both sides of the midpoint already have
/// enough non-spanning text. Without that filter a single gutter-crossing
/// line collapses a two-column article into one column and `h`/`l` stop
/// jumping. Image lines are ignored for the same reason. If either side is
/// too thin, detection falls back to every non-empty line so a single-column
/// page stays one column.
pub fn column_ranges(lines: &[ContentLine]) -> Vec<(f32, f32)> {
    let non_empty: Vec<&ContentLine> = lines.iter().filter(|l| !l.cells.is_empty()).collect();
    if non_empty.is_empty() {
        return Vec::new();
    }

    let text_lines: Vec<&ContentLine> = non_empty
        .iter()
        .copied()
        .filter(|l| !l.cells.iter().any(|c| matches!(c.kind, CellKind::Image)))
        .collect();

    if let Some(seed) = column_seed_lines(&text_lines) {
        let cols = coalesce_column_ranges(accumulate_column_ranges(&seed));
        // Slightly wide lines can still glue the two bands during overlap
        // merge even after gutter-spanners are filtered. Rebuild from line
        // centres when seeding clearly saw both sides.
        if cols.len() >= 2 {
            cols
        } else {
            center_cluster_columns(&seed).unwrap_or(cols)
        }
    } else {
        coalesce_column_ranges(accumulate_column_ranges(&non_empty))
    }
}

fn line_center_x(line: &ContentLine) -> f32 {
    (line.bbox.x0 + line.bbox.x1) * 0.5
}

/// Non-spanning text lines to seed column detection, when both sides of the
/// page midpoint look like real columns.
fn column_seed_lines<'a>(text_lines: &[&'a ContentLine]) -> Option<Vec<&'a ContentLine>> {
    if text_lines.len() < 4 {
        return None;
    }
    let page_x0 = text_lines
        .iter()
        .map(|l| l.bbox.x0)
        .fold(f32::INFINITY, f32::min);
    let page_x1 = text_lines
        .iter()
        .map(|l| l.bbox.x1)
        .fold(f32::NEG_INFINITY, f32::max);
    if !page_x0.is_finite() || page_x1 <= page_x0 {
        return None;
    }
    let mid = (page_x0 + page_x1) * 0.5;
    // Drop true gutter-spanners; classify the rest by centre so a line that
    // merely overhangs the mid by a few points still belongs to one side.
    let columnar: Vec<&ContentLine> = text_lines
        .iter()
        .copied()
        .filter(|l| !(l.bbox.x0 < mid && l.bbox.x1 > mid))
        .collect();
    let left = columnar.iter().filter(|l| line_center_x(l) < mid).count();
    let right = columnar.iter().filter(|l| line_center_x(l) > mid).count();
    (left >= 2 && right >= 2).then_some(columnar)
}

/// Split seed lines into two columns at the largest gap between sorted centres.
fn center_cluster_columns(lines: &[&ContentLine]) -> Option<Vec<(f32, f32)>> {
    if lines.len() < 4 {
        return None;
    }
    let mut indexed: Vec<(f32, usize)> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| (line_center_x(l), i))
        .collect();
    indexed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut best_gap = 0.0_f32;
    let mut split = 0usize;
    for (k, w) in indexed.windows(2).enumerate() {
        let gap = w[1].0 - w[0].0;
        if gap > best_gap {
            best_gap = gap;
            split = k + 1;
        }
    }
    if split < 2 || indexed.len() - split < 2 {
        return None;
    }
    // Require a real gutter, not ordinary indentation jitter.
    let page_width = {
        let x0 = lines
            .iter()
            .map(|l| l.bbox.x0)
            .fold(f32::INFINITY, f32::min);
        let x1 = lines
            .iter()
            .map(|l| l.bbox.x1)
            .fold(f32::NEG_INFINITY, f32::max);
        x1 - x0
    };
    if best_gap < 0.08 * page_width {
        return None;
    }
    let left_idxs: Vec<usize> = indexed[..split].iter().map(|(_, i)| *i).collect();
    let right_idxs: Vec<usize> = indexed[split..].iter().map(|(_, i)| *i).collect();
    let range = |idxs: &[usize]| {
        let x0 = idxs
            .iter()
            .map(|&i| lines[i].bbox.x0)
            .fold(f32::INFINITY, f32::min);
        let x1 = idxs
            .iter()
            .map(|&i| lines[i].bbox.x1)
            .fold(f32::NEG_INFINITY, f32::max);
        (x0, x1)
    };
    let (l0, l1) = range(&left_idxs);
    let (r0, r1) = range(&right_idxs);
    if l1 >= r0 {
        // Ranges still overlap — not a clean split.
        return None;
    }
    Some(vec![(l0, l1), (r0, r1)])
}

/// Greedy overlap grouping: a line starts a column or merges into every
/// column it overlaps.
fn accumulate_column_ranges(lines: &[&ContentLine]) -> Vec<(f32, f32)> {
    let mut cols: Vec<(f32, f32)> = Vec::new();
    for line in lines {
        let (lx0, lx1) = (line.bbox.x0, line.bbox.x1);
        let overlapping: Vec<usize> = cols
            .iter()
            .enumerate()
            .filter(|(_, (cx0, cx1))| lx0 <= *cx1 && lx1 >= *cx0)
            .map(|(i, _)| i)
            .collect();
        match overlapping.as_slice() {
            [] => cols.push((lx0, lx1)),
            &[i] => {
                cols[i].0 = cols[i].0.min(lx0);
                cols[i].1 = cols[i].1.max(lx1);
            }
            _ => {
                let mut nx0 = lx0;
                let mut nx1 = lx1;
                for &i in &overlapping {
                    nx0 = nx0.min(cols[i].0);
                    nx1 = nx1.max(cols[i].1);
                }
                let mut idxs = overlapping;
                idxs.sort_unstable_by(|a, b| b.cmp(a));
                for i in idxs {
                    cols.remove(i);
                }
                cols.push((nx0, nx1));
            }
        }
    }
    cols.sort_by(|a, b| a.0.total_cmp(&b.0));
    cols
}

/// Collapse nested or overlapping column ranges left by processing order.
fn coalesce_column_ranges(cols: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let mut merged: Vec<(f32, f32)> = Vec::new();
    for (x0, x1) in cols {
        if let Some(last) = merged.last_mut() {
            if x0 <= last.1 {
                last.1 = last.1.max(x1);
                continue;
            }
        }
        merged.push((x0, x1));
    }
    merged
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

/// The part of an inclusive `start..=end` cell span that falls on `page`, as one
/// page-space rectangle per covered line.
///
/// A fully covered block — a table, an image or a display equation — becomes a
/// single rectangle over its own bounds: per-line boxes would leave a table's
/// rules and empty cells unpainted, and a formula's rows ragged, which reads as
/// a broken highlight.
///
/// Deliberately keyed on how far the span reaches rather than on the scope it
/// came from, so the two agree without this having to know about scopes: from
/// line scope up a span covers a block end to end and collapses, while a word-
/// or char-sized span inside one covers only part of it and does not.
///
/// Pure and page-local so it can serve both consumers of a span — the overlay,
/// which asks only about visible pages, and storing a highlight, which asks
/// about all of them — without either growing its own copy of the geometry
/// rules.
pub fn page_span_rects(
    content: &PageContent,
    page: usize,
    start: Caret,
    end: Caret,
) -> Vec<syodep_pdf::Rect> {
    let mut rects = Vec::new();
    if page < start.page || page > end.page {
        return rects;
    }
    let lines = &content.lines;
    let first_line = if page == start.page { start.line } else { 0 };
    let last_line = if page == end.page {
        end.line
    } else {
        lines.len().saturating_sub(1)
    };
    let mut line_idx = first_line;
    while line_idx <= last_line {
        if let Some(object) = content.block_object_at(line_idx) {
            let whole = object.start_line == line_idx
                && object.end_line <= last_line
                && !(page == start.page && start.line == line_idx && start.cell > 0)
                && !(page == end.page
                    && end.line == object.end_line
                    && end.cell + 1 < lines.get(object.end_line).map_or(0, |l| l.cells.len()));
            if whole {
                rects.push(object.bbox);
                line_idx = object.end_line + 1;
                continue;
            }
        }
        let Some(line) = lines.get(line_idx) else {
            line_idx += 1;
            continue;
        };
        if line.cells.is_empty() {
            line_idx += 1;
            continue;
        }
        let at_start = page == start.page && line_idx == start.line;
        let at_end = page == end.page && line_idx == end.line;
        let span = match (at_start, at_end) {
            (true, true) => line.cells.get(start.cell).map(|s| {
                let e = line.cells.get(end.cell).map_or(s.bbox, |c| c.bbox);
                (s.bbox.x0.min(e.x0), s.bbox.x1.max(e.x1))
            }),
            (true, false) => line
                .cells
                .get(start.cell)
                .map(|s| (s.bbox.x0, line.bbox.x1)),
            (false, true) => line.cells.get(end.cell).map(|e| (line.bbox.x0, e.bbox.x1)),
            (false, false) => Some((line.bbox.x0, line.bbox.x1)),
        };
        // A cell index off the end of its line means the span and the content
        // disagree; skipping the line degrades to a shorter highlight, which is
        // always safer than a wrong one.
        if let Some((x0, x1)) = span {
            rects.push(syodep_pdf::Rect {
                x0,
                y0: line.bbox.y0,
                x1,
                y1: line.bbox.y1,
            });
        }
        line_idx += 1;
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use syodep_pdf::{CellKind, Rect};

    #[test]
    fn caret_ordering_is_document_order() {
        let at = |page, line, cell| Caret { page, line, cell };
        // Page dominates line, which dominates cell.
        assert!(at(0, 9, 9) < at(1, 0, 0));
        assert!(at(0, 0, 9) < at(0, 1, 0));
        assert!(at(0, 0, 0) < at(0, 0, 1));
        // min/max pick the document-order ends regardless of argument order.
        let (a, b) = (at(2, 1, 3), at(0, 5, 0));
        assert_eq!(a.min(b), at(0, 5, 0));
        assert_eq!(a.max(b), at(2, 1, 3));
    }

    fn char_cell_at(c: char, x0: f32, x1: f32) -> Cell {
        Cell {
            kind: CellKind::Char(c),
            bbox: Rect {
                x0,
                y0: 0.0,
                x1,
                y1: 10.0,
            },
            synthetic: false,
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
            synthetic: false,
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
    fn column_ranges_ignores_a_gutter_spanning_line() {
        // IOP shape: two columns of body text plus a Keywords / caption line
        // that crosses the gutter. Without filtering, that one line merges
        // both columns and `h`/`l` stop jumping.
        let lines = [
            line(0.0, 0.0, 100.0, 10.0),
            line(200.0, 0.0, 300.0, 10.0),
            line(0.0, 20.0, 100.0, 30.0),
            line(200.0, 20.0, 300.0, 30.0),
            line(40.0, 40.0, 260.0, 50.0), // spans the midpoint
        ];
        let cols = column_ranges(&lines);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0], (0.0, 100.0));
        assert_eq!(cols[1], (200.0, 300.0));
    }

    #[test]
    fn column_ranges_ignores_images_when_seeding() {
        let mut image = line(50.0, 40.0, 250.0, 120.0);
        image.cells = vec![image_cell()];
        image.cells[0].bbox = image.bbox;
        let lines = [
            line(0.0, 0.0, 100.0, 10.0),
            line(200.0, 0.0, 300.0, 10.0),
            line(0.0, 20.0, 100.0, 30.0),
            line(200.0, 20.0, 300.0, 30.0),
            image,
        ];
        let cols = column_ranges(&lines);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0], (0.0, 100.0));
        assert_eq!(cols[1], (200.0, 300.0));
    }

    #[test]
    fn column_ranges_merges_nested_fragments() {
        // A short mid-column line processed before its full-width neighbour
        // used to leave a nested second column inside the left band.
        let lines = [
            line(0.0, 0.0, 60.0, 10.0),
            line(40.0, 20.0, 100.0, 30.0),
            line(0.0, 40.0, 100.0, 50.0),
            line(200.0, 0.0, 300.0, 10.0),
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
    fn column_ranges_recovers_two_columns_when_overlap_merge_would_glue_them() {
        // Body columns whose boxes nearly touch: greedy x-overlap merge
        // collapses them, but centres still show a clear gutter.
        let lines = [
            line(40.0, 0.0, 290.0, 10.0),
            line(300.0, 0.0, 550.0, 10.0),
            line(40.0, 20.0, 290.0, 30.0),
            line(300.0, 20.0, 550.0, 30.0),
            line(40.0, 40.0, 290.0, 50.0),
            line(300.0, 40.0, 550.0, 50.0),
        ];
        let cols = column_ranges(&lines);
        assert_eq!(
            cols.len(),
            2,
            "expected centre-cluster recovery, got {cols:?}"
        );
        assert!(cols[0].1 < cols[1].0);
    }

    #[test]
    fn column_index_of_picks_best_overlap() {
        let cols = [(0.0, 100.0), (200.0, 300.0)];
        assert_eq!(column_index_of(&cols, 10.0, 90.0), Some(0));
        assert_eq!(column_index_of(&cols, 210.0, 290.0), Some(1));
        assert_eq!(column_index_of(&[], 0.0, 1.0), None);
    }

    #[test]
    fn sentence_classifiers() {
        assert!(is_sentence_terminator('.'));
        assert!(is_sentence_terminator('!'));
        assert!(is_sentence_terminator('?'));
        assert!(!is_sentence_terminator(','));
        assert!(is_sentence_trailer('"'));
        assert!(is_sentence_trailer(')'));
        assert!(!is_sentence_trailer('a'));
    }

    #[test]
    fn paragraph_segments_groups_tightly_spaced_lines() {
        // Three lines stacked with ~line-height spacing: one paragraph.
        let lines = [
            line(0.0, 0.0, 100.0, 10.0),
            line(0.0, 12.0, 100.0, 22.0),
            line(0.0, 24.0, 100.0, 34.0),
        ];
        assert_eq!(paragraph_segments(&lines), vec![(0, 2)]);
    }

    #[test]
    fn paragraph_segments_splits_on_large_gap() {
        // A big vertical gap before the third line starts a new paragraph.
        let lines = [
            line(0.0, 0.0, 100.0, 10.0),
            line(0.0, 12.0, 100.0, 22.0),
            line(0.0, 80.0, 100.0, 90.0),
        ];
        assert_eq!(paragraph_segments(&lines), vec![(0, 1), (2, 2)]);
    }

    #[test]
    fn paragraph_segments_splits_on_column_change() {
        // Two columns: left lines, then a right-column line.
        let lines = [
            line(0.0, 0.0, 100.0, 10.0),
            line(0.0, 12.0, 100.0, 22.0),
            line(200.0, 0.0, 300.0, 10.0),
        ];
        assert_eq!(paragraph_segments(&lines), vec![(0, 1), (2, 2)]);
    }

    #[test]
    fn paragraph_segments_single_line_and_empty() {
        assert_eq!(
            paragraph_segments(&[line(0.0, 0.0, 100.0, 10.0)]),
            vec![(0, 0)]
        );
        assert!(paragraph_segments(&[]).is_empty());
    }

    /// One content line of `text` at `y`, with a 6pt-wide cell per character.
    fn text_line(y: f32, text: &str) -> ContentLine {
        let cells: Vec<Cell> = text
            .chars()
            .enumerate()
            .map(|(i, ch)| {
                let x = i as f32 * 6.0;
                Cell {
                    kind: CellKind::Char(ch),
                    bbox: Rect {
                        x0: x,
                        y0: y,
                        x1: x + 6.0,
                        y1: y + 10.0,
                    },
                    synthetic: false,
                }
            })
            .collect();
        ContentLine {
            bbox: Rect {
                x0: 0.0,
                y0: y,
                x1: text.chars().count() as f32 * 6.0,
                y1: y + 10.0,
            },
            cells,
        }
    }

    #[test]
    fn line_final_colon_at_eol_and_with_trailing_spaces() {
        let cells = text_line(0.0, "lead-in:").cells;
        assert!(is_line_final_colon(&cells, cells.len() - 1));
        assert!(line_ends_with_colon(&cells));

        let cells = text_line(0.0, "lead-in:  ").cells;
        assert!(is_line_final_colon(&cells, 7));
        assert!(!is_line_final_colon(&cells, 8)); // a trailing space
        assert!(line_ends_with_colon(&cells));
    }

    #[test]
    fn mid_line_colon_is_not_line_final() {
        let cells = text_line(0.0, "Note: more.").cells;
        assert!(!is_line_final_colon(&cells, 4));
        assert!(!line_ends_with_colon(&cells));
    }

    #[test]
    fn colon_followed_by_an_image_is_not_line_final() {
        let mut cells = text_line(0.0, "lead-in:").cells;
        cells.push(image_cell());
        assert!(!is_line_final_colon(&cells, 7));
        assert!(!line_ends_with_colon(&cells));
    }

    #[test]
    fn paragraph_segments_splits_after_a_line_final_colon() {
        // Tightly spaced — no vertical gap — but the colon line still breaks.
        let lines = [
            text_line(0.0, "A lead-in:"),
            text_line(12.0, "Continued here."),
        ];
        assert_eq!(paragraph_segments(&lines), vec![(0, 0), (1, 1)]);
    }

    #[test]
    fn paragraph_segments_keeps_a_mid_line_colon_together() {
        let lines = [
            text_line(0.0, "Note: more words."),
            text_line(12.0, "Still same para."),
        ];
        assert_eq!(paragraph_segments(&lines), vec![(0, 1)]);
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

    fn object(start_line: usize, end_line: usize) -> ContentObject {
        ContentObject {
            kind: syodep_pdf::ObjectKind::Table,
            bbox: Rect {
                x0: 0.0,
                y0: 0.0,
                x1: 100.0,
                y1: 100.0,
            },
            start_line,
            end_line,
        }
    }

    #[test]
    fn dotted_initials_are_recognised_by_shape() {
        for token in ["e.g.", "i.e.", "U.S.", "Ph.D.", "a.k.a.", "e.g"] {
            assert!(is_dotted_initials(token), "{token}");
        }
    }

    #[test]
    fn ordinary_words_are_not_dotted_initials() {
        // One stop after a whole word is not a run of initials, and a decimal
        // is not one either.
        for token in ["end.", "etc.", "3.14", "Fig.", "word", "."] {
            assert!(!is_dotted_initials(token), "{token}");
        }
    }

    #[test]
    fn known_abbreviations_are_matched_without_their_stop_and_by_any_case() {
        // Tokens are whitespace-delimited, so `et al.` arrives as two of them
        // and it is `al.` that carries the stop.
        for token in ["etc.", "Fig.", "fig", "AL.", "vol.", "Dr.", "Sept."] {
            assert!(is_known_abbreviation(token), "{token}");
        }
        for token in ["end", "the", "database", ""] {
            assert!(!is_known_abbreviation(token), "{token}");
        }
    }

    #[test]
    fn a_capital_after_the_stop_opens_a_sentence() {
        assert!(opens_a_sentence(" The next one"));
        assert!(opens_a_sentence(" (Then again"));
        // A lower-case word or a figure continues what came before.
        assert!(!opens_a_sentence(" and then"));
        assert!(!opens_a_sentence(" 3 items"));
        assert!(!opens_a_sentence(""));
    }

    #[test]
    fn continuing_punctuation_before_the_capital_does_not() {
        // "(e.g., Smith 2020)" — the comma says the text runs on, whatever the
        // capital after it suggests.
        assert!(!opens_a_sentence(", Smith 2020)"));
        assert!(!opens_a_sentence("; Then again"));
        assert!(!opens_a_sentence(": Then again"));
        // It has to come first: a capital reached before any such mark still
        // opens a sentence.
        assert!(opens_a_sentence(" Then again, more"));
    }

    #[test]
    fn a_separator_between_digits_is_inside_a_number() {
        assert!(is_inside_number(Some('3'), '.', Some('1')));
        assert!(is_inside_number(Some('1'), ',', Some('2')));
    }

    #[test]
    fn a_separator_without_digits_on_both_sides_is_not() {
        // "costs 3." — nothing follows, so the stop still ends the sentence.
        assert!(!is_inside_number(Some('3'), '.', Some(' ')));
        assert!(!is_inside_number(Some('3'), '.', None));
        // "etc. 5" — a full stop that merely happens to precede a figure
        // (with a space between; the adjacent-char case `c.5` is a dotted
        // token, not a number).
        assert!(!is_inside_number(Some('c'), '.', Some('5')));
        // Only a decimal point or a grouping comma joins digits.
        assert!(!is_inside_number(Some('3'), '-', Some('1')));
        assert!(!is_inside_number(Some('3'), '!', Some('1')));
    }

    #[test]
    fn a_dot_between_alphanumeric_sides_is_inside_a_dotted_token() {
        assert!(is_inside_dotted_token(Some('I'), '.', Some('0'))); // VII.0
        assert!(is_inside_dotted_token(Some('e'), '.', Some('t'))); // file.txt
        assert!(is_inside_dotted_token(Some('a'), '.', Some('b'))); // a.b.c
        assert!(is_inside_dotted_token(Some('3'), '.', Some('1'))); // 3.14
        assert!(is_inside_dotted_token(Some('_'), '.', Some('x')));
    }

    #[test]
    fn a_dot_without_alphanumeric_sides_is_not_a_dotted_token() {
        assert!(!is_inside_dotted_token(Some('3'), '.', Some(' ')));
        assert!(!is_inside_dotted_token(Some('3'), '.', None));
        assert!(!is_inside_dotted_token(Some(' '), '.', Some('5')));
        assert!(!is_inside_dotted_token(Some('c'), ',', Some('5')));
    }

    /// The link `link_span` finds in `token`, for readable assertions.
    fn link_of(token: &str) -> Option<String> {
        let chars: Vec<char> = token.chars().collect();
        link_span(token).map(|(lo, hi)| chars[lo..=hi].iter().collect())
    }

    #[test]
    fn link_span_finds_a_url_inside_the_prose_around_it() {
        assert_eq!(
            link_of("https://example.com/a?x=1").as_deref(),
            Some("https://example.com/a?x=1")
        );
        // Trailing sentence punctuation is not part of the address.
        assert_eq!(
            link_of("https://example.com.").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            link_of("(https://example.com/a),").as_deref(),
            Some("https://example.com/a")
        );
        // ...but a bracket the link itself opened is.
        assert_eq!(
            link_of("https://x.org/wiki/Glob_(pattern)").as_deref(),
            Some("https://x.org/wiki/Glob_(pattern)")
        );
        // Scheme-only and host forms.
        assert_eq!(
            link_of("mailto:jane@example.com").as_deref(),
            Some("mailto:jane@example.com")
        );
        assert_eq!(
            link_of("www.example.com").as_deref(),
            Some("www.example.com")
        );
        assert_eq!(
            link_of("doi.org/10.1000/182").as_deref(),
            Some("doi.org/10.1000/182")
        );
        assert_eq!(
            link_of("jane.doe@example.com").as_deref(),
            Some("jane.doe@example.com")
        );
    }

    #[test]
    fn link_span_leaves_ordinary_tokens_alone() {
        // A slash between two words is not a path.
        assert_eq!(link_of("and/or"), None);
        assert_eq!(link_of("km/h"), None);
        assert_eq!(link_of("TCP/IP"), None);
        // A bare host is not recognised: extraction that drops a space makes
        // ordinary prose look exactly like one.
        assert_eq!(link_of("sentence.Next"), None);
        assert_eq!(link_of("example.com"), None);
        // Abbreviations and figures keep out of it.
        assert_eq!(link_of("e.g."), None);
        assert_eq!(link_of("1,234.56"), None);
        assert_eq!(link_of("10.1000/182"), None);
        // Nothing left after trimming.
        assert_eq!(link_of("(),"), None);
        assert_eq!(link_of(""), None);
    }

    #[test]
    fn a_signed_exponent_needs_a_marker_and_a_digit_behind_it() {
        // "2.3E+5" and "1.5e-10".
        assert!(is_inside_scientific_exponent(
            Some('3'),
            Some('E'),
            '+',
            Some('5')
        ));
        assert!(is_inside_scientific_exponent(
            Some('5'),
            Some('e'),
            '-',
            Some('1')
        ));
        // The Unicode minus a typesetter may have used instead.
        assert!(is_inside_scientific_exponent(
            Some('5'),
            Some('e'),
            '\u{2212}',
            Some('1')
        ));
        // "cache+1": an identifier that merely ends in `e`.
        assert!(!is_inside_scientific_exponent(
            Some('h'),
            Some('e'),
            '+',
            Some('1')
        ));
        // "3.5+1": no exponent marker at all.
        assert!(!is_inside_scientific_exponent(
            Some('.'),
            Some('5'),
            '+',
            Some('1')
        ));
        // "2.3E+x": the exponent has to be a figure.
        assert!(!is_inside_scientific_exponent(
            Some('3'),
            Some('E'),
            '+',
            Some('x')
        ));
    }

    #[test]
    fn a_proportion_sign_after_a_digit_belongs_to_the_number() {
        assert!(is_attached_number_suffix(Some('5'), '%'));
        assert!(is_attached_number_suffix(Some('0'), '‰'));
        // "the % sign" and "45.5 %": nothing to attach to.
        assert!(!is_attached_number_suffix(Some(' '), '%'));
        assert!(!is_attached_number_suffix(None, '%'));
        // A unit is not a suffix of this kind.
        assert!(!is_attached_number_suffix(Some('5'), '°'));
    }

    #[test]
    fn a_hyphen_between_word_characters_is_inside_a_compound() {
        assert!(is_inside_hyphenated_word(Some('l'), '-', Some('k')));
        // Letters and digits both count: "COVID-19", "3-D".
        assert!(is_inside_hyphenated_word(Some('D'), '-', Some('1')));
        assert!(is_inside_hyphenated_word(Some('3'), '-', Some('D')));
        // The typographic hyphens join too, including the soft one a PDF can
        // carry where a word was set to break.
        assert!(is_inside_hyphenated_word(Some('l'), '\u{2010}', Some('k')));
        assert!(is_inside_hyphenated_word(Some('l'), '\u{2011}', Some('k')));
        assert!(is_inside_hyphenated_word(Some('l'), '\u{00ad}', Some('k')));
    }

    #[test]
    fn a_hyphen_without_word_characters_on_both_sides_is_not() {
        // "one - two" and "well- known": a dash standing on its own.
        assert!(!is_inside_hyphenated_word(Some(' '), '-', Some('t')));
        assert!(!is_inside_hyphenated_word(Some('l'), '-', Some(' ')));
        assert!(!is_inside_hyphenated_word(Some('l'), '-', None));
        // "one--two": neither hyphen has a word character on both sides.
        assert!(!is_inside_hyphenated_word(Some('e'), '-', Some('-')));
        // Dashes punctuate rather than join, however tightly they are set.
        assert!(!is_inside_hyphenated_word(Some('e'), '\u{2013}', Some('t')));
        assert!(!is_inside_hyphenated_word(Some('e'), '\u{2014}', Some('t')));
    }

    #[test]
    fn object_index_at_finds_the_containing_object() {
        let objects = [object(2, 5), object(8, 8)];
        assert_eq!(object_index_at(&objects, 1), None);
        assert_eq!(object_index_at(&objects, 2), Some(0));
        assert_eq!(object_index_at(&objects, 4), Some(0));
        assert_eq!(object_index_at(&objects, 5), Some(0));
        assert_eq!(object_index_at(&objects, 6), None);
        assert_eq!(object_index_at(&objects, 8), Some(1));
        assert_eq!(object_index_at(&[], 0), None);
    }

    #[test]
    fn split_segments_cuts_a_paragraph_that_straddles_an_object() {
        // One paragraph swallowing a figure becomes prose / figure / prose.
        let segments = [(0, 9)];
        assert_eq!(
            split_segments_at_objects(&segments, &[object(4, 6)]),
            vec![(0, 3), (4, 6), (7, 9)]
        );
    }

    #[test]
    fn split_segments_handles_an_object_at_either_edge() {
        assert_eq!(
            split_segments_at_objects(&[(0, 5)], &[object(0, 2)]),
            vec![(0, 2), (3, 5)]
        );
        assert_eq!(
            split_segments_at_objects(&[(0, 5)], &[object(3, 5)]),
            vec![(0, 2), (3, 5)]
        );
    }

    #[test]
    fn split_segments_leaves_object_free_pages_untouched() {
        let segments = [(0, 3), (5, 9)];
        assert_eq!(split_segments_at_objects(&segments, &[]), segments.to_vec());
        // An object that is already its own segment is preserved as-is.
        assert_eq!(
            split_segments_at_objects(&segments, &[object(5, 9)]),
            segments.to_vec()
        );
    }
}

# Rust for Understanding Syodep

This guide explains the Rust knowledge needed to read and change Syodep. It
assumes very little programming experience. You do not need to read a separate
Rust textbook first.

The goal is not to teach every feature in the Rust language. It is to explain
the features, conventions, and design choices that appear in this repository,
and to show where they appear.

## 1. What Rust does in Syodep

Syodep is split into two main layers:

- Rust contains the application rules, document model, PDF integration,
  configuration, and persistent storage.
- C++ and Qt create the windows, draw the interface, receive operating-system
  events, and call Rust through a C-compatible interface.

The dependency direction is approximately:

```text
Qt user interface (C++)
        |
        v
syodep-ffi       C-compatible boundary
        |
        v
syodep-core      application state and behavior
   |       |       |
   v       v       v
config    pdf    storage
```

`syodep-ffi` also uses configuration and storage types when it creates the
application. The important architectural rule is that the Rust core does not
know about Qt. It returns descriptions of effects that the Qt layer should
perform instead of calling the user interface directly.

For example, a key press travels through the program like this:

```text
Qt key event
  -> C-compatible key description
  -> Rust key chord
  -> keymap and input state
  -> command
  -> App::execute
  -> Effects value
  -> C-compatible effect flags
  -> Qt redraws, changes the cursor, or performs another requested action
```

This separation makes most application behavior testable without opening a
window.

## 2. The workspace and its crates

Rust projects are normally built with **Cargo**. Cargo reads files named
`Cargo.toml`, downloads dependencies, builds code, runs tests, and creates
documentation.

The root `Cargo.toml` defines a **workspace**. A workspace is a group of related
Rust packages that share dependency versions and one `Cargo.lock` file. Syodep
uses Rust edition 2021 and Cargo's version-2 dependency resolver.

In everyday Rust conversation, a **package** is a directory described by a
`Cargo.toml`, while a **crate** is one compiled Rust library or executable.
People often use the two words loosely. Syodep's Rust packages each produce one
main library crate:

| Crate | Responsibility |
| --- | --- |
| `syodep-config` | Read, validate, and write user configuration and key bindings. |
| `syodep-storage` | Store dynamic application data in SQLite. |
| `syodep-pdf` | Present a safe, Syodep-specific wrapper around MuPDF. |
| `syodep-core` | Hold application state and implement user-visible behavior. |
| `syodep-ffi` | Expose the Rust application to C++ through a C ABI. |

The `syodep-ffi` crate is built as both an `rlib` and a `staticlib`:

- An `rlib` is Rust's normal library format and is useful to Rust tools and
  tests.
- A `staticlib` is a native static library that CMake can link into the Qt
  application.

The top-level `Cargo.lock` records the exact dependency versions. It is
committed because Syodep is an application and reproducible builds matter.

The release profile keeps debug information. That makes release crash reports
and native debugging more useful; it does not mean the release is unoptimized.

## 3. Files, modules, and names

A Rust source file usually ends in `.rs`. A crate begins in `src/lib.rs` for a
library or `src/main.rs` for an executable.

Large crates are divided into **modules**:

```rust
mod layout;
pub mod command;
```

`mod layout;` tells Rust to compile `layout.rs` or `layout/mod.rs` as a child
module. `pub mod command;` also makes the module visible to other crates.

Names are brought into the current file with `use`:

```rust
use std::collections::HashMap;
use crate::command::Command;
```

The path prefixes have specific meanings:

- `std::` starts in Rust's standard library.
- `crate::` starts at the current crate's root.
- `super::` starts at the parent module.
- A dependency name such as `syodep_pdf::` starts in another crate.

`pub use` both imports and re-exports a name. `syodep-core/src/lib.rs` uses this
to present a smaller public API while keeping implementation details in
separate files.

Visibility is private by default:

- no modifier: visible only in the current module and its children;
- `pub(crate)`: visible anywhere in the current crate;
- `pub`: visible through the crate's public API, if its containing modules are
  also public.

Keeping most names private makes it easier to change internal code without
breaking other crates.

## 4. Variables, values, and basic types

### Bindings and mutability

`let` gives a value a name:

```rust
let page_count = document.page_count();
```

Bindings are immutable by default. `mut` permits later changes:

```rust
let mut effects = Effects::NONE;
effects |= Effects::REDRAW;
```

Rust often infers the type from its use. A type can be written explicitly when
it helps the compiler or the reader:

```rust
let page_index: usize = 0;
```

`const` defines a compile-time constant. Constants in Syodep include limits,
default values, and lists of all supported commands.

### Common scalar types

The types most often seen in this repository are:

- `bool`: `true` or `false`;
- `u8`: an unsigned 8-bit integer, useful for color channels and stable compact
  enum values;
- `u32`: an unsigned 32-bit integer, used for flags and some counters;
- `i64`: a signed 64-bit integer, used by SQLite identifiers and timestamps;
- `usize`: an unsigned integer sized for the current machine, used for indexes,
  lengths, and collection positions;
- `f32`: a 32-bit floating-point number, used for PDF and screen coordinates;
- `char`: one Unicode scalar value, not one arbitrary byte.

An integer literal such as `0` gets its type from context. Suffixes can make it
explicit, as in `0u32` or `1.0f32`.

Floating-point values need special care. Not every bit pattern is an ordinary
number; `NaN` means “not a number.” Therefore `f32` does not implement the
normal total ordering trait. Code that must sort coordinates uses
`f32::total_cmp` so all bit patterns have a defined order.

### Strings

Rust commonly uses two string types:

- `String` owns growable UTF-8 text.
- `&str` borrows a UTF-8 view from text owned elsewhere.

Use an owned `String` when a struct must keep the text. Accept `&str` when a
function only needs to inspect text during the call.

```rust
fn parse_command(name: &str) -> Result<Command, ParseCommandError> {
    // The function borrows `name`; it does not keep it.
}
```

Rust strings are UTF-8, so indexing a string with a number is deliberately not
allowed. Code works with characters, bytes, or substrings explicitly.

### Tuples, arrays, slices, and vectors

A **tuple** groups a fixed number of possibly different values:

```rust
let point = (10.0f32, 24.0f32);
let (x, y) = point;
```

An array such as `[u8; 4]` has a fixed length known at compile time. A slice,
`&[T]`, is a borrowed view of consecutive values. A `Vec<T>` owns a growable
sequence.

```rust
fn first_page(pages: &[PageContent]) -> Option<&PageContent> {
    pages.first()
}
```

### Paths

File-system paths are not assumed to be valid UTF-8:

- `PathBuf` owns a path and can be changed.
- `&Path` borrows a path for inspection.

These have the same owned-versus-borrowed relationship as `String` and `&str`.

## 5. Structs, enums, and domain types

### Structs

A **struct** groups named fields:

```rust
struct Size {
    width: f32,
    height: f32,
}
```

A value is constructed by naming its fields:

```rust
let size = Size {
    width: 800.0,
    height: 600.0,
};
```

Methods and associated functions are written in an `impl` block:

```rust
impl Size {
    fn area(&self) -> f32 {
        self.width * self.height
    }

    fn square(side: f32) -> Self {
        Self {
            width: side,
            height: side,
        }
    }
}
```

`&self` borrows the value, `&mut self` borrows it and permits changes, and
`self` consumes it. `Self` means the type being implemented.

### Tuple structs and newtypes

A tuple struct names the whole type but not each field:

```rust
struct HighlightId(i64);
```

This is often called the **newtype pattern**. Even if two IDs both contain an
`i64`, Rust will not accidentally accept a `TextAnnotationId` where a
`HighlightId` is required. The small wrapper turns a database primitive into a
domain-specific type.

### Enums

An **enum** represents one value chosen from a fixed set of variants:

```rust
enum Mode {
    Normal,
    Extend,
    Command,
}
```

Variants can carry different data:

```rust
enum OpenResult {
    Opened(Document),
    Cancelled,
    Failed(String),
}
```

This is stronger than a numeric status code: Rust makes the caller handle the
data belonging to each case. Syodep uses enums extensively for commands,
interaction modes, caret scopes, PDF object kinds, errors, and persistence
states.

Some persisted enums use `#[repr(u8)]`. This gives each unit variant a defined
underlying byte representation. Once such values are stored in a database or
cross an interface, changing their numeric meaning can break compatibility.

## 6. Ownership and borrowing

Ownership is the Rust feature most likely to look unfamiliar.

Every ordinary Rust value has one owner. When the owner goes out of scope, Rust
drops the value and releases its resources. This provides automatic cleanup
without a garbage collector.

### Moves

Assigning or passing an owned value usually **moves** it:

```rust
let first = String::from("document.pdf");
let second = first;
// `first` cannot be used here; `second` now owns the String.
```

Moving prevents two places from trying to free the same allocation.

Small, plain values such as integers and many Syodep coordinate structs
implement `Copy`. Assigning a `Copy` value duplicates it instead of moving it.
Larger owned values may implement `Clone`, which performs an explicit copy:

```rust
let second = first.clone();
```

Clone only when another independent owner is genuinely needed. Unnecessary
cloning can hide an unclear ownership design or copy large buffers.

### Shared borrowing

`&T` is a shared reference. It allows reading without taking ownership:

```rust
fn page_width(page: &PageContent) -> f32 {
    page.size.width
}
```

Any number of shared references may exist at once, but they cannot mutate the
borrowed value.

### Exclusive borrowing

`&mut T` is an exclusive mutable reference:

```rust
fn clear_cache(cache: &mut RenderCache) {
    cache.clear();
}
```

While it exists, no other usable reference to the same value may exist. This
rule prevents data races and invalid references at compile time.

An `&mut self` method does not mean the entire program is mutable. It means the
method temporarily receives exclusive access to that particular value. Methods
on `App`, `ContentSession`, caches, and input state use this to make controlled
state transitions.

### Lifetimes

A **lifetime** describes how long a reference remains valid. The compiler
infers most lifetimes:

```rust
fn name(config: &Config) -> &str {
    &config.name
}
```

Sometimes a relationship must be written explicitly:

```rust
fn choose<'a>(first: &'a str, use_first: bool, second: &'a str) -> &'a str {
    if use_first { first } else { second }
}
```

`'a` is not a duration. It says that the returned reference cannot outlive the
input references. `'_` asks the compiler to infer a lifetime in a place where
the syntax requires one. `'static` means a reference is valid for the entire
program, as string literals are.

Syodep's input and keymap code uses named lifetimes when it returns views into
stored keymap structures. Read them as relationships between borrowed values,
not as manual memory management.

### Drop and RAII

Rust calls `Drop::drop` when an owning value leaves scope. This pattern is also
called **RAII**: acquisition and release are tied to an object's lifetime.

Files close, SQLite resources release, and native PDF objects clean up through
their owners. `App` also uses drop-time behavior for final persistence. Code
should still save at intentional state boundaries; `Drop` is a last lifecycle
hook, not an excuse to defer all important work.

`std::mem::take(&mut value)` replaces a value with its default and returns the
old value. It is useful when code needs to move a field out through a mutable
reference without leaving the struct partly uninitialized.

## 7. Control flow and pattern matching

Rust has familiar `if`, `else`, `while`, `for`, and `loop` constructs. Most are
expressions, so they can produce values:

```rust
let scale = if zoomed { 2.0 } else { 1.0 };
```

A block's final expression has no semicolon and becomes the block's value. A
semicolon discards a value and makes the expression return `()` (the unit
type).

`match` handles all cases of an enum or another pattern:

```rust
let label = match mode {
    Mode::Normal => "normal",
    Mode::Extend => "extend",
    Mode::Command => "command",
};
```

The compiler normally requires exhaustive coverage. `_` is a catch-all pattern,
but use it carefully: naming variants separately helps the compiler reveal
missing behavior when an enum grows.

Patterns can unpack values:

```rust
match result {
    OpenResult::Opened(document) => use_document(document),
    OpenResult::Cancelled => {}
    OpenResult::Failed(message) => report(message),
}
```

Useful shorter forms are:

```rust
if let Some(session) = app.session() {
    // Use the session only when it exists.
}

let Some(page) = pages.get(index) else {
    return;
};

if matches!(mode, Mode::Normal | Mode::Extend) {
    // Either listed pattern matched.
}
```

Ranges use `start..end` with an excluded end or `start..=end` with an included
end. Page and collection loops commonly use ranges or iterators.

## 8. `Option`: a value may be absent

Rust has no ordinary null references. Optional data is represented by:

```rust
enum Option<T> {
    Some(T),
    None,
}
```

Examples in Syodep include:

- `Option<Session>`: no document may be open;
- `Option<Storage>`: the application may be running in a degraded mode without
  persistence;
- `Option<&ContentObject>`: a search may find no matching object;
- optional configuration fields: the user may not have supplied an override.

The meaning of `None` depends on the field, so use domain names and comments to
make it clear.

Common operations include:

- `option.is_some()` and `option.is_none()`;
- `option.as_ref()` to borrow the contained value;
- `option.map(f)` to transform `Some` and preserve `None`;
- `option.and_then(f)` for a transformation that itself returns an `Option`;
- `option.unwrap_or(default)` or `unwrap_or_else(f)` for a fallback;
- pattern matching to handle both cases explicitly.

`unwrap()` crashes if the value is `None`. It is appropriate in tests or after
an invariant has been clearly established, but user-controlled input and
recoverable runtime states should be handled without crashing.

## 9. `Result`, errors, and the `?` operator

Fallible operations return:

```rust
enum Result<T, E> {
    Ok(T),
    Err(E),
}
```

`T` is the success value and `E` is the error type. A function returning
`Result<(), AppError>` either succeeds without a meaningful value (`Ok(())`) or
returns an `AppError`.

The `?` operator provides concise error propagation:

```rust
fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path)?;
    parse_config(&text)
}
```

If reading succeeds, `text` receives the string. If reading fails, the function
returns early with the converted error.

Syodep error enums commonly derive `thiserror::Error`:

```rust
#[derive(Debug, thiserror::Error)]
enum AppError {
    #[error("storage failed: {0}")]
    Storage(#[from] StorageError),
}
```

The displayed message comes from `#[error(...)]`. `#[from]` also creates a
conversion, allowing `?` to turn a `StorageError` into an `AppError`.

Other common methods are:

- `map` to transform a success value;
- `map_err` to transform an error;
- `and_then` to run another fallible operation after success;
- `unwrap_or` and `unwrap_or_else` to deliberately recover with a fallback;
- `expect("reason")` to crash with context when failure indicates a programmer
  error rather than a runtime condition.

Syodep intentionally treats some failures differently:

- A malformed optional configuration should be reported and replaced by safe
  defaults when possible.
- A failed database transaction must roll back rather than leave partial state.
- A failed document operation should become an application error or status,
  not cross the FFI boundary as a panic.

## 10. Traits and derived behavior

A **trait** describes behavior that types can implement. It is similar to an
interface, but traits can also provide default methods and participate in
generic programming.

```rust
trait Label {
    fn label(&self) -> &str;
}
```

Important standard traits in this repository include:

- `Debug`: developer-oriented formatting with `{:?}`;
- `Display`: user-facing formatting with `{}`;
- `Default`: a sensible default value;
- `Clone` and `Copy`: explicit or implicit duplication;
- `PartialEq` and `Eq`: equality comparisons;
- `Hash`: support as a `HashMap` or `HashSet` key;
- `From` and `Into`: value conversion;
- `FromStr`: parsing from a string;
- `Iterator` and `IntoIterator`: sequence processing;
- `Error`: standard error behavior.

Simple implementations can be generated with `derive`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PageIndex(usize);
```

The configuration crate also derives Serde's `Serialize` and `Deserialize` to
convert between Rust values and TOML. Field attributes such as
`#[serde(default)]` define how missing configuration values behave.

`Display` and `FromStr` are important for commands and key descriptions. They
keep text formatting and parsing attached to the domain type instead of spread
through unrelated code.

## 11. Generics, trait bounds, and closures

Generics let one definition work with multiple types:

```rust
fn first<T>(values: &[T]) -> Option<&T> {
    values.first()
}
```

`T` is a type parameter. A **trait bound** states what the code needs from it:

```rust
fn render<E>(operation: impl FnOnce() -> Result<Image, E>)
where
    E: std::fmt::Display,
{
    // ...
}
```

`impl FnOnce()` accepts a **closure**, an inline function-like value that may
capture surrounding state and is called at most once:

```rust
let image = cache.get_or_render(key, || document.render_page(index))?;
```

The three closure traits describe how captures are used:

- `Fn`: can be called repeatedly without mutation;
- `FnMut`: can be called repeatedly while mutating captured state;
- `FnOnce`: may consume captured state, so it is guaranteed only one call.

A `move` closure takes ownership of captured values. Without `move`, the
compiler usually chooses borrowing when possible.

You do not need to mentally expand generic code into every possible type. Read
the bounds as a list of capabilities the implementation requires.

## 12. Collections

Syodep uses standard collections according to the data's shape:

- `Vec<T>`: ordered, growable items addressed by position;
- `HashMap<K, V>`: fast key-to-value lookup with no stable iteration order;
- `BTreeMap<K, V>`: sorted key-to-value lookup, useful when deterministic order
  matters;
- `HashSet<T>`: unique values with fast membership checks;
- `VecDeque<T>`: a queue that efficiently adds or removes at either end.

Examples include page objects in vectors, render and content caches in hash
maps, configuration maps with deterministic ordering, sets of visited or
selected objects, and queued key replays in a deque.

Collection access is often safe and optional:

```rust
let page = pages.get(index); // Option<&PageContent>
```

Index syntax, `pages[index]`, panics if the index is invalid. Use it only where
the index has already been proven valid.

`entry` APIs combine lookup and insertion:

```rust
let value = cache.entry(key).or_insert_with(|| compute_value());
```

This avoids computing or looking up the same key twice.

## 13. Iterators

An iterator produces a sequence one value at a time. Iterator pipelines are
common Rust, including in Syodep's caret navigation and PDF object processing:

```rust
let visible: Vec<_> = objects
    .iter()
    .filter(|object| object.is_visible())
    .map(|object| object.bounds())
    .collect();
```

Read this from top to bottom:

1. borrow each object with `iter()`;
2. keep visible objects with `filter`;
3. transform each object into its bounds with `map`;
4. collect the results into a vector.

Iterator adapters are lazy: they do no work until a consumer such as `collect`,
`find`, `any`, `all`, `count`, or a `for` loop requests values.

Related methods include:

- `enumerate()` to pair each item with its index;
- `zip()` to walk two sequences together;
- `filter_map()` to transform and discard absent results in one step;
- `flat_map()` or `flatten()` to combine nested sequences;
- `fold()` to accumulate one result;
- `copied()` or `cloned()` to turn iterated references into values.

`into_iter()` consumes an owned collection, `iter()` borrows its items, and
`iter_mut()` mutably borrows its items.

## 14. Useful syntax and macros

This compact reference covers punctuation that appears often:

| Syntax | Meaning |
| --- | --- |
| `Type::name` | A module item, enum variant, or associated function. |
| `value.method()` | Call a method on a value. |
| `&value` | Borrow immutably. |
| `&mut value` | Borrow mutably and exclusively. |
| `*reference` | Dereference a reference. |
| `value?` | Return early if a `Result` is `Err` or an `Option` is `None`. |
| `pattern => value` | One arm of a `match`. |
| `|x| expression` | A closure. |
| `..` / `..=` | Exclusive-end / inclusive-end range, or pattern shorthand. |
| `_` | An intentionally ignored value or a compiler-inferred type. |
| `::<Type>` | Explicit generic type arguments, called the turbofish. |
| `#[attribute]` | Metadata controlling compilation or generated behavior. |
| `macro!()` | Invoke a macro. |

Macros generate Rust syntax. Common examples are:

- `vec![...]` to construct a vector;
- `format!(...)` to construct a `String`;
- `println!(...)` and `eprintln!(...)` for output;
- `matches!(...)` for pattern tests;
- `assert!`, `assert_eq!`, and `debug_assert!` for invariants;
- `unreachable!(...)` for a branch proven impossible by program logic.

Unlike normal functions, macros end in `!` and can accept syntax rather than
only evaluated values.

## 15. Attributes and conditional compilation

Attributes begin with `#[...]`. Important ones here include:

- `#[derive(...)]`: generate trait implementations;
- `#[test]`: mark a function as a test;
- `#[cfg(test)]`: compile code only while testing;
- `#[cfg(feature = "test-support")]`: compile code only when a Cargo feature is
  enabled;
- `#[cfg(target_os = "...")]`: select platform-specific code;
- `#[repr(C)]`: use a C-compatible representation for an FFI type;
- `#[repr(u8)]`: give an enum a specified integer representation;
- `#[no_mangle]`: export a symbol using its written name for native callers.

A Cargo **feature** enables an optional part of a crate. The `syodep-pdf`
`test-support` feature exposes deterministic fixture construction used by
higher-level tests. It should not accidentally become part of ordinary release
behavior.

Conditional code is still real code. When changing it, run the targets and
features that compile that branch.

## 16. The application core as a state machine

`syodep-core/src/app.rs` is the largest and most central Rust file. The `App`
value owns the current application state. A useful mental model is a state
machine:

```text
current App state + one Command
              |
              v
        App::execute
              |
              v
new App state + Effects or an AppError
```

Important related concepts are:

- **Session**: state that exists while a document is open;
- **Command**: a semantic action such as moving the caret or changing a mode;
- **Effects**: a compact description of work the UI must do after a state
  change;
- **status**: information intended for display to the user;
- **document anchor**: a persistent logical location within a document;
- **highlight lifecycle**: pending, embedded, or external annotation state.

Commands are deliberately separate from keys. The same command can be reached
through different key bindings, and tests can execute commands without faking
native keyboard events.

`Effects` values are combined as flags. One command may require several
responses, such as updating the view and redrawing. The FFI layer converts
these Rust effects into C-compatible bits that Qt understands.

When reading `app.rs`, follow one command at a time instead of trying to absorb
the whole file sequentially. Find its command variant, its `execute` branch,
the state fields it changes, its returned effects, and its tests.

## 17. Input, commands, caret, and layout

### Commands

`syodep-core/src/command.rs` defines the command vocabulary. It also supplies
string parsing and display forms used by configuration. A list of all commands
supports validation and discoverability.

Adding a command normally requires more than adding an enum variant. Its parser,
name, execution behavior, tests, documentation, and possibly default keymap
must remain consistent.

### Input state and keymaps

Key bindings can contain sequences, so input is not always resolved after one
key. The input code uses a trie-like structure: each chord can lead to a command
or to another level waiting for more input. A `VecDeque` supports replay when a
partial sequence stops matching.

Keymap overlays allow modes or contexts to change the active bindings without
rewriting the base keymap. Lifetimes in this code usually mean “this match
borrows entries stored in these keymaps.”

### Caret and object policy

The caret is a logical document position, not a Qt text cursor. Caret movement
works with PDF-derived objects such as pages, blocks, lines, and characters.
Scope and object-kind enums allow pattern matching to express which operations
are valid for which object.

`object_policy.rs` centralizes these rules. Keeping policy separate prevents
slightly different interpretations from spreading through commands.

### Layout

Layout types connect document coordinates to viewport coordinates. Most values
are `f32` because PDF and rendering geometry is continuous rather than an
integer pixel grid.

Be explicit about coordinate spaces. A rectangle in PDF page coordinates is
not automatically a rectangle in window coordinates; scale, scroll position,
page placement, and device pixel ratio may all matter.

Internal page indexes are generally zero-based because they index Rust
collections. User-visible page numbers are generally one-based. Conversions
must happen at an intentional boundary.

## 18. Lazy content and render caches

PDF extraction and page rendering can be expensive. `ContentSession` and
render-cache code delay work until it is needed and remember completed results.

A typical cache operation is:

```text
look for key
  -> present: borrow or return cached value
  -> absent: perform fallible computation, insert value, return it
```

This explains several Rust features appearing together:

- `HashMap` for keyed storage;
- `Option` for cache presence;
- `Result` because loading or rendering can fail;
- an `FnOnce` closure so the expensive operation runs only on a miss;
- `&mut self` because inserting changes the cache;
- generic error types so the cache does not dictate every caller's error enum.

Do not hold a borrow into a cache while also trying to mutate the same cache.
When the borrow checker rejects this, separate lookup from insertion, narrow
the reference's scope, or use the collection's entry API. Do not reach for
unsafe code to bypass an ordinary borrowing problem.

## 19. Configuration with Serde and TOML

`syodep-config` maps TOML text to typed Rust values. **Serde** is the library
that generates most of this conversion.

```rust
#[derive(Debug, Deserialize)]
struct ViewConfig {
    #[serde(default)]
    margin: f32,
}
```

The derived implementation checks field types while parsing. Defaults make
missing optional settings safe. Custom parsing handles domain formats such as
key chords and colors.

Configuration is user-editable and may contain mistakes. The desired behavior
is normally:

1. detect the specific invalid value;
2. report a useful diagnostic;
3. preserve usable settings or fall back safely where the architecture allows;
4. avoid corrupting the user's file.

Use `BTreeMap` when serialized ordering should be stable. Stable output is
easier for people to read and produces smaller version-control diffs.

Configuration contains user preferences and key definitions. Dynamic state
such as document progress or stored annotations belongs in SQLite instead.

## 20. Storage and SQLite

`syodep-storage` owns persistence. It uses `rusqlite`, whose Rust API wraps the
SQLite C library.

Database work should use a transaction when several statements represent one
logical change:

```text
BEGIN
  perform every related write
COMMIT if all succeeded
ROLLBACK if any failed
```

This is atomicity: the database sees all of the change or none of it. Some code
manages transaction statements directly so storage methods can operate through
the chosen connection ownership model.

Schema changes are represented as ordered migrations. Treat existing
migrations as history: append a new migration instead of rewriting one that a
user may already have applied. Test both a fresh database and an upgrade from
the previous schema.

Newtype IDs prevent mixing unrelated rows. Hashes identify document content
without relying only on a path that can be renamed. SQL parameters must be
bound through `rusqlite`, not assembled from user strings.

Storage errors propagate through typed `Result` values. A transaction must
roll back on all error paths. Tests use temporary files or databases so they do
not touch real user data.

## 21. PDF integration and MuPDF

`syodep-pdf` is the only place where the rest of the Rust application should
need to understand MuPDF. It converts the dependency's types into Syodep types
such as:

- `Document`;
- `PageContent`;
- `ContentObject`;
- `Rect`, `Size`, and cell or text structures;
- `PdfError`.

This wrapper is a safety and architecture boundary. Higher layers ask for
domain operations instead of depending on MuPDF details.

PDFs do not necessarily store text in human reading order. Content extraction
therefore includes heuristics for grouping and ordering objects. Changes to
those heuristics need focused fixtures because a result can be valid Rust while
still being wrong for a document.

The document wrapper is not assumed to be `Send`. In Rust, `Send` means a value
can safely move to another thread. Native library context and ownership rules
make that unsafe unless explicitly guaranteed. Keep MuPDF access and rendering
on the intended thread; do not add a thread merely to make an operation appear
asynchronous.

The `test-support` feature supplies programmatically constructed document data
for tests. It avoids committing generated binary PDF fixtures and keeps unit
tests deterministic.

## 22. Unsafe Rust and the FFI boundary

Most Rust is **safe Rust**: the compiler enforces reference validity, bounds,
and ownership. `unsafe` permits a small set of operations the compiler cannot
prove safe, such as dereferencing a raw pointer.

Unsafe code is not automatically incorrect. It means the programmer must state
and uphold the missing proof. In Syodep, unsafe operations should be
concentrated at the native boundary in `syodep-ffi`, with a small safe Rust API
behind them.

### C ABI

An **ABI**, or application binary interface, defines how separately compiled
code represents values and calls functions. Rust's normal ABI is not stable for
C++ callers, so exported functions use `extern "C"` and stable exported names.

FFI structs use `#[repr(C)]` so field layout follows C rules. Only
C-compatible data crosses the boundary: fixed-width numbers, plain structs,
raw pointers, and explicitly managed strings or buffers. Rust-specific types
such as `String`, `Vec`, `Result`, trait objects, or enums with data must be
converted first.

### Opaque handles and ownership

C++ normally holds an opaque pointer to a Rust-owned application value:

```text
create: Box<App> -> raw pointer handed to C++
use:    validate pointer -> temporary Rust reference -> call safe method
free:   raw pointer -> Box<App> -> normal Rust drop
```

`Box::into_raw` transfers responsibility out of Rust's automatic ownership
tracking. `Box::from_raw` takes it back exactly once. Taking it back twice is a
double free; never taking it back leaks memory. Every allocation crossing FFI
needs a documented matching release function.

Raw pointers may be null, misaligned, dangling, or point to the wrong type. An
unsafe wrapper must validate what it can and clearly document what the caller
must guarantee before creating a Rust reference.

### Strings

C strings end with a zero byte and are not guaranteed to contain UTF-8. Rust
uses `CStr` to borrow input bytes and `CString` to own a zero-terminated output.
Conversions must define how invalid UTF-8, interior zero bytes, null pointers,
and allocation ownership are handled.

### Panics

A Rust panic must not unwind through C or C++ frames. Exported entry points use
`catch_unwind` where necessary, convert failures into stable error values, and
leave the application in a valid state. `AssertUnwindSafe` is a promise about
the captured state during this containment; it deserves careful review.

An FFI function should be thin:

1. validate and translate C inputs;
2. call a safe Rust operation;
3. translate the result or error back to C;
4. contain panics and preserve ownership rules.

Business logic belongs in `syodep-core`, not inside pointer-manipulation code.

### Generated header

`syodep-ffi/build.rs` uses cbindgen to produce a C header from Rust exports.
The generated header is a build artifact. Change the Rust declarations or the
cbindgen configuration, then regenerate it; do not hand-edit generated output.

## 23. Tests and test-only code

Rust unit tests commonly live beside the code they test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cache_has_no_page() {
        let cache = RenderCache::default();
        assert!(cache.get(0).is_none());
    }
}
```

`#[cfg(test)]` removes the module from ordinary builds. `use super::*` imports
names from the module being tested. A test passes if it returns normally and
fails if it panics or returns an error from a test function designed to do so.

Good Syodep tests describe behavior in their names. For a bug fix, first add a
test that fails for the old behavior, then make the smallest production change
that passes it. Include error paths, boundary indexes, empty state, and
persistence round trips—not only the happy path.

Useful assertions are:

- `assert!(condition)`;
- `assert_eq!(actual, expected)`;
- `assert_ne!(actual, unexpected)`;
- `matches!` inside an assertion for enum shapes.

`unwrap()` and `expect()` are common in tests because setup failure should fail
the test immediately. Production code must distinguish invariants from
recoverable input.

Temporary directories and databases isolate tests from the developer's actual
configuration. PDF test support builds fixtures from source data rather than
checking in generated binaries.

## 24. Documentation and comments

Rust has two main comment forms:

- `//` explains an implementation detail to future maintainers;
- `///` documents the public item immediately following it and appears in
  generated Rust documentation.

`//!` documents the containing module or crate.

Useful comments explain **why** a constraint exists, especially around unsafe
preconditions, PDF heuristics, compatibility values, or counterintuitive
workarounds. Comments that only translate one obvious line into English tend to
become noise.

Documentation examples can be compiled as tests by rustdoc. Links such as
``[`Command`]`` connect concepts to their API definitions when documentation is
generated.

## 25. Reading the repository in a practical order

For a first pass, use this order:

1. Read the root `Cargo.toml` to see the workspace and shared dependencies.
2. Read `docs/architecture.md` for system-wide rules and data flow.
3. Read `syodep-core/src/lib.rs` to see the core's module and public API map.
4. Read `command.rs` to learn the application's action vocabulary.
5. Read the input-state code to see how keys become commands.
6. Read layout, object-policy, caret, content-session, and render-cache modules
   separately; each establishes concepts used by the application.
7. In `app.rs`, trace one command end to end, including its tests.
8. Read `syodep-config` to understand text configuration and key parsing.
9. Read `syodep-storage`, starting with types and migrations, then operations.
10. Read the public types in `syodep-pdf` before its extraction heuristics.
11. Read `syodep-ffi` last, after the safe types it exposes make sense.
12. Follow the matching call from the Qt code when you need the complete native
    interaction.

Use symbol search instead of reading the largest files from top to bottom. For
example:

```sh
rg "Command::Move" crates ui-qt
rg "struct RenderCache|impl RenderCache" crates/syodep-core
rg "extern \"C\"" crates/syodep-ffi
```

## 26. Common changes and where they lead

### Adding a command

Expect to inspect or change:

1. the `Command` enum and its stable text name;
2. parsing, formatting, and the list of commands;
3. `App::execute` behavior;
4. state-specific policies and returned effects;
5. default key bindings or configuration validation if applicable;
6. focused core tests;
7. user and architecture documentation.

### Adding a configuration option

Define a typed field and default, update Serde behavior, validate invalid input,
use it at the correct layer, and add tests for absent, valid, and invalid
values. Preserve backward compatibility for existing configuration files.

### Changing persistence

Add an append-only migration, update domain methods, keep writes transactional,
and test both fresh creation and upgrade. Do not store dynamic state in the
configuration file merely because TOML looks easier.

### Adding PDF behavior

Keep MuPDF-specific work in `syodep-pdf`, expose Syodep domain types, add a
small source-defined fixture, test ordering and boundary cases, then consume
the operation from the core. Respect document thread affinity.

### Adding an FFI operation

First implement and test a safe core operation. Then add a thin exported
wrapper, C-compatible types, null and error behavior, panic containment, and a
matching ownership release if memory crosses the boundary. Let cbindgen update
the header and then add the Qt call.

## 27. Repository invariants and common mistakes

Keep these rules in mind while changing Rust code:

- The core must not depend on Qt or call UI functions.
- Native pointer manipulation belongs at the FFI edge, not in domain logic.
- MuPDF details belong inside `syodep-pdf`.
- Existing database migrations are history; append instead of rewriting.
- Dynamic document state belongs in SQLite; user preferences belong in config.
- User-controlled errors should not become `unwrap()` panics.
- A Rust panic must never unwind into C++.
- Every raw allocation crossing FFI needs exactly one matching release path.
- Internal page indexes and visible page numbers use different bases.
- Coordinate spaces must be named or otherwise clear.
- Floating-point sorting should use a deliberate total comparison.
- Caches improve performance but must not change semantic results.
- Test-only PDF builders must remain behind their feature or test condition.
- Do not hand-edit a generated C header.
- Do not add threads around MuPDF values without a proven native safety model.
- Preserve safe degraded behavior when optional configuration or storage cannot
  be used.

The borrow checker often reveals a design issue rather than an obstacle to work
around. When it reports overlapping mutable and shared access, shorten a
borrow, split data into independent fields, use a collection API designed for
the operation, or reconsider which object owns the state.

## 28. Build and verification commands

From the repository root, the main Rust checks are:

```sh
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps
```

Their purposes are:

- `cargo test`: compile and run the Rust tests;
- `cargo fmt --check`: verify standard Rust formatting without rewriting files;
- `cargo clippy`: run deeper correctness and style lints, treating warnings as
  failures;
- `cargo doc`: verify public documentation and links.

The repository's documentation check should also run:

```sh
./scripts/check-docs.sh
```

These commands do not replace the Qt/CMake build. A change to FFI, generated
headers, rendering, startup behavior, or native dependencies also needs the
full CMake build and the applicable smoke tests described in the development
documentation.

Rustfmt is authoritative for `.rs` formatting. Run `cargo fmt --all` to apply
formatting when needed, then inspect the diff.

## 29. Rust features you do not need first

Syodep does not require mastery of every advanced Rust topic. You can understand
the current architecture without first learning:

- asynchronous Rust, futures, or Tokio;
- web frameworks;
- procedural macro implementation;
- complex lock-free concurrency;
- a garbage collector;
- nightly-only language features.

Learn those only if a future, well-motivated design introduces them. The most
important skills here are ownership and borrowing, enums and pattern matching,
`Option` and `Result`, iterators, typed domain boundaries, tests, and careful FFI
ownership.

## 30. Final mental model

When reading unfamiliar Syodep Rust, ask these questions in order:

1. Which crate owns this responsibility?
2. Which type owns the data?
3. Is this value owned, shared-borrowed, or mutably borrowed?
4. Which enum variants represent the possible states?
5. Can the value be absent (`Option`) or can the operation fail (`Result`)?
6. Which invariant is enforced by the type system, and which still needs a
   runtime check?
7. Does this code mutate core state, request a UI effect, access persistence,
   or cross the native boundary?
8. Which test proves the intended behavior and its important error case?

If those answers are clear, the remaining syntax is usually local detail. The
repository is organized so that Rust's ownership and type system express the
same boundaries as Syodep's architecture: the core owns behavior, specialized
crates own external systems, and the FFI boundary translates carefully between
safe Rust and the Qt application.

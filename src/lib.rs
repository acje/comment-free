//! Pure logic for the `comment-free` tool: parse, re-emit, lint doc-comment budget.
#![forbid(unsafe_code)]
#![warn(clippy::missing_const_for_fn)]
use ra_ap_rustc_lexer::{FrontmatterAllowed, TokenKind, tokenize};
use similar::{ChangeTag, TextDiff};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use syn::ext::IdentExt as _;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, File, Meta, Token};
use walkdir::WalkDir;
/// Doctrine warning carried by the `doc_lint_header` record for every kind.
pub const DOC_LINT_DOCTRINE_MSG: &str = "Rust docs must contain a concise summary, optionally clear code examples (fenced ``` or ~~~ blocks), and sections explaining edge cases like panics, errors, and safety. Fenced code examples are excluded from the prose word length. If applicable references to ADRs must be given.";
/// Version carried by the `v` field of every `doc_lint_*` record.
///
/// Consumers reject records whose `v` exceeds the version they
/// understand. See `docs/record-format.md` for the record grammar and
/// the compatibility rules that let new fields, kinds, and outcomes
/// arrive without a bump.
pub const DOC_LINT_RECORD_VERSION: u32 = 2;
/// One-line JSON templates for the `doc_lint_*` record family, for the
/// binary's `--help` output.
///
/// Authoritative grammar: `docs/record-format.md`.
pub const DOC_LINT_RECORD_GRAMMAR: &str = "\
{\"record\":\"doc_lint_finding\",\"v\":<N>,\"outcome\":<OUTCOME>,\"kind\":<KIND>,\"path\":<PATH>,\"line\":<U32>,\"item\":<LABEL>,\"words\":<U32>,\"budget\":<U32>,\"fail_closed\":<BOOL>}
{\"record\":\"doc_lint_header\",\"v\":<N>,\"kind\":<KIND>,\"doctrine\":<STRING>}
{\"record\":\"doc_lint_hint\",\"v\":<N>,\"outcome\":<OUTCOME>,\"kind\":<KIND>,\"path\":<PATH>,\"line\":<U32>,\"item\":<LABEL>,\"words\":<U32>,\"budget\":<U32>}
{\"record\":\"doc_lint_truncated\",\"v\":<N>,\"kind\":<KIND>,\"remaining\":<U32>}
{\"record\":\"doc_lint_undecided\",\"v\":<N>,\"outcome\":\"configuration_dependent\",\"kind\":<KIND>,\"path\":<PATH>,\"line\":<U32>,\"item\":<LABEL>,\"words\":<U32>,\"budget\":<U32>,\"words_all_cfgs\":<U32>,\"fail_closed\":<BOOL>}
{\"record\":\"doc_lint_undecided\",\"v\":<N>,\"outcome\":\"unreadable_doc_payload\",\"kind\":<KIND>,\"path\":<PATH>,\"line\":<U32>,\"item\":<LABEL>,\"budget\":<U32>}
{\"record\":\"doc_lint_undecided\",\"v\":<N>,\"outcome\":\"uninspected_macro_body\",\"kind\":<KIND>,\"path\":<PATH>,\"line\":<U32>,\"item\":<LABEL>,\"budget\":<U32>}";
/// Version carried by the `v` field of the `rewrite_summary` record.
///
/// Independent of [`DOC_LINT_RECORD_VERSION`]: the rewrite-summary
/// family evolves on its own cadence. See `docs/record-format.md`.
pub const REWRITE_RECORD_VERSION: u32 = 2;
/// One-line JSON template for the `rewrite_summary` record, for the
/// binary's `--help` output.
///
/// Authoritative grammar: `docs/record-format.md`.
pub const REWRITE_RECORD_GRAMMAR: &str = "\
{\"record\":\"rewrite_summary\",\"v\":<N>,\"mode\":<MODE>,\"comments_removed\":<U32>,\"inline_trimmed\":<U32>,\"blank_lines_collapsed\":<U32>,\"doc_links_rewritten\":<U32>}";
/// Version carried by the `v` field of every run-diagnostic record:
/// `run_error`, `doc_file_warning`, `rewrite_file`, `strip_summary`,
/// and `lint_summary`.
///
/// Independent of [`DOC_LINT_RECORD_VERSION`] and
/// [`REWRITE_RECORD_VERSION`]. These lines were previously emitted as
/// the unversioned tab-separated `SUMMARY`, `REWRITE`, `WOULD_REWRITE`,
/// `DOC_WARN`, `WALK_ERROR`, `IO_ERROR`, `PARSE_ERROR` and
/// `CONFLICT_ERROR` diagnostics, which carried raw unescaped paths; they
/// join the JSON Lines contract at the same version the other families
/// bumped to. See `docs/record-format.md`.
pub const DIAGNOSTIC_RECORD_VERSION: u32 = 2;
/// One-line JSON templates for the run-diagnostic record family, for the
/// binary's `--help` output.
///
/// Authoritative grammar: `docs/record-format.md`.
pub const DIAGNOSTIC_RECORD_GRAMMAR: &str = "\
{\"record\":\"run_error\",\"v\":<N>,\"kind\":<ERROR_KIND>,\"path\":<PATH>,\"message\":<STRING>}
{\"record\":\"doc_file_warning\",\"v\":<N>,\"path\":<PATH>}
{\"record\":\"rewrite_file\",\"v\":<N>,\"mode\":<MODE>,\"path\":<PATH>}
{\"record\":\"strip_summary\",\"v\":<N>,\"mode\":<MODE>,\"rewritten\":<U32>,\"unchanged\":<U32>,\"errors\":<U32>}
{\"record\":\"lint_summary\",\"v\":<N>,\"files\":<U32>,\"findings\":<U32>,\"undecided\":<U32>,\"errors\":<U32>}";
/// Which pass a `--rewrite` run made over the tree.
///
/// The unified-diff context width is carried by [`RewriteMode::DryRun`]
/// alone, so a write-mode run cannot name a diff width it will never
/// render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RewriteMode {
    /// Files were replaced on disk.
    Write,
    /// Diffs were printed and no file was touched.
    DryRun {
        /// Unified-diff context line count.
        context: usize,
    },
}
impl RewriteMode {
    /// The `mode` field value carried by emitted records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::DryRun { .. } => "dry-run",
        }
    }
}
/// Finding kind carried by the `kind` field of every `doc_lint_*` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocLintKind {
    /// Doc-comment prose exceeded the configured word budget.
    OverlongDoc,
}
impl DocLintKind {
    /// The `kind` field value carried by emitted records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OverlongDoc => "overlong_doc",
        }
    }
}
/// What a `doc_lint_*` record asserts about the item it names.
///
/// A pass that cannot decide an item — an unresolved `cfg` predicate, a
/// doc payload it cannot read — reports an explicit indeterminate
/// outcome as an added variant rather than a record-version bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocLintOutcome {
    /// The item was inspected and violates the budget.
    Finding,
    /// The item's doc set depends on unresolved `cfg` predicates: it is
    /// within budget for at least one configuration and over budget for
    /// at least one other. Not a finding, and not clean.
    ConfigurationDependent,
    /// The item carries a doc payload this tool cannot read — a macro
    /// call such as `include_str!` or `concat!` in the doc-value
    /// position, resolved only by expansion. Not a finding, and not
    /// clean.
    UnreadableDocPayload,
    /// A macro token body carries a doc attribute. What the expansion
    /// attaches it to, and how many times, is resolved only by
    /// expansion. Not a finding, and not clean.
    UninspectedMacroBody,
}
impl DocLintOutcome {
    /// The `outcome` field value carried by emitted records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Finding => "finding",
            Self::ConfigurationDependent => "configuration_dependent",
            Self::UnreadableDocPayload => "unreadable_doc_payload",
            Self::UninspectedMacroBody => "uninspected_macro_body",
        }
    }
}
/// Per-file counters surfaced by the rewrite passes, aggregated across
/// files in the run's `rewrite_summary` record.
///
/// Read them through the accessors; [`Default`] is the only
/// constructor, which is what keeps `inline_trimmed` ⊆
/// `comments_removed`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RewriteCounts {
    comments_removed: u32,
    inline_trimmed: u32,
    blank_lines_collapsed: u32,
    doc_links_rewritten: u32,
}
impl RewriteCounts {
    /// Non-doc line and block comments dropped.
    #[must_use]
    pub const fn comments_removed(self) -> u32 {
        self.comments_removed
    }
    /// Mid-line (post-code) drops that trimmed trailing whitespace; a
    /// subset of [`RewriteCounts::comments_removed`] excluding
    /// solo-line drops.
    #[must_use]
    pub const fn inline_trimmed(self) -> u32 {
        self.inline_trimmed
    }
    /// Symmetric-pad collapses, one per removed comment block with
    /// blanks on both sides.
    #[must_use]
    pub const fn blank_lines_collapsed(self) -> u32 {
        self.blank_lines_collapsed
    }
    /// Doc-link idiom splices applied, one per rewritten literal span.
    #[must_use]
    pub const fn doc_links_rewritten(self) -> u32 {
        self.doc_links_rewritten
    }
}
impl std::ops::AddAssign for RewriteCounts {
    fn add_assign(&mut self, rhs: Self) {
        self.comments_removed = self.comments_removed.saturating_add(rhs.comments_removed);
        self.inline_trimmed = self.inline_trimmed.saturating_add(rhs.inline_trimmed);
        self.blank_lines_collapsed = self
            .blank_lines_collapsed
            .saturating_add(rhs.blank_lines_collapsed);
        self.doc_links_rewritten = self
            .doc_links_rewritten
            .saturating_add(rhs.doc_links_rewritten);
    }
}
/// All terminal-error variants raised by the `comment-free` binary.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CommentFreeError {
    /// ROOT path passed on the CLI is not a directory.
    #[error("ROOT is not a directory")]
    NotADirectory,
    /// Generic IO error surfaced from [`std::io`].
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    /// Doc-lint run that did not establish a clean tree: at least one
    /// `DOC_LINT` finding or one undecided item under default lint mode.
    #[error("doc lint did not establish a clean tree")]
    DocLintFailure,
}
/// A directory-traversal failure, preserved rather than skipped.
///
/// A walk failure is an indeterminate result, never the absence of a
/// file: folding it into "no entry" lets a partial scan report zero
/// errors. [`WalkError::path`] is the entry that failed, falling back
/// to the walk root when the underlying error carries none.
///
/// The underlying traversal failure is rendered once, at construction,
/// into an owned neutral source: no foreign error type is reachable
/// from this error's [`std::error::Error::source`] chain. Take
/// [`WalkError::message`] for its text.
#[derive(Debug, thiserror::Error)]
#[error("cannot traverse {}: {source}", path.display())]
pub struct WalkError {
    path: PathBuf,
    #[source]
    source: TraversalFailure,
}
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct TraversalFailure(String);
impl WalkError {
    /// The entry that could not be traversed.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// The underlying traversal failure rendered as operator-facing
    /// text, for the `message` field of a `run_error` record.
    #[must_use]
    pub fn message(&self) -> String {
        self.source.0.clone()
    }
    fn rooted_at(base: &Path, source: &walkdir::Error) -> Self {
        let path = source.path().unwrap_or(base).to_path_buf();
        Self {
            path,
            source: TraversalFailure(source.to_string()),
        }
    }
}
impl From<&CommentFreeError> for ExitCode {
    fn from(e: &CommentFreeError) -> Self {
        match e {
            CommentFreeError::NotADirectory => Self::from(2),
            CommentFreeError::DocLintFailure => Self::from(4),
            CommentFreeError::Io(_) => Self::from(1),
        }
    }
}
/// Outcome of processing one source file.
///
/// Not `#[non_exhaustive]`: a caller that fails to handle an outcome
/// has silently dropped a file, so a new variant should break their
/// build. [`FileError`] is the growable half.
#[derive(Debug)]
pub enum FileOutcome {
    /// Write mode: the new content replaced the file on disk.
    Rewritten(RewriteSummary),
    /// Dry-run mode: nothing was written; the preview carries the diff
    /// the write would have produced.
    WouldRewrite(RewritePreview),
    /// No bytes changed; the file was left alone in either mode.
    Unchanged(RewriteSummary),
    /// The file was not processed, and was left exactly as found.
    Failed(FileError),
}
/// Counters for the passes [`process_file`] applied to one file.
///
/// Has no public constructor: an outcome payload is evidence of work
/// this crate performed, not a value a caller can assert.
#[derive(Debug)]
pub struct RewriteSummary {
    counts: RewriteCounts,
}
impl RewriteSummary {
    /// Counters for the passes applied to this file.
    #[must_use]
    pub const fn counts(&self) -> RewriteCounts {
        self.counts
    }
}
/// The dry-run preview of a rewrite: the unified diff plus its counters.
///
/// Has no public constructor, so an empty diff — a state
/// [`process_file`] never emits — cannot be presented as a preview.
#[derive(Debug)]
pub struct RewritePreview {
    diff: String,
    counts: RewriteCounts,
}
impl RewritePreview {
    /// Unified diff between the original and the rewrite.
    #[must_use]
    pub fn diff(&self) -> &str {
        &self.diff
    }
    /// Counters for the passes applied to this file.
    #[must_use]
    pub const fn counts(&self) -> RewriteCounts {
        self.counts
    }
}
/// Why [`process_file`] could not process a file.
///
/// Map a variant to its record field with [`FileError::kind`] and to
/// its operator-facing text through [`std::fmt::Display`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FileError {
    /// The file could not be parsed as Rust, so the doc-link pass could
    /// not run. The file is untouched on disk.
    #[error("{0}")]
    Parse(syn::Error),
    /// The file could not be read.
    #[error("{0}")]
    Read(io::Error),
    /// The rewrite could not be written.
    #[error("io error: {0}")]
    Write(#[from] io::Error),
    /// The destination is a symbolic link. Rewriting it would replace
    /// the link rather than its target, so it is refused.
    #[error(
        "destination is a symbolic link; rewriting it would replace the link rather than its target"
    )]
    SymlinkDestination,
    /// The destination no longer held the bytes originally read, so the
    /// rewrite was abandoned and the file left exactly as found.
    #[error("destination changed since it was read")]
    Conflict,
}
impl FileError {
    /// The `kind` field a `run_error` record carries for this failure.
    #[must_use]
    pub const fn kind(&self) -> RunErrorKind {
        match self {
            Self::Parse(_) => RunErrorKind::Parse,
            Self::Read(_) | Self::Write(_) | Self::SymlinkDestination => RunErrorKind::Io,
            Self::Conflict => RunErrorKind::Conflict,
        }
    }
}
struct TempFileGuard {
    path: PathBuf,
    armed: bool,
}
impl TempFileGuard {
    const fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.armed {
            drop(fs::remove_file(&self.path));
        }
    }
}
fn write_atomically(path: &Path, expected: &str, rewritten: &str) -> Result<(), FileError> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(FileError::SymlinkDestination);
    }
    let permissions = fs::metadata(path)?.permissions();
    let (mut file, mut guard) = create_sibling_temp(path, dir)?;
    file.write_all(rewritten.as_bytes())?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::set_permissions(&guard.path, permissions)?;
    if !destination_still_holds(path, expected)? {
        return Err(FileError::Conflict);
    }
    fs::rename(&guard.path, path)?;
    guard.disarm();
    Ok(())
}
fn destination_still_holds(path: &Path, expected: &str) -> Result<bool, FileError> {
    Ok(fs::read_to_string(path)? == expected)
}
fn create_sibling_temp(path: &Path, dir: &Path) -> Result<(fs::File, TempFileGuard), FileError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stem = path.file_name().map_or_else(
        || String::from("source"),
        |n| n.to_string_lossy().into_owned(),
    );
    let pid = std::process::id();
    for _ in 0..TEMP_NAME_ATTEMPTS {
        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = dir.join(format!(".{stem}.{pid}.{nonce}.comment-free-tmp"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                let guard = TempFileGuard {
                    path: candidate,
                    armed: true,
                };
                return Ok((file, guard));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(FileError::Write(e)),
        }
    }
    Err(FileError::Write(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "exhausted temporary file name attempts",
    )))
}
const TEMP_NAME_ATTEMPTS: u32 = 1024;
fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                write!(out, "\\u{:04x}", c as u32).expect("Write for String never fails");
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
fn push_text(out: &mut String, key: &str, value: &str) {
    push_json_string(out, key);
    out.push(':');
    push_json_string(out, value);
}
fn push_number(out: &mut String, key: &str, value: u32) {
    push_json_string(out, key);
    write!(out, ":{value}").expect("Write for String never fails");
}
fn push_count(out: &mut String, key: &str, value: usize) {
    push_number(out, key, u32::try_from(value).unwrap_or(u32::MAX));
}
fn open_record(kind: &str, version: u32) -> String {
    let mut out = String::from("{");
    push_text(&mut out, "record", kind);
    out.push(',');
    push_number(&mut out, "v", version);
    out
}
/// The `doc_lint_finding` record naming one item over budget.
///
/// Returns a single JSON Lines record without its terminating newline.
/// `path` is rendered with [`Path::display`], so a path that is not
/// valid UTF-8 is reported lossily; every other byte, including tabs
/// and newlines in a path or an item label, round-trips as a JSON
/// escape.
#[must_use]
pub fn doc_lint_finding_record(
    kind: DocLintKind,
    path: &Path,
    line: usize,
    item: &str,
    words: usize,
    budget: usize,
    fail_closed: bool,
) -> String {
    let mut out = open_record("doc_lint_finding", DOC_LINT_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "outcome", DocLintOutcome::Finding.as_str());
    out.push(',');
    push_hint_body(&mut out, kind, path, line, item, words, budget);
    out.push(',');
    push_json_string(&mut out, "fail_closed");
    write!(out, ":{fail_closed}").expect("Write for String never fails");
    out.push('}');
    out
}
/// The `doc_lint_undecided` record naming one item whose doc set the
/// linter could not decide.
///
/// The evidence fields are keyed on `outcome`, because each cause
/// supports different evidence. `configuration_dependent` carries
/// `words` — the unconditional doc set alone, present in every
/// configuration — plus `words_all_cfgs` and `fail_closed` for the
/// count with every `cfg_attr` doc payload active.
/// `unreadable_doc_payload` carries no word count at all: the text was
/// never read, so no count of it exists. Neither is a finding.
#[must_use]
pub fn doc_lint_undecided_record(
    kind: DocLintKind,
    path: &Path,
    undecided: &DocUndecided,
) -> String {
    let mut out = open_record("doc_lint_undecided", DOC_LINT_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "outcome", undecided.outcome().as_str());
    out.push(',');
    match undecided.cause() {
        UndecidedCause::ConfigurationDependent {
            unconditional,
            all_configurations,
        } => {
            push_hint_body(
                &mut out,
                kind,
                path,
                undecided.line(),
                undecided.item_label(),
                unconditional.count(),
                undecided.budget(),
            );
            out.push(',');
            push_count(&mut out, "words_all_cfgs", all_configurations.count());
            out.push(',');
            push_json_string(&mut out, "fail_closed");
            let fail_closed = all_configurations.is_fail_closed();
            write!(out, ":{fail_closed}").expect("Write for String never fails");
        }
        UndecidedCause::UnreadableDocPayload | UndecidedCause::UninspectedMacroBody => {
            push_undecided_head(
                &mut out,
                kind,
                path,
                undecided.line(),
                undecided.item_label(),
            );
            out.push(',');
            push_count(&mut out, "budget", undecided.budget());
        }
    }
    out.push('}');
    out
}
fn push_undecided_head(out: &mut String, kind: DocLintKind, path: &Path, line: usize, item: &str) {
    push_text(out, "kind", kind.as_str());
    out.push(',');
    push_text(out, "path", &path.display().to_string());
    out.push(',');
    push_count(out, "line", line);
    out.push(',');
    push_text(out, "item", item);
}
fn push_hint_body(
    out: &mut String,
    kind: DocLintKind,
    path: &Path,
    line: usize,
    item: &str,
    words: usize,
    budget: usize,
) {
    push_text(out, "kind", kind.as_str());
    out.push(',');
    push_text(out, "path", &path.display().to_string());
    out.push(',');
    push_count(out, "line", line);
    out.push(',');
    push_text(out, "item", item);
    out.push(',');
    push_count(out, "words", words);
    out.push(',');
    push_count(out, "budget", budget);
}
/// The `doc_lint_header` record naming the doctrine once per kind.
#[must_use]
pub fn doc_lint_header_record(kind: DocLintKind) -> String {
    let mut out = open_record("doc_lint_header", DOC_LINT_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "kind", kind.as_str());
    out.push(',');
    push_text(&mut out, "doctrine", DOC_LINT_DOCTRINE_MSG);
    out.push('}');
    out
}
/// The `doc_lint_hint` record carrying one finding's site coordinates.
#[must_use]
pub fn doc_lint_hint_record(
    kind: DocLintKind,
    path: &Path,
    line: usize,
    item: &str,
    words: usize,
    budget: usize,
) -> String {
    let mut out = open_record("doc_lint_hint", DOC_LINT_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "outcome", DocLintOutcome::Finding.as_str());
    out.push(',');
    push_hint_body(&mut out, kind, path, line, item, words, budget);
    out.push('}');
    out
}
/// The `doc_lint_truncated` record counting findings past the hint cap.
#[must_use]
pub fn doc_lint_truncated_record(kind: DocLintKind, remaining: usize) -> String {
    let mut out = open_record("doc_lint_truncated", DOC_LINT_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "kind", kind.as_str());
    out.push(',');
    push_count(&mut out, "remaining", remaining);
    out.push('}');
    out
}
/// The `rewrite_summary` record aggregating a run's rewrite counters.
#[must_use]
pub fn rewrite_summary_record(mode: RewriteMode, counts: &RewriteCounts) -> String {
    let mut out = open_record("rewrite_summary", REWRITE_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "mode", mode.as_str());
    out.push(',');
    push_number(&mut out, "comments_removed", counts.comments_removed);
    out.push(',');
    push_number(&mut out, "inline_trimmed", counts.inline_trimmed);
    out.push(',');
    push_number(
        &mut out,
        "blank_lines_collapsed",
        counts.blank_lines_collapsed,
    );
    out.push(',');
    push_number(&mut out, "doc_links_rewritten", counts.doc_links_rewritten);
    out.push('}');
    out
}
/// Which per-file failure a `run_error` record reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunErrorKind {
    /// Directory traversal could not read an entry.
    Walk,
    /// The file could not be read or written.
    Io,
    /// The file could not be parsed as Rust.
    Parse,
    /// The destination changed between read and write, so it was left as found.
    Conflict,
}
impl RunErrorKind {
    /// The `kind` field value carried by emitted records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Walk => "walk",
            Self::Io => "io",
            Self::Parse => "parse",
            Self::Conflict => "conflict",
        }
    }
}
/// The `run_error` record naming one failed path and its cause.
///
/// `path` is rendered with [`Path::display`], so a path that is not
/// valid UTF-8 is reported lossily; every other byte, including tabs
/// and newlines, round-trips as a JSON escape.
#[must_use]
pub fn run_error_record(kind: RunErrorKind, path: &Path, message: &str) -> String {
    let mut out = open_record("run_error", DIAGNOSTIC_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "kind", kind.as_str());
    out.push(',');
    push_text(&mut out, "path", &path.display().to_string());
    out.push(',');
    push_text(&mut out, "message", message);
    out.push('}');
    out
}
/// The `doc_file_warning` record naming one documentation file the
/// rewrite passes will not touch.
#[must_use]
pub fn doc_file_warning_record(path: &Path) -> String {
    let mut out = open_record("doc_file_warning", DIAGNOSTIC_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "path", &path.display().to_string());
    out.push('}');
    out
}
/// The `rewrite_file` record naming one file the rewrite passes changed.
///
/// `mode` distinguishes a file written on disk from one that only would
/// have been written under `--dry-run`.
#[must_use]
pub fn rewrite_file_record(mode: RewriteMode, path: &Path) -> String {
    let mut out = open_record("rewrite_file", DIAGNOSTIC_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "mode", mode.as_str());
    out.push(',');
    push_text(&mut out, "path", &path.display().to_string());
    out.push('}');
    out
}
/// The `strip_summary` record closing a `--rewrite` run.
#[must_use]
pub fn strip_summary_record(
    mode: RewriteMode,
    rewritten: u32,
    unchanged: u32,
    errors: u32,
) -> String {
    let mut out = open_record("strip_summary", DIAGNOSTIC_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "mode", mode.as_str());
    out.push(',');
    push_number(&mut out, "rewritten", rewritten);
    out.push(',');
    push_number(&mut out, "unchanged", unchanged);
    out.push(',');
    push_number(&mut out, "errors", errors);
    out.push('}');
    out
}
/// The `lint_summary` record closing a default-mode lint run.
#[must_use]
pub fn lint_summary_record(files: u32, findings: u32, undecided: u32, errors: u32) -> String {
    let mut out = open_record("lint_summary", DIAGNOSTIC_RECORD_VERSION);
    out.push(',');
    push_number(&mut out, "files", files);
    out.push(',');
    push_number(&mut out, "findings", findings);
    out.push(',');
    push_number(&mut out, "undecided", undecided);
    out.push(',');
    push_number(&mut out, "errors", errors);
    out.push('}');
    out
}
/// Canonicalise rustdoc link idioms in `path`, then strip every
/// non-doc `//` and `/* */` comment. Every other byte is preserved.
///
/// The mode selects the [`FileOutcome`] variant; see its variant docs.
///
/// A write lands via a sibling temporary file renamed over the
/// destination, so a partial rewrite is never observable. The temporary
/// file is removed on every returning error path and, best-effort,
/// while a panic unwinds; an abort can still leave it behind.
#[must_use]
pub fn process_file(path: &Path, mode: RewriteMode) -> FileOutcome {
    let original = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return FileOutcome::Failed(FileError::Read(e)),
    };
    let ast: File = match syn::parse_file(&original) {
        Ok(f) => f,
        Err(e) => return FileOutcome::Failed(FileError::Parse(e)),
    };
    let splices = collect_doc_splices(&ast, &original);
    let doc_links_rewritten = u32::try_from(splices.len()).unwrap_or(u32::MAX);
    let after_links = if splices.is_empty() {
        original.clone()
    } else {
        apply_splices(&original, splices)
    };
    let (rewritten, mut counts) = strip_line_comments_with_counts(&after_links);
    counts.doc_links_rewritten = doc_links_rewritten;
    if rewritten == original {
        return FileOutcome::Unchanged(RewriteSummary { counts });
    }
    match mode {
        RewriteMode::DryRun { context } => FileOutcome::WouldRewrite(RewritePreview {
            diff: unified_diff(path, &original, &rewritten, context),
            counts,
        }),
        RewriteMode::Write => match write_atomically(path, &original, &rewritten) {
            Ok(()) => FileOutcome::Rewritten(RewriteSummary { counts }),
            Err(e) => FileOutcome::Failed(e),
        },
    }
}
/// Strip non-doc line and block comments from `src` using
/// [`ra_ap_rustc_lexer`], preserving every other byte verbatim. Thin
/// wrapper over [`strip_line_comments_with_counts`] discarding the
/// [`RewriteCounts`] tally; see that function's docs for the full
/// stripping and blank-line-collapse algorithm.
#[must_use]
pub fn strip_line_comments(src: &str) -> String {
    strip_line_comments_with_counts(src).0
}
/// Like [`strip_line_comments`] but also returns a [`RewriteCounts`]
/// tally. `doc_links_rewritten` stays 0; [`process_file`] fills it.
///
/// Drops a token iff it is a `LineComment` or `BlockComment` with
/// `doc_style: None`. Comment-looking text inside a string literal is
/// structurally unreachable here and round-trips byte-identical.
///
/// A solo-line drop collapses its trailing blank-line scar; an inline
/// (post-code) drop trims the preceding horizontal whitespace instead.
/// A contiguous run of solo-line drops with blanks on both sides emits
/// `max(blanks_above, blanks_below)` rather than their sum.
#[must_use]
pub fn strip_line_comments_with_counts(src: &str) -> (String, RewriteCounts) {
    let mut out = String::with_capacity(src.len());
    let mut counts = RewriteCounts::default();
    let mut cursor = 0usize;
    let mut pending_blank_collapse = false;
    let mut drop_run: Option<DropRun> = None;
    for token in tokenize(src, FrontmatterAllowed::Yes) {
        let end = cursor + token.len as usize;
        let text = &src[cursor..end];
        cursor = end;
        let is_comment = matches!(
            token.kind,
            TokenKind::LineComment { .. } | TokenKind::BlockComment { .. }
        );
        let drop = matches!(
            token.kind,
            TokenKind::LineComment { doc_style: None }
                | TokenKind::BlockComment {
                    doc_style: None,
                    ..
                }
        );
        if drop {
            let before_comment = &src[..end - text.len()];
            let was_line_alone = line_was_blank_before(before_comment);
            let trimmed_horizontal = trim_trailing_whitespace_to_last_newline(&mut out);
            counts.comments_removed = counts.comments_removed.saturating_add(1);
            if !was_line_alone && trimmed_horizontal > 0 {
                counts.inline_trimmed = counts.inline_trimmed.saturating_add(1);
            }
            if was_line_alone {
                if drop_run.is_none() {
                    drop_run = Some(DropRun {
                        blanks_above: trailing_blank_lines(&out),
                        blanks_below_emitted: 0,
                    });
                }
                pending_blank_collapse = true;
            }
            continue;
        }
        if pending_blank_collapse && matches!(token.kind, TokenKind::Whitespace) {
            let trimmed = text.strip_prefix('\n').unwrap_or(text);
            if let Some(run) = drop_run.as_mut() {
                run.blanks_below_emitted += count_newlines(trimmed);
            }
            out.push_str(trimmed);
            pending_blank_collapse = false;
        } else {
            if !matches!(token.kind, TokenKind::Whitespace)
                && let Some(run) = drop_run.take()
                && run.blanks_above >= 1
                && run.blanks_below_emitted >= 1
            {
                pop_one_trailing_newline(&mut out);
                counts.blank_lines_collapsed = counts.blank_lines_collapsed.saturating_add(1);
            }
            out.push_str(text);
            if !matches!(token.kind, TokenKind::Whitespace) || !is_comment {
                pending_blank_collapse = false;
            }
        }
    }
    if let Some(run) = drop_run
        && run.blanks_above >= 1
        && run.blanks_below_emitted >= 1
    {
        pop_one_trailing_newline(&mut out);
        counts.blank_lines_collapsed = counts.blank_lines_collapsed.saturating_add(1);
    }
    (out, counts)
}
struct DropRun {
    blanks_above: usize,
    blanks_below_emitted: usize,
}
fn trailing_blank_lines(s: &str) -> usize {
    let trailing = s.bytes().rev().take_while(|b| *b == b'\n').count();
    trailing.saturating_sub(1)
}
fn count_newlines(s: &str) -> usize {
    s.bytes().filter(|b| *b == b'\n').count()
}
fn pop_one_trailing_newline(s: &mut String) {
    let bytes = s.as_bytes();
    let mut indent_start = bytes.len();
    while indent_start > 0 {
        match bytes[indent_start - 1] {
            b' ' | b'\t' => indent_start -= 1,
            _ => break,
        }
    }
    if indent_start == 0 || bytes[indent_start - 1] != b'\n' {
        return;
    }
    let newline_pos = indent_start - 1;
    s.remove(newline_pos);
}
fn line_was_blank_before(prefix: &str) -> bool {
    let line_start = prefix.rfind('\n').map_or(0, |p| p + 1);
    prefix[line_start..].chars().all(|c| c == ' ' || c == '\t')
}
fn trim_trailing_whitespace_to_last_newline(s: &mut String) -> usize {
    let mut popped = 0;
    while matches!(s.chars().last(), Some(' ' | '\t')) {
        s.pop();
        popped += 1;
    }
    popped
}
#[derive(Debug, Clone)]
struct DocSplice {
    range: std::ops::Range<usize>,
    replacement: String,
}
fn apply_splices(original: &str, mut splices: Vec<DocSplice>) -> String {
    splices.sort_by_key(|s| std::cmp::Reverse(s.range.start));
    let mut out = original.to_string();
    for splice in splices {
        out.replace_range(splice.range, &splice.replacement);
    }
    out
}
fn collect_doc_splices(ast: &syn::File, original: &str) -> Vec<DocSplice> {
    let mut out = Vec::new();
    collect_attr_run_splices(&ast.attrs, original, &mut out);
    collect_cfg_attr_doc_splices(&ast.attrs, original, &mut out);
    for item in &ast.items {
        collect_item_splices(item, original, &mut out);
    }
    out
}
fn collect_item_splices(item: &syn::Item, original: &str, out: &mut Vec<DocSplice>) {
    if let Some(attrs) = item_attrs(item) {
        collect_attr_run_splices(attrs, original, out);
        collect_cfg_attr_doc_splices(attrs, original, out);
    }
    match item {
        syn::Item::Struct(s) => {
            for field in &s.fields {
                collect_attr_run_splices(&field.attrs, original, out);
                collect_cfg_attr_doc_splices(&field.attrs, original, out);
            }
        }
        syn::Item::Enum(e) => {
            for v in &e.variants {
                collect_attr_run_splices(&v.attrs, original, out);
                collect_cfg_attr_doc_splices(&v.attrs, original, out);
                for f in &v.fields {
                    collect_attr_run_splices(&f.attrs, original, out);
                    collect_cfg_attr_doc_splices(&f.attrs, original, out);
                }
            }
        }
        syn::Item::Union(u) => {
            for f in &u.fields.named {
                collect_attr_run_splices(&f.attrs, original, out);
                collect_cfg_attr_doc_splices(&f.attrs, original, out);
            }
        }
        syn::Item::Trait(t) => {
            for ti in &t.items {
                if let Some(attrs) = trait_item_attrs(ti) {
                    collect_attr_run_splices(attrs, original, out);
                    collect_cfg_attr_doc_splices(attrs, original, out);
                }
            }
        }
        syn::Item::Impl(i) => {
            for ii in &i.items {
                if let Some(attrs) = impl_item_attrs(ii) {
                    collect_attr_run_splices(attrs, original, out);
                    collect_cfg_attr_doc_splices(attrs, original, out);
                }
            }
        }
        syn::Item::Mod(m) => {
            if let Some((_, items)) = &m.content {
                for inner in items {
                    collect_item_splices(inner, original, out);
                }
            }
        }
        _ => {}
    }
}
const fn item_attrs(item: &syn::Item) -> Option<&Vec<Attribute>> {
    use syn::Item::{
        Const, Enum, ExternCrate, Fn, Impl, Macro, Mod, Static, Struct, Trait, TraitAlias, Type,
        Union, Use,
    };
    Some(match item {
        Const(i) => &i.attrs,
        Enum(i) => &i.attrs,
        ExternCrate(i) => &i.attrs,
        Fn(i) => &i.attrs,
        Impl(i) => &i.attrs,
        Macro(i) => &i.attrs,
        Mod(i) => &i.attrs,
        Static(i) => &i.attrs,
        Struct(i) => &i.attrs,
        Trait(i) => &i.attrs,
        TraitAlias(i) => &i.attrs,
        Type(i) => &i.attrs,
        Union(i) => &i.attrs,
        Use(i) => &i.attrs,
        _ => return None,
    })
}
const fn trait_item_attrs(item: &syn::TraitItem) -> Option<&Vec<Attribute>> {
    use syn::TraitItem::{Const, Fn, Macro, Type};
    Some(match item {
        Const(i) => &i.attrs,
        Fn(i) => &i.attrs,
        Macro(i) => &i.attrs,
        Type(i) => &i.attrs,
        _ => return None,
    })
}
const fn impl_item_attrs(item: &syn::ImplItem) -> Option<&Vec<Attribute>> {
    use syn::ImplItem::{Const, Fn, Macro, Type};
    Some(match item {
        Const(i) => &i.attrs,
        Fn(i) => &i.attrs,
        Macro(i) => &i.attrs,
        Type(i) => &i.attrs,
        _ => return None,
    })
}
fn collect_attr_run_splices(attrs: &[Attribute], original: &str, out: &mut Vec<DocSplice>) {
    let mut i = 0;
    while i < attrs.len() {
        let Some(_) = doc_attr_literal_span(&attrs[i], original, DocShape::SafeLineOrAttr) else {
            i += 1;
            continue;
        };
        let start = i;
        while i < attrs.len()
            && doc_attr_literal_span(&attrs[i], original, DocShape::SafeLineOrAttr).is_some()
        {
            i += 1;
        }
        emit_run_splices(&attrs[start..i], original, out);
    }
}
#[derive(Debug, Clone, Copy)]
enum DocShape {
    SafeLineOrAttr,
}
fn ident_is(id: &syn::Ident, name: &str) -> bool {
    id.unraw() == name
}
fn path_is(path: &syn::Path, name: &str) -> bool {
    path.get_ident().is_some_and(|id| ident_is(id, name))
}
fn doc_attr_literal_span(
    attr: &Attribute,
    original: &str,
    _shape: DocShape,
) -> Option<DocLiteralSite> {
    let Meta::NameValue(nv) = &attr.meta else {
        return None;
    };
    if !path_is(&nv.path, "doc") {
        return None;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &nv.value
    else {
        return None;
    };
    let range = s.span().byte_range();
    let body = original.get(range.clone())?;
    let kind = classify_doc_literal(body)?;
    Some(DocLiteralSite {
        range,
        kind,
        value: s.value(),
    })
}
#[derive(Debug, Clone)]
struct DocLiteralSite {
    range: std::ops::Range<usize>,
    kind: DocLiteralKind,
    value: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocLiteralKind {
    OuterLine,
    InnerLine,
    QuotedAttr,
}
fn classify_doc_literal(body: &str) -> Option<DocLiteralKind> {
    if body.starts_with("///") && !body.starts_with("////") {
        Some(DocLiteralKind::OuterLine)
    } else if body.starts_with("//!") {
        Some(DocLiteralKind::InnerLine)
    } else if body.starts_with('"') && body.ends_with('"') {
        Some(DocLiteralKind::QuotedAttr)
    } else {
        None
    }
}
fn emit_run_splices(run: &[Attribute], original: &str, out: &mut Vec<DocSplice>) {
    let sites: Vec<DocLiteralSite> = run
        .iter()
        .filter_map(|a| doc_attr_literal_span(a, original, DocShape::SafeLineOrAttr))
        .collect();
    if sites.is_empty() {
        return;
    }
    let joined = sites
        .iter()
        .map(|s| s.value.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let rewritten = rewrite_rustdoc_link_idioms(&joined);
    if rewritten == joined {
        return;
    }
    let parts: Vec<&str> = rewritten.split('\n').collect();
    if parts.len() != sites.len() {
        return;
    }
    for (site, new_payload) in sites.iter().zip(parts) {
        if new_payload == site.value {
            continue;
        }
        let Some(body) = original.get(site.range.clone()) else {
            continue;
        };
        let replacement = match site.kind {
            DocLiteralKind::OuterLine => render_line_doc(body, "///", new_payload),
            DocLiteralKind::InnerLine => render_line_doc(body, "//!", new_payload),
            DocLiteralKind::QuotedAttr => Some(render_quoted_doc_literal(new_payload)),
        };
        let Some(replacement) = replacement else {
            continue;
        };
        out.push(DocSplice {
            range: site.range.clone(),
            replacement,
        });
    }
}
fn render_line_doc(body: &str, marker: &str, new_payload: &str) -> Option<String> {
    if !body.starts_with(marker) {
        return None;
    }
    let mut out = String::with_capacity(marker.len() + new_payload.len());
    out.push_str(marker);
    out.push_str(new_payload);
    Some(out)
}
fn render_quoted_doc_literal(value: &str) -> String {
    proc_macro2::Literal::string(value).to_string()
}
fn collect_cfg_attr_doc_splices(attrs: &[Attribute], original: &str, out: &mut Vec<DocSplice>) {
    for attr in attrs {
        if !is_cfg_attr(attr) {
            continue;
        }
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        collect_cfg_attr_list_doc_splices(list, original, out);
    }
}
fn collect_cfg_attr_list_doc_splices(
    list: &syn::MetaList,
    original: &str,
    out: &mut Vec<DocSplice>,
) {
    let Ok(args) = cfg_predicate_args(list) else {
        return;
    };
    for arg in args.iter().skip(1) {
        let CfgPredicate::Meta(meta) = arg else {
            continue;
        };
        match &**meta {
            Meta::List(inner) if path_is(&inner.path, "cfg_attr") => {
                collect_cfg_attr_list_doc_splices(inner, original, out);
            }
            Meta::NameValue(nv) if path_is(&nv.path, "doc") => {
                push_doc_literal_splice(nv, original, out);
            }
            _ => {}
        }
    }
}
fn push_doc_literal_splice(nv: &syn::MetaNameValue, original: &str, out: &mut Vec<DocSplice>) {
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &nv.value
    else {
        return;
    };
    let range = s.span().byte_range();
    let Some(body) = original.get(range.clone()) else {
        return;
    };
    if !(body.starts_with('"') && body.ends_with('"')) {
        return;
    }
    let value = s.value();
    let rewritten = rewrite_rustdoc_link_idioms(&value);
    if rewritten == value {
        return;
    }
    let replacement = render_quoted_doc_literal(&rewritten);
    out.push(DocSplice { range, replacement });
}
fn push_single_line(out: &mut String, path: &Path) {
    push_single_line_str(out, &path.display().to_string());
}
fn push_single_line_str(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                write!(out, "\\u{:04x}", c as u32).expect("Write for String never fails");
            }
            c => out.push(c),
        }
    }
}
/// Collapse `text` to a single line for a human-facing stream.
///
/// Escapes LF, CR, TAB, every other C0 code point and DEL, so rendered
/// prose can never introduce a column-zero line a record consumer would
/// mistake for a JSON Lines record.
#[must_use]
pub fn single_line(text: &str) -> String {
    let mut out = String::new();
    push_single_line_str(&mut out, text);
    out
}
#[must_use]
fn unified_diff(path: &Path, original: &str, rewritten: &str, context: usize) -> String {
    let diff = TextDiff::from_lines(original, rewritten);
    let mut out = String::new();
    out.push_str("--- a/");
    push_single_line(&mut out, path);
    out.push('\n');
    out.push_str("+++ b/");
    push_single_line(&mut out, path);
    out.push('\n');
    for hunk in diff.unified_diff().context_radius(context).iter_hunks() {
        writeln!(out, "{}", hunk.header()).expect("Write for String never fails");
        for change in hunk.iter_changes() {
            let sign = match change.tag() {
                ChangeTag::Equal => ' ',
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
            };
            let value = change.value();
            out.push(sign);
            out.push_str(value);
            if !value.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}
#[must_use]
fn is_cfg_attr(attr: &Attribute) -> bool {
    match &attr.meta {
        Meta::List(list) => path_is(&list.path, "cfg_attr"),
        _ => false,
    }
}
/// Result of [`scan_doc_files`]: the documentation files found, plus
/// every traversal failure encountered. Callers must count
/// [`DocScan::errors`] towards the run error total; a scan that could
/// not read part of the tree has not established that the tree is clean.
///
/// Has no public constructor: a scan is evidence of a traversal this
/// crate performed, not a value a caller can assert.
#[derive(Debug)]
#[non_exhaustive]
pub struct DocScan {
    files: Vec<PathBuf>,
    errors: Vec<WalkError>,
}
impl DocScan {
    /// Documentation files the scan found.
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
    /// Traversal failures the scan could not resolve into an entry.
    #[must_use]
    pub fn errors(&self) -> &[WalkError] {
        &self.errors
    }
}
/// Walk `root` and report every file that looks like documentation,
/// together with every traversal failure.
///
/// Skips dotfiles/dotdirs, `target/`, and common vendor/build directories
/// (`node_modules`, `vendor`, `dist`, `build`) to avoid polyglot-repo noise.
/// Unreadable entries are reported in [`DocScan::errors`], never dropped.
#[must_use]
pub fn scan_doc_files(root: &Path) -> DocScan {
    let mut scan = DocScan {
        files: Vec::new(),
        errors: Vec::new(),
    };
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            if e.file_type().is_dir()
                && (name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()))
            {
                return false;
            }
            true
        });
    for entry in walker {
        match entry {
            Err(e) => scan.errors.push(WalkError::rooted_at(root, &e)),
            Ok(entry) if entry.file_type().is_file() && is_doc_path(entry.path(), root) => {
                scan.files.push(entry.into_path());
            }
            Ok(_) => {}
        }
    }
    scan
}
const SKIP_DIRS: &[&str] = &["target", "node_modules", "vendor", "dist", "build"];
const ALLOWED_ROOT_DIRS: &[&str] = &["crates", "src"];
fn resolve_walk_roots(root: &Path) -> Vec<PathBuf> {
    let in_scope = root
        .components()
        .any(|c| matches!(c.as_os_str().to_str(), Some(n) if ALLOWED_ROOT_DIRS.contains(& n)));
    if in_scope {
        return vec![root.to_path_buf()];
    }
    ALLOWED_ROOT_DIRS
        .iter()
        .map(|d| root.join(d))
        .filter(|p| p.is_dir())
        .collect()
}
/// Iterate every `.rs` file under `root`, yielding traversal failures
/// rather than discarding them.
///
/// Restricts traversal to `.rs` under allowlisted Rust source roots
/// (`crates/`, `src/`) — `comment-free` is a Rust-only tool. Within those
/// roots, build-output directories (notably nested `target/`) are still pruned.
/// An unreadable entry surfaces as [`WalkError`], never as "no entry".
pub fn walk_rs_files(root: &Path) -> impl Iterator<Item = Result<PathBuf, WalkError>> + use<'_> {
    resolve_walk_roots(root).into_iter().flat_map(|base| {
        WalkDir::new(&base)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                if e.file_type().is_dir()
                    && (name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()))
                {
                    return false;
                }
                true
            })
            .filter_map(move |entry| match entry {
                Err(e) => Some(Err(WalkError::rooted_at(&base, &e))),
                Ok(e) if e.file_type().is_file() => {
                    let path = e.into_path();
                    (path.extension().and_then(|s| s.to_str()) == Some("rs")).then_some(Ok(path))
                }
                Ok(_) => None,
            })
    })
}
#[must_use]
fn is_doc_path(path: &Path, root: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let ext_lc = ext.to_ascii_lowercase();
        if matches!(
            ext_lc.as_str(),
            "md" | "markdown" | "rst" | "adoc" | "asciidoc" | "txt"
        ) {
            return true;
        }
    }
    if let Ok(rel) = path.strip_prefix(root)
        && let Some(first) = rel.components().next()
        && let Some(s) = first.as_os_str().to_str()
    {
        let s_lc = s.to_ascii_lowercase();
        if s_lc == "docs" || s_lc == "doc" {
            return true;
        }
    }
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        let name_uc = name.to_ascii_uppercase();
        for known in BARE_DOC_STEMS {
            if name_uc == *known {
                return true;
            }
        }
    }
    false
}
const BARE_DOC_STEMS: &[&str] = &[
    "LICENSE",
    "LICENCE",
    "NOTICE",
    "COPYING",
    "README",
    "CHANGELOG",
    "AUTHORS",
    "CONTRIBUTORS",
];
/// Doc-comment word budget for the [`doc_lint_file`] linter.
///
/// Examples are defined mechanically as fenced code blocks (` ``` ` or
/// `~~~`) and do not count toward the prose word budget. The linter has
/// no semantic notion of an "example" — fence delimiters are the only
/// signal — and enforces no limit on how many a doc comment may carry.
#[derive(Debug, Clone, Copy)]
pub struct DocBudget {
    /// Maximum words allowed per doc comment (prose only; fenced code,
    /// including ` ``` ` and `~~~` blocks, is excluded from the count).
    pub max_words: usize,
}
/// Result of counting prose words in a doc comment.
///
/// Not `#[non_exhaustive]`: an undecidable lint result is a
/// [`DocLintOutcome`] variant, not a third count.
#[derive(Debug, Clone, Copy)]
pub enum WordCount {
    /// Fence state was balanced; `count` excludes fenced code.
    Balanced(usize),
    /// Fence was opened but never closed; `count` is the fail-closed
    /// recount treating every line as prose.
    FailClosed(usize),
}
impl WordCount {
    /// Return the numeric count, regardless of balance state.
    #[must_use]
    pub const fn count(self) -> usize {
        match self {
            Self::Balanced(n) | Self::FailClosed(n) => n,
        }
    }
    /// True iff this count came from the fail-closed recount path.
    #[must_use]
    pub const fn is_fail_closed(self) -> bool {
        matches!(self, Self::FailClosed(_))
    }
}
/// A single doc-comment over-budget finding emitted by [`doc_lint_file`].
///
/// The count carries its own fail-closed provenance rather than sitting
/// beside a boolean that could contradict it. Has no public
/// constructor: a finding is evidence of a lint this crate ran.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DocFinding {
    item_label: String,
    line: usize,
    words: WordCount,
    budget: usize,
}
impl DocFinding {
    /// Human-readable label for the docced item, e.g. `"fn foo"`.
    #[must_use]
    pub fn item_label(&self) -> &str {
        &self.item_label
    }
    /// Approximate source line of the docced item.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }
    /// Prose word count of the item's doc comment, carrying whether it
    /// came from the fail-closed recount path.
    #[must_use]
    pub const fn words(&self) -> WordCount {
        self.words
    }
    /// The budget the count exceeded.
    #[must_use]
    pub const fn budget(&self) -> usize {
        self.budget
    }
}
/// Why the linter could not decide an item, carrying the evidence that
/// cause — and only that cause — supports.
///
/// A word bound exists only for the `cfg` case: an unreadable payload
/// resolves to text this tool never sees, so no count of it is
/// representable here.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum UndecidedCause {
    /// Part of the doc set sits behind unresolved `cfg` predicates.
    /// Carries both bounds: the count present in every configuration,
    /// and the count with every `cfg_attr` doc payload active.
    ConfigurationDependent {
        /// Prose word count of the doc set present in every configuration.
        unconditional: WordCount,
        /// Prose word count with every `cfg_attr` doc payload active.
        all_configurations: WordCount,
    },
    /// The doc-value position holds an expression this tool cannot
    /// read — a macro call resolved only by expansion.
    UnreadableDocPayload,
    /// A macro token body carries a doc attribute this tool does not
    /// expand, so the item it documents does not exist to be counted.
    UninspectedMacroBody,
}
/// An item whose doc set the linter could not decide.
///
/// The cause carries its own evidence, so a count that no reading
/// produced cannot be attached to an item whose text was never read.
/// Has no public constructor.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DocUndecided {
    item_label: String,
    line: usize,
    budget: usize,
    cause: UndecidedCause,
}
impl DocUndecided {
    /// Human-readable label for the docced item, e.g. `"fn foo"`.
    #[must_use]
    pub fn item_label(&self) -> &str {
        &self.item_label
    }
    /// Approximate source line of the docced item.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }
    /// The budget in force when the item was found undecidable.
    #[must_use]
    pub const fn budget(&self) -> usize {
        self.budget
    }
    /// Why the item could not be decided, with the evidence for it.
    #[must_use]
    pub const fn cause(&self) -> UndecidedCause {
        self.cause
    }
    /// The outcome this item's record asserts.
    #[must_use]
    pub const fn outcome(&self) -> DocLintOutcome {
        match self.cause {
            UndecidedCause::ConfigurationDependent { .. } => DocLintOutcome::ConfigurationDependent,
            UndecidedCause::UnreadableDocPayload => DocLintOutcome::UnreadableDocPayload,
            UndecidedCause::UninspectedMacroBody => DocLintOutcome::UninspectedMacroBody,
        }
    }
}
/// What one [`doc_lint_file`] pass established about a file.
///
/// Findings and undecided items are separate sequences rather than one
/// sequence tagged by outcome, so a caller counting findings for an exit
/// code cannot count an item the linter never decided.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DocLintReport {
    findings: Vec<DocFinding>,
    undecided: Vec<DocUndecided>,
}
impl DocLintReport {
    /// Items proven over budget in every configuration.
    #[must_use]
    pub fn findings(&self) -> &[DocFinding] {
        &self.findings
    }
    /// Items whose verdict depends on unresolved `cfg` predicates.
    #[must_use]
    pub fn undecided(&self) -> &[DocUndecided] {
        &self.undecided
    }
}
/// Lint `ast` for doc-comments whose prose word count exceeds `budget.max_words`.
///
/// `///`, `//!` and `#[doc=...]` payloads count as one document; fenced
/// lines are excluded. `cfg_attr` payloads, nested ones included, are
/// held separately: constant predicates fold, others stay unresolved.
/// A finding requires the unconditional set alone to be over budget.
///
/// Anything the pass could not read or could not decide is reported as
/// an explicit indeterminate — see [`UndecidedCause`] — never as clean.
#[must_use]
pub fn doc_lint_file(ast: &syn::File, budget: DocBudget) -> DocLintReport {
    let mut visitor = DocLintVisitor {
        budget,
        report: DocLintReport::default(),
        macro_name_hint: None,
    };
    visitor.lint_attrs(&ast.attrs, "file-level", None);
    syn::visit::Visit::visit_file(&mut visitor, ast);
    visitor.report
}
struct DocLintVisitor {
    budget: DocBudget,
    report: DocLintReport,
    macro_name_hint: Option<String>,
}
impl DocLintVisitor {
    fn lint_attrs(&mut self, attrs: &[Attribute], label: &str, span_line: Option<usize>) {
        let Some(docs) = extract_doc_text(attrs) else {
            return;
        };
        let line = span_line.unwrap_or(docs.line);
        match decide_doc_budget(&docs, self.budget.max_words) {
            DocVerdict::Clean => {}
            DocVerdict::Overlong(words) => self.report.findings.push(DocFinding {
                item_label: label.to_string(),
                line,
                words,
                budget: self.budget.max_words,
            }),
            DocVerdict::Undecided(cause) => self.report.undecided.push(DocUndecided {
                item_label: label.to_string(),
                line,
                budget: self.budget.max_words,
                cause,
            }),
        }
    }
}
enum DocVerdict {
    Clean,
    Overlong(WordCount),
    Undecided(UndecidedCause),
}
fn decide_doc_budget(docs: &ItemDocs, budget: usize) -> DocVerdict {
    if docs.has_unreadable() {
        return DocVerdict::Undecided(UndecidedCause::UnreadableDocPayload);
    }
    let all = prose_word_count(&docs.all_configurations_text());
    if !docs.has_conditional() {
        return if all.count() > budget {
            DocVerdict::Overlong(all)
        } else {
            DocVerdict::Clean
        };
    }
    let unconditional = prose_word_count(&docs.unconditional_text());
    let fence_state_resolved = !unconditional.is_fail_closed() && !docs.conditional_toggles_fence();
    if fence_state_resolved && unconditional.count() > budget {
        return DocVerdict::Overlong(unconditional);
    }
    DocVerdict::Undecided(UndecidedCause::ConfigurationDependent {
        unconditional,
        all_configurations: all,
    })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocOrigin {
    Unconditional,
    Conditional,
}
#[derive(Debug, Clone)]
enum DocPayload {
    Text(String),
    Unreadable,
}
#[derive(Debug, Clone)]
struct DocPart {
    origin: DocOrigin,
    payload: DocPayload,
}
#[derive(Debug, Clone)]
struct ItemDocs {
    line: usize,
    parts: Vec<DocPart>,
}
impl ItemDocs {
    fn has_conditional(&self) -> bool {
        self.parts
            .iter()
            .any(|p| p.origin == DocOrigin::Conditional)
    }
    fn has_unreadable(&self) -> bool {
        self.parts
            .iter()
            .any(|p| matches!(p.payload, DocPayload::Unreadable))
    }
    fn conditional_toggles_fence(&self) -> bool {
        self.parts
            .iter()
            .filter(|p| p.origin == DocOrigin::Conditional)
            .any(|p| self::payload_text(p).is_some_and(|t| t.lines().any(opens_or_closes_fence)))
    }
    fn unconditional_text(&self) -> String {
        self.text(|p| p.origin == DocOrigin::Unconditional)
    }
    fn all_configurations_text(&self) -> String {
        self.text(|_| true)
    }
    fn text(&self, keep: impl Fn(&DocPart) -> bool) -> String {
        self.parts
            .iter()
            .filter(|p| keep(p))
            .filter_map(self::payload_text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}
const fn payload_text(part: &DocPart) -> Option<&str> {
    match &part.payload {
        DocPayload::Text(t) => Some(t.as_str()),
        DocPayload::Unreadable => None,
    }
}
fn extract_doc_text(attrs: &[Attribute]) -> Option<ItemDocs> {
    let mut parts: Vec<DocPart> = Vec::new();
    let mut first_line: Option<usize> = None;
    for attr in attrs {
        let line = attr
            .path()
            .get_ident()
            .map_or_else(|| attr.span().start().line, |id| id.span().start().line);
        if let Some(payload) = doc_payload(attr) {
            if first_line.is_none() {
                first_line = Some(line);
            }
            parts.push(DocPart {
                origin: DocOrigin::Unconditional,
                payload,
            });
        } else if is_cfg_attr(attr) {
            let before = parts.len();
            collect_cfg_attr_doc_parts(attr, &mut parts);
            if parts.len() > before && first_line.is_none() {
                first_line = Some(line);
            }
        }
    }
    let line = first_line?;
    Some(ItemDocs { line, parts })
}
/// The doc payload of one `doc = <expr>` attribute.
///
/// `None` means the attribute carries no doc prose at all — it is not a
/// `doc` name-value attribute, or it is the `#[doc(...)]` metadata form.
/// An expression that is not a string literal is a payload this tool
/// cannot read, never an absent one: `#[doc = include_str!("x.md")]`
/// carries prose that only expansion resolves.
fn doc_payload(attr: &Attribute) -> Option<DocPayload> {
    let Meta::NameValue(nv) = &attr.meta else {
        return None;
    };
    if !path_is(&nv.path, "doc") {
        return None;
    }
    Some(doc_value_payload(&nv.value))
}
fn doc_value_payload(value: &syn::Expr) -> DocPayload {
    match value {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => DocPayload::Text(s.value()),
        _ => DocPayload::Unreadable,
    }
}
/// Truth value of a `cfg` predicate under a build configuration this
/// crate has not been told, folded as far as the predicate's own
/// boolean constants allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CfgTruth {
    Always,
    Never,
    Unresolved,
}
impl CfgTruth {
    const fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Never, _) | (_, Self::Never) => Self::Never,
            (Self::Always, Self::Always) => Self::Always,
            _ => Self::Unresolved,
        }
    }
    const fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Always, _) | (_, Self::Always) => Self::Always,
            (Self::Never, Self::Never) => Self::Never,
            _ => Self::Unresolved,
        }
    }
    const fn negate(self) -> Self {
        match self {
            Self::Always => Self::Never,
            Self::Never => Self::Always,
            Self::Unresolved => Self::Unresolved,
        }
    }
    const fn doc_origin(self) -> Option<DocOrigin> {
        match self {
            Self::Always => Some(DocOrigin::Unconditional),
            Self::Unresolved => Some(DocOrigin::Conditional),
            Self::Never => None,
        }
    }
}
/// One argument of a `cfg`-style predicate list, which rustc accepts as
/// either a boolean literal or a `Meta` (path, `key = "value"`, or a
/// `all`/`any`/`not` list). `syn::Meta` alone cannot hold `true` or
/// `false`, because they are keywords rather than paths.
enum CfgPredicate {
    Bool(bool),
    Meta(Box<Meta>),
}
impl syn::parse::Parse for CfgPredicate {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(syn::LitBool) {
            return Ok(Self::Bool(input.parse::<syn::LitBool>()?.value()));
        }
        Ok(Self::Meta(Box::new(input.parse()?)))
    }
}
fn fold_cfg_predicate(predicate: &CfgPredicate) -> CfgTruth {
    let meta = match predicate {
        CfgPredicate::Bool(true) => return CfgTruth::Always,
        CfgPredicate::Bool(false) => return CfgTruth::Never,
        CfgPredicate::Meta(meta) => &**meta,
    };
    let Meta::List(list) = meta else {
        return CfgTruth::Unresolved;
    };
    let Ok(inner) = cfg_predicate_args(list) else {
        return CfgTruth::Unresolved;
    };
    if path_is(&list.path, "all") {
        return inner
            .iter()
            .fold(CfgTruth::Always, |acc, m| acc.and(fold_cfg_predicate(m)));
    }
    if path_is(&list.path, "any") {
        return inner
            .iter()
            .fold(CfgTruth::Never, |acc, m| acc.or(fold_cfg_predicate(m)));
    }
    match (path_is(&list.path, "not"), inner.first()) {
        (true, Some(only)) if inner.len() == 1 => fold_cfg_predicate(only).negate(),
        _ => CfgTruth::Unresolved,
    }
}
fn cfg_predicate_args(list: &syn::MetaList) -> syn::Result<Punctuated<CfgPredicate, Token![,]>> {
    list.parse_args_with(Punctuated::parse_terminated)
}
fn collect_cfg_attr_doc_parts(attr: &Attribute, out: &mut Vec<DocPart>) {
    let Meta::List(list) = &attr.meta else {
        return;
    };
    if !path_is(&list.path, "cfg_attr") {
        return;
    }
    collect_cfg_attr_list_doc_parts(list, CfgTruth::Always, out);
}
fn collect_cfg_attr_list_doc_parts(list: &syn::MetaList, outer: CfgTruth, out: &mut Vec<DocPart>) {
    let Ok(args) = cfg_predicate_args(list) else {
        return;
    };
    let mut args = args.into_iter();
    let Some(predicate) = args.next() else {
        return;
    };
    let truth = outer.and(fold_cfg_predicate(&predicate));
    let Some(origin) = truth.doc_origin() else {
        return;
    };
    for arg in args {
        let CfgPredicate::Meta(meta) = arg else {
            continue;
        };
        match &*meta {
            Meta::NameValue(nv) if path_is(&nv.path, "doc") => out.push(DocPart {
                origin,
                payload: doc_value_payload(&nv.value),
            }),
            Meta::List(inner) if path_is(&inner.path, "cfg_attr") => {
                collect_cfg_attr_list_doc_parts(inner, truth, out);
            }
            _ => {}
        }
    }
}
const fn leading_columns(line: &str) -> (usize, usize) {
    let bytes = line.as_bytes();
    let mut col = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b' ' => col += 1,
            b'\t' => col = (col / 4 + 1) * 4,
            _ => return (col, i),
        }
        i += 1;
    }
    (col, i)
}
fn fence_delimiter_run(line: &str) -> Option<(u8, usize, &str)> {
    let (col, offset) = leading_columns(line);
    if col > 3 {
        return None;
    }
    let rest = &line[offset..];
    let marker = *rest.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let run = rest.bytes().take_while(|b| *b == marker).count();
    if run < 3 {
        return None;
    }
    Some((marker, run, &rest[run..]))
}
fn opens_or_closes_fence(line: &str) -> bool {
    fence_delimiter_run(line).is_some()
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FenceState {
    Closed,
    Open { marker: u8, run: usize },
}
impl FenceState {
    fn advance(self, line: &str) -> (Self, bool) {
        match (self, fence_delimiter_run(line)) {
            (Self::Closed, Some((marker, run, info))) if marker == b'~' || !info.contains('`') => {
                (Self::Open { marker, run }, true)
            }
            (Self::Open { marker, run }, Some((found, found_run, tail)))
                if found == marker && found_run >= run && tail.trim().is_empty() =>
            {
                (Self::Closed, true)
            }
            (state, _) => (state, false),
        }
    }
    const fn is_open(self) -> bool {
        matches!(self, Self::Open { .. })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineFrame {
    Code,
    Definition,
    Prose,
}
struct BlockScan {
    fence: FenceState,
    indented_code: bool,
    block_start: bool,
}
impl BlockScan {
    const fn new() -> Self {
        Self {
            fence: FenceState::Closed,
            indented_code: false,
            block_start: true,
        }
    }
    fn classify(&mut self, line: &str) -> LineFrame {
        if self.fence.is_open() {
            let (next, _) = self.fence.advance(line);
            self.fence = next;
            self.block_start = !next.is_open();
            return LineFrame::Code;
        }
        let blank = line.trim().is_empty();
        let (col, _) = leading_columns(line);
        if self.indented_code {
            if blank || col >= 4 {
                return LineFrame::Code;
            }
            self.indented_code = false;
            self.block_start = true;
        }
        if blank {
            self.block_start = true;
            return LineFrame::Prose;
        }
        let (next, delimiter) = self.fence.advance(line);
        if delimiter {
            self.fence = next;
            self.block_start = false;
            return LineFrame::Code;
        }
        if self.block_start && col >= 4 {
            self.indented_code = true;
            return LineFrame::Code;
        }
        if self.block_start && is_reference_definition(line) {
            return LineFrame::Definition;
        }
        self.block_start = false;
        LineFrame::Prose
    }
}
fn prose_word_count(doc_text: &str) -> WordCount {
    let mut fence = FenceState::Closed;
    let mut words = 0usize;
    for line in doc_text.lines() {
        let (next, delimiter) = fence.advance(line);
        fence = next;
        if delimiter || fence.is_open() {
            continue;
        }
        words += line.split_whitespace().count();
    }
    if fence.is_open() {
        let recount = doc_text.lines().map(|l| l.split_whitespace().count()).sum();
        return WordCount::FailClosed(recount);
    }
    WordCount::Balanced(words)
}
impl<'ast> syn::visit::Visit<'ast> for DocLintVisitor {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if let Some((label, attrs, line)) = item_label_and_attrs(node) {
            self.lint_attrs(attrs, &label, Some(line));
        }
        syn::visit::visit_item(self, node);
    }
    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        if let Some((label, attrs, line)) = trait_item_label_and_attrs(node) {
            self.lint_attrs(attrs, &label, Some(line));
        }
        syn::visit::visit_trait_item(self, node);
    }
    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        if let Some((label, attrs, line)) = impl_item_label_and_attrs(node) {
            self.lint_attrs(attrs, &label, Some(line));
        }
        syn::visit::visit_impl_item(self, node);
    }
    fn visit_item_macro(&mut self, node: &'ast syn::ItemMacro) {
        self.macro_name_hint = node.ident.as_ref().map(|id| format!("macro {id}"));
        syn::visit::visit_item_macro(self, node);
        self.macro_name_hint = None;
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let hint = self.macro_name_hint.take();
        if macro_tokens_carry_doc_attribute(node.tokens.clone()) {
            let item_label = hint.unwrap_or_else(|| macro_invocation_label(node));
            self.report.undecided.push(DocUndecided {
                item_label,
                line: node.path.span().start().line,
                budget: self.budget.max_words,
                cause: UndecidedCause::UninspectedMacroBody,
            });
        }
        syn::visit::visit_macro(self, node);
    }
    fn visit_field(&mut self, node: &'ast syn::Field) {
        let line = node.span().start().line;
        let label = node
            .ident
            .as_ref()
            .map_or_else(|| "field (tuple)".to_string(), |id| format!("field {id}"));
        self.lint_attrs(&node.attrs, &label, Some(line));
        syn::visit::visit_field(self, node);
    }
    fn visit_variant(&mut self, node: &'ast syn::Variant) {
        let line = node.span().start().line;
        let label = format!("variant {}", node.ident);
        self.lint_attrs(&node.attrs, &label, Some(line));
        syn::visit::visit_variant(self, node);
    }
}
fn macro_invocation_label(mac: &syn::Macro) -> String {
    let path = mac
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");
    format!("macro {path}")
}
fn macro_tokens_carry_doc_attribute(tokens: proc_macro2::TokenStream) -> bool {
    let trees: Vec<proc_macro2::TokenTree> = tokens.into_iter().collect();
    trees.iter().enumerate().any(|(i, tree)| {
        let proc_macro2::TokenTree::Group(group) = tree else {
            return false;
        };
        let is_attribute_body = group.delimiter() == proc_macro2::Delimiter::Bracket
            && attribute_pound_precedes(&trees, i);
        (is_attribute_body && assigns_doc(group.stream()))
            || macro_tokens_carry_doc_attribute(group.stream())
    })
}
fn attribute_pound_precedes(trees: &[proc_macro2::TokenTree], index: usize) -> bool {
    let is_punct = |offset: usize, want: char| {
        index
            .checked_sub(offset)
            .and_then(|i| trees.get(i))
            .is_some_and(|t| matches!(t, proc_macro2::TokenTree::Punct(p) if p.as_char() == want))
    };
    is_punct(1, '#') || (is_punct(1, '!') && is_punct(2, '#'))
}
fn assigns_doc(tokens: proc_macro2::TokenStream) -> bool {
    let trees: Vec<proc_macro2::TokenTree> = tokens.into_iter().collect();
    trees.iter().enumerate().any(|(i, tree)| match tree {
        proc_macro2::TokenTree::Ident(id) => {
            ident_is(id, "doc")
                && matches!(
                    trees.get(i + 1),
                    Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '='
                )
        }
        proc_macro2::TokenTree::Group(group) => assigns_doc(group.stream()),
        _ => false,
    })
}
fn item_label_and_attrs(item: &syn::Item) -> Option<(String, &[Attribute], usize)> {
    use syn::Item::{
        Const, Enum, ExternCrate, Fn, Impl, Macro, Mod, Static, Struct, Trait, Type, Union, Use,
    };
    let (label, attrs, line): (String, &[Attribute], usize) = match item {
        Fn(i) => (
            format!("fn {}", i.sig.ident),
            &i.attrs,
            i.sig.fn_token.span.start().line,
        ),
        Struct(i) => (
            format!("struct {}", i.ident),
            &i.attrs,
            i.struct_token.span.start().line,
        ),
        Enum(i) => (
            format!("enum {}", i.ident),
            &i.attrs,
            i.enum_token.span.start().line,
        ),
        Trait(i) => (
            format!("trait {}", i.ident),
            &i.attrs,
            i.trait_token.span.start().line,
        ),
        Mod(i) => (
            format!("mod {}", i.ident),
            &i.attrs,
            i.mod_token.span.start().line,
        ),
        Const(i) => (
            format!("const {}", i.ident),
            &i.attrs,
            i.const_token.span.start().line,
        ),
        Static(i) => (
            format!("static {}", i.ident),
            &i.attrs,
            i.static_token.span.start().line,
        ),
        Type(i) => (
            format!("type {}", i.ident),
            &i.attrs,
            i.type_token.span.start().line,
        ),
        Union(i) => (
            format!("union {}", i.ident),
            &i.attrs,
            i.union_token.span.start().line,
        ),
        Impl(i) => ("impl".to_string(), &i.attrs, i.impl_token.span.start().line),
        Use(i) => ("use".to_string(), &i.attrs, i.use_token.span.start().line),
        ExternCrate(i) => (
            format!("extern crate {}", i.ident),
            &i.attrs,
            i.extern_token.span.start().line,
        ),
        Macro(i) => (
            i.ident
                .as_ref()
                .map_or_else(|| "macro".to_string(), |id| format!("macro {id}")),
            &i.attrs,
            i.mac.span().start().line,
        ),
        _ => return None,
    };
    Some((label, attrs, line))
}
fn trait_item_label_and_attrs(item: &syn::TraitItem) -> Option<(String, &[Attribute], usize)> {
    use syn::TraitItem::{Const, Fn, Type};
    let (label, attrs, line): (String, &[Attribute], usize) = match item {
        Fn(i) => (
            format!("trait fn {}", i.sig.ident),
            &i.attrs,
            i.sig.fn_token.span.start().line,
        ),
        Const(i) => (
            format!("trait const {}", i.ident),
            &i.attrs,
            i.const_token.span.start().line,
        ),
        Type(i) => (
            format!("trait type {}", i.ident),
            &i.attrs,
            i.type_token.span.start().line,
        ),
        _ => return None,
    };
    Some((label, attrs, line))
}
fn impl_item_label_and_attrs(item: &syn::ImplItem) -> Option<(String, &[Attribute], usize)> {
    use syn::ImplItem::{Const, Fn, Type};
    let (label, attrs, line): (String, &[Attribute], usize) = match item {
        Fn(i) => (
            format!("impl fn {}", i.sig.ident),
            &i.attrs,
            i.sig.fn_token.span.start().line,
        ),
        Const(i) => (
            format!("impl const {}", i.ident),
            &i.attrs,
            i.const_token.span.start().line,
        ),
        Type(i) => (
            format!("impl type {}", i.ident),
            &i.attrs,
            i.type_token.span.start().line,
        ),
        _ => return None,
    };
    Some((label, attrs, line))
}
#[cfg(test)]
mod walk_error_tests {
    use super::walk_rs_files;
    use std::error::Error as _;
    use std::path::Path;
    fn a_walk_error() -> super::WalkError {
        walk_rs_files(Path::new("/comment-free-no-such-root/src"))
            .next()
            .expect("an unreadable root yields one traversal failure")
            .expect_err("the entry cannot be traversed")
    }
    #[test]
    fn the_error_chain_does_not_leak_the_walker_error_type() {
        let err = a_walk_error();
        let source = err.source().expect("a walk error carries its source");
        assert!(
            source.downcast_ref::<walkdir::Error>().is_none(),
            "walkdir::Error is reachable through WalkError::source"
        );
    }
    #[test]
    fn the_rendered_message_survives_the_seal() {
        let err = a_walk_error();
        assert!(!err.message().is_empty());
        assert!(
            err.to_string().contains(&err.message()),
            "display drops the source text: {err}"
        );
    }
}
#[cfg(test)]
mod atomic_write_tests {
    use super::{
        FileError, FileOutcome, RewriteMode, RunErrorKind, create_sibling_temp, process_file,
        write_atomically,
    };
    use std::fs;
    const fn write_opts() -> RewriteMode {
        RewriteMode::Write
    }
    fn dir_entry_names(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
    #[test]
    fn every_file_error_class_maps_to_its_run_error_kind() {
        let io = || std::io::Error::other("boom");
        let parse = syn::parse_file("fn f( {").unwrap_err();
        assert_eq!(FileError::Parse(parse).kind(), RunErrorKind::Parse);
        assert_eq!(FileError::Read(io()).kind(), RunErrorKind::Io);
        assert_eq!(FileError::Write(io()).kind(), RunErrorKind::Io);
        assert_eq!(FileError::SymlinkDestination.kind(), RunErrorKind::Io);
        assert_eq!(FileError::Conflict.kind(), RunErrorKind::Conflict);
    }
    #[test]
    fn an_unreadable_file_is_a_read_failure_not_a_parse_failure() {
        let td = tempfile::tempdir().unwrap();
        let missing = td.path().join("nope.rs");
        match process_file(&missing, write_opts()) {
            FileOutcome::Failed(e @ FileError::Read(_)) => {
                assert_eq!(e.kind(), RunErrorKind::Io);
            }
            other => panic!("expected Failed(Read), got {other:?}"),
        }
    }
    #[test]
    fn a_conflict_reports_itself_as_a_conflict_run_error() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "concurrent editor won\n").unwrap();
        let err = write_atomically(&path, "bytes we read earlier\n", "our rewrite\n").unwrap_err();
        assert_eq!(err.kind(), RunErrorKind::Conflict);
        assert_eq!(err.to_string(), "destination changed since it was read");
        let record = super::run_error_record(err.kind(), &path, &err.to_string());
        assert!(record.contains("\"kind\":\"conflict\""), "{record}");
        assert!(
            record.contains("\"message\":\"destination changed since it was read\""),
            "{record}"
        );
    }
    #[test]
    fn destination_changed_since_read_is_a_conflict_and_leaves_bytes_untouched() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "concurrent editor won\n").unwrap();
        let err = write_atomically(&path, "bytes we read earlier\n", "our rewrite\n").unwrap_err();
        assert!(matches!(err, FileError::Conflict), "got {err:?}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "concurrent editor won\n"
        );
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
    #[test]
    fn a_conflicted_destination_is_still_rewritable_afterwards() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "// drop me\nfn f() {}\n").unwrap();
        assert!(matches!(
            write_atomically(&path, "stale bytes\n", "never lands\n"),
            Err(FileError::Conflict)
        ));
        match process_file(&path, write_opts()) {
            FileOutcome::Rewritten(_) => {}
            other => panic!("expected Rewritten after a conflict, got {other:?}"),
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn f() {}\n");
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
    #[test]
    fn successful_write_leaves_no_temp_files_behind() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "// drop me\nfn f() {}\n").unwrap();
        match process_file(&path, write_opts()) {
            FileOutcome::Rewritten(_) => {}
            other => panic!("expected Rewritten, got {other:?}"),
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn f() {}\n");
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
    #[cfg(unix)]
    #[test]
    fn rewrite_preserves_the_destination_file_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "// drop me\nfn f() {}\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        match process_file(&path, write_opts()) {
            FileOutcome::Rewritten(_) => {}
            other => panic!("expected Rewritten, got {other:?}"),
        }
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "mode was {mode:o}");
    }
    #[cfg(unix)]
    #[test]
    fn a_symlink_destination_is_refused_and_its_target_is_left_alone() {
        let td = tempfile::tempdir().unwrap();
        let target = td.path().join("real.rs");
        let link = td.path().join("link.rs");
        fs::write(&target, "// drop me\nfn f() {}\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        match process_file(&link, write_opts()) {
            FileOutcome::Failed(e @ FileError::SymlinkDestination) => {
                assert_eq!(e.kind(), RunErrorKind::Io);
                assert!(e.to_string().contains("symbolic link"), "got {e}");
            }
            other => panic!("expected Failed(SymlinkDestination), got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "// drop me\nfn f() {}\n"
        );
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            dir_entry_names(td.path()),
            vec!["link.rs".to_string(), "real.rs".to_string()]
        );
    }
    #[test]
    fn a_panic_after_temp_creation_unwinds_through_the_guard_and_removes_the_temp() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "fn f() {}\n").unwrap();
        let dir = td.path().to_path_buf();
        let panicked = std::panic::catch_unwind(|| {
            let (_file, _guard) = create_sibling_temp(&path, &dir).unwrap();
            assert_eq!(dir_entry_names(&dir).len(), 2, "temp should exist here");
            panic!("simulated failure after the temp file exists");
        });
        assert!(panicked.is_err(), "expected the closure to unwind");
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
    #[test]
    fn an_io_failure_after_temp_creation_leaves_no_residue() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::create_dir(&path).unwrap();
        let err = write_atomically(&path, "bytes we read earlier\n", "our rewrite\n").unwrap_err();
        assert!(matches!(err, FileError::Write(_)), "got {err:?}");
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
}
#[cfg(test)]
mod process_file_tests {
    use super::{FileOutcome, RewriteMode, process_file, strip_line_comments};
    use std::fs;
    const fn opts() -> RewriteMode {
        RewriteMode::DryRun { context: 3 }
    }
    #[test]
    fn whitespace_only_file_is_unchanged() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "   \n\t\n  \n").unwrap();
        match process_file(&path, opts()) {
            FileOutcome::Unchanged(_) => {}
            other => panic!("expected Unchanged for whitespace-only file, got {other:?}"),
        }
    }
    #[test]
    fn empty_file_is_unchanged() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "").unwrap();
        match process_file(&path, opts()) {
            FileOutcome::Unchanged(_) => {}
            other => panic!("expected Unchanged for empty file, got {other:?}"),
        }
    }
    #[test]
    fn safety_only_file_is_rewritten_and_counts_the_comment_removed() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "// SAFETY: pointer is valid\nfn f() {}\n").unwrap();
        match process_file(&path, opts()) {
            FileOutcome::WouldRewrite(preview) => {
                assert_eq!(preview.counts().comments_removed(), 1);
                assert!(!preview.diff().is_empty(), "dry run must carry a diff");
            }
            other => panic!("expected WouldRewrite for SAFETY-only file, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "// SAFETY: pointer is valid\nfn f() {}\n",
            "dry run must not touch the file"
        );
    }
    #[test]
    fn strip_line_comments_drops_ordinary_line_comments() {
        let src = "// kill me\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), "fn f() {}\n");
    }
    #[test]
    fn strip_line_comments_drops_ordinary_block_comments() {
        let src = "/* kill me */\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), "fn f() {}\n");
    }
    #[test]
    fn strip_line_comments_keeps_doc_comments() {
        let src = "/// keep me\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
        let src = "//! keep me\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
        let src = "/** keep me */\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_drops_safety_idiom() {
        let src = "// SAFETY: hand-written invariant\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), "fn f() {}\n");
    }
    #[test]
    fn strip_line_comments_drops_prose_merely_containing_safety() {
        let src = "// this code is SAFETY critical, review carefully\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), "fn f() {}\n");
    }
    #[test]
    fn strip_line_comments_drops_auto_trait_policy_markers() {
        let src = "// AUTO-TRAIT-POLICY-BEGIN\nfn f() {}\n// AUTO-TRAIT-POLICY-END\n";
        assert_eq!(strip_line_comments(src), "fn f() {}\n");
    }
    #[test]
    fn strip_line_comments_preserves_string_literals_with_marker_text() {
        let src = "const X: &str = \"// not actually a comment\";\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
        let src = "const X: &str = \"// SAFETY: still inside a string\";\nfn f() {}\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_is_a_fixed_point_on_clean_source() {
        let src = "pub fn f(x: u32) -> u32 { x + 1 }\n";
        let once = strip_line_comments(src);
        let twice = strip_line_comments(&once);
        assert_eq!(once, twice);
        assert_eq!(once, src);
    }
    #[test]
    fn strip_line_comments_does_not_reflow_code() {
        let src = "fn f(  x  :  u32  )  ->  u32  {\n    // kill me\n    x  +  1\n}\n";
        let expected = "fn f(  x  :  u32  )  ->  u32  {\n    x  +  1\n}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_line_comment_trims_preceding_whitespace() {
        let src = "let x = 1; // trailing\nlet y = 2;\n";
        let expected = "let x = 1;\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_block_comment_trims_preceding_whitespace() {
        let src = "let x = 1; /* inline */\nlet y = 2;\n";
        let expected = "let x = 1;\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_trim_handles_multiple_consecutive_trailing_comments() {
        let src = "let x = 1; /* a */ /* b */ // tail\nlet y = 2;\n";
        let expected = "let x = 1;\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_trim_handles_mixed_tabs_and_spaces() {
        let src = "let x = 1;\t \t// tabs\nlet y = 2;\n";
        let expected = "let x = 1;\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_trim_drop_receiver_pattern() {
        let src = "drop(rx); // close receiver\n";
        let expected = "drop(rx);\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_inline_trim_does_not_touch_line_with_no_removed_comment() {
        let src = "let x = 1;   \nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_inline_trim_removes_trailing_safety_line() {
        let src = "let x = 1; // SAFETY: invariant\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), "let x = 1;\nlet y = 2;\n");
    }
    #[test]
    fn strip_line_comments_inline_trim_does_not_touch_doc_comment() {
        let src = "let x = 1; /// doc-shaped (illegal but lexer keeps it)\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_inline_trim_already_clean_is_noop() {
        let src = "let x = 1;\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_solo_comment_between_code_leaves_no_blank() {
        let src = "fn a() {}\n// removed\nfn b() {}\n";
        let expected = "fn a() {}\nfn b() {}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_contiguous_solo_block_between_code_leaves_no_blank() {
        let src = "fn a() {}\n// r1\n// r2\n// r3\nfn b() {}\n";
        let expected = "fn a() {}\nfn b() {}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_preserves_pre_existing_blank_above_removed_block() {
        let src = "fn a() {}\n\n// removed\nfn b() {}\n";
        let expected = "fn a() {}\n\nfn b() {}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_preserves_pre_existing_blank_below_removed_block() {
        let src = "fn a() {}\n// removed\n\nfn b() {}\n";
        let expected = "fn a() {}\n\nfn b() {}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_collapses_symmetric_blanks_around_removed_block() {
        let src = "fn a() {}\n\n// removed\n\nfn b() {}\n";
        let expected = "fn a() {}\n\nfn b() {}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_collapses_symmetric_blanks_around_section_header_pattern() {
        let src = "    let x = 1;\n\n    // ── Step 2: do thing ──\n\n    let y = 2;\n";
        let expected = "    let x = 1;\n\n    let y = 2;\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_collapses_symmetric_blanks_around_block_with_internal_lines() {
        let src = "fn a() {}\n\n// header\n//\n// body line\n\nfn b() {}\n";
        let expected = "fn a() {}\n\nfn b() {}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_preserves_double_blank_above_only() {
        let src = "fn a() {}\n\n\n// removed\nfn b() {}\n";
        let expected = "fn a() {}\n\n\nfn b() {}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_symmetric_collapse_takes_max_of_above_and_below() {
        let src = "fn a() {}\n\n\n// removed\n\nfn b() {}\n";
        let expected = "fn a() {}\n\n\nfn b() {}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_blank_below_only_at_block_start_is_preserved() {
        let src = "fn f() {\n    // removed\n\n    let x = 1;\n}\n";
        let expected = "fn f() {\n\n    let x = 1;\n}\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_comment_only_file_is_empty() {
        let src = "// only a comment\n";
        let expected = "";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_whitespace_only_file_unchanged() {
        let src = "   \n\t\n  \n";
        assert_eq!(strip_line_comments(src), src);
    }
    #[test]
    fn strip_line_comments_inline_drop_preserves_blank_line_below() {
        let src = "let x = 1; // tail\n\nlet y = 2;\n";
        let expected = "let x = 1;\n\nlet y = 2;\n";
        assert_eq!(strip_line_comments(src), expected);
    }
    #[test]
    fn strip_line_comments_with_counts_zero_on_clean_source() {
        let (out, counts) = super::strip_line_comments_with_counts("fn f() {}\n");
        assert_eq!(out, "fn f() {}\n");
        assert_eq!(counts, super::RewriteCounts::default());
    }
    #[test]
    fn strip_line_comments_with_counts_counts_solo_line_drops() {
        let (_, counts) = super::strip_line_comments_with_counts("// a\n// b\nfn f() {}\n// c\n");
        assert_eq!(counts.comments_removed, 3);
        assert_eq!(counts.inline_trimmed, 0);
    }
    #[test]
    fn strip_line_comments_with_counts_counts_inline_trim() {
        let (_, counts) =
            super::strip_line_comments_with_counts("let x = 1; // a\nlet y = 2; /* b */\n");
        assert_eq!(counts.comments_removed, 2);
        assert_eq!(counts.inline_trimmed, 2);
    }
    #[test]
    fn strip_line_comments_with_counts_counts_blank_lines_collapsed() {
        let (_, counts) =
            super::strip_line_comments_with_counts("fn a() {}\n\n// removed\n\nfn b() {}\n");
        assert_eq!(counts.comments_removed, 1);
        assert_eq!(counts.blank_lines_collapsed, 1);
    }
    #[test]
    fn strip_line_comments_with_counts_does_not_count_blank_collapse_for_asymmetric() {
        let (_, counts) =
            super::strip_line_comments_with_counts("fn a() {}\n\n// removed\nfn b() {}\n");
        assert_eq!(counts.comments_removed, 1);
        assert_eq!(counts.blank_lines_collapsed, 0);
    }
    #[test]
    fn strip_line_comments_with_counts_counts_safety_line_as_removed() {
        let (_, counts) =
            super::strip_line_comments_with_counts("// SAFETY: invariant\nfn f() {}\n");
        assert_eq!(counts.comments_removed, 1);
    }
    #[test]
    fn strip_line_comments_with_counts_counts_auto_trait_markers_as_removed() {
        let src = "// AUTO-TRAIT-POLICY-BEGIN\nfn f() {}\n// AUTO-TRAIT-POLICY-END\n";
        let (_, counts) = super::strip_line_comments_with_counts(src);
        assert_eq!(counts.comments_removed, 2);
    }
    #[test]
    fn strip_line_comments_thin_wrapper_matches_full() {
        let src = "// a\nfn f() {}\n";
        let direct = super::strip_line_comments(src);
        let (with_counts, _) = super::strip_line_comments_with_counts(src);
        assert_eq!(direct, with_counts);
    }
}
#[cfg(test)]
mod doc_lint_tests {
    use super::{DocBudget, doc_lint_file};
    fn file(src: &str) -> syn::File {
        syn::parse_file(src).expect("doc-lint fixture must parse as Rust source")
    }
    fn lint(file: &syn::File, max_words: usize) -> Vec<super::DocFinding> {
        doc_lint_file(file, DocBudget { max_words })
            .findings()
            .to_vec()
    }
    fn report(file: &syn::File, max_words: usize) -> super::DocLintReport {
        doc_lint_file(file, DocBudget { max_words })
    }
    fn cfg_bounds(u: &super::DocUndecided) -> (super::WordCount, super::WordCount) {
        match u.cause() {
            super::UndecidedCause::ConfigurationDependent {
                unconditional,
                all_configurations,
            } => (unconditional, all_configurations),
            other => panic!("expected a configuration-dependent cause, got {other:?}"),
        }
    }
    #[test]
    fn no_docs_yields_no_findings() {
        let f = file(
            r"
pub fn foo() {}
        ",
        );
        assert!(lint(&f, 40).is_empty());
    }
    #[test]
    fn short_doc_under_budget_yields_no_findings() {
        let f = file(
            r#"
#[doc = " one two three four five"] pub fn foo() {}
        "#,
        );
        assert!(lint(&f, 40).is_empty());
    }
    #[test]
    fn long_doc_over_budget_yields_one_finding() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
" w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
" w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
" w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
" w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].words.count(), 50);
        assert_eq!(findings[0].budget, 40);
        assert_eq!(findings[0].item_label, "fn foo");
    }
    #[test]
    fn fenced_code_excluded_brings_under_budget() {
        let f = file(
            r#"
#[doc = " p01 p02 p03 p04 p05 p06 p07 p08 p09 p10"] #[doc = " ```"] #[doc =
" c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
" c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
" c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
" c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
" c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] #[doc = " ```"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 40);
        assert!(findings.is_empty(), "{findings:?}");
    }
    #[test]
    fn multi_attr_docs_concatenate() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05"] #[doc = " w06 w07 w08 w09 w10"] #[doc =
"w11 w12 w13 w14 w15"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 10);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].words.count(), 15);
    }
    #[test]
    fn cfg_attr_doc_payload_counted_towards_the_undecided_upper_bound() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05"] #[cfg_attr(test, doc =
"w06 w07 w08 w09 w10")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 7);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1);
        assert_eq!(cfg_bounds(&r.undecided()[0]).0.count(), 5);
        assert_eq!(cfg_bounds(&r.undecided()[0]).1.count(), 10);
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::ConfigurationDependent
        );
    }
    #[test]
    fn mutually_exclusive_cfg_docs_are_not_summed_into_a_finding() {
        let f = file(
            r#"
#[cfg_attr(unix, doc = " w01 w02 w03 w04 w05 w06")] #[cfg_attr(windows, doc =
" w07 w08 w09 w10 w11 w12")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(cfg_bounds(&r.undecided()[0]).0.count(), 0);
        assert_eq!(cfg_bounds(&r.undecided()[0]).1.count(), 12);
        assert_eq!(r.undecided()[0].budget(), 8);
    }
    #[test]
    fn unconditional_docs_over_budget_stay_a_finding_beside_cfg_docs() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[cfg_attr(unix, doc =
" w11 w12 w13")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1);
        assert_eq!(
            r.findings()[0].words().count(),
            10,
            "a finding must report the count every configuration carries"
        );
    }
    #[test]
    fn cfg_docs_within_the_aggregate_budget_are_undecided_not_clean() {
        let f = file(
            r#"
#[doc = " w01 w02"] #[cfg_attr(unix, doc = " w03 w04")] #[cfg_attr(windows, doc =
" w05 w06")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 6);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(
            r.undecided().len(),
            1,
            "the all-payload aggregate is not an attainable configuration: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn a_conditional_fence_hiding_unconditional_prose_is_undecided() {
        let f = file(
            r#"
#[cfg_attr(unix, doc = " ```")] #[doc =
" w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[cfg_attr(unix, doc = " ```")]
pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(
            r.undecided().len(),
            1,
            "a conditional fence leaves the unconditional prose undecided: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn nested_cfg_attr_doc_payload_is_not_invisible() {
        let f = file(
            r#"
#[cfg_attr(unix, cfg_attr(windows, doc =
" w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"))] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(
            r.undecided().len(),
            1,
            "a nested cfg_attr doc payload must not vanish: {:?}",
            r.undecided()
        );
        assert_eq!(cfg_bounds(&r.undecided()[0]).1.count(), 10);
    }
    #[test]
    fn a_trivially_true_cfg_attr_doc_is_a_finding() {
        let f = file(
            r#"
#[cfg_attr(all(), doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")]
pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].words().count(), 10);
    }
    #[test]
    fn a_nested_trivially_true_cfg_attr_doc_is_a_finding() {
        let f = file(
            r#"
#[cfg_attr(all(), cfg_attr(all(), doc =
" w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"))] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].words().count(), 10);
    }
    #[test]
    fn a_trivially_false_cfg_attr_doc_carries_no_words() {
        let f = file(
            r#"
#[cfg_attr(any(), doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")]
pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
    }
    #[test]
    fn boolean_constant_predicates_fold_through_not_and_nesting() {
        let f = file(
            r#"
#[cfg_attr(not(any()), doc = " w01 w02 w03 w04 w05")] #[cfg_attr(any(all(), unix),
doc = " w06 w07 w08 w09 w10")] #[cfg_attr(all(any(), unix), doc =
" x01 x02 x03 x04 x05 x06 x07 x08 x09 x10")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].words().count(), 10);
    }
    #[test]
    fn a_literal_true_cfg_attr_doc_is_a_finding() {
        let f = file(
            r#"
#[cfg_attr(true, doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")]
pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].words().count(), 10);
    }
    #[test]
    fn a_literal_false_cfg_attr_doc_carries_no_words() {
        let f = file(
            r#"
#[cfg_attr(false, doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")]
pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
    }
    #[test]
    fn literal_boolean_predicates_fold_through_all_any_and_not() {
        let f = file(
            r#"
#[cfg_attr(all(true), doc = " w01 w02 w03 w04 w05")] #[cfg_attr(any(true, unix),
doc = " w06 w07 w08 w09 w10")] #[cfg_attr(not(false), cfg_attr(true, doc =
" x01 x02 x03 x04 x05"))] #[cfg_attr(any(false), doc =
" y01 y02 y03 y04 y05 y06 y07 y08 y09 y10")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].words().count(), 15);
    }
    #[test]
    fn a_file_level_trivially_true_cfg_attr_doc_is_a_finding() {
        let f = file(
            r#"
#![cfg_attr(all(), doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")]
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].item_label(), "file-level");
    }
    #[test]
    fn a_fence_split_across_a_cfg_boundary_is_undecided_not_a_finding() {
        let f = file(
            r#"
#[doc = " ```"] #[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[cfg_attr(unix,
doc = " ```")] #[doc = " w11 w12 w13 w14 w15"] pub fn foo() {}
        "#,
        );
        let r = report(&f, 3);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert!(cfg_bounds(&r.undecided()[0]).0.is_fail_closed());
    }
    #[test]
    fn a_concat_doc_expression_is_undecided_not_clean() {
        let f = file(r#"#[doc = concat!(" w01", " w02 w03 w04 w05")] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::UnreadableDocPayload
        );
        assert_eq!(r.undecided()[0].item_label(), "fn foo");
    }
    #[test]
    fn an_include_str_doc_expression_is_undecided_not_clean() {
        let f = file(r#"#[doc = include_str!("prose.md")] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::UnreadableDocPayload
        );
    }
    #[test]
    fn a_file_level_unreadable_doc_expression_is_undecided_not_clean() {
        let f = file(r#"#![doc = include_str!("README.md")]"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(r.undecided()[0].item_label(), "file-level");
    }
    #[test]
    fn an_unreadable_cfg_attr_doc_expression_is_undecided_not_clean() {
        let f = file(r#"#[cfg_attr(all(), doc = concat!(" a b c"))] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::UnreadableDocPayload
        );
    }
    #[test]
    fn a_raw_spelled_doc_attribute_is_the_same_path_as_doc() {
        let f = file(r#"#[r#doc = concat!(" a b c")] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::UnreadableDocPayload
        );
    }
    #[test]
    fn a_raw_spelled_doc_attribute_in_a_macro_body_is_the_same_path_as_doc() {
        let f = file(r#"pub fn f() { generate! { #[r#doc = " a b c"] fn g() {} } }"#);
        let r = report(&f, 1);
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::UninspectedMacroBody
        );
    }
    #[test]
    fn a_raw_spelled_cfg_attr_is_the_same_path_as_cfg_attr() {
        let f = file(r#"#[r#cfg_attr(all(), doc = concat!(" a b c"))] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::UnreadableDocPayload
        );
    }
    #[test]
    fn raw_spelled_cfg_predicate_operators_fold_like_their_plain_spellings() {
        for src in [
            r#"#[cfg_attr(r#all(), doc = concat!(" a b c"))] pub fn foo() {}"#,
            r#"#[cfg_attr(r#not(r#any()), doc = concat!(" a b c"))] pub fn foo() {}"#,
        ] {
            let f = file(src);
            let r = report(&f, 1);
            assert_eq!(r.undecided().len(), 1, "{src}: {:?}", r.undecided());
            assert_eq!(
                r.undecided()[0].outcome(),
                super::DocLintOutcome::UnreadableDocPayload,
                "{src}"
            );
        }
    }
    #[test]
    fn an_unresolved_cfg_attr_carrying_an_unreadable_doc_expression_is_unreadable() {
        let f = file(r#"#[cfg_attr(unix, doc = concat!(" a b c"))] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::UnreadableDocPayload,
            "an unreadable payload is the stronger indeterminate: no word bound holds"
        );
    }
    #[test]
    fn a_never_taken_cfg_attr_unreadable_doc_expression_is_dropped() {
        let f = file(r#"#[cfg_attr(any(), doc = concat!(" a b c"))] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(
            r.undecided().is_empty(),
            "a payload present in no configuration is not an indeterminate: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn an_unreadable_payload_beside_overlong_prose_is_undecided_not_a_finding() {
        let f = file(r#"#[doc = " w01 w02 w03 w04 w05"] #[doc = concat!(" x")] pub fn foo() {}"#);
        let r = report(&f, 2);
        assert!(
            r.findings().is_empty(),
            "an unreadable payload may open a code fence, so no word count is provable: {:?}",
            r.findings()
        );
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::UnreadableDocPayload
        );
    }
    #[test]
    fn a_non_string_literal_doc_payload_is_undecided_not_clean() {
        let f = file("#[doc = 1] pub fn foo() {}");
        let r = report(&f, 1);
        assert_eq!(
            r.undecided().len(),
            1,
            "rustc accepts this with a malformed-input warning and it carries no readable prose; \
             reporting it is a conservative over-report, never a false clean: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn a_doc_list_attribute_carries_no_prose_and_is_not_unreadable() {
        let f = file(r#"#[doc(hidden)] #[doc(alias = "bar")] #[doc = " w01"] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(
            r.undecided().is_empty(),
            "`#[doc(...)]` is rustdoc metadata, not doc prose: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn an_overlong_doc_inside_macro_rules_is_undecided_not_silence() {
        let f = file(
            r#"
macro_rules! noisy { () => {
#[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"]
#[doc = " w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"]
pub fn inner() {} }; }
"#,
        );
        let r = report(&f, 5);
        assert!(
            r.findings().is_empty(),
            "the expansion is not performed, so no word count is provable: {:?}",
            r.findings()
        );
        assert_eq!(
            r.undecided().len(),
            1,
            "an uninspected macro body carrying doc attributes is a reported gap, \
             not silence: {:?}",
            r.undecided()
        );
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::UninspectedMacroBody
        );
        assert_eq!(r.undecided()[0].item_label(), "macro noisy");
    }
    #[test]
    fn a_doc_comment_inside_a_macro_invocation_body_is_undecided() {
        let f = file(
            r"
generate! {
    /// w01 w02 w03 w04 w05 w06 w07 w08 w09 w10
    pub fn inner() {}
}
",
        );
        let r = report(&f, 5);
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            super::DocLintOutcome::UninspectedMacroBody
        );
        assert_eq!(r.undecided()[0].item_label(), "macro generate");
    }
    #[test]
    fn a_macro_body_without_doc_attributes_is_clean() {
        let f = file(
            r#"
macro_rules! quiet { ($x:expr) => { $x + 1 }; }
pub fn f() {
    let v = vec![1, 2, 3];
    println!("this text mentions doc and docs but carries no attribute");
    let _ = quiet!(v.len());
}
"#,
        );
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(
            r.undecided().is_empty(),
            "a macro body with no doc attribute carries no doc payload: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn a_doc_bearing_macro_body_is_reported_in_every_position() {
        let positions = [
            (
                "statement",
                "pub fn f() { generate! { #[doc = \" a b c\"] fn g() {} } }",
            ),
            (
                "expression",
                "pub fn f() -> u32 { generate!(#[doc = \" a b c\"] fn g() {}) }",
            ),
            (
                "impl item",
                "pub struct S; impl S { generate! { #[doc = \" a b c\"] fn g() {} } }",
            ),
            (
                "trait item",
                "pub trait T { generate! { #[doc = \" a b c\"] fn g() {} } }",
            ),
            (
                "extern block",
                "unsafe extern \"C\" { generate! { #[doc = \" a b c\"] fn g(); } }",
            ),
            (
                "type position",
                "pub type A = generate!(#[doc = \" a b c\"] fn g() {});",
            ),
            (
                "pattern position",
                "pub fn f() { let generate!(#[doc = \" a b c\"] x) = 1; }",
            ),
            ("inner doc comment", "generate! { //! w01 w02 w03\n }"),
        ];
        for (position, src) in positions {
            let r = report(&file(src), 1);
            assert_eq!(
                r.undecided().len(),
                1,
                "a doc-bearing macro body in {position} position must be reported: {:?}",
                r.undecided()
            );
            assert_eq!(
                r.undecided()[0].outcome(),
                super::DocLintOutcome::UninspectedMacroBody,
                "{position}"
            );
        }
    }
    #[test]
    fn a_nested_doc_bearing_macro_body_is_reported_at_the_outermost_body() {
        let f = file(r#"outer! { inner! { #[doc = " a b c"] fn g() {} } }"#);
        let r = report(&f, 1);
        assert_eq!(
            r.undecided().len(),
            1,
            "the inner invocation is tokens inside the outer opaque body, not a second \
             item: one indeterminate covers the whole body: {:?}",
            r.undecided()
        );
        assert_eq!(r.undecided()[0].item_label(), "macro outer");
    }
    #[test]
    fn a_doc_list_attribute_inside_a_macro_body_carries_no_prose() {
        let f = file(r"generate! { #[doc(hidden)] fn g() {} }");
        let r = report(&f, 1);
        assert!(
            r.undecided().is_empty(),
            "`#[doc(...)]` is rustdoc metadata, not doc prose: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn a_cfg_attr_doc_inside_a_macro_body_is_reported() {
        let f = file(r#"generate! { #[cfg_attr(unix, doc = " a b c")] fn g() {} }"#);
        let r = report(&f, 1);
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
    }
    #[test]
    fn field_and_variant_docs_linted_independently() {
        let f = file(
            r#"
pub struct S { #[doc = " w01 w02 w03 w04 w05"] pub a : u32, #[doc =
" w01 w02 w03 w04 w05 w06"] pub b : u32, }
        "#,
        );
        let findings = lint(&f, 3);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings.iter().all(|f| f.item_label.starts_with("field ")));
        let f = file(
            r#"
pub enum E { #[doc = " w01 w02 w03 w04 w05"] One, #[doc =
" w01 w02 w03 w04 w05 w06"] Two, }
        "#,
        );
        let findings = lint(&f, 3);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(
            findings
                .iter()
                .all(|f| f.item_label.starts_with("variant "))
        );
    }
    #[test]
    fn closing_fence_returns_to_prose() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05"] #[doc = " ```"] #[doc = " c01 c02 c03"] #[doc
= " ```"] #[doc = " w06 w07 w08 w09 w10 w11"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 10);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].words.count(), 11);
    }
    #[test]
    fn equal_to_budget_does_not_trigger() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05"] pub fn foo() {}
        "#,
        );
        assert!(lint(&f, 5).is_empty());
    }
    #[test]
    fn tilde_fence_excludes_code() {
        let f = file(
            r#"
#[doc = " p01 p02 p03 p04 p05 p06 p07 p08 p09 p10"] #[doc = " ~~~"] #[doc =
" c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
" c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
" c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
" c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
" c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] #[doc = " ~~~"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 40);
        assert!(findings.is_empty(), "{findings:?}");
    }
    #[test]
    fn unclosed_fence_fails_closed() {
        let f = file(
            r#"
#[doc = " p01 p02 p03 p04 p05"] #[doc = " ```"] #[doc =
" c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
" c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
" c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
" c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
" c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].words.count(), 56);
        assert!(
            findings[0].words.is_fail_closed(),
            "unbalanced fence must set fail_closed=true: {:?}",
            findings[0]
        );
    }
    #[test]
    fn over_budget_doc_on_pub_use_is_linted() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
" w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
" w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
" w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
" w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] pub use crate ::foo::Bar;
        "#,
        );
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].item_label, "use");
        assert_eq!(findings[0].words.count(), 50);
    }
    #[test]
    fn over_budget_doc_on_extern_crate_is_linted() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
" w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
" w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
" w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
" w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] extern crate alloc;
        "#,
        );
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].item_label, "extern crate alloc");
    }
}
/// Rewrite mechanically-safe Rust item links in `doc_text`.
///
/// Operates on the prose of a single doc-comment block (concatenated
/// payloads of one item, joined by `\n`). Block structure is resolved
/// before inline structure, as `CommonMark` requires:
///
/// ```text
/// - fenced code: 0-3 columns of indent, closed only by the same marker
///   with a run at least as long as the opener
/// - indented code: 4+ columns at a block start, tab counting to column 4
/// - reference definitions: recognised only at a block start
/// - inline code spans: tracked only within the remaining prose lines; a
///   run with no matching closing run inside its block is literal text
/// ```
///
/// Rules applied only when the label is a conservative Rust item
/// token:
///
/// ```text
/// [Type](Type)             -> [`Type`]              (redundant target collapsed)
/// [Type]                   -> [`Type`]              (shortcut form gets ticks)
/// [label](Target)          -> [`label`](Target)    (label ticked; target kept)
/// ```
///
/// Skipped (left verbatim):
///
/// ```text
/// - lines inside fenced code blocks and indented code blocks
/// - spans inside inline code, for any backtick run length
/// - URL targets (contain ://, or start with /, #, mailto:)
/// - reference definitions ([label]: <url>) and reference links ([label][ref])
/// - shortcut references whose label is defined anywhere in the block
/// - labels opened by an escaped bracket (\[label])
/// - targets with generics, disambiguators, or fragments (< > @ # ( ) ! ?)
/// - labels already wrapped in backticks (idempotent)
/// - prose labels — anything not matching is_codeish_path
/// - empty link bodies
/// ```
#[must_use]
pub fn rewrite_rustdoc_link_idioms(doc_text: &str) -> String {
    let labels = ReferenceLabels::index(doc_text);
    let lines: Vec<&str> = doc_text.split('\n').collect();
    let mut out = String::with_capacity(doc_text.len());
    let mut blocks = BlockScan::new();
    let mut span = CodeSpanState::Closed;
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let following = &lines[index + 1..];
        match blocks.classify(line) {
            LineFrame::Code | LineFrame::Definition => {
                span = CodeSpanState::Closed;
                out.push_str(line);
            }
            LineFrame::Prose => {
                span = rewrite_line_links(line, following, &labels, span, &mut out);
            }
        }
    }
    out
}
struct ReferenceLabels(BTreeSet<String>);
impl ReferenceLabels {
    fn index(doc_text: &str) -> Self {
        let lines: Vec<&str> = doc_text.split('\n').collect();
        let mut labels = BTreeSet::new();
        let mut blocks = BlockScan::new();
        let mut span = CodeSpanState::Closed;
        for (index, line) in lines.iter().enumerate() {
            let following = &lines[index + 1..];
            match blocks.classify(line) {
                LineFrame::Code => span = CodeSpanState::Closed,
                LineFrame::Prose => span = advance_span(line, following, span),
                LineFrame::Definition => {
                    span = CodeSpanState::Closed;
                    if let Some(label) = reference_definition_label(line) {
                        let normalised = normalise_link_label(label);
                        if !normalised.is_empty() {
                            labels.insert(normalised);
                        }
                    }
                }
            }
        }
        Self(labels)
    }
    fn defines(&self, label_src: &str) -> bool {
        self.0.contains(&normalise_link_label(label_src))
    }
}
fn normalise_link_label(label_src: &str) -> String {
    let mut normalised = String::with_capacity(label_src.len());
    let mut gap = false;
    for ch in label_src.trim().chars() {
        if ch.is_whitespace() {
            gap = true;
            continue;
        }
        if gap && !normalised.is_empty() {
            normalised.push(' ');
        }
        gap = false;
        normalised.extend(ch.to_lowercase());
    }
    normalised
}
fn reference_definition_label(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('[') {
        return None;
    }
    let close = find_matching_bracket(trimmed, 0)?;
    if !trimmed[close + 1..].starts_with(':') {
        return None;
    }
    Some(&trimmed[1..close])
}
fn is_reference_definition(line: &str) -> bool {
    reference_definition_label(line).is_some()
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodeSpanState {
    Closed,
    Open { run: usize },
}
impl CodeSpanState {
    const fn is_open(self) -> bool {
        matches!(self, Self::Open { .. })
    }
}
fn span_after_run(
    span: CodeSpanState,
    run: usize,
    rest: &str,
    following: &[&str],
) -> CodeSpanState {
    match span {
        CodeSpanState::Open { run: open } if open == run => CodeSpanState::Closed,
        open @ CodeSpanState::Open { .. } => open,
        CodeSpanState::Closed
            if line_has_backtick_run(rest, run) || closing_run_follows(following, run) =>
        {
            CodeSpanState::Open { run }
        }
        CodeSpanState::Closed => CodeSpanState::Closed,
    }
}
fn advance_span(line: &str, following: &[&str], entry: CodeSpanState) -> CodeSpanState {
    let bytes = line.as_bytes();
    let mut span = entry;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = backtick_run_len(bytes, i);
            span = span_after_run(span, run, &line[i + run..], following);
            i += run;
            continue;
        }
        i += 1;
    }
    span
}
fn closing_run_follows(following: &[&str], run: usize) -> bool {
    for line in following {
        if line.trim().is_empty() || FenceState::Closed.advance(line).1 {
            return false;
        }
        if line_has_backtick_run(line, run) {
            return true;
        }
    }
    false
}
const fn line_has_backtick_run(line: &str, run: usize) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let found = backtick_run_len(bytes, i);
            if found == run {
                return true;
            }
            i += found;
            continue;
        }
        i += 1;
    }
    false
}
const fn backtick_run_len(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end] == b'`' {
        end += 1;
    }
    end - start
}
fn rewrite_line_links(
    line: &str,
    following: &[&str],
    labels: &ReferenceLabels,
    entry: CodeSpanState,
    out: &mut String,
) -> CodeSpanState {
    let bytes = line.as_bytes();
    let mut span = entry;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = backtick_run_len(bytes, i);
            span = span_after_run(span, run, &line[i + run..], following);
            out.push_str(&line[i..i + run]);
            i += run;
            continue;
        }
        if !span.is_open() && bytes[i] == b'\\' {
            let escaped = escape_pair_len(line, i);
            out.push_str(&line[i..i + escaped]);
            i += escaped;
            continue;
        }
        if span.is_open() || bytes[i] != b'[' {
            let step = char_len_at(line, i);
            out.push_str(&line[i..i + step]);
            i += step;
            continue;
        }
        if let Some((shape, consumed)) = parse_link_at(line, i) {
            emit_link(out, &shape, labels);
            i += consumed;
        } else {
            out.push('[');
            i += 1;
        }
    }
    span
}
fn escape_pair_len(line: &str, start: usize) -> usize {
    let mut chars = line[start..].chars();
    let backslash = chars.next().map_or(0, char::len_utf8);
    backslash + chars.next().map_or(0, char::len_utf8)
}
fn char_len_at(line: &str, start: usize) -> usize {
    line[start..].chars().next().map_or(1, char::len_utf8)
}
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkShape {
    Inline {
        label_src: String,
        target_src: String,
    },
    Reference {
        raw: String,
    },
    Shortcut {
        label_src: String,
    },
}
fn parse_link_at(line: &str, start: usize) -> Option<(LinkShape, usize)> {
    let bytes = line.as_bytes();
    let label_end = find_matching_bracket(line, start)?;
    let label_src = line[start + 1..label_end].to_string();
    let after_label = label_end + 1;
    if after_label < bytes.len() && bytes[after_label] == b'(' {
        let paren_end = find_matching_paren(line, after_label)?;
        let target_src = line[after_label + 1..paren_end].to_string();
        return Some((
            LinkShape::Inline {
                label_src,
                target_src,
            },
            paren_end + 1 - start,
        ));
    }
    if after_label < bytes.len() && bytes[after_label] == b'[' {
        let ref_end = find_matching_bracket(line, after_label)?;
        let raw = line[start..=ref_end].to_string();
        return Some((LinkShape::Reference { raw }, ref_end + 1 - start));
    }
    Some((LinkShape::Shortcut { label_src }, after_label - start))
}
const fn find_matching_bracket(line: &str, open: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b']' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}
const fn find_matching_paren(line: &str, open: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b')' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}
fn emit_link(out: &mut String, shape: &LinkShape, labels: &ReferenceLabels) {
    match shape {
        LinkShape::Reference { raw } => {
            out.push_str(raw);
        }
        LinkShape::Inline {
            label_src,
            target_src,
        } => emit_inline_link(out, label_src, target_src),
        LinkShape::Shortcut { label_src } => emit_shortcut_link(out, label_src, labels),
    }
}
fn emit_inline_link(out: &mut String, label_src: &str, target_src: &str) {
    let target_trim = target_src.trim();
    if target_trim.is_empty() || !is_safe_intra_doc_target(target_trim) {
        write_inline(out, label_src, target_src);
        return;
    }
    if label_src == target_trim && is_codeish_path(label_src) {
        write_shortcut_ticked(out, label_src);
        return;
    }
    if is_codeish_path(label_src) && !label_src_has_backticks(label_src) {
        write_inline_label_ticked(out, label_src, target_src);
        return;
    }
    write_inline(out, label_src, target_src);
}
fn emit_shortcut_link(out: &mut String, label_src: &str, labels: &ReferenceLabels) {
    if is_codeish_path(label_src)
        && !label_src_has_backticks(label_src)
        && !labels.defines(label_src)
    {
        write_shortcut_ticked(out, label_src);
    } else {
        out.push('[');
        out.push_str(label_src);
        out.push(']');
    }
}
fn write_shortcut_ticked(out: &mut String, label: &str) {
    out.push('[');
    out.push('`');
    out.push_str(label);
    out.push('`');
    out.push(']');
}
fn write_inline_label_ticked(out: &mut String, label: &str, target: &str) {
    out.push('[');
    out.push('`');
    out.push_str(label);
    out.push('`');
    out.push(']');
    out.push('(');
    out.push_str(target);
    out.push(')');
}
fn write_inline(out: &mut String, label: &str, target: &str) {
    out.push('[');
    out.push_str(label);
    out.push(']');
    out.push('(');
    out.push_str(target);
    out.push(')');
}
fn label_src_has_backticks(label: &str) -> bool {
    label.contains('`')
}
fn is_safe_intra_doc_target(target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    if target.contains("://")
        || target.starts_with('#')
        || target.starts_with('/')
        || target.starts_with("mailto:")
    {
        return false;
    }
    for ch in target.chars() {
        match ch {
            '<' | '>' | '@' | '#' | '(' | ')' | '!' | '?' | ' ' | '\t' => return false,
            _ => {}
        }
    }
    is_codeish_path(target)
}
fn is_codeish_path(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let trimmed = s.strip_prefix("::").unwrap_or(s);
    if trimmed.is_empty() {
        return false;
    }
    for segment in trimmed.split("::") {
        if !is_rust_ident(segment) {
            return false;
        }
    }
    true
}
fn is_rust_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    if s.len() == 1 && first == '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
#[cfg(test)]
mod rustdoc_link_idiom_tests {
    use super::{is_codeish_path, rewrite_rustdoc_link_idioms};
    #[test]
    fn multibyte_utf8_survives_rewrite_pure() {
        let input = "see [Type] — also русский and 🦀";
        let out = super::rewrite_rustdoc_link_idioms(input);
        assert_eq!(out, "see [`Type`] — also русский and 🦀");
    }
    #[test]
    fn em_dash_survives_a_quoted_attr_doc_splice() {
        let original = "#[doc = \" see [Type] — русский 🦀\"]\npub fn f() {}\n";
        let ast: syn::File = syn::parse_file(original).unwrap();
        let splices = super::collect_doc_splices(&ast, original);
        let rewritten = super::apply_splices(original, splices);
        assert_eq!(
            rewritten, "#[doc = \" see [`Type`] — русский 🦀\"]\npub fn f() {}\n",
            "non-ASCII payload mangled by the quoted-attr splice: {rewritten:?}"
        );
    }
    fn rw(s: &str) -> String {
        rewrite_rustdoc_link_idioms(s)
    }
    #[test]
    fn redundant_explicit_link_collapses_and_ticks() {
        assert_eq!(rw("see [Type](Type) for"), "see [`Type`] for");
    }
    #[test]
    fn redundant_explicit_path_link_collapses_and_ticks() {
        assert_eq!(rw("via [foo::Bar](foo::Bar) here"), "via [`foo::Bar`] here");
    }
    #[test]
    fn explicit_target_retained_label_ticked() {
        assert_eq!(
            rw("call [begin](Self::begin) first"),
            "call [`begin`](Self::begin) first"
        );
        assert_eq!(
            rw("see [Reader](crate::Reader) docs"),
            "see [`Reader`](crate::Reader) docs"
        );
    }
    #[test]
    fn shortcut_camel_case_ticked() {
        assert_eq!(rw("the [Type] applies"), "the [`Type`] applies");
    }
    #[test]
    fn shortcut_path_ticked() {
        assert_eq!(rw("see [foo::Bar] above"), "see [`foo::Bar`] above");
    }
    #[test]
    fn shortcut_self_super_crate_ticked() {
        assert_eq!(rw("the [Self] of"), "the [`Self`] of");
        assert_eq!(rw("from [super::Foo]"), "from [`super::Foo`]");
        assert_eq!(rw("via [crate::Reader]"), "via [`crate::Reader`]");
    }
    #[test]
    fn shortcut_snake_case_ticked() {
        assert_eq!(rw("call [do_thing] next"), "call [`do_thing`] next");
    }
    #[test]
    fn prose_label_not_rewritten() {
        assert_eq!(
            rw("see [the writer](Writer) for"),
            "see [the writer](Writer) for"
        );
    }
    #[test]
    fn url_link_not_rewritten() {
        assert_eq!(
            rw("see [docs](https://example.com)"),
            "see [docs](https://example.com)"
        );
        assert_eq!(rw("the [home](/index.html)"), "the [home](/index.html)");
        assert_eq!(
            rw("mail [admin](mailto:a@example.com)"),
            "mail [admin](mailto:a@example.com)"
        );
    }
    #[test]
    fn fragment_target_not_rewritten() {
        assert_eq!(rw("see [Foo](#anchor)"), "see [Foo](#anchor)");
    }
    #[test]
    fn target_with_generics_not_rewritten() {
        assert_eq!(rw("see [Vec](Vec<u8>) usage"), "see [Vec](Vec<u8>) usage");
    }
    #[test]
    fn target_with_disambiguator_not_rewritten() {
        assert_eq!(rw("call [foo](foo()) here"), "call [foo](foo()) here");
        assert_eq!(rw("call [m](m!) macro"), "call [m](m!) macro");
        assert_eq!(
            rw("see [t](struct@Type) struct"),
            "see [t](struct@Type) struct"
        );
    }
    #[test]
    fn reference_style_link_not_rewritten() {
        assert_eq!(rw("see [Type][ref] later"), "see [Type][ref] later");
    }
    #[test]
    fn reference_definition_not_rewritten() {
        assert_eq!(
            rw("[ref]: https://example.com"),
            "[ref]: https://example.com"
        );
    }
    #[test]
    fn fenced_code_block_left_verbatim() {
        let input = "before\n```\nlet x: [Type] = foo();\n[Type](Type)\n```\nafter [Type]";
        let expected = "before\n```\nlet x: [Type] = foo();\n[Type](Type)\n```\nafter [`Type`]";
        assert_eq!(rw(input), expected);
    }
    #[test]
    fn tilde_fenced_code_block_left_verbatim() {
        let input = "before\n~~~\n[Type](Type)\n~~~\nafter [Type]";
        let expected = "before\n~~~\n[Type](Type)\n~~~\nafter [`Type`]";
        assert_eq!(rw(input), expected);
    }
    #[test]
    fn inline_code_span_left_verbatim() {
        assert_eq!(
            rw("use `[Type]` syntax for [Type]"),
            "use `[Type]` syntax for [`Type`]"
        );
    }
    #[test]
    fn already_ticked_shortcut_left_verbatim() {
        assert_eq!(rw("see [`Type`] above"), "see [`Type`] above");
    }
    #[test]
    fn already_ticked_inline_left_verbatim() {
        assert_eq!(
            rw("call [`foo`](Self::foo) now"),
            "call [`foo`](Self::foo) now"
        );
    }
    #[test]
    fn empty_link_body_not_rewritten() {
        assert_eq!(rw("an [] empty"), "an [] empty");
    }
    #[test]
    fn empty_target_not_rewritten() {
        assert_eq!(rw("a [Type]() blank"), "a [Type]() blank");
    }
    #[test]
    fn is_codeish_path_basic() {
        assert!(is_codeish_path("Type"));
        assert!(is_codeish_path("foo_bar"));
        assert!(is_codeish_path("foo::Bar"));
        assert!(is_codeish_path("Self"));
        assert!(is_codeish_path("self"));
        assert!(is_codeish_path("super::Foo"));
        assert!(is_codeish_path("crate::Reader"));
        assert!(is_codeish_path("::foo::Bar"));
        assert!(!is_codeish_path(""));
        assert!(!is_codeish_path("two words"));
        assert!(!is_codeish_path("foo()"));
        assert!(!is_codeish_path("Vec<u8>"));
        assert!(!is_codeish_path("foo!"));
        assert!(!is_codeish_path("_"));
        assert!(!is_codeish_path("9bad"));
        assert!(!is_codeish_path("foo:bar"));
        assert!(!is_codeish_path("foo::"));
    }
    #[test]
    fn idempotent_rewrite() {
        let inputs = [
            "see [Type](Type)",
            "call [begin](Self::begin)",
            "the [Type]",
            "see `[Type]` literal",
            "in a fence\n```\n[Type]\n```\nout",
            "see [the writer](Writer)",
        ];
        for input in inputs {
            let once = rw(input);
            let twice = rw(&once);
            assert_eq!(once, twice, "non-idempotent for: {input}");
        }
    }
    #[test]
    fn multiline_fence_state_persists() {
        let input = "p1\n```\n[A](A)\n```\np2 [B]\n```\n[C]\n```\np3 [D](D)";
        let expected = "p1\n```\n[A](A)\n```\np2 [`B`]\n```\n[C]\n```\np3 [`D`]";
        assert_eq!(rw(input), expected);
    }
    #[test]
    fn multiple_links_on_one_line() {
        assert_eq!(
            rw("see [A] and [B](B) then [C](Self::C)"),
            "see [`A`] and [`B`] then [`C`](Self::C)"
        );
    }
    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(rw(""), "");
    }
    #[test]
    fn no_links_passthrough() {
        let s = "plain prose with no links\nand a second line\n";
        assert_eq!(rw(s), s);
    }
}
#[cfg(test)]
mod doc_path_tests {
    use super::is_doc_path;
    use std::path::Path;
    #[test]
    fn bare_doc_stem_requires_exact_match() {
        let root = Path::new("");
        assert!(is_doc_path(Path::new("README"), root));
        assert!(is_doc_path(Path::new("README.md"), root));
        assert!(is_doc_path(Path::new("LICENSE"), root));
        assert!(!is_doc_path(Path::new("READMEISH"), root));
        assert!(!is_doc_path(Path::new("READMEISH.rs"), root));
        assert!(!is_doc_path(Path::new("LICENSEABLE.rs"), root));
        assert!(!is_doc_path(Path::new("NOTICED.rs"), root));
    }
    #[test]
    fn docs_dir_matches_only_at_top_level_under_root() {
        let root = Path::new("/proj");
        assert!(is_doc_path(Path::new("/proj/docs/guide.rs"), root));
        assert!(is_doc_path(Path::new("/proj/doc/inner.rs"), root));
        assert!(!is_doc_path(Path::new("/proj/src/docs/mod.rs"), root));
        assert!(!is_doc_path(
            Path::new("/proj/crates/foo/doc/inner.rs"),
            root
        ));
        assert!(!is_doc_path(Path::new("/proj/src/doc/util.rs"), root));
    }
    #[test]
    fn bare_doc_stem_does_not_match_a_non_doc_extension() {
        let root = Path::new("");
        assert!(is_doc_path(Path::new("README"), root));
        assert!(is_doc_path(Path::new("LICENSE"), root));
        assert!(is_doc_path(Path::new("NOTICE"), root));
        assert!(!is_doc_path(Path::new("README.rs"), root));
        assert!(!is_doc_path(Path::new("LICENSE.rs"), root));
        assert!(!is_doc_path(Path::new("NOTICE.toml"), root));
        assert!(!is_doc_path(Path::new("COPYING.rs"), root));
        assert!(!is_doc_path(Path::new("CHANGELOG.rs"), root));
        assert!(is_doc_path(Path::new("README.md"), root));
        assert!(is_doc_path(Path::new("CHANGELOG.markdown"), root));
    }
}
#[cfg(test)]
mod record_tests {
    use super::{
        DOC_LINT_RECORD_VERSION, DocLintKind, REWRITE_RECORD_VERSION, RewriteCounts, RewriteMode,
        doc_lint_finding_record, doc_lint_header_record, doc_lint_hint_record,
        doc_lint_truncated_record, rewrite_summary_record, unified_diff,
    };
    use std::path::Path;
    fn hint(path: &str, item: &str) -> String {
        doc_lint_hint_record(DocLintKind::OverlongDoc, Path::new(path), 12, item, 100, 80)
    }
    #[test]
    fn hint_record_carries_record_name_version_and_outcome() {
        let line = hint("src/lib.rs", "fn f");
        assert!(
            line.starts_with("{\"record\":\"doc_lint_hint\",\"v\":2,"),
            "{line}"
        );
        assert!(line.contains("\"outcome\":\"finding\""), "{line}");
        assert!(line.contains("\"kind\":\"overlong_doc\""), "{line}");
        assert!(line.contains("\"path\":\"src/lib.rs\""), "{line}");
        assert!(line.contains("\"line\":12"), "{line}");
        assert!(line.contains("\"words\":100"), "{line}");
        assert!(line.contains("\"budget\":80"), "{line}");
    }
    #[test]
    fn diff_header_renders_a_hostile_path_as_a_single_line() {
        let path = Path::new("src/we\nird\u{1}.rs");
        let diff = unified_diff(path, "a\n", "b\n", 1);
        let header: Vec<&str> = diff
            .lines()
            .filter(|l| l.starts_with("--- ") || l.starts_with("+++ "))
            .collect();
        assert_eq!(header.len(), 2, "exactly two header lines: {diff}");
        for line in header {
            assert!(
                line.ends_with("src/we\\nird\\u0001.rs"),
                "the header must escape control characters in place: {line}"
            );
        }
        for line in diff.lines() {
            assert!(
                !line.starts_with('{'),
                "no diff line may be mistaken for a record: {line}"
            );
        }
    }
    #[test]
    fn record_is_a_single_line_even_when_the_path_contains_a_newline() {
        let line = hint("src/we\nird.rs", "fn f");
        assert!(
            !line.contains('\n'),
            "record must not contain a raw newline: {line}"
        );
        assert!(line.contains("src/we\\nird.rs"), "{line}");
    }
    #[test]
    fn record_escapes_tabs_in_paths_and_item_labels() {
        let line = hint("src/a\tb.rs", "fn we\tird");
        assert!(
            !line.contains('\t'),
            "record must not contain a raw tab: {line}"
        );
        assert!(line.contains("src/a\\tb.rs"), "{line}");
        assert!(line.contains("fn we\\tird"), "{line}");
    }
    #[test]
    fn record_escapes_quotes_backslashes_and_control_characters() {
        let line = hint("src/\"q\\b.rs", "fn \u{1}ctl");
        assert!(line.contains("src/\\\"q\\\\b.rs"), "{line}");
        assert!(line.contains("fn \\u0001ctl"), "{line}");
    }
    #[test]
    fn finding_record_carries_fail_closed_as_a_boolean() {
        let base = |fc| {
            doc_lint_finding_record(
                DocLintKind::OverlongDoc,
                Path::new("src/lib.rs"),
                3,
                "fn f",
                9,
                8,
                fc,
            )
        };
        assert!(base(true).contains("\"fail_closed\":true"));
        assert!(base(false).contains("\"fail_closed\":false"));
    }
    #[test]
    fn header_record_carries_the_doctrine_without_a_numeric_example_promise() {
        let line = doc_lint_header_record(DocLintKind::OverlongDoc);
        assert!(line.contains("\"record\":\"doc_lint_header\""), "{line}");
        assert!(
            line.contains("Rust docs must contain a concise summary"),
            "{line}"
        );
        assert!(
            !line.contains("0-3"),
            "the unenforced 0-3 example promise must not appear in machine output: {line}"
        );
    }
    #[test]
    fn truncated_record_carries_the_residual_count() {
        let line = doc_lint_truncated_record(DocLintKind::OverlongDoc, 7);
        assert!(line.contains("\"record\":\"doc_lint_truncated\""), "{line}");
        assert!(line.contains("\"remaining\":7"), "{line}");
    }
    #[test]
    fn rewrite_summary_record_has_no_preservation_counters() {
        let counts = RewriteCounts {
            comments_removed: 3,
            inline_trimmed: 1,
            blank_lines_collapsed: 2,
            doc_links_rewritten: 4,
        };
        let line = rewrite_summary_record(RewriteMode::Write, &counts);
        assert!(
            line.starts_with("{\"record\":\"rewrite_summary\",\"v\":2,"),
            "{line}"
        );
        assert!(line.contains("\"mode\":\"write\""), "{line}");
        assert!(line.contains("\"comments_removed\":3"), "{line}");
        assert!(!line.contains("safety_preserved"), "{line}");
        assert!(!line.contains("auto_trait_preserved"), "{line}");
    }
    #[test]
    fn rewrite_summary_record_names_dry_run_mode() {
        let line = rewrite_summary_record(
            RewriteMode::DryRun { context: 3 },
            &RewriteCounts::default(),
        );
        assert!(line.contains("\"mode\":\"dry-run\""), "{line}");
    }
    #[test]
    fn both_record_versions_are_two() {
        assert_eq!(DOC_LINT_RECORD_VERSION, 2);
        assert_eq!(REWRITE_RECORD_VERSION, 2);
    }
}
#[cfg(test)]
mod markdown_framing_grammar_tests {
    use super::rewrite_rustdoc_link_idioms as rw;
    fn unchanged(input: &str) {
        assert_eq!(rw(input), input, "expected byte-identical for {input:?}");
    }
    #[test]
    fn g01_shortcut_with_later_definition_is_preserved() {
        unchanged("see [Type] here\n\n[Type]: https://example.com/t");
    }
    #[test]
    fn g02_shortcut_without_definition_is_still_ticked() {
        assert_eq!(
            rw("see [Type] here\n\n[Other]: https://example.com/o"),
            "see [`Type`] here\n\n[Other]: https://example.com/o"
        );
    }
    #[test]
    fn g03_definition_before_reference_is_preserved() {
        unchanged("[Type]: https://example.com/t\n\nsee [Type] here");
    }
    #[test]
    fn g04_label_match_is_case_and_whitespace_normalised() {
        unchanged("see [type] here\n\n[Type]: https://example.com/t");
        unchanged("see [Foo   Bar] here\n\n[foo bar]: https://example.com/f");
    }
    #[test]
    fn g05_collapsed_reference_is_preserved() {
        unchanged("see [Type][] here\n\n[Type]: https://example.com/t");
    }
    #[test]
    fn g06_full_reference_is_preserved() {
        unchanged("see [Type][ref] here\n\n[ref]: https://example.com/t");
    }
    #[test]
    fn g07_inline_link_still_collapses_even_with_a_definition_present() {
        assert_eq!(
            rw("see [Type](Type) here\n\n[Type]: https://example.com/t"),
            "see [`Type`] here\n\n[Type]: https://example.com/t"
        );
    }
    #[test]
    fn g08_definition_indented_up_to_three_spaces_is_indexed() {
        unchanged("see [Type] here\n\n   [Type]: https://example.com/t");
    }
    #[test]
    fn g10_definition_with_title_on_same_or_next_line_is_indexed() {
        unchanged("see [Type] here\n\n[Type]: https://example.com/t \"A title\"");
        unchanged("see [Type] here\n\n[Type]: https://example.com/t\n   \"A title\"");
    }
    #[test]
    fn g11_label_spanning_a_line_break_is_never_rewritten() {
        unchanged("see [Type\nName] here");
    }
    #[test]
    fn g12_escaped_open_bracket_is_not_a_link() {
        unchanged("literal \\[Type] stays");
        unchanged("literal \\[Type\\] stays");
    }
    #[test]
    fn g13_single_backtick_span_is_preserved() {
        unchanged("use `[Type]` verbatim");
    }
    #[test]
    fn g14_two_backtick_span_is_preserved() {
        unchanged("use ``[Type]`` verbatim");
    }
    #[test]
    fn g15_three_backtick_inline_span_is_preserved() {
        unchanged("use ```[Type]``` verbatim");
    }
    #[test]
    fn g16_opening_run_longer_than_closing_run_is_literal_and_rewrites() {
        assert_eq!(rw("use ```[Type]` verbatim"), "use ```[`Type`]` verbatim");
    }
    #[test]
    fn g17_closing_run_longer_than_opening_run_is_literal_and_rewrites() {
        assert_eq!(rw("use `[Type]``` verbatim"), "use `[`Type`]``` verbatim");
    }
    #[test]
    fn g18_backticks_inside_a_fenced_block_are_inert() {
        unchanged("before\n```\n``[Type]``\n```\nafter [Other][r]\n\n[r]: https://e.com/r");
    }
    #[test]
    fn g19_unmatched_run_does_not_panic_and_still_rewrites() {
        assert_eq!(rw("dangling `` [Type] tail"), "dangling `` [`Type`] tail");
        assert_eq!(rw("dangling ` [Type] tail"), "dangling ` [`Type`] tail");
    }
    #[test]
    fn g20_multi_line_code_span_is_preserved() {
        unchanged("open ``spanning\n[Type] still code`` closed");
    }
    #[test]
    fn g21_stray_backtick_does_not_disable_the_next_line() {
        assert_eq!(
            rw("stray ` tail\nthe [Type] applies"),
            "stray ` tail\nthe [`Type`] applies"
        );
    }
    #[test]
    fn g22_definition_inside_a_fenced_block_does_not_shield_a_shortcut() {
        assert_eq!(
            rw("see [Type] here\n```\n[Type]: https://example.com/t\n```\ndone"),
            "see [`Type`] here\n```\n[Type]: https://example.com/t\n```\ndone"
        );
    }
    #[test]
    fn g09_definition_indented_four_spaces_is_indented_code_not_a_definition() {
        assert_eq!(
            rw("see [Type] here\n\n    [Type]: https://example.com/t"),
            "see [`Type`] here\n\n    [Type]: https://example.com/t"
        );
    }
    #[test]
    fn g23_inline_span_opening_with_three_ticks_is_not_a_fence() {
        assert_eq!(
            rw("```[Type]``` tail\n[Other] applies"),
            "```[Type]``` tail\n[`Other`] applies"
        );
    }
    #[test]
    fn g24_three_tick_line_does_not_close_a_four_tick_fence() {
        assert_eq!(
            rw("````\n```\n[Type]\n````\nafter [Other]"),
            "````\n```\n[Type]\n````\nafter [`Other`]"
        );
    }
    #[test]
    fn g25_open_code_span_survives_a_definition_looking_line() {
        unchanged("``open\n[Type]: /type\n[Other]\nclose``");
    }
    #[test]
    fn g26_tab_indented_definition_is_indented_code_not_a_definition() {
        assert_eq!(
            rw("see [Type] here\n\n\t[Type]: https://example.com/t"),
            "see [`Type`] here\n\n\t[Type]: https://example.com/t"
        );
    }
    #[test]
    fn g27_fence_opener_indented_four_spaces_is_not_a_fence() {
        assert_eq!(rw("    ```\n[Type] here"), "    ```\n[`Type`] here");
    }
    #[test]
    fn g28_backtick_line_does_not_close_a_tilde_fence() {
        assert_eq!(
            rw("~~~\n[Type]\n```\n[Other]\n~~~\ndone [Third]"),
            "~~~\n[Type]\n```\n[Other]\n~~~\ndone [`Third`]"
        );
    }
    #[test]
    fn g29_shorter_closer_does_not_close_and_fence_runs_to_end() {
        unchanged("````\n[Type]\n```\n[Other]");
    }
    #[test]
    fn g40_invalid_backtick_opener_does_not_interrupt_a_code_span() {
        unchanged("``open [Type]\n```foo`bar\nclose``");
    }
    #[test]
    fn g41_valid_backtick_opener_with_info_interrupts_a_code_span() {
        assert_eq!(
            rw("``open [Type]\n```foo\nclose``"),
            "``open [`Type`]\n```foo\nclose``"
        );
    }
    #[test]
    fn g30_definition_cannot_interrupt_a_paragraph() {
        assert_eq!(
            rw("text paragraph\n[Type]: https://example.com/t\n\nsee [Type] here"),
            "text paragraph\n[`Type`]: https://example.com/t\n\nsee [`Type`] here"
        );
    }
    #[test]
    fn g31_blank_line_terminates_an_open_code_span() {
        assert_eq!(
            rw("open ``run\n\n[Type] applies\nclose``"),
            "open ``run\n\n[`Type`] applies\nclose``"
        );
    }
    #[test]
    fn g32_span_closing_mid_line_leaves_the_tail_rewritable() {
        assert_eq!(
            rw("``open close`` and [Type]"),
            "``open close`` and [`Type`]"
        );
    }
    #[test]
    fn g33_fence_closer_with_trailing_text_does_not_close() {
        unchanged("```\n[Type]\n``` tail\n[Other]");
    }
    #[test]
    fn g35_indented_code_block_at_block_start_is_preserved() {
        unchanged("para\n\n    [Type]\n\nnope");
    }
    #[test]
    fn g37_a_fence_interrupts_an_open_code_span() {
        unchanged("``open\n```\nclose``\nafter [Type]");
    }
    #[test]
    fn g39_a_span_cannot_close_past_a_fence_so_its_delimiter_is_literal() {
        assert_eq!(
            rw("``open\ntext [Type] here\n```\nclose``"),
            "``open\ntext [`Type`] here\n```\nclose``"
        );
    }
    #[test]
    fn g38_rewriting_resumes_after_a_fence_that_interrupted_a_span() {
        assert_eq!(
            rw("``open\n```\ncode\n```\nafter [Type]"),
            "``open\n```\ncode\n```\nafter [`Type`]"
        );
    }
    #[test]
    fn g36_four_space_indent_interrupting_a_paragraph_is_prose() {
        assert_eq!(rw("para\n    [Type]"), "para\n    [`Type`]");
    }
    #[test]
    fn g34_lint_path_fence_tracking_matches_the_rewriter() {
        assert!(matches!(
            super::prose_word_count("````\n```\none two\n````\nthree"),
            super::WordCount::Balanced(1)
        ));
        assert!(matches!(
            super::prose_word_count("    ```\none two"),
            super::WordCount::Balanced(3)
        ));
    }
}

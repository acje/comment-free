//! Pure logic for the `comment-free` tool: parse, re-emit, lint doc-comment budget.
#![forbid(unsafe_code)]
#![warn(clippy::missing_const_for_fn)]
use ra_ap_rustc_lexer::{FrontmatterAllowed, TokenKind, tokenize};
use similar::{ChangeTag, TextDiff};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
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
{\"record\":\"doc_lint_truncated\",\"v\":<N>,\"kind\":<KIND>,\"remaining\":<U32>}";
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
{\"record\":\"lint_summary\",\"v\":<N>,\"files\":<U32>,\"findings\":<U32>,\"errors\":<U32>}";
/// Which pass a `--rewrite` run made over the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RewriteMode {
    /// Files were replaced on disk.
    Write,
    /// Diffs were printed and no file was touched.
    DryRun,
}
impl RewriteMode {
    /// The `mode` field value carried by emitted records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::DryRun => "dry-run",
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
/// Reserved so a later lint pass that cannot decide an item — an
/// unresolved `cfg` predicate, an uninspected macro body — reports an
/// explicit indeterminate outcome as an added variant rather than a
/// record-version bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DocLintOutcome {
    /// The item was inspected and violates the budget.
    Finding,
}
impl DocLintOutcome {
    /// The `outcome` field value carried by emitted records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Finding => "finding",
        }
    }
}
/// Per-file counters surfaced by the rewrite passes, aggregated across
/// files in the run's `rewrite_summary` record. All fields default to
/// zero; `#[non_exhaustive]` allows new counters without a breaking
/// change. Construct with `RewriteCounts::default()` and update fields
/// explicitly.
///
/// - `comments_removed` — non-doc line and block comments dropped.
/// - `inline_trimmed` — subset of `comments_removed`: mid-line
///   (post-code) drops that trimmed trailing whitespace. Solo-line
///   drops are excluded.
/// - `blank_lines_collapsed` — symmetric-pad collapses, one per
///   removed comment block with blanks on both sides.
/// - `doc_links_rewritten` — splices applied by the doc-link idiom
///   canonicaliser, one per rewritten literal span.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RewriteCounts {
    pub comments_removed: u32,
    pub inline_trimmed: u32,
    pub blank_lines_collapsed: u32,
    pub doc_links_rewritten: u32,
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
    /// Doc-lint violation: at least one `DOC_LINT` finding emitted under default lint mode.
    #[error("doc lint failure")]
    DocLintFailure,
}
/// A directory-traversal failure, preserved rather than skipped.
///
/// A walk failure is an indeterminate result, never the absence of a
/// file: folding it into "no entry" lets a partial scan report zero
/// errors. `path` is the entry that failed, falling back to the walk
/// root when the underlying error carries none.
#[derive(Debug, thiserror::Error)]
#[error("cannot traverse {}: {source}", path.display())]
#[non_exhaustive]
pub struct WalkError {
    pub path: PathBuf,
    #[source]
    pub source: walkdir::Error,
}
impl WalkError {
    /// Attribute `source` to the entry it failed on, or to `base` when
    /// the underlying error carries no path.
    #[must_use]
    pub fn rooted_at(base: &Path, source: walkdir::Error) -> Self {
        let path = source.path().unwrap_or(base).to_path_buf();
        Self { path, source }
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
#[derive(Debug)]
pub enum FileOutcome {
    Rewritten {
        diff: Option<String>,
        counts: RewriteCounts,
    },
    Unchanged {
        counts: RewriteCounts,
    },
    ParseError(String),
    IoError(String),
    /// The destination no longer held the bytes originally read, so the
    /// rewrite was abandoned and the file left exactly as found.
    Conflict,
}
#[derive(Debug, thiserror::Error)]
enum WriteError {
    #[error("destination changed since it was read")]
    Conflict,
    #[error(
        "destination is a symbolic link; rewriting it would replace the link rather than its target"
    )]
    SymlinkDestination,
    #[error("io error: {0}")]
    Io(#[from] io::Error),
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
fn write_atomically(path: &Path, expected: &str, rewritten: &str) -> Result<(), WriteError> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(WriteError::SymlinkDestination);
    }
    let permissions = fs::metadata(path)?.permissions();
    let (mut file, mut guard) = create_sibling_temp(path, dir)?;
    file.write_all(rewritten.as_bytes())?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::set_permissions(&guard.path, permissions)?;
    if !destination_still_holds(path, expected)? {
        return Err(WriteError::Conflict);
    }
    fs::rename(&guard.path, path)?;
    guard.disarm();
    Ok(())
}
const SIMULATE_WRITE_CONFLICT_ENV: &str = "COMMENT_FREE_SIMULATE_WRITE_CONFLICT";
fn destination_still_holds(path: &Path, expected: &str) -> Result<bool, WriteError> {
    if std::env::var_os(SIMULATE_WRITE_CONFLICT_ENV).is_some() {
        return Ok(false);
    }
    Ok(fs::read_to_string(path)? == expected)
}
fn create_sibling_temp(path: &Path, dir: &Path) -> Result<(fs::File, TempFileGuard), WriteError> {
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
            Err(e) => return Err(WriteError::Io(e)),
        }
    }
    Err(WriteError::Io(io::Error::new(
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
pub fn lint_summary_record(files: u32, findings: u32, errors: u32) -> String {
    let mut out = open_record("lint_summary", DIAGNOSTIC_RECORD_VERSION);
    out.push(',');
    push_number(&mut out, "files", files);
    out.push(',');
    push_number(&mut out, "findings", findings);
    out.push(',');
    push_number(&mut out, "errors", errors);
    out.push('}');
    out
}
/// Knobs [`process_file`] reads. `main.rs`'s clap `Options` is intentionally a
/// superset; this trims the surface to what the pure logic actually needs.
pub struct ProcessOptions {
    pub dry_run: bool,
    pub context: usize,
}
/// Process `path`: doc-comment link-idiom canonicalisation + lexer-based
/// non-doc comment strip. Byte-preserving outside targets; code
/// formatting and whitespace outside comments are untouched.
///
/// Returns:
///
/// - [`FileOutcome::Rewritten`] — content changed (unified diff in
///   `dry_run` mode, `None` otherwise).
/// - [`FileOutcome::Unchanged`] — no bytes changed.
/// - [`FileOutcome::ParseError`] — syn parse for the doc-link pass
///   failed; file left untouched on disk.
/// - [`FileOutcome::IoError`] — any I/O failure.
/// - [`FileOutcome::Conflict`] — the file changed on disk between the
///   read and the write; the destination keeps its own bytes.
///
/// In write mode the new content lands via a sibling temporary file
/// renamed over the destination, so the destination is never truncated
/// in place and a partial rewrite cannot be observed. A destination
/// that is itself a symbolic link is refused as
/// [`FileOutcome::IoError`] rather than rewritten, because the rename
/// would replace the link instead of its target. The temporary file is
/// removed on every returning error path and, on a best-effort basis,
/// while a panic unwinds; an abort or a failing removal can still leave
/// it behind.
///
/// Stripped: every ordinary `//` line and `/* */` block comment.
/// Preserved: doc comments (`///`, `//!`, `/** */`, `/*! */`).
#[must_use]
pub fn process_file(path: &Path, opts: &ProcessOptions) -> FileOutcome {
    let original = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return FileOutcome::IoError(e.to_string()),
    };
    let ast: File = match syn::parse_file(&original) {
        Ok(f) => f,
        Err(e) => return FileOutcome::ParseError(e.to_string()),
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
        return FileOutcome::Unchanged { counts };
    }
    if opts.dry_run {
        let diff = unified_diff(path, &original, &rewritten, opts.context);
        FileOutcome::Rewritten {
            diff: Some(diff),
            counts,
        }
    } else {
        match write_atomically(path, &original, &rewritten) {
            Ok(()) => FileOutcome::Rewritten { diff: None, counts },
            Err(WriteError::Conflict) => FileOutcome::Conflict,
            Err(e @ (WriteError::SymlinkDestination | WriteError::Io(_))) => {
                FileOutcome::IoError(e.to_string())
            }
        }
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
/// tally of the strip pass.
///
/// Drops a token iff it is a `LineComment` or `BlockComment` with
/// `doc_style: None`; doc comments and every other token are preserved
/// unchanged. String-literal interiors are structurally unreachable by
/// this pass, so comment-looking text inside a string round-trips
/// byte-identical.
///
/// A solo-line drop collapses its trailing blank-line scar; an inline
/// (post-code) drop trims the preceding horizontal whitespace instead.
/// A contiguous run of solo-line drops with blanks on both sides emits
/// `max(blanks_above, blanks_below)` rather than their sum.
///
/// See [`RewriteCounts`] for field meanings; `doc_links_rewritten`
/// stays 0 here — that count comes from [`process_file`]'s upstream
/// doc-link pass.
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
/// State captured while inside a contiguous run of solo-line non-doc
/// comment drops. `blanks_above` is the count of blank lines that
/// preceded the first dropped comment in this run, derived from the
/// `\n` run at the tail of `out` at the moment the run started.
/// `blanks_below_emitted` accumulates the `\n` count contributed by
/// whitespace tokens that follow each dropped comment after their
/// leading `\n` has already been stripped by `pending_blank_collapse`.
struct DropRun {
    blanks_above: usize,
    blanks_below_emitted: usize,
}
/// Count blank lines at the tail of `s`. A blank line at the tail is
/// represented by an extra trailing `\n` beyond the one that ends the
/// last non-empty content line. Specifically: returns
/// `(trailing_newlines).saturating_sub(1)` so that `\n` alone yields 0,
/// `\n\n` yields 1, `\n\n\n` yields 2, and an empty string yields 0.
fn trailing_blank_lines(s: &str) -> usize {
    let trailing = s.bytes().rev().take_while(|b| *b == b'\n').count();
    trailing.saturating_sub(1)
}
/// Count the `\n` bytes in `s`.
fn count_newlines(s: &str) -> usize {
    s.bytes().filter(|b| *b == b'\n').count()
}
/// Pop exactly one `\n` from the trailing-blank region of `s` — the
/// run of `\n` bytes that ends `s` after any trailing horizontal
/// whitespace (the indent of the next code line). Leaves any
/// horizontal whitespace and prior bytes intact. Used to collapse one
/// unit of redundant blank-line padding when both sides of a removed
/// comment block had at least one blank line in the source.
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
/// True iff `prefix` ends with a sequence that includes no characters
/// other than horizontal whitespace since the most recent `\n` (or the
/// start of input). Caller passes the source bytes up to but excluding
/// the comment whose blankness is being judged.
fn line_was_blank_before(prefix: &str) -> bool {
    let line_start = prefix.rfind('\n').map_or(0, |p| p + 1);
    prefix[line_start..].chars().all(|c| c == ' ' || c == '\t')
}
/// In-place: drop any trailing run of horizontal whitespace from `s`,
/// leaving prior `\n` and earlier content intact. Used to clean up the
/// indentation that preceded a stripped solo-line comment so the
/// collapse leaves no trailing-whitespace residue, and to trim the
/// inline whitespace between code and a removed mid-line comment.
/// Returns the number of characters popped.
fn trim_trailing_whitespace_to_last_newline(s: &mut String) -> usize {
    let mut popped = 0;
    while matches!(s.chars().last(), Some(' ' | '\t')) {
        s.pop();
        popped += 1;
    }
    popped
}
/// One byte-range replacement against the original source.
///
/// `range` is a byte range in the original source string; `replacement`
/// is the substitute. Splices are applied in reverse start-order so
/// earlier offsets are not invalidated by later mutations.
#[derive(Debug, Clone)]
struct DocSplice {
    range: std::ops::Range<usize>,
    replacement: String,
}
/// Apply `splices` to `original` and return the rewritten source.
///
/// Splices must not overlap. Applied in reverse order of start byte
/// so each application leaves the not-yet-applied splices' offsets
/// valid.
fn apply_splices(original: &str, mut splices: Vec<DocSplice>) -> String {
    splices.sort_by_key(|s| std::cmp::Reverse(s.range.start));
    let mut out = original.to_string();
    for splice in splices {
        out.replace_range(splice.range, &splice.replacement);
    }
    out
}
/// Walk `ast`, collect a [`DocSplice`] for every doc surface whose
/// payload changes under [`rewrite_rustdoc_link_idioms`].
///
/// Surfaces handled: file-level inner attributes (`#![doc = "..."]`,
/// `//!`); per-item attributes (`#[doc = "..."]`, `///`) grouped by
/// run; `cfg_attr(_, doc = "...")` payloads in isolation; trait-item,
/// impl-item, field, and variant attributes (same model).
///
/// Block doc comments (`/** ... */`) are NOT touched — the in-memory
/// payload and on-disk source bytes diverge (lexer strips leading
/// `*`).
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
/// Group `attrs` into contiguous runs of safe-to-splice doc payloads
/// and emit one [`DocSplice`] per literal whose rewrite differs from
/// the original payload text.
///
/// "Safe to splice" excludes block doc comments (`/** ... */`); the
/// run is broken on encountering one or on any non-doc attribute.
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
/// Return the byte-range and shape of a doc literal's storage in
/// `original` if `attr` is one of:
///
/// - `///` / `//!` (line doc, range covers the whole `///`+payload line)
/// - `#[doc = "…"]` / `#![doc = "…"]` (range covers the quoted literal token)
///
/// Returns `None` for non-`doc` attributes, `cfg_attr`, and block
/// doc comments (`/** … */`).
fn doc_attr_literal_span(
    attr: &Attribute,
    original: &str,
    _shape: DocShape,
) -> Option<DocLiteralSite> {
    let Meta::NameValue(nv) = &attr.meta else {
        return None;
    };
    if !nv.path.is_ident("doc") {
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
/// Classify the source-byte form of a doc literal.
///
/// Returns `None` for block doc comments (`/** … */`) — these are
/// deliberately left untouched by the safe path.
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
/// Emit splices for a run of contiguous doc literals.
///
/// All literals in `run` are spliceable line- or attribute-form docs
/// (i.e. classified by [`classify_doc_literal`]). The run is joined with
/// `\n`, transformed once so the fenced-code tracker sees the whole block,
/// then split back into the same number of lines. Each line is individually
/// spliced (its quoted form for `#[doc = …]`, or its `///`/`//!`-prefixed
/// form for line docs) so non-doc bytes between docs in the run are
/// preserved verbatim.
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
/// Render a replacement for a `///` or `//!` line-doc storage range.
///
/// The original `body` is the full `///…` or `//!…` source line. Its
/// leading marker may be `///`, `////` (impossible — filtered earlier),
/// or `//!`. We preserve the *exact* marker bytes the source used
/// (just `///` or `//!`) and substitute the trailing payload with
/// `new_payload`.
///
/// Returns `None` if the body doesn't start with the expected marker
/// (defensive; should not happen given [`classify_doc_literal`]).
fn render_line_doc(body: &str, marker: &str, new_payload: &str) -> Option<String> {
    if !body.starts_with(marker) {
        return None;
    }
    let mut out = String::with_capacity(marker.len() + new_payload.len());
    out.push_str(marker);
    out.push_str(new_payload);
    Some(out)
}
/// Render a properly-quoted Rust string literal for a `#[doc = "…"]`
/// payload value. Uses [`proc_macro2::Literal::string`] for the
/// quoting/escaping rules; converts to its source-form via [`ToString`].
fn render_quoted_doc_literal(value: &str) -> String {
    proc_macro2::Literal::string(value).to_string()
}
/// Collect splices for every `#[cfg_attr(_, doc = "…")]` payload in `attrs`.
///
/// Each `doc = "…"` payload literal inside a `cfg_attr` list is transformed
/// in isolation (gating predicates may differ) and spliced at its own
/// [`syn::LitStr`] byte-range.
fn collect_cfg_attr_doc_splices(attrs: &[Attribute], original: &str, out: &mut Vec<DocSplice>) {
    for attr in attrs {
        if !is_cfg_attr(attr) {
            continue;
        }
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        let parsed: Result<Punctuated<Meta, Token![,]>, _> =
            list.parse_args_with(Punctuated::parse_terminated);
        let Ok(metas) = parsed else {
            continue;
        };
        for meta in metas.iter().skip(1) {
            let Meta::NameValue(nv) = meta else {
                continue;
            };
            if !nv.path.is_ident("doc") {
                continue;
            }
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            else {
                continue;
            };
            let range = s.span().byte_range();
            let Some(body) = original.get(range.clone()) else {
                continue;
            };
            if !(body.starts_with('"') && body.ends_with('"')) {
                continue;
            }
            let value = s.value();
            let rewritten = rewrite_rustdoc_link_idioms(&value);
            if rewritten == value {
                continue;
            }
            let replacement = render_quoted_doc_literal(&rewritten);
            out.push(DocSplice { range, replacement });
        }
    }
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
/// Render a unified diff between `original` and `rewritten` for `path`.
#[must_use]
pub fn unified_diff(path: &Path, original: &str, rewritten: &str, context: usize) -> String {
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
/// True if `attr` is a `#[cfg_attr(...)]` attribute.
#[must_use]
pub fn is_cfg_attr(attr: &Attribute) -> bool {
    match &attr.meta {
        Meta::List(list) => list.path.is_ident("cfg_attr"),
        _ => false,
    }
}
/// Result of [`scan_doc_files`]: the documentation files found, plus
/// every traversal failure encountered. Callers must count `errors`
/// towards the run error total; a scan that could not read part of the
/// tree has not established that the tree is clean.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct DocScan {
    pub files: Vec<PathBuf>,
    pub errors: Vec<WalkError>,
}
/// Walk `root` and report every file that looks like documentation,
/// together with every traversal failure.
///
/// Skips dotfiles/dotdirs, `target/`, and common vendor/build directories
/// (`node_modules`, `vendor`, `dist`, `build`) to avoid polyglot-repo noise.
/// Unreadable entries are reported in [`DocScan::errors`], never dropped.
#[must_use]
pub fn scan_doc_files(root: &Path) -> DocScan {
    let mut scan = DocScan::default();
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
            Err(e) => scan.errors.push(WalkError::rooted_at(root, e)),
            Ok(entry) if entry.file_type().is_file() && is_doc_path(entry.path(), root) => {
                scan.files.push(entry.into_path());
            }
            Ok(_) => {}
        }
    }
    scan
}
/// Directories `scan_doc_files` and `.rs` traversal skip wholesale.
pub const SKIP_DIRS: &[&str] = &["target", "node_modules", "vendor", "dist", "build"];
/// True if `path` looks like documentation: doc file extension, bare
/// README/LICENSE-style stem, or living under a top-level `docs/` / `doc/`
/// directory directly beneath `root`.
///
/// The `docs/`/`doc/` rule is **scoped to the first relative component
/// under `root`**, so `src/docs/mod.rs` or `crates/foo/doc/inner.rs` do
/// NOT match. This narrows the strip-mode `doc_file_warning` noise to
/// genuine top-level documentation directories.
#[must_use]
pub fn is_doc_path(path: &Path, root: &Path) -> bool {
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
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        let stem_uc = stem.to_ascii_uppercase();
        for known in BARE_DOC_STEMS {
            if stem_uc == *known {
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
#[derive(Debug, Clone)]
pub struct DocFinding {
    /// Human-readable label for the docced item, e.g. `"fn foo"` or `"struct Bar"`.
    pub item_label: String,
    /// Approximate source line of the docced item (from `proc_macro2` spans).
    pub line: usize,
    /// Word count of the item's doc-comment prose (fenced code excluded).
    pub word_count: usize,
    /// The budget the count exceeded.
    pub budget: usize,
    /// True when `word_count` came from the fail-closed recount path
    /// (unbalanced fence at EOF). `words=` is then an inflated number,
    /// not the real prose count.
    pub fail_closed: bool,
}
/// Lint `ast` for doc-comments whose prose word count exceeds `budget.max_words`.
///
/// Concatenates `///`, `//!`, `#[doc=...]`, and `cfg_attr` doc payloads in
/// source order; only triple-backtick fenced lines are excluded from the count.
/// Docs inside opaque macro bodies are not visited.
#[must_use]
pub fn doc_lint_file(ast: &syn::File, budget: DocBudget) -> Vec<DocFinding> {
    let mut visitor = DocLintVisitor {
        budget,
        findings: Vec::new(),
    };
    visitor.lint_attrs(&ast.attrs, "file-level", None);
    syn::visit::Visit::visit_file(&mut visitor, ast);
    visitor.findings
}
struct DocLintVisitor {
    budget: DocBudget,
    findings: Vec<DocFinding>,
}
impl DocLintVisitor {
    fn lint_attrs(&mut self, attrs: &[Attribute], label: &str, span_line: Option<usize>) {
        let Some((text, attr_line)) = extract_doc_text(attrs) else {
            return;
        };
        let words = prose_word_count(&text);
        if words.count() > self.budget.max_words {
            self.findings.push(DocFinding {
                item_label: label.to_string(),
                line: span_line.unwrap_or(attr_line),
                word_count: words.count(),
                budget: self.budget.max_words,
                fail_closed: words.is_fail_closed(),
            });
        }
    }
}
/// Concatenate doc payloads from `attrs` (in source order) and return the
/// combined text plus the approximate source line of the first doc attribute.
/// `None` if `attrs` carries no doc payloads.
fn extract_doc_text(attrs: &[Attribute]) -> Option<(String, usize)> {
    let mut parts: Vec<String> = Vec::new();
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
            parts.push(payload);
        } else if is_cfg_attr(attr) {
            for payload in cfg_attr_doc_payloads(attr) {
                if first_line.is_none() {
                    first_line = Some(line);
                }
                parts.push(payload);
            }
        }
    }
    let line = first_line?;
    Some((parts.join("\n"), line))
}
/// Extract the string payload of a `#[doc = "..."]` attribute, if it is one.
fn doc_payload(attr: &Attribute) -> Option<String> {
    let Meta::NameValue(nv) = &attr.meta else {
        return None;
    };
    if !nv.path.is_ident("doc") {
        return None;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &nv.value
    else {
        return None;
    };
    Some(s.value())
}
/// Extract every `doc = "..."` payload from inside a `#[cfg_attr(<pred>, ...)]`
/// list, ignoring the predicate. Returns empty vec if none.
fn cfg_attr_doc_payloads(attr: &Attribute) -> Vec<String> {
    let Meta::List(list) = &attr.meta else {
        return Vec::new();
    };
    if !list.path.is_ident("cfg_attr") {
        return Vec::new();
    }
    let parsed: Result<Punctuated<Meta, Token![,]>, _> =
        list.parse_args_with(Punctuated::parse_terminated);
    let Ok(metas) = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for meta in metas.into_iter().skip(1) {
        if let Meta::NameValue(nv) = &meta
            && nv.path.is_ident("doc")
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
        {
            out.push(s.value());
        }
    }
    out
}
/// Walk `file`, mutating every doc-comment payload through
/// [`rewrite_rustdoc_link_idioms`]. Reaches every surface
/// [`doc_lint_file`] inspects (top-level attrs, items, trait/impl
/// items, fields, variants).
///
/// Per item, contiguous runs of unconditional `#[doc = "..."]`
/// attributes are joined with `\n`, transformed once (so a fenced
/// block spanning multiple `///` lines tracks fence state correctly),
/// then split back per attribute — the transform is line-count
/// invariant. `cfg_attr(_, doc = "...")` payloads transform
/// independently (different gating predicates).
pub fn apply_rustdoc_link_idioms_to_ast(file: &mut syn::File) {
    use syn::visit_mut::VisitMut;
    struct Visitor;
    impl VisitMut for Visitor {
        fn visit_file_mut(&mut self, node: &mut syn::File) {
            rewrite_attrs_doc_links(&mut node.attrs);
            syn::visit_mut::visit_file_mut(self, node);
        }
        fn visit_item_mut(&mut self, node: &mut syn::Item) {
            if let Some(attrs) = item_attrs_mut(node) {
                rewrite_attrs_doc_links(attrs);
            }
            syn::visit_mut::visit_item_mut(self, node);
        }
        fn visit_trait_item_mut(&mut self, node: &mut syn::TraitItem) {
            if let Some(attrs) = trait_item_attrs_mut(node) {
                rewrite_attrs_doc_links(attrs);
            }
            syn::visit_mut::visit_trait_item_mut(self, node);
        }
        fn visit_impl_item_mut(&mut self, node: &mut syn::ImplItem) {
            if let Some(attrs) = impl_item_attrs_mut(node) {
                rewrite_attrs_doc_links(attrs);
            }
            syn::visit_mut::visit_impl_item_mut(self, node);
        }
        fn visit_field_mut(&mut self, node: &mut syn::Field) {
            rewrite_attrs_doc_links(&mut node.attrs);
            syn::visit_mut::visit_field_mut(self, node);
        }
        fn visit_variant_mut(&mut self, node: &mut syn::Variant) {
            rewrite_attrs_doc_links(&mut node.attrs);
            syn::visit_mut::visit_variant_mut(self, node);
        }
    }
    Visitor.visit_file_mut(file);
}
/// Borrow the `attrs` slot of any `syn::Item` variant that carries one.
const fn item_attrs_mut(item: &mut syn::Item) -> Option<&mut Vec<Attribute>> {
    use syn::Item::{
        Const, Enum, ExternCrate, Fn, Impl, Macro, Mod, Static, Struct, Trait, TraitAlias, Type,
        Union, Use,
    };
    Some(match item {
        Const(i) => &mut i.attrs,
        Enum(i) => &mut i.attrs,
        ExternCrate(i) => &mut i.attrs,
        Fn(i) => &mut i.attrs,
        Impl(i) => &mut i.attrs,
        Macro(i) => &mut i.attrs,
        Mod(i) => &mut i.attrs,
        Static(i) => &mut i.attrs,
        Struct(i) => &mut i.attrs,
        Trait(i) => &mut i.attrs,
        TraitAlias(i) => &mut i.attrs,
        Type(i) => &mut i.attrs,
        Union(i) => &mut i.attrs,
        Use(i) => &mut i.attrs,
        _ => return None,
    })
}
const fn trait_item_attrs_mut(item: &mut syn::TraitItem) -> Option<&mut Vec<Attribute>> {
    use syn::TraitItem::{Const, Fn, Macro, Type};
    Some(match item {
        Const(i) => &mut i.attrs,
        Fn(i) => &mut i.attrs,
        Macro(i) => &mut i.attrs,
        Type(i) => &mut i.attrs,
        _ => return None,
    })
}
const fn impl_item_attrs_mut(item: &mut syn::ImplItem) -> Option<&mut Vec<Attribute>> {
    use syn::ImplItem::{Const, Fn, Macro, Type};
    Some(match item {
        Const(i) => &mut i.attrs,
        Fn(i) => &mut i.attrs,
        Macro(i) => &mut i.attrs,
        Type(i) => &mut i.attrs,
        _ => return None,
    })
}
/// Apply the link-idiom transform to one item's `attrs` slice.
///
/// Contiguous runs of unconditional `#[doc = "..."]` are joined and
/// transformed together; `cfg_attr(_, doc = "...")` payloads are each
/// transformed in isolation (they may be gated independently).
fn rewrite_attrs_doc_links(attrs: &mut [Attribute]) {
    let mut i = 0;
    while i < attrs.len() {
        if doc_string_payload(&attrs[i]).is_some() {
            let start = i;
            while i < attrs.len() && doc_string_payload(&attrs[i]).is_some() {
                i += 1;
            }
            rewrite_doc_run(&mut attrs[start..i]);
            continue;
        }
        if is_cfg_attr(&attrs[i]) {
            rewrite_cfg_attr_doc_payloads(&mut attrs[i]);
        }
        i += 1;
    }
}
/// Join, transform, and split-back a contiguous run of `#[doc = "..."]`
/// attributes. The transform is line-count-preserving, so the split has
/// the same length as the input — assigned 1:1 back into each attribute.
fn rewrite_doc_run(run: &mut [Attribute]) {
    if run.is_empty() {
        return;
    }
    let originals: Vec<String> = run
        .iter()
        .map(|a| doc_string_payload(a).unwrap_or_default())
        .collect();
    let joined = originals.join("\n");
    let rewritten = rewrite_rustdoc_link_idioms(&joined);
    if rewritten == joined {
        return;
    }
    let parts: Vec<&str> = rewritten.split('\n').collect();
    if parts.len() != originals.len() {
        return;
    }
    for (attr, new) in run.iter_mut().zip(parts) {
        set_doc_string_payload(attr, new);
    }
}
/// Rewrite every `doc = "..."` payload inside a `#[cfg_attr(_, ...)]`
/// list independently. Predicate position is left untouched.
fn rewrite_cfg_attr_doc_payloads(attr: &mut Attribute) {
    let Meta::List(list) = &mut attr.meta else {
        return;
    };
    if !list.path.is_ident("cfg_attr") {
        return;
    }
    let parsed: Result<Punctuated<Meta, Token![,]>, _> =
        list.parse_args_with(Punctuated::parse_terminated);
    let Ok(mut metas) = parsed else {
        return;
    };
    let mut changed = false;
    for meta in metas.iter_mut().skip(1) {
        if let Meta::NameValue(nv) = meta
            && nv.path.is_ident("doc")
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &mut nv.value
        {
            let original = s.value();
            let rewritten = rewrite_rustdoc_link_idioms(&original);
            if rewritten != original {
                *s = syn::LitStr::new(&rewritten, s.span());
                changed = true;
            }
        }
    }
    if !changed {
        return;
    }
    list.tokens = quote::quote!(#metas);
}
/// Return the string payload of `#[doc = "..."]`, if it's literal.
fn doc_string_payload(attr: &Attribute) -> Option<String> {
    let Meta::NameValue(nv) = &attr.meta else {
        return None;
    };
    if !nv.path.is_ident("doc") {
        return None;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &nv.value
    else {
        return None;
    };
    Some(s.value())
}
/// Replace the string payload of a `#[doc = "..."]` attribute in place.
fn set_doc_string_payload(attr: &mut Attribute, new: &str) {
    let Meta::NameValue(nv) = &mut attr.meta else {
        return;
    };
    if !nv.path.is_ident("doc") {
        return;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &mut nv.value
    else {
        return;
    };
    *s = syn::LitStr::new(new, s.span());
}
/// Count words in `doc_text`, excluding fenced code.
///
/// Recognises ` ``` ` and `~~~` fences. Fail-closed: if a fence opens but
/// never closes, returns [`WordCount::FailClosed`] with a whole-text
/// recount so a malformed doc cannot silently suppress budget checking.
fn prose_word_count(doc_text: &str) -> WordCount {
    let mut in_fence = false;
    let mut words = 0usize;
    for line in doc_text.lines() {
        let stripped = line.trim_start();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        words += line.split_whitespace().count();
    }
    if in_fence {
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
mod atomic_write_tests {
    use super::{
        FileOutcome, ProcessOptions, WriteError, create_sibling_temp, process_file,
        write_atomically,
    };
    use std::fs;
    fn write_opts() -> ProcessOptions {
        ProcessOptions {
            dry_run: false,
            context: 3,
        }
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
    fn destination_changed_since_read_is_a_conflict_and_leaves_bytes_untouched() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "concurrent editor won\n").unwrap();
        let err = write_atomically(&path, "bytes we read earlier\n", "our rewrite\n").unwrap_err();
        assert!(matches!(err, WriteError::Conflict), "got {err:?}");
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
            Err(WriteError::Conflict)
        ));
        match process_file(&path, &write_opts()) {
            FileOutcome::Rewritten { .. } => {}
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
        match process_file(&path, &write_opts()) {
            FileOutcome::Rewritten { .. } => {}
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
        match process_file(&path, &write_opts()) {
            FileOutcome::Rewritten { .. } => {}
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
        match process_file(&link, &write_opts()) {
            FileOutcome::IoError(msg) => assert!(msg.contains("symbolic link"), "got {msg}"),
            other => panic!("expected IoError, got {other:?}"),
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
        assert!(matches!(err, WriteError::Io(_)), "got {err:?}");
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
}
#[cfg(test)]
mod process_file_tests {
    use super::{FileOutcome, ProcessOptions, process_file, strip_line_comments};
    use std::fs;
    fn opts() -> ProcessOptions {
        ProcessOptions {
            dry_run: true,
            context: 3,
        }
    }
    #[test]
    fn whitespace_only_file_is_unchanged() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "   \n\t\n  \n").unwrap();
        match process_file(&path, &opts()) {
            FileOutcome::Unchanged { .. } => {}
            other => panic!("expected Unchanged for whitespace-only file, got {other:?}"),
        }
    }
    #[test]
    fn empty_file_is_unchanged() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "").unwrap();
        match process_file(&path, &opts()) {
            FileOutcome::Unchanged { .. } => {}
            other => panic!("expected Unchanged for empty file, got {other:?}"),
        }
    }
    #[test]
    fn safety_only_file_is_rewritten_and_counts_the_comment_removed() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "// SAFETY: pointer is valid\nfn f() {}\n").unwrap();
        match process_file(&path, &opts()) {
            FileOutcome::Rewritten { counts, .. } => {
                assert_eq!(counts.comments_removed, 1);
            }
            other => panic!("expected Rewritten for SAFETY-only file, got {other:?}"),
        }
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
    use syn::parse_quote;
    fn lint(file: &syn::File, max_words: usize) -> Vec<super::DocFinding> {
        doc_lint_file(file, DocBudget { max_words })
    }
    #[test]
    fn no_docs_yields_no_findings() {
        let f: syn::File = parse_quote! {
            pub fn foo() {}
        };
        assert!(lint(&f, 40).is_empty());
    }
    #[test]
    fn short_doc_under_budget_yields_no_findings() {
        let f: syn::File = parse_quote! {
            #[doc = " one two three four five"] pub fn foo() {}
        };
        assert!(lint(&f, 40).is_empty());
    }
    #[test]
    fn long_doc_over_budget_yields_one_finding() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
            " w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
            " w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
            " w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
            " w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] pub fn foo() {}
        };
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].word_count, 50);
        assert_eq!(findings[0].budget, 40);
        assert_eq!(findings[0].item_label, "fn foo");
    }
    #[test]
    fn fenced_code_excluded_brings_under_budget() {
        let f: syn::File = parse_quote! {
            #[doc = " p01 p02 p03 p04 p05 p06 p07 p08 p09 p10"] #[doc = " ```"] #[doc =
            " c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
            " c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
            " c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
            " c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
            " c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] #[doc = " ```"] pub fn foo() {}
        };
        let findings = lint(&f, 40);
        assert!(findings.is_empty(), "{findings:?}");
    }
    #[test]
    fn multi_attr_docs_concatenate() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05"] #[doc = " w06 w07 w08 w09 w10"] #[doc =
            "w11 w12 w13 w14 w15"] pub fn foo() {}
        };
        let findings = lint(&f, 10);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].word_count, 15);
    }
    #[test]
    fn cfg_attr_doc_payload_counted() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05"] #[cfg_attr(test, doc =
            "w06 w07 w08 w09 w10")] pub fn foo() {}
        };
        let findings = lint(&f, 7);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].word_count, 10);
    }
    #[test]
    fn doc_inside_macro_rules_not_linted() {
        let f: syn::File = parse_quote! {
            macro_rules! noisy { () => { #[doc =
            " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
            " w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
            " w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
            " w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
            " w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] pub fn inner() {} }; }
        };
        let findings = lint(&f, 5);
        assert!(findings.is_empty(), "{findings:?}");
    }
    #[test]
    fn field_and_variant_docs_linted_independently() {
        let f: syn::File = parse_quote! {
            pub struct S { #[doc = " w01 w02 w03 w04 w05"] pub a : u32, #[doc =
            " w01 w02 w03 w04 w05 w06"] pub b : u32, }
        };
        let findings = lint(&f, 3);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings.iter().all(|f| f.item_label.starts_with("field ")));
        let f: syn::File = parse_quote! {
            pub enum E { #[doc = " w01 w02 w03 w04 w05"] One, #[doc =
            " w01 w02 w03 w04 w05 w06"] Two, }
        };
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
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05"] #[doc = " ```"] #[doc = " c01 c02 c03"] #[doc
            = " ```"] #[doc = " w06 w07 w08 w09 w10 w11"] pub fn foo() {}
        };
        let findings = lint(&f, 10);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].word_count, 11);
    }
    #[test]
    fn equal_to_budget_does_not_trigger() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05"] pub fn foo() {}
        };
        assert!(lint(&f, 5).is_empty());
    }
    #[test]
    fn tilde_fence_excludes_code() {
        let f: syn::File = parse_quote! {
            #[doc = " p01 p02 p03 p04 p05 p06 p07 p08 p09 p10"] #[doc = " ~~~"] #[doc =
            " c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
            " c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
            " c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
            " c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
            " c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] #[doc = " ~~~"] pub fn foo() {}
        };
        let findings = lint(&f, 40);
        assert!(findings.is_empty(), "{findings:?}");
    }
    #[test]
    fn unclosed_fence_fails_closed() {
        let f: syn::File = parse_quote! {
            #[doc = " p01 p02 p03 p04 p05"] #[doc = " ```"] #[doc =
            " c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
            " c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
            " c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
            " c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
            " c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] pub fn foo() {}
        };
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].word_count, 56);
        assert!(
            findings[0].fail_closed,
            "unbalanced fence must set fail_closed=true: {:?}",
            findings[0]
        );
    }
    #[test]
    fn over_budget_doc_on_pub_use_is_linted() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
            " w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
            " w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
            " w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
            " w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] pub use crate ::foo::Bar;
        };
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].item_label, "use");
        assert_eq!(findings[0].word_count, 50);
    }
    #[test]
    fn over_budget_doc_on_extern_crate_is_linted() {
        let f: syn::File = parse_quote! {
            #[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
            " w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
            " w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
            " w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
            " w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] extern crate alloc;
        };
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].item_label, "extern crate alloc");
    }
}
/// Rewrite mechanically-safe Rust item links in `doc_text`.
///
/// Operates on the prose of a single doc-comment block (concatenated
/// payloads of one item, joined by `\n`). Maintains fenced-code state
/// across lines (` ``` ` and `~~~`); transforms inside a fence are
/// suppressed, as are byte ranges covered by inline code spans
/// (single-backtick pairs).
///
/// Rules applied only when the label is a conservative Rust item
/// token (see [`is_codeish_token`]):
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
/// - lines inside fenced code blocks
/// - spans inside inline code (`code`)
/// - URL targets (contain ://, or start with /, #, mailto:)
/// - reference definitions ([label]: <url>) and reference links ([label][ref])
/// - targets with generics, disambiguators, or fragments (< > @ # ( ) ! ?)
/// - labels already wrapped in backticks (idempotent)
/// - prose labels — anything not matching is_codeish_token
/// - empty link bodies
/// ```
#[must_use]
pub fn rewrite_rustdoc_link_idioms(doc_text: &str) -> String {
    let mut out = String::with_capacity(doc_text.len());
    let mut in_fence = false;
    let mut first = true;
    for line in doc_text.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        let stripped = line.trim_start();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        if is_reference_definition(line) {
            out.push_str(line);
            continue;
        }
        rewrite_line_links(line, &mut out);
    }
    out
}
/// True if `line` is a Markdown link-reference definition
/// (`[label]: <target>` at the start of the line, ignoring leading whitespace).
fn is_reference_definition(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix('[') else {
        return false;
    };
    let Some(close) = rest.find(']') else {
        return false;
    };
    rest[close + 1..].starts_with(':')
}
/// Apply per-link rewrites to one prose `line`, appending to `out`.
///
/// Iterates the line scanning for `[`. Code-span backticks are tracked so
/// `[Type]` inside `` `code` `` is left verbatim. Each candidate link is
/// classified into [`LinkShape`] and rewritten or skipped accordingly.
///
/// Walks `char_indices` so multi-byte UTF-8 sequences round-trip intact.
/// The bracket / paren matchers operate on ASCII bytes only (`[`, `]`,
/// `(`, `)`, backslash), so the byte indices they return are always char
/// boundaries.
fn rewrite_line_links(line: &str, out: &mut String) {
    let mut chars = line.char_indices().peekable();
    let mut in_code_span = false;
    while let Some(&(i, ch)) = chars.peek() {
        if ch == '`' {
            in_code_span = !in_code_span;
            out.push('`');
            chars.next();
            continue;
        }
        if in_code_span || ch != '[' {
            out.push(ch);
            chars.next();
            continue;
        }
        if let Some((shape, consumed)) = parse_link_at(line, i) {
            emit_link(out, &shape);
            while let Some(&(j, _)) = chars.peek() {
                if j >= i + consumed {
                    break;
                }
                chars.next();
            }
        } else {
            out.push('[');
            chars.next();
        }
    }
}
/// Markdown link shapes recognised (and possibly rewritten) by
/// [`rewrite_rustdoc_link_idioms`]. Source spans are preserved verbatim
/// in the `*_src` fields so unrecognised cases can be re-emitted unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkShape {
    /// `[label](target)` — explicit inline link.
    Inline {
        label_src: String,
        target_src: String,
    },
    /// `[label][ref]` — reference-style link. Always preserved verbatim.
    Reference { raw: String },
    /// `[label]` — shortcut reference / candidate intra-doc link.
    Shortcut { label_src: String },
}
/// Parse a link starting at byte offset `start` (which must be `[`).
/// Returns `Some((shape, bytes_consumed))` if a complete link was parsed,
/// `None` if the `[` is not part of a recognisable link (e.g. unmatched).
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
/// Find the matching `]` for the `[` at `open`. Backslash-escaped
/// brackets are skipped. Nested `[` / `]` inside an item link label
/// is uncommon; treat the first unescaped `]` as the close. Returns
/// `None` if no close is found on `line`.
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
/// Find the matching `)` for the `(` at `open`, respecting backslash
/// escapes. Returns `None` if no close is found on `line`.
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
/// Decide how to re-emit a parsed link.
fn emit_link(out: &mut String, shape: &LinkShape) {
    match shape {
        LinkShape::Reference { raw } => {
            out.push_str(raw);
        }
        LinkShape::Inline {
            label_src,
            target_src,
        } => emit_inline_link(out, label_src, target_src),
        LinkShape::Shortcut { label_src } => emit_shortcut_link(out, label_src),
    }
}
/// Re-emit `[label](target)`, possibly with idiom rewrites.
fn emit_inline_link(out: &mut String, label_src: &str, target_src: &str) {
    let target_trim = target_src.trim();
    if target_trim.is_empty() || !is_safe_intra_doc_target(target_trim) {
        write_inline(out, label_src, target_src);
        return;
    }
    if label_src == target_trim && is_codeish_token(label_src) {
        write_shortcut_ticked(out, label_src);
        return;
    }
    if is_codeish_token(label_src) && !label_src_has_backticks(label_src) {
        write_inline_label_ticked(out, label_src, target_src);
        return;
    }
    write_inline(out, label_src, target_src);
}
/// Re-emit `[label]`, possibly ticking when the label is code-ish.
fn emit_shortcut_link(out: &mut String, label_src: &str) {
    if is_codeish_token(label_src) && !label_src_has_backticks(label_src) {
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
/// True if the label already contains literal backticks. Such labels
/// are left verbatim — the user has already chosen their wrapping.
fn label_src_has_backticks(label: &str) -> bool {
    label.contains('`')
}
/// True if `target` is a safe intra-doc-link target for mechanical
/// rewrite: no URL scheme, no fragment, no generic / disambiguator /
/// argument syntax. Pure paths of identifiers separated by `::`,
/// optionally prefixed with `crate`, `self`, `super`, or `Self`.
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
/// True if `s` is a single code-ish Rust item token:
/// `CamelCase`, `snake_case` identifier, path-with-`::`,
/// or one of `Self` / `self` / `super` / `crate`.
#[must_use]
pub fn is_codeish_token(s: &str) -> bool {
    is_codeish_path(s)
}
/// True if `s` is a syntactically plausible Rust path:
/// `::`-separated segments, each segment a non-empty ident
/// (`[A-Za-z_][A-Za-z0-9_]*`). Leading `::` is permitted.
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
    use super::{is_codeish_token, rewrite_rustdoc_link_idioms};
    #[test]
    fn multibyte_utf8_survives_rewrite_pure() {
        let input = "see [Type] — also русский and 🦀";
        let out = super::rewrite_rustdoc_link_idioms(input);
        assert_eq!(out, "see [`Type`] — also русский and 🦀");
    }
    #[test]
    fn em_dash_survives_litstr_new_via_set_payload() {
        use syn::Attribute;
        use syn::parse_quote;
        let mut a: Attribute = parse_quote!(#[doc = " starting payload"]);
        super::set_doc_string_payload(&mut a, " hello — world");
        let payload = super::doc_string_payload(&a).unwrap();
        assert!(
            payload.contains('—'),
            "em-dash lost via set_doc_string_payload: payload={payload:?}"
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
    fn is_codeish_token_basic() {
        assert!(is_codeish_token("Type"));
        assert!(is_codeish_token("foo_bar"));
        assert!(is_codeish_token("foo::Bar"));
        assert!(is_codeish_token("Self"));
        assert!(is_codeish_token("self"));
        assert!(is_codeish_token("super::Foo"));
        assert!(is_codeish_token("crate::Reader"));
        assert!(is_codeish_token("::foo::Bar"));
        assert!(!is_codeish_token(""));
        assert!(!is_codeish_token("two words"));
        assert!(!is_codeish_token("foo()"));
        assert!(!is_codeish_token("Vec<u8>"));
        assert!(!is_codeish_token("foo!"));
        assert!(!is_codeish_token("_"));
        assert!(!is_codeish_token("9bad"));
        assert!(!is_codeish_token("foo:bar"));
        assert!(!is_codeish_token("foo::"));
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
            ..RewriteCounts::default()
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
        let line = rewrite_summary_record(RewriteMode::DryRun, &RewriteCounts::default());
        assert!(line.contains("\"mode\":\"dry-run\""), "{line}");
    }
    #[test]
    fn both_record_versions_are_two() {
        assert_eq!(DOC_LINT_RECORD_VERSION, 2);
        assert_eq!(REWRITE_RECORD_VERSION, 2);
    }
}

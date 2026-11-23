//! Pure logic for the `comment-free` tool: parse, re-emit, lint doc-comment budget.
#![forbid(unsafe_code)]
#![allow(clippy::wildcard_enum_match_arm)]
#![warn(clippy::missing_const_for_fn)]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use syn::File;
mod atomic;
mod cfg;
mod doc;
mod markdown;
mod record;
mod splice;
mod strip;
mod walk;
use atomic::write_atomically;
pub use doc::{
    DocBudget, DocFinding, DocLintKind, DocLintOutcome, DocLintReport, DocUndecided,
    UndecidedCause, WordCount, doc_budget_source_file, doc_lint_file,
};
#[allow(deprecated)]
pub use record::lint_summary_record;
pub use record::{
    DIAGNOSTIC_RECORD_GRAMMAR, DIAGNOSTIC_RECORD_VERSION, DOC_LINT_RECORD_GRAMMAR,
    DOC_LINT_RECORD_VERSION, REWRITE_RECORD_GRAMMAR, REWRITE_RECORD_VERSION, RunErrorKind,
    doc_file_warning_record, doc_lint_finding_record, doc_lint_header_record, doc_lint_hint_record,
    doc_lint_truncated_record, doc_lint_undecided_record, rewrite_file_record,
    rewrite_summary_record, run_error_record, strip_summary_record,
};
use record::{open_record, push_number, push_text};
pub use splice::rewrite_rustdoc_link_idioms;
use splice::{apply_splices, collect_doc_splices};
pub(crate) use strip::unified_diff;
pub use strip::{single_line, strip_line_comments, strip_line_comments_with_counts};
pub use walk::{
    DirectorySelection, DocScan, plan_cli_directory, scan_doc_files, walk_cli_directory,
    walk_rs_files,
};
/// Doctrine warning carried by the `doc_lint_header` record for every kind.
pub const DOC_LINT_DOCTRINE_MSG: &str = "Rust docs must contain a concise summary, optionally clear code examples (fenced ``` or ~~~ blocks), and sections explaining edge cases like panics, errors, and safety. Code is excluded from the prose word length: fenced blocks, indented code blocks, and inline code spans. If applicable references to ADRs must be given.";
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
pub enum CommentFreeError {
    /// ROOT path passed on the CLI is not a directory.
    #[error("ROOT is not a directory")]
    NotADirectory,
    /// ROOT is neither a directory nor a regular, non-symlink `.rs` file.
    #[error("ROOT must be a directory or a regular .rs file; leaf file symlinks are refused")]
    InvalidRoot,
    /// An exact run total exceeded the representable counter range.
    #[error("run counter overflow; summary is incomplete")]
    CounterOverflow,
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
    fn at(path: &Path, message: &str) -> Self {
        Self {
            path: path.to_path_buf(),
            source: TraversalFailure(message.to_owned()),
        }
    }
}
impl From<&CommentFreeError> for ExitCode {
    fn from(e: &CommentFreeError) -> Self {
        match e {
            CommentFreeError::NotADirectory | CommentFreeError::InvalidRoot => Self::from(2),
            CommentFreeError::DocLintFailure => Self::from(4),
            CommentFreeError::Io(_) | CommentFreeError::CounterOverflow => Self::from(1),
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
/// Selected traversal policy carried by a v3 lint summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportScope {
    /// One explicitly supplied Rust file.
    File,
    /// Cargo source allowlist at the selected root.
    ProjectAllowlist,
    /// Recursive Rust traversal below the selected directory, including cwd.
    RecursiveDirectory,
    /// The manifest probe could not establish a traversal policy.
    Unresolved,
}
impl ReportScope {
    /// Canonical schema spelling of this policy.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::ProjectAllowlist => "project-allowlist",
            Self::RecursiveDirectory => "recursive-directory",
            Self::Unresolved => "unresolved",
        }
    }
}

/// Maximum warning files admitted for details; zero is summary-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningLimit {
    /// Admit at most this many warning files.
    Limited(usize),
    /// Admit every warning file.
    Unlimited,
}
impl std::str::FromStr for WarningLimit {
    type Err = &'static str;

    /// Parse ASCII decimal digits or exactly `unlimited`.
    ///
    /// # Errors
    /// Rejects empty input, non-ASCII digits, signs, whitespace, other spellings,
    /// and decimal values exceeding [`usize::MAX`].
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "unlimited" => Ok(Self::Unlimited),
            digits if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => digits
                .parse()
                .map(Self::Limited)
                .map_err(|_| "warning limit overflow"),
            _ => Err("expected ASCII decimal digits or unlimited"),
        }
    }
}
impl WarningLimit {
    /// Whether another warning file can be admitted after `shown` files.
    #[must_use]
    pub const fn admits(self, shown: u32) -> bool {
        match self {
            Self::Limited(limit) => (shown as usize) < limit,
            Self::Unlimited => true,
        }
    }
}
impl std::fmt::Display for WarningLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Limited(limit) => write!(f, "{limit}"),
            Self::Unlimited => f.write_str("unlimited"),
        }
    }
}

/// Exact corpus and admitted-detail counts for one synchronous lint run.
#[derive(Default, Clone)]
pub struct LintTotals {
    files: u32,
    errors: u32,
    warning_files: u32,
    warning_files_shown: u32,
    findings: u32,
    findings_shown: u32,
    undecided: u32,
    undecided_shown: u32,
    outcomes: std::collections::BTreeMap<&'static str, u32>,
}
fn add_exact(counter: &mut u32, amount: usize) -> Result<(), CommentFreeError> {
    *counter = counter
        .checked_add(u32::try_from(amount).map_err(|_| CommentFreeError::CounterOverflow)?)
        .ok_or(CommentFreeError::CounterOverflow)?;
    Ok(())
}
impl LintTotals {
    /// Count one enumerated source file.
    /// # Errors
    /// Returns [`CommentFreeError::CounterOverflow`] when the total overflows.
    pub fn file(&mut self) -> Result<(), CommentFreeError> {
        add_exact(&mut self.files, 1)
    }
    /// Count one visible processing or traversal error.
    /// # Errors
    /// Returns [`CommentFreeError::CounterOverflow`] when the total overflows.
    pub fn error(&mut self) -> Result<(), CommentFreeError> {
        add_exact(&mut self.errors, 1)
    }
    /// Accumulate a report, admitting all of its warnings together when selected.
    /// # Errors
    /// Returns [`CommentFreeError::CounterOverflow`] on any counter overflow,
    /// leaving the accumulator unchanged.
    pub fn observe(
        &mut self,
        report: &DocLintReport,
        admitted: bool,
    ) -> Result<(), CommentFreeError> {
        let mut next = self.clone();
        next.accumulate(report, admitted)?;
        *self = next;
        Ok(())
    }
    fn accumulate(
        &mut self,
        report: &DocLintReport,
        admitted: bool,
    ) -> Result<(), CommentFreeError> {
        let findings = report.findings().len();
        let undecided = report.undecided().len();
        if findings != 0 || undecided != 0 {
            add_exact(&mut self.warning_files, 1)?;
            if admitted {
                add_exact(&mut self.warning_files_shown, 1)?;
            }
        }
        add_exact(&mut self.findings, findings)?;
        add_exact(&mut self.undecided, undecided)?;
        if admitted {
            add_exact(&mut self.findings_shown, findings)?;
            add_exact(&mut self.undecided_shown, undecided)?;
        }
        add_exact(self.outcomes.entry("over_budget").or_default(), findings)?;
        for item in report.undecided() {
            add_exact(self.outcomes.entry(item.outcome().as_str()).or_default(), 1)?;
        }
        Ok(())
    }
    /// Warning files admitted for details so far.
    #[must_use]
    pub const fn warning_files_shown(&self) -> u32 {
        self.warning_files_shown
    }
    /// All warning files, including those hidden from details.
    #[must_use]
    pub const fn warning_files(&self) -> u32 {
        self.warning_files
    }
    /// All observed errors.
    #[must_use]
    pub const fn errors(&self) -> u32 {
        self.errors
    }
    /// Findings admitted for details and eligible for hints.
    #[must_use]
    pub const fn findings_shown(&self) -> u32 {
        self.findings_shown
    }
    /// Render a v3 summary; limit is normalized decimal or `unlimited`.
    ///
    /// Raw strings cannot construct reporting metadata.
    ///
    /// ```compile_fail,E0308
    /// use comment_free::LintTotals;
    /// use std::path::Path;
    /// LintTotals::default().record(Path::new("."), "bogus", "-1");
    /// ```
    ///
    /// ```compile_fail,E0308
    /// use comment_free::{LintTotals, WarningLimit};
    /// use std::path::Path;
    /// LintTotals::default().record(Path::new("."), "bogus", WarningLimit::Limited(0));
    /// ```
    ///
    /// ```compile_fail,E0308
    /// use comment_free::{LintTotals, ReportScope};
    /// use std::path::Path;
    /// LintTotals::default().record(Path::new("."), ReportScope::File, "-1");
    /// ```
    #[must_use]
    pub fn record(&self, root: &Path, scope: ReportScope, limit: WarningLimit) -> String {
        let mut out = open_record("lint_summary", DIAGNOSTIC_RECORD_VERSION);
        for (key, value) in [
            ("root", root.display().to_string()),
            ("scope", scope.as_str().to_owned()),
            ("max_warning_files", limit.to_string()),
        ] {
            out.push(',');
            push_text(&mut out, key, &value);
        }
        for (key, value) in [
            ("files", self.files),
            ("errors", self.errors),
            ("warning_files", self.warning_files),
            ("warning_files_shown", self.warning_files_shown),
            (
                "warning_files_hidden",
                self.warning_files - self.warning_files_shown,
            ),
            ("findings", self.findings),
            ("findings_shown", self.findings_shown),
            ("findings_hidden", self.findings - self.findings_shown),
            ("undecided", self.undecided),
            ("undecided_shown", self.undecided_shown),
            ("undecided_hidden", self.undecided - self.undecided_shown),
            ("overlong_doc_findings", self.findings),
            ("overlong_doc_undecided", self.undecided),
        ] {
            out.push(',');
            push_number(&mut out, key, value);
        }
        for name in [
            "over_budget",
            "configuration_dependent",
            "unreadable_doc_payload",
            "uninspected_macro_body",
        ] {
            out.push(',');
            push_number(
                &mut out,
                name,
                self.outcomes.get(name).copied().unwrap_or(0),
            );
        }
        out.push('}');
        out
    }
}

/// Canonicalise supported rustdoc links, then strip non-doc comments from `path`.
/// Other bytes survive except adjacent-whitespace normalization by
/// [`strip_line_comments_with_counts`]. [`RewriteMode::DryRun`] previews without writing.
///
/// On supported Unix platforms, writes rename a sibling temporary file over `path`.
/// Cleanup is best-effort on errors/unwinding; aborts, signals, or removal failures
/// can leave residue. The byte check is not locking; hard-link aliases retain old
/// contents; the parent directory is not synced.
///
/// # Errors
///
/// Returns [`FileOutcome::Failed`] for read/parse/write errors, rejected symlink
/// destinations, or pre-rename byte conflicts; see [`FileError`].
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

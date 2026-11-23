use crate::{
    DOC_LINT_DOCTRINE_MSG, DocLintKind, DocLintOutcome, DocUndecided, RewriteCounts, RewriteMode,
    UndecidedCause,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
/// Version carried by the `v` field of every `doc_lint_*` record.
///
/// Consumers reject records whose `v` exceeds the version they
/// understand. See `docs/record-format.md` for the record grammar and
/// the compatibility rules that let new fields, kinds, and outcomes
/// arrive without a bump.
pub const DOC_LINT_RECORD_VERSION: u32 = 3;
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
pub const DIAGNOSTIC_RECORD_VERSION: u32 = 3;
/// One-line JSON templates for the run-diagnostic record family, for the
/// binary's `--help` output.
///
/// Authoritative grammar: `docs/record-format.md`.
pub const DIAGNOSTIC_RECORD_GRAMMAR: &str = "\
{\"record\":\"run_error\",\"v\":<N>,\"kind\":<ERROR_KIND>,\"path\":<PATH>,\"message\":<STRING>}
{\"record\":\"doc_file_warning\",\"v\":<N>,\"path\":<PATH>}
{\"record\":\"rewrite_file\",\"v\":<N>,\"mode\":<MODE>,\"path\":<PATH>}
{\"record\":\"strip_summary\",\"v\":<N>,\"mode\":<MODE>,\"rewritten\":<U32>,\"unchanged\":<U32>,\"errors\":<U32>}
{\"record\":\"lint_summary\",\"v\":<N>,\"root\":<PATH>,\"scope\":<SCOPE>,\"max_warning_files\":<STRING>,\"files\":<U32>,\"errors\":<U32>,\"warning_files\":<U32>,\"warning_files_shown\":<U32>,\"warning_files_hidden\":<U32>,\"findings\":<U32>,\"findings_shown\":<U32>,\"findings_hidden\":<U32>,\"undecided\":<U32>,\"undecided_shown\":<U32>,\"undecided_hidden\":<U32>,\"overlong_doc_findings\":<U32>,\"overlong_doc_undecided\":<U32>,\"over_budget\":<U32>,\"configuration_dependent\":<U32>,\"unreadable_doc_payload\":<U32>,\"uninspected_macro_body\":<U32>}";
/// Counts a `lint_summary` record reports, with the run metadata it was
/// selected under.
pub(crate) struct LintSummaryPayload<'a> {
    pub(crate) root: &'a Path,
    pub(crate) scope: &'a str,
    pub(crate) limit: &'a str,
    pub(crate) files: u32,
    pub(crate) errors: u32,
    pub(crate) warning_files: u32,
    pub(crate) warning_files_shown: u32,
    pub(crate) findings: u32,
    pub(crate) findings_shown: u32,
    pub(crate) undecided: u32,
    pub(crate) undecided_shown: u32,
    pub(crate) outcomes: &'a BTreeMap<&'static str, u32>,
}
/// The v3 `lint_summary` record, keyed in the order
/// [`DIAGNOSTIC_RECORD_GRAMMAR`] declares.
#[must_use]
pub(crate) fn lint_summary_v3_record(payload: &LintSummaryPayload<'_>) -> String {
    let mut out = open_record("lint_summary", DIAGNOSTIC_RECORD_VERSION);
    out.push(',');
    push_text(&mut out, "root", &payload.root.display().to_string());
    out.push(',');
    push_text(&mut out, "scope", payload.scope);
    out.push(',');
    push_text(&mut out, "max_warning_files", payload.limit);
    for (key, value) in [
        ("files", payload.files),
        ("errors", payload.errors),
        ("warning_files", payload.warning_files),
        ("warning_files_shown", payload.warning_files_shown),
        (
            "warning_files_hidden",
            payload.warning_files - payload.warning_files_shown,
        ),
        ("findings", payload.findings),
        ("findings_shown", payload.findings_shown),
        ("findings_hidden", payload.findings - payload.findings_shown),
        ("undecided", payload.undecided),
        ("undecided_shown", payload.undecided_shown),
        (
            "undecided_hidden",
            payload.undecided - payload.undecided_shown,
        ),
        ("overlong_doc_findings", payload.findings),
        ("overlong_doc_undecided", payload.undecided),
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
            payload.outcomes.get(name).copied().unwrap_or(0),
        );
    }
    out.push('}');
    out
}

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
pub(crate) fn push_text(out: &mut String, key: &str, value: &str) {
    push_json_string(out, key);
    out.push(':');
    push_json_string(out, value);
}
pub(crate) fn push_number(out: &mut String, key: &str, value: u32) {
    push_json_string(out, key);
    write!(out, ":{value}").expect("Write for String never fails");
}
fn push_count(out: &mut String, key: &str, value: usize) {
    push_number(out, key, u32::try_from(value).unwrap_or(u32::MAX));
}
pub(crate) fn open_record(kind: &str, version: u32) -> String {
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
/// Legacy v2 summary, retained for callers that do not supply v3 scope and totals.
/// New callers use [`LintTotals::record`](crate::LintTotals::record); this helper never emits a v3 record.
#[deprecated(
    since = "0.2.0",
    note = "use LintTotals::record for the complete v3 summary"
)]
#[must_use]
pub fn lint_summary_record(files: u32, findings: u32, undecided: u32, errors: u32) -> String {
    let mut out = open_record("lint_summary", 2);
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

#[cfg(test)]
mod record_tests {
    #[test]
    fn lint_counter_overflow_preserves_prior_total() {
        let mut counter = u32::MAX;
        assert!(matches!(
            add_exact(&mut counter, 1),
            Err(CommentFreeError::CounterOverflow)
        ));
        assert_eq!(counter, u32::MAX);
    }

    use super::{
        DOC_LINT_RECORD_VERSION, DocLintKind, REWRITE_RECORD_VERSION, RewriteCounts, RewriteMode,
        doc_lint_finding_record, doc_lint_header_record, doc_lint_hint_record,
        doc_lint_truncated_record, rewrite_summary_record,
    };
    use crate::{CommentFreeError, add_exact, unified_diff};
    use std::path::Path;
    fn hint(path: &str, item: &str) -> String {
        doc_lint_hint_record(DocLintKind::OverlongDoc, Path::new(path), 12, item, 100, 80)
    }
    #[test]
    fn hint_record_carries_record_name_version_and_outcome() {
        let line = hint("src/lib.rs", "fn f");
        assert!(
            line.starts_with("{\"record\":\"doc_lint_hint\",\"v\":3,"),
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
    fn record_family_versions_are_independent() {
        assert_eq!(DOC_LINT_RECORD_VERSION, 3);
        assert_eq!(REWRITE_RECORD_VERSION, 2);
    }
}

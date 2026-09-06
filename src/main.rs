#![forbid(unsafe_code)]
#![warn(clippy::missing_const_for_fn)]
use clap::Parser;
use comment_free::{
    CommentFreeError, DirectorySelection, DocBudget, DocLintKind, FileOutcome, ReportScope,
    RewriteCounts, RewriteMode, RunErrorKind, WarningLimit, doc_file_warning_record, doc_lint_file,
    doc_lint_finding_record, doc_lint_header_record, doc_lint_hint_record,
    doc_lint_truncated_record, doc_lint_undecided_record, plan_cli_directory, process_file,
    rewrite_file_record, rewrite_summary_record, run_error_record, scan_doc_files,
    strip_summary_record,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
#[derive(Parser, Debug)]
#[command(
    name = "comment-free",
    version,
    about = "Doc-comment linter and byte-preserving rustdoc-link rewriter for Rust crates. \
             Default mode lints doc-comment word budget. \
             `--rewrite` strips every non-doc `//` and `/* */` comment via the rustc lexer (doc comments are preserved) and canonicalises Rust intra-doc-link idioms in doc-comment payloads. Both passes are byte-preserving outside their targets.",
    long_about = "Default mode is a read-only doc-comment budget linter: walks ROOT for .rs files \
                  and reports doc comments whose prose word count exceeds --doc-max-words. \
                  Fenced code blocks (` ``` ` or `~~~`) are excluded from the count and do \
                  not consume the word budget. Examples are detected mechanically by fence \
                  delimiters only — there is no semantic example detection, and no limit on \
                  the number of fenced blocks is enforced. Doc comments are NEVER deleted by \
                  this tool. Warning details default to the first warning file in native path order.\n\
                  --max-warning-files N|unlimited changes details only; 0 emits no stdout lint records.\n\
                  All scoped files are scanned, including hidden findings and undecided items; errors\n\
                  remain visible. Summaries contain full and shown/hidden totals. Doc-lint and\n\
                  diagnostic records are v3; rewrite_summary remains v2.\n\
                  Scope in ALL modes: omitted ROOT uses cwd project allowlist; a supplied directory\n\
                  with its own Cargo.toml uses that allowlist (benches, crates, examples, src, tests,\n\
                  build.rs); other explicit directories recurse with build/hidden pruning. An exact\n\
                  regular .rs file selects only that file. No upward discovery. Leaf file symlinks\n\
                  are rejected. Rewrite processes the entire selected scope and accepts no warning cap.\n\
                  \n\
                  Lint findings, run diagnostics and summaries are all emitted as JSON\n\
                  Lines, one record per line, for LLM-agent consumption. The authoritative\n\
                  grammar and the compatibility rules live in docs/record-format.md;\n\
                  one-line templates are published as\n\
                  `comment_free::DOC_LINT_RECORD_GRAMMAR`,\n\
                  `comment_free::REWRITE_RECORD_GRAMMAR` and\n\
                  `comment_free::DIAGNOSTIC_RECORD_GRAMMAR`, with the record-format versions\n\
                  as the matching `*_RECORD_VERSION` constants.\n\
                  \n\
                    doc_lint_finding   one per finding: path, line, item, words, budget\n\
                    doc_lint_header    one per finding kind, names the doctrine once\n\
                    doc_lint_hint      up to 50 per kind, sorted by overshoot descending\n\
                    doc_lint_truncated tail summary when a kind has > 50 findings\n\
                    doc_lint_undecided one per item the linter could not decide\n\
                    run_error          one per failed path: kind, path, message\n\
                    doc_file_warning   one per documentation file left untouched\n\
                    rewrite_file       one per changed file: mode, path\n\
                    strip_summary      one per --rewrite run\n\
                    lint_summary       one per lint run\n\
                  \n\
                  Rewrite mode (`--rewrite`):\n\
                  \n\
                  Two passes run in sequence, both byte-preserving outside their targets:\n\
                  \n\
                    1. Doc-link idiom canonicalisation: mutates ONLY `///`, `//!`, \
                       `#[doc = \"...\"]`, `#![doc = \"...\"]`, and `#[cfg_attr(_, doc = \"...\")]` \
                       payload text to canonical rustdoc link form ([Type](Type) -> [`Type`], etc.).\n\
                    2. Non-doc comment strip: removes every `//` line comment and `/* */` \
                       block comment via the rustc lexer. Doc comments are kept; nothing else \
                       is. String literals are structurally unreachable by the strip pass — \
                       comment-looking text inside any string round-trips byte-identical.\n\
                  \n\
                  `--dry-run` is always safe; it emits unified diffs to stdout without writing files.\n\
                  \n\
                  The `--rustdoc-link-idioms` flag is a deprecated alias retained for one release; \
                  it dispatches the same default `--rewrite` behaviour and emits a deprecation note \
                  on stderr.\n\
                  \n\
                  Exit codes:\n\
                    0  clean: every doc payload under ROOT was read and every\n\
                       one of them was decided against the budget\n\
                    1  catastrophic / unmapped IO error or exact-counter overflow\n\
                    2  invalid CLI arguments (clap rejection)\n\
                    3  `--rewrite --dry-run` previewed at least one pending\n\
                       change, so the tree is not already comment-free. Write\n\
                       mode is not a check and exits 0 whatever it rewrote;\n\
                       exit 5 outranks exit 3\n\
                    4  the tree did not come back clean in default mode: at\n\
                       least one doc-lint finding, or at least one undecided\n\
                       item. An undecided item is NOT a finding — read\n\
                       `findings` and `undecided` in `lint_summary` to tell\n\
                       them apart. A run that could not see everything does\n\
                       not report exit 0, because that code means clean\n\
                    5  per-file parse/IO errors, or directory-traversal errors,\n\
                       observed during processing (both modes); each is reported\n\
                       as a `run_error` record naming its kind\n\
                  \n\
                  Known limitation — doc payloads this tool cannot read:\n\
                  \n\
                  A doc payload written as a macro call rather than a string\n\
                  literal — `#[doc = include_str!(\"x.md\")]`,\n\
                  `#[doc = concat!(...)]`, the same inside `cfg_attr` — resolves\n\
                  only by macro expansion, which this tool does not perform. Such\n\
                  an item is reported as a `doc_lint_undecided` record with\n\
                  `outcome` `unreadable_doc_payload` and is never counted clean.\n\
                  \n\
                  A doc attribute inside a macro token body — a `macro_rules!`\n\
                  definition, or tokens passed to an invocation — is reported the\n\
                  same way, with `outcome` `uninspected_macro_body`, naming the\n\
                  file and the macro. Macro bodies carrying no doc attribute are\n\
                  not reported. Doc text synthesised by a procedural macro from\n\
                  tokens that never spell `doc` is not detected at all.\n\
                  \n\
                  Output streams: findings (doc_lint_* records, rewrite_file records, \
                  diffs) on stdout; metadata (strip_summary, lint_summary, rewrite_summary, \
                  doc_file_warning, run_error) on stderr. Every line except the --dry-run \
                  unified diff body is a JSON Lines record; the diff body is human-facing \
                  plain text and is never machine-parsed."
)]
struct Options {
    #[arg(
        value_name = "ROOT",
        help = "Directory scope or one regular .rs file; defaults to cwd, without upward discovery"
    )]
    root: Option<PathBuf>,
    #[arg(
        long,
        help = "Run the byte-preserving rewrite passes over every `.rs` file under ROOT: \
                canonicalise rustdoc-link idioms in doc payloads, then strip every non-doc \
                `//` and `/* */` comment via the rustc lexer. Doc comments are preserved; \
                nothing else is"
    )]
    rewrite: bool,
    #[arg(
        long,
        short = 'n',
        requires = "rewrite",
        help = "Preview the rewrite as a unified diff without modifying files, and exit 3 if \
                any file would change. Only meaningful with `--rewrite`. Default (lint) mode \
                is already read-only; `--dry-run` is meaningful only with `--rewrite` \
                (enforced by clap)"
    )]
    dry_run: bool,
    #[arg(
        long,
        default_value_t = 3,
        value_name = "N",
        requires = "rewrite",
        help = "Unified-diff context line count (used with `--dry-run`). Only meaningful with \
                `--rewrite`"
    )]
    context: usize,
    #[arg(
        long,
        default_value_t = 80,
        value_name = "N",
        help = "Word budget for doc-comment prose. Fenced code blocks (` ``` ` or `~~~`) are \
                excluded from the count and do not consume the budget"
    )]
    doc_max_words: usize,
    #[arg(long, value_name = "N|unlimited", conflicts_with_all = ["rewrite", "rustdoc_link_idioms"], help = "Warning files with details (default 1); 0 is summary-only; all files still scanned")]
    max_warning_files: Option<WarningLimit>,
    #[arg(
        long,
        requires = "rewrite",
        help = "DEPRECATED alias for plain `--rewrite`. Retained for one release. Dispatches \
                the same byte-preserving rewrite path `--rewrite` runs by default; emits a \
                deprecation note on stderr"
    )]
    rustdoc_link_idioms: bool,
}
enum ArgvRejection {
    Renderable(Box<clap::Error>),
    ControlBearing,
}

fn argv_has_control_bytes() -> bool {
    std::env::args_os().any(|a| a.as_encoded_bytes().iter().any(|&b| b < 0x20 || b == 0x7f))
}

fn parse_options() -> Result<Options, ArgvRejection> {
    Options::try_parse().map_err(|e| {
        if argv_has_control_bytes() {
            ArgvRejection::ControlBearing
        } else {
            ArgvRejection::Renderable(Box::new(e))
        }
    })
}

fn report_argv_rejection(rejection: &ArgvRejection) -> ExitCode {
    match rejection {
        ArgvRejection::Renderable(e) => {
            let _ = e.print();
            if e.exit_code() == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            }
        }
        ArgvRejection::ControlBearing => {
            eprintln!(
                "error: invalid arguments; an argument contains control characters and \
                 is not reproduced here. Re-run with --help for usage."
            );
            ExitCode::from(2)
        }
    }
}

type ScopedFiles<'a> = (
    ReportScope,
    Box<dyn Iterator<Item = Result<PathBuf, comment_free::WalkError>> + 'a>,
);
enum InputScope {
    Directory {
        path: PathBuf,
        selection: DirectorySelection,
    },
    RustFile(PathBuf),
}
impl InputScope {
    fn from_path(root: Option<PathBuf>) -> Result<Self, CommentFreeError> {
        let (path, selection) = match root {
            Some(path) => (path, DirectorySelection::Explicit),
            None => (PathBuf::from("."), DirectorySelection::DefaultCwd),
        };
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| CommentFreeError::InvalidRoot)?;
        match metadata.file_type() {
            kind if kind.is_dir() => Ok(Self::Directory { path, selection }),
            kind if kind.is_symlink() && path.is_dir() => Ok(Self::Directory { path, selection }),
            kind if kind.is_file() && path.extension().is_some_and(|ext| ext == "rs") => {
                Ok(Self::RustFile(path))
            }
            _ => Err(CommentFreeError::InvalidRoot),
        }
    }

    fn path(&self) -> &Path {
        match self {
            Self::Directory { path, .. } | Self::RustFile(path) => path,
        }
    }

    fn files(&self) -> ScopedFiles<'_> {
        match self {
            Self::Directory { path, selection } => {
                let (policy, files) = plan_cli_directory(path, *selection);
                (policy, Box::new(files))
            }
            Self::RustFile(path) => (
                ReportScope::File,
                Box::new(std::iter::once(Ok(path.clone()))),
            ),
        }
    }
}
enum Command {
    Lint {
        root: InputScope,
        budget: DocBudget,
        limit: WarningLimit,
    },
    Rewrite {
        root: InputScope,
        mode: RewriteMode,
    },
}
impl Command {
    fn from_options(opts: Options) -> Result<Self, CommentFreeError> {
        let root = InputScope::from_path(opts.root)?;
        Ok(if opts.rewrite {
            Self::Rewrite {
                root,
                mode: if opts.dry_run {
                    RewriteMode::DryRun {
                        context: opts.context,
                    }
                } else {
                    RewriteMode::Write
                },
            }
        } else {
            Self::Lint {
                root,
                budget: DocBudget {
                    max_words: opts.doc_max_words,
                },
                limit: opts.max_warning_files.unwrap_or(WarningLimit::Limited(1)),
            }
        })
    }
}

fn main() -> ExitCode {
    let opts = match parse_options() {
        Ok(o) => o,
        Err(rejection) => return report_argv_rejection(&rejection),
    };
    match dispatch(opts) {
        Ok(verdict) => ExitCode::from(verdict),
        Err(e) => {
            eprintln!("error: {}", comment_free::single_line(&e.to_string()));
            ExitCode::from(&e)
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunVerdict {
    Clean,
    PendingRewrite,
    Failed,
}
impl From<RunVerdict> for ExitCode {
    fn from(verdict: RunVerdict) -> Self {
        match verdict {
            RunVerdict::Clean => Self::SUCCESS,
            RunVerdict::PendingRewrite => Self::from(3),
            RunVerdict::Failed => Self::from(5),
        }
    }
}
fn dispatch(opts: Options) -> Result<RunVerdict, CommentFreeError> {
    let deprecated_alias = opts.rustdoc_link_idioms;
    let command = Command::from_options(opts)?;
    if deprecated_alias {
        eprintln!(
            "warning: --rustdoc-link-idioms is deprecated; the default --rewrite path now \
             includes rustdoc-link idiom canonicalisation along with lexer-based comment \
             stripping. This flag is a no-op alias retained for one release."
        );
    }
    run(&command)
}
fn run(command: &Command) -> Result<RunVerdict, CommentFreeError> {
    match command {
        Command::Rewrite { root, mode } => Ok(run_strip(root, *mode)),
        Command::Lint {
            root,
            budget,
            limit,
        } => run_lint(root, *budget, *limit),
    }
}
fn run_strip(root: &InputScope, mode: RewriteMode) -> RunVerdict {
    let mut errors = 0u32;
    let doc_scan = scan_doc_files(root.path());
    for path in doc_scan.files() {
        eprintln!("{}", doc_file_warning_record(path));
    }
    for e in doc_scan.errors() {
        errors += 1;
        eprintln!(
            "{}",
            run_error_record(RunErrorKind::Walk, e.path(), &e.message())
        );
    }
    let mut rewritten = 0u32;
    let mut unchanged = 0u32;
    let mut counts_total = RewriteCounts::default();
    for walked in root.files().1 {
        let path = match walked {
            Ok(p) => p,
            Err(e) => {
                errors += 1;
                eprintln!(
                    "{}",
                    run_error_record(RunErrorKind::Walk, e.path(), &e.message())
                );
                continue;
            }
        };
        match process_file(&path, mode) {
            FileOutcome::Rewritten(summary) => {
                rewritten += 1;
                counts_total += summary.counts();
                println!("{}", rewrite_file_record(mode, &path));
            }
            FileOutcome::WouldRewrite(preview) => {
                rewritten += 1;
                counts_total += preview.counts();
                println!("{}", rewrite_file_record(mode, &path));
                print!("{}", preview.diff());
            }
            FileOutcome::Unchanged(summary) => {
                unchanged += 1;
                counts_total += summary.counts();
            }
            FileOutcome::Failed(e) => {
                errors += 1;
                eprintln!("{}", run_error_record(e.kind(), &path, &e.to_string()));
            }
        }
    }
    eprintln!(
        "{}",
        strip_summary_record(mode, rewritten, unchanged, errors)
    );
    eprintln!("{}", rewrite_summary_record(mode, &counts_total));
    strip_verdict(mode, rewritten, errors)
}
const fn strip_verdict(mode: RewriteMode, rewritten: u32, errors: u32) -> RunVerdict {
    match (errors, mode, rewritten) {
        (0, RewriteMode::DryRun { .. }, 1..) => RunVerdict::PendingRewrite,
        (0, _, _) => RunVerdict::Clean,
        (1.., _, _) => RunVerdict::Failed,
    }
}
const DOC_LINT_HINT_CAP: usize = 50;

fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort();
}

fn run_lint(
    root: &InputScope,
    budget: DocBudget,
    limit: WarningLimit,
) -> Result<RunVerdict, CommentFreeError> {
    let mut hints = Vec::with_capacity(DOC_LINT_HINT_CAP);
    let mut totals = comment_free::LintTotals::default();
    let mut paths = Vec::new();
    let (scope, files) = root.files();
    for walked in files {
        match walked {
            Ok(path) => paths.push(path),
            Err(e) => {
                totals.error()?;
                eprintln!(
                    "{}",
                    run_error_record(RunErrorKind::Walk, e.path(), &e.message())
                );
            }
        }
    }
    sort_paths(&mut paths);
    for path in paths {
        totals.file()?;
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                totals.error()?;
                eprintln!(
                    "{}",
                    run_error_record(RunErrorKind::Io, &path, &e.to_string())
                );
                continue;
            }
        };
        let ast = match syn::parse_file(&source) {
            Ok(f) => f,
            Err(e) => {
                totals.error()?;
                eprintln!(
                    "{}",
                    run_error_record(RunErrorKind::Parse, &path, &e.to_string())
                );
                continue;
            }
        };
        let report = doc_lint_file(&ast, budget);
        let admitted = limit.admits(totals.warning_files_shown());
        totals.observe(&report, admitted)?;
        if !admitted {
            continue;
        }
        for finding in report.findings() {
            println!(
                "{}",
                doc_lint_finding_record(
                    DocLintKind::OverlongDoc,
                    &path,
                    finding.line(),
                    finding.item_label(),
                    finding.words().count(),
                    finding.budget(),
                    finding.words().is_fail_closed()
                )
            );
            retain_hint(&mut hints, &path, finding);
        }
        for undecided in report.undecided() {
            println!(
                "{}",
                doc_lint_undecided_record(DocLintKind::OverlongDoc, &path, undecided)
            );
        }
    }
    emit_doc_lint_hints(&hints, totals.findings_shown());
    eprintln!("{}", totals.record(root.path(), scope, limit));
    match (totals.errors(), totals.warning_files()) {
        (1.., _) => Ok(RunVerdict::Failed),
        (0, 1..) => Err(CommentFreeError::DocLintFailure),
        (0, 0) => Ok(RunVerdict::Clean),
    }
}

fn retain_hint(
    hints: &mut Vec<(PathBuf, comment_free::DocFinding)>,
    path: &Path,
    finding: &comment_free::DocFinding,
) {
    let overshoot = finding.words().count().saturating_sub(finding.budget());
    let position = hints.partition_point(|(_, current)| {
        current.words().count().saturating_sub(current.budget()) >= overshoot
    });
    if position < DOC_LINT_HINT_CAP {
        if hints.len() == DOC_LINT_HINT_CAP {
            hints.pop();
        }
        hints.insert(position, (path.to_path_buf(), finding.clone()));
    }
}

fn emit_doc_lint_hints(findings: &[(std::path::PathBuf, comment_free::DocFinding)], total: u32) {
    if findings.is_empty() {
        return;
    }
    let kind = DocLintKind::OverlongDoc;
    println!("{}", doc_lint_header_record(kind));
    for (path, f) in findings {
        println!(
            "{}",
            doc_lint_hint_record(
                kind,
                path,
                f.line(),
                f.item_label(),
                f.words().count(),
                f.budget()
            )
        );
    }
    if total as usize > DOC_LINT_HINT_CAP {
        let remaining = total as usize - DOC_LINT_HINT_CAP;
        println!("{}", doc_lint_truncated_record(kind, remaining));
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;

    #[test]
    fn hint_high_water_is_fifty() {
        let ast = syn::parse_file("/// one two three\npub fn item() {}").unwrap();
        let report = doc_lint_file(&ast, DocBudget { max_words: 1 });
        let mut hints = Vec::with_capacity(DOC_LINT_HINT_CAP);
        let mut high_water = 0;
        for i in 0..1000 {
            retain_hint(&mut hints, Path::new("input.rs"), &report.findings()[0]);
            high_water = high_water.max(hints.len());
            assert!(hints.len() <= 50, "iteration {i}");
            assert_eq!(hints.capacity(), 50);
        }
        assert_eq!(high_water, 50);
        eprintln!(
            "hint_high_water_items={high_water}; capacity_items={}",
            hints.capacity()
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_sort_distinguishes_lossy_collisions_without_filesystem_support() {
        use std::os::unix::ffi::OsStringExt;
        let a = PathBuf::from(std::ffi::OsString::from_vec(vec![0xfe]));
        let b = PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
        assert_eq!(a.to_string_lossy(), b.to_string_lossy());
        let mut paths = [b.clone(), a.clone()];
        sort_paths(&mut paths);
        assert_eq!(paths, [a, b]);
    }
}

#![forbid(unsafe_code)]
#![warn(clippy::missing_const_for_fn)]
use clap::Parser;
use comment_free::{
    CommentFreeError, DocBudget, DocLintKind, FileOutcome, RewriteCounts, RewriteMode,
    RunErrorKind, SKIP_DIRS, WalkError, doc_file_warning_record, doc_lint_file,
    doc_lint_finding_record, doc_lint_header_record, doc_lint_hint_record,
    doc_lint_truncated_record, lint_summary_record, process_file, rewrite_file_record,
    rewrite_summary_record, run_error_record, scan_doc_files, strip_summary_record,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use walkdir::WalkDir;
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
                  this tool.\n\
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
                    0  clean (no findings, no errors)\n\
                    1  catastrophic / unmapped IO error\n\
                    2  invalid CLI arguments (clap rejection)\n\
                    4  doc-lint findings observed (default mode)\n\
                    5  per-file parse/IO errors, or directory-traversal errors,\n\
                       observed during processing (both modes); each is reported\n\
                       as a `run_error` record naming its kind\n\
                  \n\
                  Output streams: findings (doc_lint_* records, rewrite_file records, \
                  diffs) on stdout; metadata (strip_summary, lint_summary, rewrite_summary, \
                  doc_file_warning, run_error) on stderr. Every line except the --dry-run \
                  unified diff body is a JSON Lines record; the diff body is human-facing \
                  plain text and is never machine-parsed."
)]
struct Options {
    #[arg(default_value = ".", value_name = "ROOT")]
    root: PathBuf,
    /// Run the byte-preserving rewrite passes over every `.rs` file
    /// under ROOT: canonicalise rustdoc-link idioms in doc payloads,
    /// then strip every non-doc `//` and `/* */` comment via the rustc
    /// lexer. Doc comments are preserved; nothing else is.
    #[arg(long)]
    rewrite: bool,
    /// Preview the rewrite as a unified diff without modifying files.
    /// Only meaningful with `--rewrite`. Default (lint) mode is
    /// already read-only; `--dry-run` is meaningful only with
    /// `--rewrite` (enforced by clap).
    #[arg(long, short = 'n', requires = "rewrite")]
    dry_run: bool,
    /// Unified-diff context line count (used with `--dry-run`).
    /// Only meaningful with `--rewrite`.
    #[arg(long, default_value_t = 3, value_name = "N", requires = "rewrite")]
    context: usize,
    /// Word budget for doc-comment prose. Fenced code blocks (` ``` `
    /// or `~~~`) are excluded from the count and do not consume the
    /// budget.
    #[arg(long, default_value_t = 80, value_name = "N")]
    doc_max_words: usize,
    /// DEPRECATED alias for plain `--rewrite`. Retained for one
    /// release. Dispatches the same byte-preserving rewrite path
    /// `--rewrite` runs by default; emits a deprecation note on stderr.
    #[arg(long, requires = "rewrite")]
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

fn main() -> ExitCode {
    let opts = match parse_options() {
        Ok(o) => o,
        Err(rejection) => return report_argv_rejection(&rejection),
    };
    match run(&opts) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(5),
        Err(e) => {
            eprintln!("error: {}", comment_free::single_line(&e.to_string()));
            ExitCode::from(&e)
        }
    }
}
fn run(opts: &Options) -> Result<u32, CommentFreeError> {
    if !opts.root.is_dir() {
        return Err(CommentFreeError::NotADirectory);
    }
    if opts.rustdoc_link_idioms {
        eprintln!(
            "warning: --rustdoc-link-idioms is deprecated; the default --rewrite path now \
             includes rustdoc-link idiom canonicalisation along with lexer-based comment \
             stripping. This flag is a no-op alias retained for one release."
        );
    }
    if opts.rewrite {
        Ok(run_strip(opts))
    } else {
        run_lint(opts)
    }
}
/// Allowlisted source-tree directory names. `comment-free` is a Rust-only
/// tool; only `.rs` files under one of these names anywhere in the path
/// are eligible for traversal.
const ALLOWED_ROOT_DIRS: &[&str] = &["crates", "src"];
/// Resolve `root` to the concrete directories `walk_rs_files` should descend.
///
/// If `root` itself sits inside (or is named) an allowlisted source dir, it
/// is returned verbatim — the caller already targeted a Rust subtree.
/// Otherwise `root` is treated as a project/workspace top: its immediate
/// `crates/` and `src/` children (whichever exist) are returned. An empty
/// result is valid and means "nothing to process".
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
/// roots, `SKIP_DIRS` (notably nested `target/`) still prune build output.
/// An unreadable entry surfaces as [`WalkError`], never as "no entry".
fn walk_rs_files(root: &Path) -> impl Iterator<Item = Result<PathBuf, WalkError>> + use<'_> {
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
                Err(e) => Some(Err(WalkError::rooted_at(&base, e))),
                Ok(e) if e.file_type().is_file() => {
                    let path = e.into_path();
                    (path.extension().and_then(|s| s.to_str()) == Some("rs")).then_some(Ok(path))
                }
                Ok(_) => None,
            })
    })
}
fn run_strip(opts: &Options) -> u32 {
    let mut errors = 0u32;
    let doc_scan = scan_doc_files(&opts.root);
    for path in &doc_scan.files {
        eprintln!("{}", doc_file_warning_record(path));
    }
    for e in &doc_scan.errors {
        errors += 1;
        eprintln!(
            "{}",
            run_error_record(RunErrorKind::Walk, &e.path, &e.source.to_string())
        );
    }
    let mode = if opts.dry_run {
        RewriteMode::DryRun {
            context: opts.context,
        }
    } else {
        RewriteMode::Write
    };
    let mut rewritten = 0u32;
    let mut unchanged = 0u32;
    let mut counts_total = RewriteCounts::default();
    for walked in walk_rs_files(&opts.root) {
        let path = match walked {
            Ok(p) => p,
            Err(e) => {
                errors += 1;
                eprintln!(
                    "{}",
                    run_error_record(RunErrorKind::Walk, &e.path, &e.source.to_string())
                );
                continue;
            }
        };
        match process_file(&path, mode) {
            FileOutcome::Rewritten { counts } => {
                rewritten += 1;
                counts_total += counts;
                println!("{}", rewrite_file_record(mode, &path));
            }
            FileOutcome::WouldRewrite { diff, counts } => {
                rewritten += 1;
                counts_total += counts;
                println!("{}", rewrite_file_record(mode, &path));
                print!("{diff}");
            }
            FileOutcome::Unchanged { counts } => {
                unchanged += 1;
                counts_total += counts;
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
    errors
}
/// Cap on `DOC_LINT_HINT` records emitted per finding kind. Beyond this,
/// the residual count is surfaced as a single `DOC_LINT_TRUNCATED` line.
/// Picked as the upper end of "comfortable to scan in an agent context
/// window"; the hard contract is the truncation record, not the cap value.
const DOC_LINT_HINT_CAP: usize = 50;

fn run_lint(opts: &Options) -> Result<u32, CommentFreeError> {
    let budget = DocBudget {
        max_words: opts.doc_max_words,
    };
    let mut all_findings: Vec<(std::path::PathBuf, comment_free::DocFinding)> = Vec::new();
    let mut errors = 0u32;
    let mut files_scanned = 0u32;
    for walked in walk_rs_files(&opts.root) {
        let path = match walked {
            Ok(p) => p,
            Err(e) => {
                errors += 1;
                eprintln!(
                    "{}",
                    run_error_record(RunErrorKind::Walk, &e.path, &e.source.to_string())
                );
                continue;
            }
        };
        files_scanned += 1;
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                errors += 1;
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
                errors += 1;
                eprintln!(
                    "{}",
                    run_error_record(RunErrorKind::Parse, &path, &e.to_string())
                );
                continue;
            }
        };
        for finding in doc_lint_file(&ast, budget) {
            all_findings.push((path.clone(), finding));
        }
    }
    let findings_total = u32::try_from(all_findings.len()).unwrap_or(u32::MAX);
    for (path, finding) in &all_findings {
        println!(
            "{}",
            doc_lint_finding_record(
                DocLintKind::OverlongDoc,
                path,
                finding.line,
                &finding.item_label,
                finding.words.count(),
                finding.budget,
                finding.words.is_fail_closed(),
            )
        );
    }
    emit_doc_lint_hints(&all_findings);
    eprintln!(
        "{}",
        lint_summary_record(files_scanned, findings_total, errors)
    );
    if errors > 0 {
        return Ok(errors);
    }
    if findings_total > 0 {
        return Err(CommentFreeError::DocLintFailure);
    }
    Ok(0)
}

/// Emit one `doc_lint_header` record per finding kind, up to
/// [`DOC_LINT_HINT_CAP`] `doc_lint_hint` records sorted by
/// `words - budget` descending, and a `doc_lint_truncated` record when
/// the kind has more findings than the cap.
fn emit_doc_lint_hints(findings: &[(std::path::PathBuf, comment_free::DocFinding)]) {
    if findings.is_empty() {
        return;
    }
    let kind = DocLintKind::OverlongDoc;
    println!("{}", doc_lint_header_record(kind));
    let mut sorted: Vec<&(std::path::PathBuf, comment_free::DocFinding)> =
        findings.iter().collect();
    sorted.sort_by(|(_, a), (_, b)| {
        let oa = a.words.count().saturating_sub(a.budget);
        let ob = b.words.count().saturating_sub(b.budget);
        ob.cmp(&oa)
    });
    let kept = sorted.iter().take(DOC_LINT_HINT_CAP);
    for (path, f) in kept {
        println!(
            "{}",
            doc_lint_hint_record(kind, path, f.line, &f.item_label, f.words.count(), f.budget)
        );
    }
    if sorted.len() > DOC_LINT_HINT_CAP {
        let remaining = sorted.len() - DOC_LINT_HINT_CAP;
        println!("{}", doc_lint_truncated_record(kind, remaining));
    }
}

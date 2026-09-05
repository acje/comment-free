#![forbid(unsafe_code)]
#![warn(clippy::missing_const_for_fn)]
use clap::Parser;
use comment_free::{
    CommentFreeError, DocBudget, DocLintKind, FileOutcome, RewriteCounts, RewriteMode,
    RunErrorKind, doc_file_warning_record, doc_lint_file, doc_lint_finding_record,
    doc_lint_header_record, doc_lint_hint_record, doc_lint_truncated_record,
    doc_lint_undecided_record, lint_summary_record, process_file, rewrite_file_record,
    rewrite_summary_record, run_error_record, scan_doc_files, strip_summary_record, walk_rs_files,
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
                    1  catastrophic / unmapped IO error\n\
                    2  invalid CLI arguments (clap rejection)\n\
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
    #[arg(default_value = ".", value_name = "ROOT")]
    root: PathBuf,
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
        help = "Preview the rewrite as a unified diff without modifying files. Only meaningful \
                with `--rewrite`. Default (lint) mode is already read-only; `--dry-run` is \
                meaningful only with `--rewrite` (enforced by clap)"
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

enum Command {
    Lint { root: PathBuf, budget: DocBudget },
    Rewrite { root: PathBuf, mode: RewriteMode },
}
impl Command {
    fn from_options(opts: Options) -> Result<Self, CommentFreeError> {
        if !opts.root.is_dir() {
            return Err(CommentFreeError::NotADirectory);
        }
        Ok(if opts.rewrite {
            Self::Rewrite {
                root: opts.root,
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
                root: opts.root,
                budget: DocBudget {
                    max_words: opts.doc_max_words,
                },
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
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(5),
        Err(e) => {
            eprintln!("error: {}", comment_free::single_line(&e.to_string()));
            ExitCode::from(&e)
        }
    }
}
fn dispatch(opts: Options) -> Result<u32, CommentFreeError> {
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
fn run(command: &Command) -> Result<u32, CommentFreeError> {
    match command {
        Command::Rewrite { root, mode } => Ok(run_strip(root, *mode)),
        Command::Lint { root, budget } => run_lint(root, *budget),
    }
}
fn run_strip(root: &Path, mode: RewriteMode) -> u32 {
    let mut errors = 0u32;
    let doc_scan = scan_doc_files(root);
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
    for walked in walk_rs_files(root) {
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
    errors
}
const DOC_LINT_HINT_CAP: usize = 50;

fn run_lint(root: &Path, budget: DocBudget) -> Result<u32, CommentFreeError> {
    let mut all_findings: Vec<(std::path::PathBuf, comment_free::DocFinding)> = Vec::new();
    let mut all_undecided: Vec<(std::path::PathBuf, comment_free::DocUndecided)> = Vec::new();
    let mut errors = 0u32;
    let mut files_scanned = 0u32;
    for walked in walk_rs_files(root) {
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
        let report = doc_lint_file(&ast, budget);
        for finding in report.findings() {
            all_findings.push((path.clone(), finding.clone()));
        }
        for undecided in report.undecided() {
            all_undecided.push((path.clone(), undecided.clone()));
        }
    }
    let findings_total = u32::try_from(all_findings.len()).unwrap_or(u32::MAX);
    let undecided_total = u32::try_from(all_undecided.len()).unwrap_or(u32::MAX);
    for (path, finding) in &all_findings {
        println!(
            "{}",
            doc_lint_finding_record(
                DocLintKind::OverlongDoc,
                path,
                finding.line(),
                finding.item_label(),
                finding.words().count(),
                finding.budget(),
                finding.words().is_fail_closed(),
            )
        );
    }
    for (path, undecided) in &all_undecided {
        println!(
            "{}",
            doc_lint_undecided_record(DocLintKind::OverlongDoc, path, undecided)
        );
    }
    emit_doc_lint_hints(&all_findings);
    eprintln!(
        "{}",
        lint_summary_record(files_scanned, findings_total, undecided_total, errors)
    );
    if errors > 0 {
        return Ok(errors);
    }
    if findings_total > 0 || undecided_total > 0 {
        return Err(CommentFreeError::DocLintFailure);
    }
    Ok(0)
}

fn emit_doc_lint_hints(findings: &[(std::path::PathBuf, comment_free::DocFinding)]) {
    if findings.is_empty() {
        return;
    }
    let kind = DocLintKind::OverlongDoc;
    println!("{}", doc_lint_header_record(kind));
    let mut sorted: Vec<&(std::path::PathBuf, comment_free::DocFinding)> =
        findings.iter().collect();
    sorted.sort_by(|(_, a), (_, b)| {
        let oa = a.words().count().saturating_sub(a.budget());
        let ob = b.words().count().saturating_sub(b.budget());
        ob.cmp(&oa)
    });
    let kept = sorted.iter().take(DOC_LINT_HINT_CAP);
    for (path, f) in kept {
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
    if sorted.len() > DOC_LINT_HINT_CAP {
        let remaining = sorted.len() - DOC_LINT_HINT_CAP;
        println!("{}", doc_lint_truncated_record(kind, remaining));
    }
}

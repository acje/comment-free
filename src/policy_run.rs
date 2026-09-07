use super::{InputScope, retain_hint};
use crate::policy::records::{self, Event, Threshold};
use crate::policy::{Fault, Policy, Thresholds, Verdict};
use comment_free::{
    DocBudget, DocLintReport, RunErrorKind, WarningLimit, doc_lint_file, run_error_record,
};
use std::io::Write;
use std::path::Path;

trait Source {
    fn read(&mut self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
    fn parse(&mut self, source: &str) -> syn::Result<syn::File> {
        syn::parse_file(source)
    }
    fn lint(&mut self, ast: &syn::File, max_words: usize) -> DocLintReport {
        doc_lint_file(ast, DocBudget { max_words })
    }
}

struct Disk;
impl Source for Disk {}

pub(super) fn run(root: &InputScope, thresholds: Thresholds, limit: WarningLimit) -> u8 {
    execute(
        root,
        thresholds,
        limit,
        &mut Disk,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

fn execute(
    root: &InputScope,
    thresholds: Thresholds,
    limit: WarningLimit,
    source: &mut impl Source,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> u8 {
    match process(root, thresholds, limit, source, stdout, stderr) {
        Ok(Verdict::Pass) => 0,
        Ok(Verdict::Fail) => 1,
        Ok(Verdict::Unknown) => 2,
        Err(fault) => {
            report_unavailable(stderr, &format!("policy evaluation unavailable: {fault:?}"))
        }
    }
}

pub(super) fn report_unavailable(stderr: &mut impl Write, message: &str) -> u8 {
    let diagnostic_delivery = writeln!(stderr, "error: {message}").and_then(|()| stderr.flush());
    match diagnostic_delivery {
        Ok(()) | Err(_) => 2,
    }
}

fn process(
    root: &InputScope,
    thresholds: Thresholds,
    limit: WarningLimit,
    source: &mut impl Source,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<Verdict, Fault> {
    let mut policy = Policy::new(thresholds);
    let mut hints = [Vec::with_capacity(50), Vec::with_capacity(50)];
    let mut paths = Vec::new();
    let (scope, files) = root.files();
    for file in files {
        match file {
            Ok(path) => paths.push(path),
            Err(error) => {
                policy.processing_error(false)?;
                records::line(
                    stderr,
                    &run_error_record(RunErrorKind::Walk, error.path(), &error.message()),
                )?;
            }
        }
    }
    paths.sort();
    for path in paths {
        let text = match source.read(&path) {
            Ok(text) => text,
            Err(error) => {
                policy.processing_error(true)?;
                records::line(
                    stderr,
                    &run_error_record(RunErrorKind::Io, &path, &error.to_string()),
                )?;
                continue;
            }
        };
        let ast = match source.parse(&text) {
            Ok(ast) => ast,
            Err(error) => {
                policy.processing_error(true)?;
                records::line(
                    stderr,
                    &run_error_record(RunErrorKind::Parse, &path, &error.to_string()),
                )?;
                continue;
            }
        };
        policy.evaluate(
            limit,
            |budget| source.lint(&ast, budget),
            |threshold, report, admitted| {
                if admitted {
                    for finding in report.findings() {
                        records::line(
                            stdout,
                            &records::detail(threshold, Event::Finding(&path, finding)),
                        )?;
                        retain_hint(&mut hints[threshold.index()], &path, finding);
                    }
                    for item in report.undecided() {
                        records::line(
                            stdout,
                            &records::detail(threshold, Event::Undecided(&path, item)),
                        )?;
                    }
                }
                Ok(())
            },
        )?;
    }
    for (threshold, totals) in [
        (Threshold::Advisory, policy.advisory()),
        (Threshold::Enforced, policy.enforced()),
    ] {
        let retained = &hints[threshold.index()];
        if !retained.is_empty() {
            records::line(stdout, &records::detail(threshold, Event::Header))?;
            for (path, finding) in retained {
                records::line(
                    stdout,
                    &records::detail(threshold, Event::Hint(path, finding)),
                )?;
            }
            if totals.findings().shown() > 50 {
                records::line(
                    stdout,
                    &records::detail(threshold, Event::Truncated(totals.findings().shown() - 50)),
                )?;
            }
        }
    }
    stdout.flush().map_err(|_| Fault::Output)?;
    records::line(
        stderr,
        &records::summary(&policy, root.path(), scope, limit)?,
    )?;
    stderr.flush().map_err(|_| Fault::Output)?;
    Ok(policy.verdict())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Instrumented {
        reads: usize,
        parses: usize,
        asts: Vec<*const syn::File>,
        fail_read: bool,
        paths: Vec<std::path::PathBuf>,
    }

    impl Source for Instrumented {
        fn read(&mut self, path: &Path) -> std::io::Result<String> {
            self.reads += 1;
            self.paths.push(path.to_path_buf());
            if self.fail_read {
                Err(std::io::Error::other("injected read"))
            } else {
                std::fs::read_to_string(path)
            }
        }

        fn parse(&mut self, source: &str) -> syn::Result<syn::File> {
            self.parses += 1;
            syn::parse_file(source)
        }

        fn lint(&mut self, ast: &syn::File, max_words: usize) -> DocLintReport {
            self.asts.push(std::ptr::from_ref(ast));
            doc_lint_file(ast, DocBudget { max_words })
        }
    }

    #[test]
    fn policy_actual_adapters_read_parse_once_same_ast() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("input.rs");
        for (text, read_failure, parses, evaluations, exit) in [
            ("fn item() {}", false, 1, 2, 0),
            ("fn item() {}", true, 0, 0, 2),
            ("fn broken( {", false, 1, 0, 2),
        ] {
            std::fs::write(&path, text).unwrap();
            let root = InputScope::from_path(Some(path.clone())).unwrap();
            let mut source = Instrumented {
                fail_read: read_failure,
                ..Instrumented::default()
            };
            let mut stderr = Vec::new();
            assert_eq!(
                execute(
                    &root,
                    Thresholds::new(80, 120).unwrap(),
                    WarningLimit::Unlimited,
                    &mut source,
                    &mut Vec::new(),
                    &mut stderr
                ),
                exit
            );
            assert_eq!(
                (source.reads, source.parses, source.asts.len()),
                (1, parses, evaluations)
            );
            if evaluations == 2 {
                assert_eq!(source.asts[0], source.asts[1]);
            }
            let output = String::from_utf8(stderr).unwrap();
            assert!(output.contains("\"files\":1"));
            assert!(output.contains(if exit == 0 {
                "\"errors\":0"
            } else {
                "\"errors\":1"
            }));
        }
    }

    struct BrokenOutput {
        bytes_left: usize,
        fail_flush: bool,
    }

    #[test]
    fn policy_multiple_files_use_each_adapter_once() {
        let td = tempfile::tempdir().unwrap();
        for name in ["a.rs", "b.rs", "c.rs"] {
            std::fs::write(td.path().join(name), "fn item() {}").unwrap();
        }
        let root = InputScope::from_path(Some(td.path().to_path_buf())).unwrap();
        let mut source = Instrumented::default();
        assert_eq!(
            execute(
                &root,
                Thresholds::new(0, 0).unwrap(),
                WarningLimit::Unlimited,
                &mut source,
                &mut Vec::new(),
                &mut Vec::new()
            ),
            0
        );
        assert_eq!((source.reads, source.parses, source.asts.len()), (3, 3, 6));
        source.paths.sort();
        source.paths.dedup();
        assert_eq!(source.paths.len(), 3);
        for pair in source.asts.as_chunks::<2>().0 {
            assert_eq!(pair[0], pair[1]);
        }
    }

    #[test]
    fn policy_hint_item_high_water_and_capacity() {
        let ast = syn::parse_file("#[doc = \"one two\"] fn item() {}").unwrap();
        let report = doc_lint_file(&ast, DocBudget { max_words: 0 });
        let mut hints = [Vec::with_capacity(50), Vec::with_capacity(50)];
        let mut high_water = 0;
        for _ in 0..1000 {
            for retained in &mut hints {
                retain_hint(retained, Path::new("input.rs"), &report.findings()[0]);
                assert!(retained.len() <= 50);
                assert_eq!(retained.capacity(), 50);
            }
            high_water = high_water.max(hints.iter().map(Vec::len).sum::<usize>());
        }
        assert_eq!(high_water, 100);
        eprintln!(
            "policy_hint_high_water_items={high_water}; capacities=50,50; iterations=1000; synchronous; debug; payload bytes excluded"
        );
    }

    #[test]
    fn policy_processing_diagnostic_delivery_faults_stay_unknown() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("input.rs");
        std::fs::write(&path, "fn invalid( {").unwrap();
        let root = InputScope::from_path(Some(path)).unwrap();
        for (bytes_left, fail_flush) in [(0, false), (17, false), (usize::MAX, true)] {
            let mut broken = BrokenOutput {
                bytes_left,
                fail_flush,
            };
            assert_eq!(
                execute(
                    &root,
                    Thresholds::new(0, 0).unwrap(),
                    WarningLimit::Limited(0),
                    &mut Disk,
                    &mut Vec::new(),
                    &mut broken
                ),
                2
            );
        }
    }

    impl Write for BrokenOutput {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.bytes_left == 0 {
                return Err(std::io::Error::other("injected write"));
            }
            let n = bytes.len().min(self.bytes_left);
            self.bytes_left -= n;
            Ok(n)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            if self.fail_flush {
                Err(std::io::Error::other("injected flush"))
            } else {
                Ok(())
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn policy_read_and_walk_diagnostic_delivery_faults_stay_unknown() {
        let td = tempfile::tempdir().unwrap();
        let file = td.path().join("input.rs");
        std::fs::write(&file, [0xff]).unwrap();
        let file_root = InputScope::from_path(Some(file.clone())).unwrap();
        let walk_dir = td.path().join("walk");
        std::fs::create_dir(&walk_dir).unwrap();
        std::os::unix::fs::symlink(walk_dir.join("absent.rs"), walk_dir.join("link.rs")).unwrap();
        let walk_root = InputScope::from_path(Some(walk_dir)).unwrap();
        for root in [&file_root, &walk_root] {
            for (bytes_left, fail_flush) in [(0, false), (17, false), (usize::MAX, true)] {
                let mut broken = BrokenOutput {
                    bytes_left,
                    fail_flush,
                };
                assert_eq!(
                    execute(
                        root,
                        Thresholds::new(0, 0).unwrap(),
                        WarningLimit::Limited(0),
                        &mut Disk,
                        &mut Vec::new(),
                        &mut broken,
                    ),
                    2
                );
            }
        }
    }

    #[test]
    fn policy_output_write_partial_write_and_flush_are_unknown() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("input.rs");
        std::fs::write(&path, "#[doc = \"one two\"] fn item() {}").unwrap();
        let root = InputScope::from_path(Some(path)).unwrap();
        for stderr_broken in [false, true] {
            for (bytes_left, fail_flush) in [(0, false), (17, false), (usize::MAX, true)] {
                let mut broken = BrokenOutput {
                    bytes_left,
                    fail_flush,
                };
                let mut good = Vec::new();
                let exit = if stderr_broken {
                    execute(
                        &root,
                        Thresholds::new(0, 0).unwrap(),
                        WarningLimit::Unlimited,
                        &mut Disk,
                        &mut good,
                        &mut broken,
                    )
                } else {
                    execute(
                        &root,
                        Thresholds::new(0, 0).unwrap(),
                        WarningLimit::Unlimited,
                        &mut Disk,
                        &mut broken,
                        &mut good,
                    )
                };
                assert_eq!(exit, 2);
                if !stderr_broken {
                    assert!(!String::from_utf8(good).unwrap().contains("policy_summary"));
                }
            }
        }
    }
}

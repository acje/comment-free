#[path = "../src/policy.rs"]
mod policy;

use comment_free::{DocBudget, WarningLimit, doc_lint_file};
use policy::{Policy, Thresholds, Verdict};

#[test]
fn policy_strict_schema_and_semantic_acceptance() {
    let status = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/policy_schema.py"
        ))
        .arg(env!("CARGO_BIN_EXE_comment-free"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn policy_invalid_root_and_threshold_diagnostics_are_distinct() {
    for (advisory, expected) in [
        ("121", "advisory threshold exceeds enforced threshold"),
        ("80", "invalid policy root"),
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_comment-free"))
            .args([
                "--check-doc-budget",
                "--doc-advisory-words",
                advisory,
                "--doc-max-words",
                "120",
                "/nonexistent-policy-root",
            ])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains(expected));
    }
}

#[test]
fn policy_cli_boundary_and_unknown() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("input.rs");
    for (source, exit) in [
        (documented(80, "item"), 0),
        (documented(81, "item"), 0),
        (documented(120, "item"), 0),
        (documented(121, "item"), 1),
        (
            format!(
                "#[cfg_attr(feature = \"x\", doc = \"extra\")] {}{}",
                documented(90, "item"),
                documented(121, "breach")
            ),
            2,
        ),
    ] {
        std::fs::write(&path, source).unwrap();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_comment-free"))
            .args([
                "--check-doc-budget",
                "--doc-advisory-words",
                "80",
                "--doc-max-words",
                "120",
                "--max-warning-files",
                "0",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(exit), "{out:?}");
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("\"kind\":\"policy_summary\""));
    }
}

fn documented(words: usize, name: &str) -> String {
    format!("#[doc = {:?}] pub fn {name}() {{}}", "word ".repeat(words))
}

#[test]
fn policy_cli_rejects_incomplete_thresholds_and_conflicts() {
    let td = tempfile::tempdir().unwrap();
    for args in [
        "--check-doc-budget",
        "--check-doc-budget --doc-advisory-words 80",
        "--check-doc-budget --doc-max-words 120",
        "--doc-advisory-words 80",
        "--check-doc-budget --doc-advisory-words 121 --doc-max-words 120",
        "--check-doc-budget --doc-advisory-words -1 --doc-max-words 120",
        "--check-doc-budget --doc-advisory-words 0 --doc-max-words -1",
        "--check-doc-budget --doc-advisory-words 0 --doc-max-words 0 --rewrite --rustdoc-link-idioms",
        "--check-doc-budget --doc-advisory-words 0 --doc-max-words 999999999999999999999999",
        "--check-doc-budget --doc-advisory-words 0 --doc-max-words 0 --rewrite",
        "--check-doc-budget --doc-advisory-words 0 --doc-max-words 0 --dry-run",
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_comment-free"))
            .args(args.split_whitespace())
            .arg(td.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn policy_cli_caps_apply_per_threshold_and_bound_hints() {
    let td = tempfile::tempdir().unwrap();
    for index in 0..3 {
        let source = (0..60)
            .map(|i| documented(121, &format!("f{i}")))
            .collect::<String>();
        std::fs::write(td.path().join(format!("{index}.rs")), source).unwrap();
    }
    for (cap, shown, hints) in [("0", 0, 0), ("1", 60, 100), ("unlimited", 180, 100)] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_comment-free"))
            .args([
                "--check-doc-budget",
                "--doc-advisory-words",
                "80",
                "--doc-max-words",
                "120",
                "--max-warning-files",
                cap,
            ])
            .arg(td.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        let stdout = String::from_utf8(out.stdout).unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert_eq!(
            stdout.matches("\"event\":\"doc_lint_finding\"").count(),
            shown * 2
        );
        assert_eq!(stdout.matches("\"event\":\"doc_lint_hint\"").count(), hints);
        assert_eq!(stderr.matches("\"findings\":180,").count(), 2);
        assert_eq!(
            stderr
                .matches(&format!("\"findings_shown\":{shown},"))
                .count(),
            2
        );
        if cap == "0" {
            assert!(stdout.is_empty());
        }
    }
}

#[test]
fn policy_threshold_boundary_differential() {
    for words in [80, 81, 120, 121] {
        let ast = syn::parse_file(&documented(words, "item")).unwrap();
        let mut policy = Policy::new(Thresholds::new(80, 120).unwrap());
        policy.observe(&ast, WarningLimit::Limited(0)).unwrap();
        for (actual, threshold) in [(policy.advisory(), 80), (policy.enforced(), 120)] {
            let expected = doc_lint_file(
                &ast,
                DocBudget {
                    max_words: threshold,
                },
            );
            assert_eq!(
                actual.findings().total(),
                u32::try_from(expected.findings().len()).unwrap()
            );
            assert_eq!(
                actual.undecided().total(),
                u32::try_from(expected.undecided().len()).unwrap()
            );
            assert_eq!(actual.findings().shown(), 0);
            assert_eq!(actual.findings().hidden(), actual.findings().total());
        }
        assert_eq!(
            policy.verdict(),
            if words > 120 {
                Verdict::Fail
            } else {
                Verdict::Pass
            }
        );
    }
}

#[test]
fn policy_conditional_independent_evaluation_unknown_dominates_breach() {
    for breach in [false, true] {
        let mut source = format!(
            "#[cfg_attr(feature = \"conditional\", doc = \"extra\")] {}",
            documented(90, "conditional")
        );
        if breach {
            source.push_str(&documented(121, "breach"));
        }
        let ast = syn::parse_file(&source).unwrap();
        for limit in [
            WarningLimit::Limited(0),
            WarningLimit::Limited(1),
            WarningLimit::Unlimited,
        ] {
            let mut policy = Policy::new(Thresholds::new(80, 120).unwrap());
            policy.observe(&ast, limit).unwrap();
            assert_eq!(policy.advisory().findings().total(), 1 + u32::from(breach));
            assert_eq!(policy.advisory().undecided().total(), 0);
            assert_eq!(policy.enforced().findings().total(), u32::from(breach));
            assert_eq!(policy.enforced().undecided().total(), 1);
            assert_eq!(policy.verdict(), Verdict::Unknown);
            assert_eq!(
                policy.reasons(),
                if breach {
                    vec!["enforced_undecided", "enforced_violation"]
                } else {
                    vec!["enforced_undecided"]
                }
            );
        }
    }
}

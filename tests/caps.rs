mod common;

use common::*;
use std::fmt::Write as _;

#[test]
fn cap_verdict_matrix() {
    let td = tempfile::tempdir().unwrap();
    let file = td.path().join("input.rs");
    for (source, expected) in [
        ("fn clean() {}", 0),
        ("#[doc = include_str!(\"missing\")] pub fn item() {}", 4),
        ("fn invalid( {", 5),
    ] {
        fs::write(&file, source).unwrap();
        for cap in ["0", "1", "2", "unlimited"] {
            let out = Command::new(bin())
                .args(["--max-warning-files", cap])
                .arg(&file)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(expected), "{cap}: {out:?}");
            if cap == "0" || expected != 4 {
                assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
            }
            let summary = one_record(&String::from_utf8_lossy(&out.stderr), "lint_summary");
            assert_eq!(summary.text("scope"), "file");
            assert_eq!(summary.number("undecided"), u32::from(expected == 4));
            assert_eq!(summary.number("errors"), u32::from(expected == 5));
        }
    }
}

#[cfg(unix)]
#[test]
fn capped_io_failures_remain_visible() {
    let td = tempfile::tempdir().unwrap();
    let file = td.path().join("input.rs");
    fs::write(&file, "fn clean() {}").unwrap();
    make_unreadable(&file);
    let mut outputs = Vec::new();
    for cap in ["0", "1", "unlimited"] {
        outputs.push(
            Command::new(bin())
                .args(["--max-warning-files", cap])
                .arg(&file)
                .output()
                .unwrap(),
        );
    }
    make_readable(&file);
    for out in outputs {
        assert_eq!(out.status.code(), Some(5), "{out:?}");
        assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
        assert_eq!(
            one_error(&String::from_utf8_lossy(&out.stderr), "io").text("path"),
            file.to_string_lossy()
        );
    }
}

#[test]
fn project_order_cap_two_and_stable_hint_ties() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir_all(td.path().join("src/nested")).unwrap();
    let mut many = String::new();
    for i in 0..60 {
        writeln!(
            many,
            "/// one two three four five six\npub fn item{i}() {{}}"
        )
        .unwrap();
    }
    for name in ["src/z.rs", "src/nested/a.rs", "build.rs"] {
        fs::write(td.path().join(name), &many).unwrap();
    }
    let out = Command::new(bin())
        .args(["--doc-max-words", "5", "--max-warning-files", "2"])
        .arg(td.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let findings = records_named(&stdout, "doc_lint_finding");
    assert_eq!(findings.len(), 120);
    assert!(findings[0].text("path").ends_with("build.rs"));
    assert!(findings[60].text("path").ends_with("src/nested/a.rs"));
    let hints = records_named(&stdout, "doc_lint_hint");
    assert_eq!(hints.len(), 50);
    for (i, hint) in hints.iter().enumerate() {
        assert!(hint.text("path").ends_with("build.rs"));
        assert_eq!(hint.text("item"), format!("fn item{i}"));
        assert_eq!(hint.number("line"), u32::try_from(i * 2 + 2).unwrap());
        assert_eq!(hint.text("kind"), "overlong_doc");
    }
    assert_eq!(
        one_record(&stdout, "doc_lint_truncated").number("remaining"),
        70
    );
    let summary = one_record(&String::from_utf8_lossy(&out.stderr), "lint_summary");
    assert_eq!(summary.number("findings"), 180);
    assert_eq!(summary.number("findings_hidden"), 60);
}

#[cfg(target_os = "linux")]
#[test]
fn native_order_is_not_lossy_display_order() {
    use std::os::unix::ffi::OsStringExt;
    let td = tempfile::tempdir().unwrap();
    for (byte, item) in [(0xff, "later"), (0xfe, "first")] {
        let name = std::ffi::OsString::from_vec(vec![byte, b'.', b'r', b's']);
        fs::write(
            td.path().join(name),
            format!("/// one two three four five six\npub fn {item}() {{}}\n"),
        )
        .unwrap();
    }
    let out = run_lint_budget(td.path(), 5);
    assert_eq!(out.status.code(), Some(4));
    let finding = one_record(&String::from_utf8_lossy(&out.stdout), "doc_lint_finding");
    assert_eq!(finding.text("item"), "fn first");
}

#[test]
fn warning_file_cap_preserves_full_verdict_and_counts() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a_clean.rs", "fn clean() {}\n");
    write(td.path(), "b_error.rs", "fn broken( {\n");
    write(
        td.path(),
        "c_unknown.rs",
        "#[doc = include_str!(\"missing\")] pub fn c() {}\n",
    );
    write(
        td.path(),
        "d_warning.rs",
        "/// one two three four five six\npub fn d() {}\n",
    );
    for (cap, shown) in [("0", 0), ("1", 1), ("unlimited", 2)] {
        let out = Command::new(bin())
            .args(["--doc-max-words", "5", "--max-warning-files", cap])
            .arg(td.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(5), "{out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let summary = one_record(&stderr, "lint_summary");
        assert_eq!(summary.number("files"), 4);
        assert_eq!(summary.number("findings"), 1);
        assert_eq!(summary.number("undecided"), 1);
        assert_eq!(summary.number("warning_files"), 2);
        assert_eq!(summary.number("warning_files_shown"), shown);
        assert_eq!(summary.number("warning_files_hidden"), 2 - shown);
        assert_eq!(records_named(&stderr, "run_error").len(), 1);
        if shown == 0 {
            assert!(stdout.is_empty());
        }
        if shown == 1 {
            assert_eq!(records_named(&stdout, "doc_lint_undecided").len(), 1);
            assert!(
                records_named(&stdout, "doc_lint_finding").is_empty(),
                "unexpected finding: {stdout}"
            );
            assert!(
                records_named(&stdout, "doc_lint_hint").is_empty(),
                "unexpected hint: {stdout}"
            );
        }
    }
}

#[test]
fn warning_cap_rejects_invalid_values_and_rewrite_conflicts() {
    let td = tempfile::tempdir().unwrap();
    for value in [
        "",
        "+1",
        "-1",
        "1.0",
        " 1",
        "1 ",
        "Unlimited",
        "١",
        "999999999999999999999999",
    ] {
        let out = Command::new(bin())
            .args(["--max-warning-files", value])
            .arg(td.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{value}: {out:?}");
    }
    for mode in ["--rewrite", "--rustdoc-link-idioms"] {
        let out = Command::new(bin())
            .args([mode, "--max-warning-files", "1"])
            .arg(td.path())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{out:?}");
    }
}

#[test]
fn default_cap_admits_a_whole_mixed_file_in_native_path_order() {
    let td = tempfile::tempdir().unwrap();
    let prose = "/// one two three four five six\npub fn item() {}\n";
    write(td.path(), "z.rs", prose);
    write(
        td.path(),
        "a.rs",
        &format!("{prose}#[doc = include_str!(\"missing\")] pub fn unknown() {{}}\n"),
    );
    let out = run_lint_budget(td.path(), 5);
    assert_eq!(out.status.code(), Some(4));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        one_record(&stdout, "doc_lint_finding")
            .text("path")
            .ends_with("a.rs")
    );
    assert_eq!(records_named(&stdout, "doc_lint_undecided").len(), 1);
    assert_eq!(records_named(&stdout, "doc_lint_hint").len(), 1);
    let summary = one_record(&String::from_utf8_lossy(&out.stderr), "lint_summary");
    assert_eq!(summary.number("findings_hidden"), 1);
    assert_eq!(summary.text("max_warning_files"), "1");
}

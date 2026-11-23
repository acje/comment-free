mod common;

use common::*;
use std::fmt::Write as _;

#[test]
fn public_summary_metadata_has_only_schema_values() {
    use comment_free::{LintTotals, ReportScope, WarningLimit};
    let totals = LintTotals::default();
    for (scope, expected) in [
        (ReportScope::File, "file"),
        (ReportScope::ProjectAllowlist, "project-allowlist"),
        (ReportScope::RecursiveDirectory, "recursive-directory"),
        (ReportScope::Unresolved, "unresolved"),
    ] {
        for (limit, spelling) in [
            (WarningLimit::Limited(0), "0"),
            (WarningLimit::Unlimited, "unlimited"),
        ] {
            let record = parse_record(&totals.record(Path::new("."), scope, limit)).unwrap();
            assert_eq!(record.text("scope"), expected);
            assert_eq!(record.text("max_warning_files"), spelling);
        }
    }
    assert_eq!(
        "000".parse::<WarningLimit>().unwrap(),
        WarningLimit::Limited(0)
    );
    assert_eq!(
        "unlimited".parse::<WarningLimit>().unwrap(),
        WarningLimit::Unlimited
    );
    assert_eq!(
        WarningLimit::Limited(usize::MAX).to_string(),
        usize::MAX.to_string()
    );
    for invalid in [
        "-1",
        "+1",
        "",
        "bogus",
        " 0",
        "0 ",
        "١",
        "Unlimited",
        "1.0",
        "99999999999999999999999999999",
    ] {
        assert!(invalid.parse::<WarningLimit>().is_err(), "{invalid}");
    }
}

#[test]
#[allow(deprecated)]
fn legacy_summary_helper_keeps_its_legacy_version() {
    let record = parse_record(&comment_free::lint_summary_record(1, 0, 0, 0)).unwrap();
    assert_eq!(record.number("v"), 2);
}

#[test]
fn summary_names_the_selected_policy_not_just_the_argument_form() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let out = run_lint_budget(td.path(), 5);
    let summary = one_record(&String::from_utf8_lossy(&out.stderr), "lint_summary");
    assert_eq!(summary.text("scope"), "recursive-directory");
}

#[test]
fn doc_lint_hint_round_trip_parses_to_struct() {
    let td = tempfile::tempdir().unwrap();
    let mut src = String::new();
    for i in 0..3 {
        for line in 0..9 {
            src.push_str("/// ");
            for w in 0..10 {
                let nw = line * 10 + w + 1;
                write!(src, "w{nw:02} ").unwrap();
            }
            src.push('\n');
        }
        writeln!(src, "pub fn f{i}() {{}}").unwrap();
    }
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hints = records_named(&stdout, "doc_lint_hint");
    assert_eq!(hints.len(), 3, "expected 3 parsed hints:\n{stdout}");
    for hint in &hints {
        assert!(
            hint.text("path").ends_with("a.rs"),
            "wrong path: {}",
            hint.text("path")
        );
        assert!(hint.number("line") > 0, "line must be 1-indexed positive");
        assert!(
            hint.text("item").starts_with("fn f"),
            "item label malformed: {}",
            hint.text("item")
        );
        assert_eq!(hint.number("words"), 90, "expected 90 words per finding");
        assert_eq!(hint.number("budget"), 80, "expected default budget 80");
        assert_eq!(hint.text("kind"), "overlong_doc");
        assert_eq!(hint.number("v"), 3, "record version drift");
    }
}

#[test]
fn doc_lint_record_grammar_const_is_published() {
    let g = comment_free::DOC_LINT_RECORD_GRAMMAR;
    for required in [
        "doc_lint_finding",
        "doc_lint_header",
        "doc_lint_hint",
        "doc_lint_truncated",
        "doc_lint_undecided",
        "\"words_all_cfgs\":<U32>",
        "\"outcome\":<OUTCOME>",
        "\"doctrine\":<STRING>",
        "\"words\":<U32>",
        "\"budget\":<U32>",
        "\"item\":<LABEL>",
        "\"remaining\":<U32>",
    ] {
        assert!(
            g.contains(required),
            "grammar missing required token `{required}`: {g}"
        );
    }
}

#[test]
fn doc_lint_record_version_is_three() {
    assert_eq!(
        comment_free::DOC_LINT_RECORD_VERSION,
        3,
        "v=3 permits bounded warning details"
    );
}

#[test]
fn doc_lint_truncated_record_round_trip_parses() {
    let td = tempfile::tempdir().unwrap();
    let mut src = String::new();
    let n_items = 60usize;
    for i in 0..n_items {
        for line in 0..9 {
            src.push_str("/// ");
            for w in 0..10 {
                let nw = line * 10 + w + 1;
                write!(src, "w{nw:02} ").unwrap();
            }
            src.push('\n');
        }
        writeln!(src, "pub fn f{i}() {{}}").unwrap();
    }
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let truncated = one_record(&stdout, "doc_lint_truncated");
    assert_eq!(truncated.text("kind"), "overlong_doc");
    assert_eq!(
        truncated.number("remaining"),
        u32::try_from(n_items - 50).unwrap()
    );
    assert_eq!(truncated.number("v"), 3);
}

#[test]
fn doc_lint_header_record_round_trip_parses() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "/// w01 w02 w03 w04 w05 w06\npub fn f() {}\n",
    );
    let out = run_lint_budget(td.path(), 5);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let header = one_record(&stdout, "doc_lint_header");
    assert_eq!(header.text("kind"), "overlong_doc");
    assert_eq!(header.number("v"), 3);
    assert!(
        header
            .text("doctrine")
            .contains("Rust docs must contain a concise summary"),
        "header doctrine field must carry the full doctrine sentence"
    );
    assert!(
        !header.text("doctrine").contains("0-3"),
        "the unenforced 0-3 fenced-example promise must be gone from machine output"
    );
}

#[test]
fn rewrite_summary_record_emitted_on_stderr_with_counters() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "// kill me\nlet x = 1; // tail\nfn f() {}\n",
    );
    let out = run(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let summary = one_record(&stderr, "rewrite_summary");
    for field in [
        "comments_removed",
        "inline_trimmed",
        "blank_lines_collapsed",
        "doc_links_rewritten",
    ] {
        assert!(
            summary.keys().contains(&field),
            "rewrite_summary missing `{field}` field:\n{stderr}"
        );
    }
    assert_eq!(summary.number("v"), 2);
    assert_eq!(summary.text("mode"), "write");
    for gone in ["safety_preserved", "auto_trait_preserved"] {
        assert!(
            !stderr.contains(gone),
            "preservation counter `{gone}` must be gone from the record:\n{stderr}"
        );
    }
}

#[test]
fn rewrite_summary_record_counts_aggregate_over_files() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "// one\nfn a() {}\n");
    write(td.path(), "b.rs", "// two\n// three\nfn b() {}\n");
    let out = run(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let summary = one_record(&stderr, "rewrite_summary");
    assert_eq!(
        summary.number("comments_removed"),
        3,
        "expected aggregate comments_removed=3 across two files:\n{stderr}"
    );
}

#[test]
fn rewrite_summary_record_present_in_dry_run() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "// removed\nfn f() {}\n");
    let out = run_dry(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let summary = one_record(&stderr, "rewrite_summary");
    assert_eq!(
        summary.text("mode"),
        "dry-run",
        "rewrite_summary must be emitted in --dry-run too:\n{stderr}"
    );
}

#[test]
fn strip_summary_is_a_versioned_record_not_a_tab_separated_line() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "// removed\nfn f() {}\n");
    let out = run(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let summary = one_record(&stderr, "strip_summary");
    assert_eq!(summary.text("mode"), "write");
    assert_eq!(summary.number("rewritten"), 1);
    assert_eq!(summary.number("unchanged"), 0);
    assert_eq!(summary.number("errors"), 0);
    assert!(
        !stderr.contains("SUMMARY\t"),
        "the unversioned tab-separated SUMMARY line must be gone:\n{stderr}"
    );
}

#[test]
fn rewrite_record_grammar_constant_documents_record() {
    let grammar = comment_free::REWRITE_RECORD_GRAMMAR;
    for needle in [
        "rewrite_summary",
        "\"mode\":<MODE>",
        "\"comments_removed\":<U32>",
        "\"inline_trimmed\":<U32>",
        "\"blank_lines_collapsed\":<U32>",
        "\"doc_links_rewritten\":<U32>",
        "\"v\":<N>",
    ] {
        assert!(
            grammar.contains(needle),
            "REWRITE_RECORD_GRAMMAR missing `{needle}`:\n{grammar}"
        );
    }
    for gone in ["safety_preserved", "auto_trait_preserved"] {
        assert!(
            !grammar.contains(gone),
            "REWRITE_RECORD_GRAMMAR must not mention `{gone}`:\n{grammar}"
        );
    }
}

#[test]
fn rewrite_record_version_constant_is_two() {
    assert_eq!(comment_free::REWRITE_RECORD_VERSION, 2);
}

#[test]
fn version_flag_exits_zero_and_prints_crate_version() {
    let out = Command::new(bin())
        .arg("--version")
        .output()
        .expect("failed to spawn comment-free");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "exit {:?}", out.status.code());
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected --version output to contain the crate version:\n{stdout}"
    );
}

#[test]
fn record_survives_a_path_containing_a_tab_and_a_newline() {
    let td = tempfile::tempdir().unwrap();
    let src = td.path().join("src");
    fs::create_dir_all(&src).expect("mkdir src");
    let name = "we\tird\nname.rs";
    let doc = "/// w01 w02 w03 w04 w05 w06\npub fn f() {}\n";
    fs::write(src.join(name), doc).expect("write hostile fixture");
    let out = run_lint_budget(td.path(), 5);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let finding = one_record(&stdout, "doc_lint_finding");
    assert!(
        finding.text("path").ends_with(name),
        "the tab and newline in the path must round-trip through the record, got {:?}",
        finding.text("path")
    );
    let hint = one_record(&stdout, "doc_lint_hint");
    assert!(hint.text("path").ends_with(name), "{stdout}");
}

#[test]
fn record_survives_an_item_label_containing_a_tab_and_a_newline() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "/// w01 w02 w03 w04 w05 w06\npub struct S {}\n",
    );
    let out = run_lint_budget(td.path(), 5);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let finding = one_record(&stdout, "doc_lint_finding");
    let item = finding.text("item");
    assert!(
        !item.contains('\t') && !item.contains('\n'),
        "item: {item:?}"
    );
    assert!(!item.is_empty(), "{stdout}");
}

#[test]
fn every_emitted_record_line_parses_strictly() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "/// w01 w02 w03 w04 w05 w06\npub fn f() {}\n",
    );
    let out = run_lint_budget(td.path(), 5);
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        parse_record(line)
            .unwrap_or_else(|e| panic!("emitted line failed strict parse: {line}\n{e:?}"));
    }
}

#[test]
fn parser_rejects_a_duplicate_field() {
    let line = "{\"record\":\"doc_lint_truncated\",\"v\":2,\"kind\":\"overlong_doc\",\"kind\":\"other\",\"remaining\":1}";
    assert_eq!(
        parse_record(line),
        Err(RecordError::DuplicateField("kind".to_string())),
        "a duplicated field must be rejected, never last-value-wins"
    );
}

#[test]
fn parser_rejects_an_unknown_field() {
    let line = "{\"record\":\"doc_lint_truncated\",\"v\":2,\"kind\":\"overlong_doc\",\"remaining\":1,\"surprise\":3}";
    assert_eq!(
        parse_record(line),
        Err(RecordError::UnknownField("surprise".to_string()))
    );
}

#[test]
fn parser_rejects_an_unknown_record_name() {
    let line = "{\"record\":\"doc_lint_indeterminate\",\"v\":2}";
    assert_eq!(
        parse_record(line),
        Err(RecordError::UnknownRecord(
            "doc_lint_indeterminate".to_string()
        )),
        "a consumer that does not know a record must say so, not silently drop it"
    );
}

#[test]
fn parser_rejects_a_record_version_it_does_not_understand() {
    let line =
        "{\"record\":\"doc_lint_truncated\",\"v\":99,\"kind\":\"overlong_doc\",\"remaining\":1}";
    assert_eq!(parse_record(line), Err(RecordError::VersionTooNew(99)));
}

#[test]
fn parser_rejects_malformed_records() {
    for line in [
        "{\"record\":\"doc_lint_truncated\",\"v\":2,\"kind\":\"overlong_doc\",\"remaining\":1",
        "{\"record\":\"doc_lint_truncated\" \"v\":2}",
        "{\"record\":\"doc_lint_truncated\",\"v\":}",
        "{\"record\":\"doc_lint_truncated\",\"v\":2,\"kind\":\"unterminated}",
        "{\"record\":\"doc_lint_truncated\",\"v\":2,\"kind\":\"overlong_doc\",\"remaining\":1} trailing",
        "{\"v\":2,\"kind\":\"overlong_doc\",\"remaining\":1}",
    ] {
        assert!(
            matches!(parse_record(line), Err(RecordError::Malformed(_))),
            "expected a malformed-record rejection for: {line}"
        );
    }
}

#[test]
fn parser_accepts_the_escapes_the_emitter_produces() {
    let line = "{\"record\":\"doc_lint_hint\",\"v\":2,\"outcome\":\"finding\",\"kind\":\"overlong_doc\",\"path\":\"a\\tb\\nc\\u0001d\",\"line\":1,\"item\":\"fn f\",\"words\":9,\"budget\":8}";
    let record = parse_record(line).expect("emitter escapes must round-trip");
    assert_eq!(record.text("path"), "a\tb\nc\u{1}d");
}

#[test]
fn diagnostic_record_version_constant_is_three() {
    assert_eq!(comment_free::DIAGNOSTIC_RECORD_VERSION, 3);
}

#[test]
fn diagnostic_record_grammar_constant_documents_the_family() {
    let grammar = comment_free::DIAGNOSTIC_RECORD_GRAMMAR;
    for needle in [
        "run_error",
        "doc_file_warning",
        "rewrite_file",
        "strip_summary",
        "lint_summary",
        "\"undecided\":<U32>",
        "\"v\":<N>",
        "\"path\":<PATH>",
    ] {
        assert!(
            grammar.contains(needle),
            "DIAGNOSTIC_RECORD_GRAMMAR missing `{needle}`:\n{grammar}"
        );
    }
}

#[test]
fn run_error_survives_a_path_containing_a_tab_and_a_newline() {
    let td = tempfile::tempdir().unwrap();
    let src = td.path().join("src");
    fs::create_dir_all(&src).expect("mkdir src");
    let name = "we\tird\nbroken.rs";
    fs::write(src.join(name), "fn f( {\n").expect("write hostile fixture");
    let out = run(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let err = one_error(&stderr, "parse");
    assert!(
        err.text("path").ends_with(name),
        "the tab and newline in the path must round-trip through run_error, got {:?}",
        err.text("path")
    );
    assert_eq!(
        stderr.lines().filter(|l| l.starts_with('{')).count(),
        3,
        "a hostile path must not forge extra record lines:\n{stderr}"
    );
}

#[test]
fn rewrite_file_record_survives_a_path_containing_a_tab_and_a_newline() {
    let td = tempfile::tempdir().unwrap();
    let src = td.path().join("src");
    fs::create_dir_all(&src).expect("mkdir src");
    let name = "we\tird\nname.rs";
    fs::write(src.join(name), "// kill me\nfn f() {}\n").expect("write hostile fixture");
    let out = run(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        one_record(&stdout, "rewrite_file")
            .text("path")
            .ends_with(name),
        "rewrite_file must JSON-escape a hostile path:\n{stdout}"
    );
}

#[test]
fn doc_file_warning_survives_a_path_containing_a_tab_and_a_newline() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("we\tird\nREADME.md"), "hi\n").expect("write hostile doc");
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = run_dry(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        one_record(&stderr, "doc_file_warning")
            .text("path")
            .ends_with("we\tird\nREADME.md"),
        "doc_file_warning must JSON-escape a hostile path:\n{stderr}"
    );
}

#[test]
fn every_emitted_strip_mode_diagnostic_parses_strictly() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("README.md"), "hi\n").expect("write README");
    write(td.path(), "a.rs", "// removed\nfn f() {}\n");
    write(td.path(), "broken.rs", "fn f( {\n");
    let out = run(td.path());
    for stream in [
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    ] {
        for line in stream.lines().filter(|l| l.starts_with('{')) {
            parse_record(line)
                .unwrap_or_else(|e| panic!("emitted line failed strict parse: {line}\n{e:?}"));
        }
    }
}

#[test]
fn dry_run_diff_body_is_plain_text_and_never_a_record_line() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "// kill me\nfn f() {\n    let x = 1;\n}\n",
    );
    let out = run_dry(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--- a/") && stdout.contains("@@"),
        "the human-facing diff body must stay plain text:\n{stdout}"
    );
    let body: Vec<&str> = stdout.lines().filter(|l| !l.starts_with('{')).collect();
    assert!(
        !body.is_empty(),
        "expected a plain-text diff body alongside the records:\n{stdout}"
    );
    for line in &body {
        assert!(
            line.is_empty() || matches!(line.as_bytes()[0], b' ' | b'-' | b'+' | b'@'),
            "every diff body line is prefixed, so it can never be mistaken for a record: {line:?}"
        );
    }
    for line in stdout.lines().filter(|l| l.starts_with('{')) {
        parse_record(line)
            .unwrap_or_else(|e| panic!("record line failed strict parse: {line}\n{e:?}"));
    }
}

#[test]
fn dry_run_diff_body_prefixes_an_inserted_line_that_opens_with_a_brace() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "fn f() -> i32 {\n    let x =\n{ 1 }; // kill me\n    x\n}\n",
    );
    let out = run_dry(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("+{ 1 };"),
        "an inserted source line opening with `{{` must carry its `+` prefix, \
         or it is indistinguishable from a record line:\n{stdout}"
    );
    for line in stdout.lines().filter(|l| l.starts_with('{')) {
        parse_record(line)
            .unwrap_or_else(|e| panic!("record line failed strict parse: {line}\n{e:?}"));
    }
}

#[test]
fn parser_rejects_a_duplicate_field_on_a_diagnostic_record() {
    let line = "{\"record\":\"run_error\",\"v\":2,\"kind\":\"walk\",\"path\":\"a\",\"path\":\"b\",\"message\":\"m\"}";
    assert_eq!(
        parse_record(line),
        Err(RecordError::DuplicateField("path".to_string())),
        "the newly versioned diagnostics get the same strict treatment"
    );
}

#[test]
fn parser_rejects_an_unknown_field_on_a_diagnostic_record() {
    let line = "{\"record\":\"strip_summary\",\"v\":2,\"mode\":\"write\",\"rewritten\":1,\"unchanged\":0,\"errors\":0,\"surprise\":1}";
    assert_eq!(
        parse_record(line),
        Err(RecordError::UnknownField("surprise".to_string()))
    );
}

#[test]
fn parser_rejects_a_malformed_diagnostic_record() {
    for line in [
        "{\"record\":\"lint_summary\",\"v\":2,\"files\":1,\"findings\":0,\"errors\":0",
        "{\"record\":\"doc_file_warning\",\"v\":2,\"path\":\"unterminated}",
        "{\"record\":\"rewrite_file\",\"v\":2,\"mode\":\"write\",\"path\":}",
    ] {
        assert!(
            matches!(parse_record(line), Err(RecordError::Malformed(_))),
            "expected a malformed-record rejection for: {line}"
        );
    }
}

#[test]
fn a_hostile_root_cannot_forge_a_record_through_the_doc_file_prose_warning() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join(hostile_root_name());
    fs::create_dir_all(&root).expect("mkdir hostile root");
    fs::write(root.join("README.md"), "hi\n").expect("write doc file");
    write(&root, "a.rs", "fn f() {}\n");
    let out = run(&root);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_no_forged_record_line(&stderr, "stderr");
}

#[test]
fn a_hostile_root_cannot_forge_a_record_through_the_dry_run_diff_header() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join(hostile_root_name());
    fs::create_dir_all(&root).expect("mkdir hostile root");
    write(&root, "a.rs", "// kill me\nfn f() {}\n");
    let out = run_dry(&root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_no_forged_record_line(&stdout, "stdout");
}

#[test]
fn a_hostile_root_cannot_forge_a_record_through_the_fatal_error_line() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join(hostile_root_name()).join("absent");
    let out = run(&root);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "ROOT is not a directory");
    assert_no_forged_record_line(&stderr, "stderr");
}

#[test]
fn a_hostile_option_looking_root_cannot_forge_a_record_through_clap() {
    let arg = format!("--{}", hostile_argv_fragment());
    let out = run_argv(&[&arg]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "invalid argv exits 2");
    assert_no_forged_record_line(&stderr, "stderr");
    assert_no_forged_record_line(&stdout, "stdout");
}

#[test]
fn a_hostile_option_value_cannot_forge_a_record_through_clap() {
    let value = hostile_argv_fragment();
    let out = run_argv(&["--doc-max-words", &value]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "invalid argv exits 2");
    assert_no_forged_record_line(&stderr, "stderr");
    assert_no_forged_record_line(&stdout, "stdout");
}

#[test]
fn a_hostile_positional_root_cannot_forge_a_record_through_clap() {
    let out = run_argv(&["--dry-run", &hostile_argv_fragment()]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "invalid argv exits 2");
    assert_no_forged_record_line(&stderr, "stderr");
}

#[test]
fn control_bearing_argv_is_rejected_with_static_prose_only() {
    let arg = format!("--{}", hostile_argv_fragment());
    let out = run_argv(&[&arg]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains(FORGED),
        "clap diagnostics must not echo control-bearing argv at all:\n{stderr}"
    );
    assert_eq!(
        stderr.lines().count(),
        1,
        "one static prose line:\n{stderr}"
    );
}

#[test]
fn a_control_free_argv_error_still_renders_a_clap_diagnostic() {
    let out = run_argv(&["--no-such-flag"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "invalid argv exits 2");
    assert!(
        stderr.contains("--no-such-flag"),
        "control-free argv keeps clap's normal diagnostic:\n{stderr}"
    );
    assert_no_forged_record_line(&stderr, "stderr");
}

#[test]
fn help_and_version_emit_no_column_zero_record_lines() {
    for flags in [["--help"], ["--version"]] {
        let out = run_argv(&flags);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_no_forged_record_line(&stdout, "stdout");
    }
}

#[test]
fn long_help_documents_every_exit_code_the_tool_returns() {
    let out = run_argv(&["--help"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let table = stdout
        .split_once("Exit codes:")
        .expect("long help carries an exit-code table")
        .1;
    for code in ["0", "1", "2", "3", "4", "5"] {
        assert!(
            table
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("{code}  "))),
            "exit code {code} is missing from the long help table:\n{table}"
        );
    }
}

#[test]
fn single_line_collapses_hostile_prose_to_one_line() {
    let hostile = format!("io error: bad thing\n{FORGED}\r\ttail\u{7f}\u{1}");
    let rendered = comment_free::single_line(&hostile);
    assert_eq!(rendered.lines().count(), 1, "one line: {rendered}");
    assert!(!rendered.contains('\n'), "no raw LF: {rendered}");
    assert!(!rendered.contains('\r'), "no raw CR: {rendered}");
    assert!(!rendered.contains('\u{7f}'), "no DEL: {rendered}");
    assert!(rendered.contains("\\u0001"), "C0 escaped: {rendered}");
    assert!(
        !rendered.starts_with('{'),
        "never column-zero record: {rendered}"
    );
}

#[test]
fn parser_rejects_a_diagnostic_record_version_it_does_not_understand() {
    let line = "{\"record\":\"lint_summary\",\"v\":99,\"files\":1,\"findings\":0,\"errors\":0}";
    assert_eq!(parse_record(line), Err(RecordError::VersionTooNew(99)));
}

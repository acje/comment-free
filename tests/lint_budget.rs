mod common;

use common::*;
use std::fmt::Write as _;

#[test]
fn default_mode_is_lint() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// w01 w02 w03 w04 w05 w06 w07 w08 w09 w10\n\
               /// w11 w12 w13 w14 w15 w16 w17 w18 w19 w20\n\
               /// w21 w22 w23 w24 w25 w26 w27 w28 w29 w30\n\
               /// w31 w32 w33 w34 w35 w36 w37 w38 w39 w40\n\
               /// w41 w42 w43 w44 w45 w46 w47 w48 w49 w50\n\
               /// w51 w52 w53 w54 w55 w56 w57 w58 w59 w60\n\
               /// w61 w62 w63 w64 w65 w66 w67 w68 w69 w70\n\
               /// w71 w72 w73 w74 w75 w76 w77 w78 w79 w80\n\
               /// w81 w82 w83 w84 w85 w86 w87 w88 w89 w90\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = Command::new(bin())
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "default mode must be lint, expected exit 4, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    assert_eq!(
        records_named(&stdout, "doc_lint_finding").len(),
        1,
        "missing doc_lint_finding record:\n{stdout}"
    );
    let after = read(td.path(), "a.rs");
    assert_eq!(after, doc, "default mode must not modify files");
}

#[test]
fn lint_within_budget_exits_zero() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "/// one two three four five six seven eight nine ten\npub fn f() {}\n",
    );
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    assert!(
        records(&stdout).is_empty(),
        "no lint record expected within budget:\n{stdout}"
    );
}

#[test]
fn lint_over_budget_exits_four() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// w01 w02 w03 w04 w05 w06 w07 w08 w09 w10\n\
               /// w11 w12 w13 w14 w15 w16 w17 w18 w19 w20\n\
               /// w21 w22 w23 w24 w25 w26 w27 w28 w29 w30\n\
               /// w31 w32 w33 w34 w35 w36 w37 w38 w39 w40\n\
               /// w41 w42 w43 w44 w45 w46 w47 w48 w49 w50\n\
               /// w51 w52 w53 w54 w55 w56 w57 w58 w59 w60\n\
               /// w61 w62 w63 w64 w65 w66 w67 w68 w69 w70\n\
               /// w71 w72 w73 w74 w75 w76 w77 w78 w79 w80\n\
               /// w81 w82 w83 w84 w85 w86 w87 w88 w89 w90\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "expected exit 4, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(finding.number("words"), 90, "wrong words field:\n{stdout}");
    assert_eq!(
        finding.number("budget"),
        80,
        "wrong budget field:\n{stdout}"
    );
    assert!(
        !finding.boolean("fail_closed"),
        "unexpected fail-closed recount:\n{stdout}"
    );
}

#[test]
fn mutually_exclusive_cfg_docs_are_undecided_not_a_finding() {
    let td = tempfile::tempdir().unwrap();
    let unix_doc = prose_words("u", 45);
    let win_doc = prose_words("w", 45);
    let src = format!(
        "#[cfg_attr(unix, doc = \"{unix_doc}\")]\n\
         #[cfg_attr(windows, doc = \"{win_doc}\")]\n\
         pub fn f() {{}}\n"
    );
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "mutually exclusive cfg doc sets must not sum into a finding, but an undecided item \
         is not clean either, so the run must not exit 0, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    assert!(
        records_named(&stdout, "doc_lint_finding").is_empty(),
        "no configuration carries both doc sets:\n{stdout}"
    );
    let undecided = one_record(&stdout, "doc_lint_undecided");
    assert_eq!(undecided.text("outcome"), "configuration_dependent");
    assert_eq!(undecided.text("kind"), "overlong_doc");
    assert_eq!(undecided.number("v"), 3, "record version drift");
    assert_eq!(
        undecided.number("words"),
        0,
        "the unconditional set is empty:\n{stdout}"
    );
    assert_eq!(
        undecided.number("words_all_cfgs"),
        90,
        "the all-configurations upper bound:\n{stdout}"
    );
    assert_eq!(undecided.number("budget"), 80);
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(summary.number("findings"), 0);
    assert_eq!(summary.number("undecided"), 1);
    assert_eq!(summary.number("errors"), 0);
}

#[test]
fn a_conditional_fence_around_unconditional_prose_is_undecided_not_clean() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!(
        "#[cfg_attr(unix, doc = \" ```\")]\n\
         #[doc = \"{long}\"]\n\
         #[cfg_attr(unix, doc = \" ```\")]\n\
         pub fn f() {{}}\n"
    );
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        records_named(&stdout, "doc_lint_finding").is_empty(),
        "a unix build fences the prose, so no finding holds everywhere:\n{stdout}"
    );
    let undecided = one_record(&stdout, "doc_lint_undecided");
    assert_eq!(undecided.text("outcome"), "configuration_dependent");
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(
        summary.number("undecided"),
        1,
        "a conditional fence must never be reported clean:\n{stderr}"
    );
    assert_eq!(summary.number("findings"), 0);
}

#[test]
fn a_raw_spelled_doc_payload_is_not_reported_clean() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "#[r#doc = include_str!(\"x.md\")]\npub fn f() {}\n",
    );
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "r#doc is the same path as doc to rustc, so an unreadable payload spelled \
         raw must not exit clean\nstdout: {stdout}\nstderr: {stderr}"
    );
    let undecided = one_record(&stdout, "doc_lint_undecided");
    assert_eq!(undecided.text("outcome"), "unreadable_doc_payload");
    assert_eq!(undecided.number("v"), 3, "record version drift");
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(summary.number("undecided"), 1);
    assert_eq!(summary.number("findings"), 0);
    assert_eq!(summary.number("errors"), 0);
}

#[test]
fn a_raw_spelled_doc_inside_a_macro_body_is_not_reported_clean() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "pub fn f() { generate! { #[r#doc = \" hidden\"] fn g() {} } }\n",
    );
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "a raw-spelled doc attribute inside an opaque macro body must not exit \
         clean\nstdout: {stdout}\nstderr: {stderr}"
    );
    let undecided = one_record(&stdout, "doc_lint_undecided");
    assert_eq!(undecided.text("outcome"), "uninspected_macro_body");
    assert_eq!(undecided.number("v"), 3, "record version drift");
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(summary.number("undecided"), 1);
    assert_eq!(summary.number("errors"), 0);
}

#[test]
fn a_trivially_true_cfg_attr_doc_reaches_exit_four() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!("#[cfg_attr(all(), doc = \"{long}\")]\npub fn f() {{}}\n");
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "cfg_attr(all(), ...) applies the doc unconditionally\nstdout: {stdout}\nstderr: {stderr}"
    );
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(finding.number("words"), 90);
}

#[test]
fn a_nested_trivially_true_cfg_attr_doc_reaches_exit_four() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!("#[cfg_attr(all(), cfg_attr(all(), doc = \"{long}\"))]\npub fn f() {{}}\n");
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "a nested always-true cfg_attr doc is unconditional\nstdout: {stdout}\nstderr: {stderr}"
    );
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(finding.number("words"), 90);
}

#[test]
fn a_nested_conditional_cfg_attr_doc_is_undecided_not_clean() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!("#[cfg_attr(unix, cfg_attr(windows, doc = \"{long}\"))]\npub fn f() {{}}\n");
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        records_named(&stdout, "doc_lint_finding").is_empty(),
        "{stdout}"
    );
    let undecided = one_record(&stdout, "doc_lint_undecided");
    assert_eq!(undecided.number("words_all_cfgs"), 90);
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(
        summary.number("undecided"),
        1,
        "a nested conditional doc must never be reported clean:\n{stderr}"
    );
}

#[test]
fn a_file_level_trivially_true_cfg_attr_doc_reaches_exit_four() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!("#![cfg_attr(all(), doc = \"{long}\")]\npub fn f() {{}}\n");
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "a file-level always-true cfg_attr doc is unconditional\nstdout: {stdout}\nstderr: {stderr}"
    );
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(finding.text("item"), "file-level");
}

#[test]
fn a_trivially_false_cfg_attr_doc_is_neither_finding_nor_undecided() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!("#[cfg_attr(any(), doc = \"{long}\")]\npub fn f() {{}}\n");
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(summary.number("findings"), 0);
    assert_eq!(
        summary.number("undecided"),
        0,
        "cfg_attr(any(), ...) applies in no configuration:\n{stderr}"
    );
}

#[test]
fn a_literal_true_cfg_attr_doc_reaches_exit_four() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!("#[cfg_attr(true, doc = \"{long}\")]\npub fn f() {{}}\n");
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "cfg_attr(true, ...) applies the doc unconditionally\nstdout: {stdout}\nstderr: {stderr}"
    );
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(finding.number("words"), 90);
}

#[test]
fn a_file_level_literal_true_cfg_attr_doc_reaches_exit_four() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!("#![cfg_attr(true, doc = \"{long}\")]\npub fn f() {{}}\n");
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "a file-level cfg_attr(true, ...) doc is unconditional\nstdout: {stdout}\nstderr: {stderr}"
    );
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(finding.text("item"), "file-level");
    assert_eq!(finding.number("words"), 90);
}

#[test]
fn a_nested_literal_boolean_cfg_attr_doc_reaches_exit_four() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!(
        "#[cfg_attr(all(true), cfg_attr(not(false), doc = \"{long}\"))]\npub fn f() {{}}\n"
    );
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "all(true)/not(false) fold to unconditional\nstdout: {stdout}\nstderr: {stderr}"
    );
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(finding.number("words"), 90);
}

#[test]
fn a_literal_false_cfg_attr_doc_is_neither_finding_nor_undecided() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!("#[cfg_attr(false, doc = \"{long}\")]\npub fn f() {{}}\n");
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(summary.number("findings"), 0);
    assert_eq!(
        summary.number("undecided"),
        0,
        "cfg_attr(false, ...) applies in no configuration:\n{stderr}"
    );
}

#[test]
fn an_overlong_unconditional_doc_beside_cfg_docs_is_still_a_finding() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!(
        "#[doc = \"{long}\"]\n\
         #[cfg_attr(unix, doc = \" extra unix prose\")]\n\
         pub fn f() {{}}\n"
    );
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "an unconditional doc over budget is over budget in every configuration, expected exit 4, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(finding.text("outcome"), "finding");
    assert_eq!(
        finding.number("words"),
        90,
        "a finding reports the count every configuration carries:\n{stdout}"
    );
    assert!(
        records_named(&stdout, "doc_lint_undecided").is_empty(),
        "a proven finding is not also undecided:\n{stdout}"
    );
    assert_eq!(one_record(&stderr, "lint_summary").number("undecided"), 0);
}

#[test]
fn lint_over_budget_emits_header_once_then_hint() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// w01 w02 w03 w04 w05 w06 w07 w08 w09 w10\n\
               /// w11 w12 w13 w14 w15 w16 w17 w18 w19 w20\n\
               /// w21 w22 w23 w24 w25 w26 w27 w28 w29 w30\n\
               /// w31 w32 w33 w34 w35 w36 w37 w38 w39 w40\n\
               /// w41 w42 w43 w44 w45 w46 w47 w48 w49 w50\n\
               /// w51 w52 w53 w54 w55 w56 w57 w58 w59 w60\n\
               /// w61 w62 w63 w64 w65 w66 w67 w68 w69 w70\n\
               /// w71 w72 w73 w74 w75 w76 w77 w78 w79 w80\n\
               /// w81 w82 w83 w84 w85 w86 w87 w88 w89 w90\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let header = one_record(&stdout, "doc_lint_header");
    assert_eq!(header.text("kind"), "overlong_doc");
    assert!(
        header
            .text("doctrine")
            .contains("Rust docs must contain a concise summary"),
        "header should embed the doctrine sentence once:\n{stdout}"
    );
    let hint = one_record(&stdout, "doc_lint_hint");
    assert_eq!(hint.number("words"), 90);
    assert_eq!(hint.number("budget"), 80);
    assert_eq!(hint.text("item"), "fn f");
    assert_eq!(hint.text("kind"), "overlong_doc");
    assert_eq!(hint.text("outcome"), "finding");
    assert_eq!(hint.number("v"), 3, "hint must carry record-version v=3");
    assert!(
        records_named(&stdout, "doc_lint_truncated").is_empty(),
        "no truncation expected for single finding:\n{stdout}"
    );
}

#[test]
fn a_doc_under_budget_only_because_of_code_spans_is_clean() {
    let td = tempfile::tempdir().unwrap();
    let mut doc = doc_prose_lines(70);
    doc.push_str("/// `c01` `c02` `c03` `c04` `c05` `c06` `c07` `c08`\n");
    doc.push_str("/// `c09` `c10` `c11` `c12` `c13` `c14` `c15`\n");
    doc.push_str("pub fn f() {}\n");
    write(td.path(), "a.rs", &doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        records_named(&stdout, "doc_lint_finding").is_empty(),
        "85 tokens but 70 prose words is clean:\n{stdout}"
    );
    assert_eq!(one_record(&stderr, "lint_summary").number("findings"), 0);
    assert_eq!(out.status.code(), Some(0), "clean tree exits 0:\n{stderr}");
}

#[test]
fn code_spans_do_not_buy_a_doc_past_the_budget() {
    let td = tempfile::tempdir().unwrap();
    let mut doc = doc_prose_lines(81);
    doc.push_str("/// `c01` `c02` `c03` `c04` `c05`\n");
    doc.push_str("pub fn f() {}\n");
    write(td.path(), "a.rs", &doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(
        finding.number("words"),
        81,
        "spans are excluded, genuine prose is not:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn a_doc_under_budget_only_because_of_an_indented_code_block_is_clean() {
    let td = tempfile::tempdir().unwrap();
    let mut doc = doc_prose_lines(70);
    doc.push_str("///\n");
    doc.push_str("///     let sample = one_two_three(alpha, beta, gamma);\n");
    doc.push_str("///     let other = four_five_six(delta, epsilon, zeta);\n");
    doc.push_str("///     let third = seven_eight_nine(eta, theta, iota);\n");
    doc.push_str("pub fn f() {}\n");
    write(td.path(), "a.rs", &doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        records_named(&stdout, "doc_lint_finding").is_empty(),
        "an indented code block is code, not prose:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(0), "clean tree exits 0:\n{stderr}");
}

#[test]
fn an_indented_code_block_does_not_buy_a_doc_past_the_budget() {
    let td = tempfile::tempdir().unwrap();
    let mut doc = doc_prose_lines(85);
    doc.push_str("///\n");
    doc.push_str("///     let sample = one_two_three(alpha, beta, gamma);\n");
    doc.push_str("pub fn f() {}\n");
    write(td.path(), "a.rs", &doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(
        finding.number("words"),
        85,
        "indented code is excluded, genuine prose is not:\n{stdout}"
    );
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn lint_header_emitted_once_for_many_findings() {
    let td = tempfile::tempdir().unwrap();
    let mut src = String::new();
    for i in 0..5 {
        for line in 0..9 {
            for w in 0..10 {
                let n = line * 10 + w + 1;
                if w == 0 {
                    src.push_str("/// ");
                }
                write!(src, "w{n:02} ").unwrap();
                if w == 9 {
                    src.push('\n');
                }
            }
        }
        writeln!(src, "pub fn f{i}() {{}}").unwrap();
    }
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        records_named(&stdout, "doc_lint_header").len(),
        1,
        "header must be emitted exactly once regardless of finding count:\n{stdout}"
    );
    assert_eq!(
        records_named(&stdout, "doc_lint_hint").len(),
        5,
        "expected 5 doc_lint_hint records:\n{stdout}"
    );
}

#[test]
fn lint_truncates_hints_beyond_fifty_with_residual() {
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
    assert_eq!(
        records_named(&stdout, "doc_lint_hint").len(),
        50,
        "hints must be capped at 50:\n{stdout}"
    );
    let truncated = one_record(&stdout, "doc_lint_truncated");
    assert_eq!(truncated.text("kind"), "overlong_doc");
    assert_eq!(
        truncated.number("remaining"),
        u32::try_from(n_items - 50).unwrap(),
        "truncation must carry remaining=10 (60 findings - 50 cap)"
    );
}

#[test]
fn lint_hint_record_is_a_json_object_with_named_fields() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// w01 w02 w03 w04 w05 w06\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = run_lint_budget(td.path(), 5);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hint = one_record(&stdout, "doc_lint_hint");
    assert!(hint.text("path").ends_with("a.rs"), "{stdout}");
    assert!(hint.number("line") > 0, "line must be 1-indexed positive");
    assert!(!hint.text("item").is_empty(), "missing hint item");
    assert_eq!(hint.number("words"), 6);
    assert_eq!(hint.number("budget"), 5);
    assert_eq!(hint.text("kind"), "overlong_doc");
    assert_eq!(hint.text("outcome"), "finding");
    assert_eq!(hint.number("v"), 3);
}

#[test]
fn lint_hints_sorted_by_overshoot_descending_before_truncation() {
    let td = tempfile::tempdir().unwrap();
    let mut src = String::new();
    for i in 0..55usize {
        let extra = i + 1;
        for line in 0..9 {
            src.push_str("/// ");
            for w in 0..10 {
                let nw = line * 10 + w + 1;
                write!(src, "w{nw:02} ").unwrap();
            }
            src.push('\n');
        }
        src.push_str("/// ");
        for w in 0..extra {
            write!(src, "x{w:02} ").unwrap();
        }
        src.push('\n');
        writeln!(src, "pub fn f{i}() {{}}").unwrap();
    }
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hint_word_counts: Vec<u32> = records_named(&stdout, "doc_lint_hint")
        .iter()
        .map(|r| r.number("words"))
        .collect();
    assert_eq!(hint_word_counts.len(), 50, "expected 50 hints:\n{stdout}");
    let sorted = {
        let mut s = hint_word_counts.clone();
        s.sort_by(|a, b| b.cmp(a));
        s
    };
    assert_eq!(
        hint_word_counts, sorted,
        "hint word_counts must appear sorted descending; got={hint_word_counts:?}"
    );
    let smallest_kept = *hint_word_counts.iter().min().unwrap();
    assert!(
        smallest_kept >= 96,
        "truncation dropped the wrong tail: smallest kept = {smallest_kept}"
    );
}

#[test]
fn lint_fenced_code_excluded() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// p01 p02 p03 p04 p05 p06 p07 p08 p09 p10\n\
               /// ```\n\
               /// c01 c02 c03 c04 c05 c06 c07 c08 c09 c10\n\
               /// c11 c12 c13 c14 c15 c16 c17 c18 c19 c20\n\
               /// c21 c22 c23 c24 c25 c26 c27 c28 c29 c30\n\
               /// c31 c32 c33 c34 c35 c36 c37 c38 c39 c40\n\
               /// c41 c42 c43 c44 c45 c46 c47 c48 c49 c50\n\
               /// ```\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout:\n{stdout}");
    assert!(
        records(&stdout).is_empty(),
        "fenced code should be excluded:\n{stdout}"
    );
}

#[test]
fn lint_tilde_fenced_code_excluded() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// p01 p02 p03 p04 p05 p06 p07 p08 p09 p10\n\
               /// ~~~\n\
               /// c01 c02 c03 c04 c05 c06 c07 c08 c09 c10\n\
               /// c11 c12 c13 c14 c15 c16 c17 c18 c19 c20\n\
               /// c21 c22 c23 c24 c25 c26 c27 c28 c29 c30\n\
               /// c31 c32 c33 c34 c35 c36 c37 c38 c39 c40\n\
               /// c41 c42 c43 c44 c45 c46 c47 c48 c49 c50\n\
               /// ~~~\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "tilde-fenced example body must be excluded from word budget; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        records(&stdout).is_empty(),
        "tilde-fenced code should be excluded from word count:\n{stdout}"
    );
}

#[test]
fn lint_with_parse_error_exits_five() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "broken.rs", "fn f( {\n");
    let over_budget = "/// w01 w02 w03 w04 w05 w06 w07 w08 w09 w10\n\
                       /// w11 w12 w13 w14 w15 w16 w17 w18 w19 w20\n\
                       /// w21 w22 w23 w24 w25 w26 w27 w28 w29 w30\n\
                       /// w31 w32 w33 w34 w35 w36 w37 w38 w39 w40\n\
                       /// w41 w42 w43 w44 w45 w46 w47 w48 w49 w50\n\
                       pub fn g() {}\n";
    write(td.path(), "wordy.rs", over_budget);
    let out = run_lint(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "parse error during lint must exit 5:\nstderr: {stderr}"
    );
    assert!(
        one_error(&stderr, "parse")
            .text("path")
            .ends_with("broken.rs"),
        "missing parse run_error naming broken.rs:\n{stderr}"
    );
}

#[test]
fn lint_custom_budget_honoured() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "/// one two three four five six\npub fn f() {}\n",
    );
    let out = run_lint_budget(td.path(), 5);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(4), "stdout:\n{stdout}");
    let finding = one_record(&stdout, "doc_lint_finding");
    assert_eq!(finding.number("budget"), 5, "{stdout}");
    assert_eq!(finding.number("words"), 6, "{stdout}");
}

#[cfg(unix)]
#[test]
fn lint_mode_counts_unreadable_directory_as_error_and_exits_5() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "fn f() {}\n");
    let locked = td.path().join("src").join("locked");
    fs::create_dir_all(&locked).expect("mkdir locked");
    make_unreadable(&locked);
    let out = Command::new(bin())
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    make_readable(&locked);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "unreadable directory must exit 5, not clean:\n{stderr}"
    );
    assert!(
        one_error(&stderr, "walk").text("path").contains("locked"),
        "expected a walk run_error naming the unreadable path:\n{stderr}"
    );
    assert_eq!(
        one_record(&stderr, "lint_summary").number("errors"),
        1,
        "expected the run error count to include the walk error:\n{stderr}"
    );
}

#[test]
fn an_unreadable_doc_payload_alone_does_not_exit_zero() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "prose.md", "a b c d e f g h i j\n");
    write(
        td.path(),
        "a.rs",
        "#[doc = include_str!(\"prose.md\")]\npub fn f() {}\n",
    );
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "a doc payload the tool cannot read must not be reported clean\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        records_named(&stdout, "doc_lint_finding").is_empty(),
        "the payload was never read, so no finding is provable:\n{stdout}"
    );
    let undecided = one_record(&stdout, "doc_lint_undecided");
    assert_eq!(undecided.text("outcome"), "unreadable_doc_payload");
    assert_eq!(undecided.text("kind"), "overlong_doc");
    assert_eq!(undecided.number("v"), 3, "record version drift");
    assert_eq!(undecided.text("item"), "fn f");
    assert_eq!(undecided.number("budget"), 80);
    assert_eq!(
        undecided.keys(),
        vec![
            "record", "v", "outcome", "kind", "path", "line", "item", "budget"
        ],
        "an unreadable payload carries no word count: no reading produced one"
    );
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(summary.number("findings"), 0);
    assert_eq!(summary.number("undecided"), 1);
    assert_eq!(summary.number("errors"), 0);
}

#[test]
fn a_configuration_dependent_undecided_alone_does_not_exit_zero() {
    let td = tempfile::tempdir().unwrap();
    let unix_doc = prose_words("u", 45);
    let win_doc = prose_words("w", 45);
    let src = format!(
        "#[cfg_attr(unix, doc = \"{unix_doc}\")]\n\
         #[cfg_attr(windows, doc = \"{win_doc}\")]\n\
         pub fn f() {{}}\n"
    );
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "both indeterminates share one exit rule: a run that could not see everything \
         must not claim it did\nstdout: {stdout}\nstderr: {stderr}"
    );
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(summary.number("findings"), 0);
    assert_eq!(summary.number("undecided"), 1);
}

#[test]
fn a_non_literal_cfg_attr_doc_expression_does_not_exit_zero() {
    let td = tempfile::tempdir().unwrap();
    let src = "#[cfg_attr(all(), doc = concat!(\" a\", \" b c\"))]\npub fn f() {}\n";
    write(td.path(), "a.rs", src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "folding the predicate does not recover a payload the tool cannot read\nstdout: {stdout}\nstderr: {stderr}"
    );
    let undecided = one_record(&stdout, "doc_lint_undecided");
    assert_eq!(undecided.text("outcome"), "unreadable_doc_payload");
}

#[test]
fn a_macro_generated_overlong_doc_does_not_exit_zero() {
    let td = tempfile::tempdir().unwrap();
    let long = prose_words("w", 90);
    let src = format!(
        "macro_rules! noisy {{\n    () => {{\n        #[doc = \"{long}\"]\n        pub fn inner() {{}}\n    }};\n}}\nnoisy!();\n"
    );
    write(td.path(), "a.rs", &src);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "a doc budget bypassed through a macro body must not come back clean\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        records_named(&stdout, "doc_lint_finding").is_empty(),
        "the expansion was never performed, so no finding is provable:\n{stdout}"
    );
    let undecided = records_named(&stdout, "doc_lint_undecided");
    assert_eq!(
        undecided.len(),
        1,
        "the doc attribute lives in the definition's body; the `noisy!()` invocation \
         passes no tokens and is not itself a doc payload:\n{stdout}"
    );
    assert!(
        undecided
            .iter()
            .all(|r| r.text("outcome") == "uninspected_macro_body"),
        "{stdout}"
    );
    assert_eq!(undecided[0].text("item"), "macro noisy");
    assert_eq!(
        undecided[0].keys(),
        vec![
            "record", "v", "outcome", "kind", "path", "line", "item", "budget"
        ],
        "an uninspected body carries no word count: no reading produced one"
    );
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(summary.number("findings"), 0);
    assert_eq!(summary.number("undecided"), 1);
    assert_eq!(summary.number("errors"), 0);
}

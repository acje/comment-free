mod common;

use common::*;

#[test]
fn strips_auto_trait_policy_markers() {
    let td = tempfile::tempdir().unwrap();
    let original = "// AUTO-TRAIT-POLICY-BEGIN\n\
                    // Mission rescue-pardosa-59y0: bucket every pub type.\n\
                    assert_auto_traits! {\n    \
                        SendSync { Foo, Bar }\n    \
                        SendOnly { }\n    \
                        NotSend { }\n\
                    }\n\
                    // AUTO-TRAIT-POLICY-END\n\
                    pub struct Foo;\n\
                    pub struct Bar;\n";
    write(td.path(), "lib.rs", original);
    run(td.path());
    let out = read(td.path(), "lib.rs");
    assert!(
        !out.contains("AUTO-TRAIT-POLICY"),
        "gh-report marker comments are no longer carved out; they must be stripped:\n{out}"
    );
    assert!(
        out.contains("assert_auto_traits"),
        "the macro invocation is code, not a comment, and must survive:\n{out}"
    );
    assert!(
        !out.contains("Mission rescue-pardosa-59y0"),
        "ordinary line comment must be stripped:\n{out}"
    );
}

#[test]
fn strips_auto_trait_policy_markers_around_multiple_macro_blocks() {
    let td = tempfile::tempdir().unwrap();
    let original = "// AUTO-TRAIT-POLICY-BEGIN\n\
                    // Mission rescue-pardosa-59y0: bucket every pub type.\n\
                    assert_auto_traits! {\n    \
                        SendSync { Foo, Bar }\n    \
                        SendOnly { }\n    \
                        NotSend { }\n\
                    }\n\
                    #[cfg(any(test, feature = \"test-support\"))]\n\
                    assert_auto_traits! {\n    \
                        SendSync { Gated }\n\
                    }\n\
                    // AUTO-TRAIT-POLICY-END\n\
                    pub struct Foo;\n\
                    pub struct Bar;\n\
                    #[cfg(any(test, feature = \"test-support\"))]\n\
                    pub struct Gated;\n";
    write(td.path(), "lib.rs", original);
    run(td.path());
    let out = read(td.path(), "lib.rs");
    assert!(
        !out.contains("AUTO-TRAIT-POLICY"),
        "both marker comments must be stripped:\n{out}"
    );
    let macro_count = out.matches("assert_auto_traits").count();
    assert_eq!(
        macro_count, 2,
        "both assert_auto_traits! blocks are code and must survive, found {macro_count}:\n{out}"
    );
    assert!(
        out.contains("test-support"),
        "the cfg-gated second block must survive:\n{out}"
    );
    assert!(
        !out.contains("Mission rescue-pardosa-59y0"),
        "ordinary line comment must be stripped:\n{out}"
    );
}

#[test]
fn auto_trait_policy_markers_unaffected_when_absent() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "lib.rs", "// kill me\nfn f() {}\n");
    run(td.path());
    let out = read(td.path(), "lib.rs");
    assert!(
        !out.contains("AUTO-TRAIT-POLICY"),
        "marker spuriously injected:\n{out}"
    );
    assert!(!out.contains("kill me"), "// not stripped:\n{out}");
}

#[test]
fn preserves_outer_line_doc() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "/// outer doc\nfn f() {}\n");
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(out.contains("outer doc"), "outer /// lost:\n{out}");
}

#[test]
fn preserves_inner_line_doc() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "//! crate-level inner doc\nfn f() {}\n");
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(out.contains("crate-level inner doc"), "//! lost:\n{out}");
}

#[test]
fn preserves_explicit_doc_attr() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "#[doc = \"explicit doc payload\"]\nfn f() {}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("explicit doc payload"),
        "#[doc=\"...\"] lost:\n{out}"
    );
}

#[test]
fn preserves_doc_hidden() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "#[doc(hidden)]\npub fn f() {}\n");
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("doc(hidden)") || out.contains("doc (hidden)"),
        "#[doc(hidden)] lost:\n{out}"
    );
}

#[test]
fn preserves_cfg_attr_doc() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "#[cfg_attr(test, doc = \"gated doc payload\")]\nfn f() {}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("gated doc payload"),
        "cfg_attr doc payload lost:\n{out}"
    );
}

#[test]
fn preserves_doc_inside_macro_rules() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "macro_rules! m {\n    () => {\n        /// inside macro doc\n        fn g() {}\n    };\n}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("inside macro doc") || out.contains("# [doc"),
        "doc inside macro_rules lost:\n{out}"
    );
}

#[test]
fn preserves_outer_doc_on_field_and_variant() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "struct S {\n    /// field doc\n    x: u8,\n}\n\nenum E {\n    /// variant doc\n    V,\n}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(out.contains("field doc"), "field doc lost:\n{out}");
    assert!(out.contains("variant doc"), "variant doc lost:\n{out}");
}

#[test]
fn strips_line_comment_above_item() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "// kill me line comment\nfn f() {}\n");
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(!out.contains("kill me"), "// survived:\n{out}");
}

#[test]
fn strips_block_comment() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "/* kill me block */\nfn f() { let _x = /* inline kill */ 1; }\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(!out.contains("kill me"), "/* */ survived:\n{out}");
    assert!(
        !out.contains("inline kill"),
        "inline /* */ survived:\n{out}"
    );
}

#[test]
fn strips_line_comment_inside_fn_body() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "fn f() {\n    // kill me inner\n    let _x = 1;\n}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(!out.contains("kill me inner"), "inner // survived:\n{out}");
}

#[test]
fn strips_line_comment_inside_macro_invocation() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "fn f() {\n    println!(\n        \"x\" // kill me macro arg\n    );\n}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        !out.contains("kill me macro arg"),
        "// in macro survived:\n{out}"
    );
}

#[test]
fn strips_non_doc_but_preserves_doc_in_same_file() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "//! keep inner\n\n// kill outer line\n/// keep outer\nfn f() {\n    // kill inner line\n    /* kill block */\n    let _x = 1;\n}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(out.contains("keep inner"), "//! lost:\n{out}");
    assert!(out.contains("keep outer"), "/// lost:\n{out}");
    assert!(
        !out.contains("kill outer line"),
        "outer // survived:\n{out}"
    );
    assert!(
        !out.contains("kill inner line"),
        "inner // survived:\n{out}"
    );
    assert!(!out.contains("kill block"), "/* */ survived:\n{out}");
}

#[test]
fn leaves_unparseable_file_untouched() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// keep doc on broken file\n// kill on broken file\nfn f() {\n";
    write(td.path(), "broken.rs", original);
    let out = run(td.path());
    let after = read(td.path(), "broken.rs");
    assert_eq!(after, original, "unparseable file was modified");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let err = one_error(&stderr, "parse");
    assert!(
        err.text("path").ends_with("broken.rs"),
        "parse run_error must name the unparseable file, got: {stderr}"
    );
}

#[test]
fn dry_run_does_not_modify_files() {
    let td = tempfile::tempdir().unwrap();
    let original = "// kill me\nfn f() {}\n";
    write(td.path(), "a.rs", original);
    let _ = run_dry(td.path());
    let after = read(td.path(), "a.rs");
    assert_eq!(after, original, "dry-run modified the file on disk");
}

#[test]
fn dry_run_emits_unified_diff() {
    let td = tempfile::tempdir().unwrap();
    let original = "// kill me\nfn f() {}\n";
    write(td.path(), "a.rs", original);
    let out = run_dry(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        one_record(&stdout, "rewrite_file").text("mode"),
        "dry-run",
        "dry-run must tag the rewrite_file record as dry-run:\n{stdout}"
    );
    assert!(
        stdout.contains("--- a/"),
        "no unified-diff '---' header:\n{stdout}"
    );
    assert!(
        stdout.contains("+++ b/"),
        "no unified-diff '+++' header:\n{stdout}"
    );
    assert!(stdout.contains("@@"), "no hunk marker:\n{stdout}");
    assert!(
        stdout.contains("-// kill me"),
        "removed line not shown in diff:\n{stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        one_record(&stderr, "strip_summary").text("mode"),
        "dry-run",
        "summary missing mode=dry-run on stderr:\n{stderr}"
    );
}

#[test]
fn dry_run_short_flag_works() {
    let td = tempfile::tempdir().unwrap();
    let original = "// kill me\nfn f() {}\n";
    write(td.path(), "a.rs", original);
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("-n")
        .arg(td.path())
        .output()
        .expect("spawn");
    let after = read(td.path(), "a.rs");
    assert_eq!(after, original, "-n modified the file");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        one_record(&stdout, "rewrite_file").text("mode"),
        "dry-run",
        "-n did not produce a dry-run rewrite_file record:\n{stdout}"
    );
}

#[test]
fn dry_run_unchanged_file_emits_no_diff() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "");
    let out = run_dry(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        rewritten_paths(&stdout).is_empty(),
        "spurious rewrite_file record for empty file:\n{stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        one_record(&stderr, "strip_summary").number("unchanged"),
        1,
        "summary did not count file as unchanged on stderr:\n{stderr}"
    );
}

#[test]
fn write_mode_summary_says_mode_write() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "// kill me\nfn f() {}\n");
    let out = run(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        one_record(&stderr, "strip_summary").text("mode"),
        "write",
        "summary missing mode=write on stderr:\n{stderr}"
    );
}

#[test]
fn dry_run_without_rewrite_is_rejected() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--dry-run")
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    assert_eq!(
        out.status.code(),
        Some(2),
        "clap should require --rewrite alongside --dry-run (exit 2), got {:?}",
        out.status.code()
    );
}

#[test]
fn strip_with_parse_error_exits_five() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "broken.rs", "fn f( {\n");
    write(td.path(), "ok.rs", "fn g() {}\n");
    let out = run(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(5),
        "strip-mode per-file error must exit 5:\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        one_error(&stderr, "parse")
            .text("path")
            .ends_with("broken.rs"),
        "missing parse run_error naming broken.rs:\n{stderr}"
    );
}

#[test]
fn strip_reporting_a_typed_failure_exits_five_and_leaves_the_file_untouched() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "broken.rs", "// drop me\nfn f( {\n");
    let out = run(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(5),
        "a typed per-file failure must exit 5:\nstdout: {stdout}\nstderr: {stderr}"
    );
    let failure = one_error(&stderr, "parse");
    assert!(
        failure.text("path").ends_with("broken.rs"),
        "the run_error must name the file it declined:\n{stderr}"
    );
    assert!(
        !failure.text("message").is_empty(),
        "the run_error must carry the failure's own text:\n{stderr}"
    );
    assert_eq!(
        read(td.path(), "broken.rs"),
        "// drop me\nfn f( {\n",
        "a declined file must keep its own bytes"
    );
    let mut residue: Vec<String> = fs::read_dir(td.path().join("src"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    residue.sort();
    assert_eq!(residue, vec!["broken.rs".to_string()]);
}

#[test]
fn a_dry_run_that_would_change_a_file_does_not_report_the_tree_clean() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "d.rs", "// removable\nfn d() {}\n");
    let out = run_dry(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        one_record(&stderr, "strip_summary").number("rewritten"),
        1,
        "the fixture must produce a pending change:\n{stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(3),
        "a dry run holding pending changes has not shown the tree clean, so it must not \
         exit 0:\n{stderr}"
    );
}

#[test]
fn a_dry_run_over_an_already_clean_tree_exits_zero() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "c.rs", "/// kept\nfn c() {}\n");
    let out = run_dry(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        one_record(&stderr, "strip_summary").number("rewritten"),
        0,
        "the fixture must be clean already:\n{stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "nothing pending is a clean tree:\n{stderr}"
    );
}

#[test]
fn writing_the_rewrite_out_exits_zero_even_though_it_changed_files() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "w.rs", "// removable\nfn w() {}\n");
    let out = run(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        one_record(&stderr, "strip_summary").number("rewritten"),
        1,
        "the fixture must be rewritten:\n{stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "write mode was asked to change the tree and did; the tree is clean afterwards:\n{stderr}"
    );
}

#[test]
fn a_run_error_outranks_a_pending_change_in_the_exit_code() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "d.rs", "// removable\nfn d() {}\n");
    write(td.path(), "broken.rs", "fn ( {\n");
    let out = run_dry(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let summary = one_record(&stderr, "strip_summary");
    assert_eq!(
        (summary.number("rewritten"), summary.number("errors")),
        (1, 1),
        "the fixture must hold both a pending change and an error:\n{stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(5),
        "a tree that could not be fully read is a stronger signal than a pending \
         change:\n{stderr}"
    );
}

#[test]
fn rewrite_preserves_code_spacing_when_only_comments_strip() {
    let td = tempfile::tempdir().unwrap();
    let original = "// strip me\n\
                    use std::fmt;\n\
                    pub enum DecodeError {\n    \
                        BufferUnderflow,\n    \
                        TagOutOfRange { tag: u32 },\n\
                    }\n\
                    impl fmt::Display for DecodeError {\n    \
                        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {\n        \
                            match self {\n            \
                                DecodeError::BufferUnderflow => {\n                \
                                    f.write_str(\"buffer underflow: not enough bytes\")\n            \
                                }\n            \
                                DecodeError::TagOutOfRange { tag } => write!(f, \"tag: {tag}\"),\n        \
                            }\n    \
                        }\n\
                    }\n";
    let expected = "\
                    use std::fmt;\n\
                    pub enum DecodeError {\n    \
                        BufferUnderflow,\n    \
                        TagOutOfRange { tag: u32 },\n\
                    }\n\
                    impl fmt::Display for DecodeError {\n    \
                        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {\n        \
                            match self {\n            \
                                DecodeError::BufferUnderflow => {\n                \
                                    f.write_str(\"buffer underflow: not enough bytes\")\n            \
                                }\n            \
                                DecodeError::TagOutOfRange { tag } => write!(f, \"tag: {tag}\"),\n        \
                            }\n    \
                        }\n\
                    }\n";
    write(td.path(), "a.rs", original);
    let out = run(td.path());
    assert!(
        out.status.success(),
        "rewrite failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = read(td.path(), "a.rs");
    assert!(
        !after.contains("strip me"),
        "comment not stripped:\n{after}"
    );
    assert_eq!(
        after, expected,
        "lexer-strip must preserve non-comment bytes byte-identical (no rustfmt reformatting); got:\n{after}"
    );
}

#[test]
fn rewrite_is_fixed_point_on_already_stripped_source() {
    let td = tempfile::tempdir().unwrap();
    let original = "pub fn f() {}\n";
    write(td.path(), "a.rs", original);
    run(td.path());
    let pass1 = read(td.path(), "a.rs");
    run(td.path());
    let pass2 = read(td.path(), "a.rs");
    assert_eq!(pass1, pass2, "second pass must be a fixed point");
    assert_eq!(pass1, original, "already-stripped source must round-trip");
}

#[test]
fn safety_line_comment_is_stripped() {
    let td = tempfile::tempdir().unwrap();
    let original = "fn f() {\n    \
                        // SAFETY: invariants documented in module-level docs\n    \
                        let x = 1;\n    \
                        // kill me ordinary comment\n    \
                        let y = 2;\n    \
                        // SAFETY:no-space-after-colon also matches\n\
                    }\n";
    write(td.path(), "a.rs", original);
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        !out.contains("SAFETY"),
        "the SAFETY carve-out is gone; every non-doc comment must be stripped:\n{out}"
    );
    assert!(
        !out.contains("kill me ordinary comment"),
        "ordinary // line must be stripped:\n{out}"
    );
}

#[test]
fn safety_block_comment_is_stripped() {
    let td = tempfile::tempdir().unwrap();
    let original = "fn f() {\n    \
                        /* SAFETY: this is a block comment, not the // SAFETY idiom */\n    \
                        let _ = 1;\n\
                    }\n";
    write(td.path(), "a.rs", original);
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        !out.contains("SAFETY: this is a block comment"),
        "/* SAFETY: */ block comment must be stripped, got:\n{out}"
    );
}

#[test]
fn string_literal_with_double_slash_marker_text_round_trips_byte_identical() {
    let td = tempfile::tempdir().unwrap();
    let original = r#"pub const FAKE_LINE_COMMENT: &str = "// not actually a comment";
pub const FAKE_BLOCK_COMMENT: &str = "/* also not a comment */";
pub const FAKE_SAFETY: &str = "// SAFETY: this is inside a string literal";
pub fn f() {}
"#;
    write(td.path(), "a.rs", original);
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "characters inside string literals must never be reclassified as comments; got:\n{out}"
    );
}

#[test]
fn raw_string_literal_with_comment_markers_round_trips_byte_identical() {
    let td = tempfile::tempdir().unwrap();
    let original = "pub const RAW: &str = r#\"// kill me\\n/* and me */\\n\"#;\npub fn f() {}\n";
    write(td.path(), "a.rs", original);
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "comment markers inside raw string literals must round-trip byte-identical; got:\n{out}"
    );
}

#[test]
fn auto_trait_policy_markers_stripped_when_surrounding_macro_is_absent() {
    let td = tempfile::tempdir().unwrap();
    let original = "// AUTO-TRAIT-POLICY-BEGIN\n\
                    pub fn f() {}\n\
                    // AUTO-TRAIT-POLICY-END\n";
    write(td.path(), "a.rs", original);
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        !out.contains("AUTO-TRAIT-POLICY"),
        "a lone marker comment must be stripped like any other comment:\n{out}"
    );
    assert!(
        out.contains("pub fn f() {}"),
        "surrounding code must survive:\n{out}"
    );
}

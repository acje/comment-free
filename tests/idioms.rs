mod common;

use common::*;

#[test]
fn rustdoc_link_idioms_is_accepted_as_deprecated_alias() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "/// see [Type](Type) here\npub struct Type;\n",
    );
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--rustdoc-link-idioms")
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    assert!(
        out.status.success(),
        "deprecated --rustdoc-link-idioms must still be accepted (exit 0); got {:?}; stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let out_text = read(td.path(), "a.rs");
    assert!(
        out_text.contains("[`Type`]"),
        "alias must dispatch the same rewrite, got:\n{out_text}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("deprecat"),
        "alias must emit a deprecation note on stderr; got:\n{stderr}"
    );
}

#[test]
fn rustdoc_link_idioms_alone_is_rejected_without_rewrite() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--rustdoc-link-idioms")
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    assert_eq!(
        out.status.code(),
        Some(2),
        "clap should still require --rewrite alongside --rustdoc-link-idioms (exit 2), got {:?}",
        out.status.code()
    );
}

#[test]
fn default_rewrite_rewrites_doc_link_idioms() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type) here\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[`Type`]"),
        "default --rewrite must now normalise doc-link idioms, got:\n{out}"
    );
    assert!(
        !out.contains("[Type](Type)"),
        "redundant explicit link must collapse under default --rewrite, got:\n{out}"
    );
}

#[test]
fn idioms_flag_collapses_redundant_explicit_link() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type) here\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[`Type`]"),
        "expected ticked shortcut after collapse, got:\n{out}"
    );
    assert!(
        !out.contains("[Type](Type)"),
        "redundant explicit link survived, got:\n{out}"
    );
}

#[test]
fn idioms_flag_ticks_shortcut_when_codeish() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// the [Type] applies\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[`Type`]"),
        "expected ticked shortcut, got:\n{out}"
    );
}

#[test]
fn idioms_flag_retains_explicit_target_ticks_label() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// call [begin](Self::begin) first\n\
                    pub struct S;\n\
                    impl S {\n    \
                        pub fn begin(&self) {}\n\
                    }\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[`begin`](Self::begin)"),
        "expected label ticked, target retained, got:\n{out}"
    );
}

#[test]
fn idioms_flag_skips_fenced_code() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// before\n\
                    /// ```\n\
                    /// let _: [Type] = todo!();\n\
                    /// [Type](Type)\n\
                    /// ```\n\
                    /// after\n\
                    pub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("let _: [Type] = todo!();"),
        "fenced [Type] must survive, got:\n{out}"
    );
    assert!(
        out.contains("[Type](Type)"),
        "fenced [Type](Type) must survive, got:\n{out}"
    );
}

#[test]
fn idioms_flag_skips_inline_code_span() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// use `[Type]` syntax verbatim\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("`[Type]`"),
        "inline code span must survive, got:\n{out}"
    );
}

#[test]
fn idioms_flag_skips_url_link() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [docs](https://example.com)\npub fn f() {}\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[docs](https://example.com)"),
        "URL link must survive, got:\n{out}"
    );
}

#[test]
fn idioms_flag_skips_reference_style() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type][ref] later\n\
                    ///\n\
                    /// [ref]: https://example.com\n\
                    pub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[Type][ref]"),
        "reference-style link must survive, got:\n{out}"
    );
    assert!(
        out.contains("[ref]: https://example.com"),
        "reference definition must survive, got:\n{out}"
    );
}

#[test]
fn idioms_flag_skips_prose_label() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [the writer](Writer) for\npub struct Writer;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[the writer](Writer)"),
        "prose label must not be rewritten, got:\n{out}"
    );
}

#[test]
fn idioms_flag_dry_run_does_not_modify_file() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type)\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    let out = run_idioms_dry(td.path());
    let after = read(td.path(), "a.rs");
    assert_eq!(after, original, "dry-run must not write");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        one_record(&stdout, "rewrite_file").text("mode"),
        "dry-run",
        "dry-run must emit a dry-run rewrite_file record:\n{stdout}"
    );
    assert!(
        stdout.contains("[`Type`]"),
        "dry-run diff should show the would-be rewrite, got stdout:\n{stdout}"
    );
}

#[test]
fn idioms_flag_preserves_fence_state_across_doc_lines() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// before [A]\n\
                    /// ```\n\
                    /// [B](B)\n\
                    /// ```\n\
                    /// after [C]\n\
                    pub fn f() {}\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[`A`]"),
        "[A] outside fence should tick:\n{out}"
    );
    assert!(
        out.contains("[B](B)"),
        "[B](B) inside fence must survive:\n{out}"
    );
    assert!(
        out.contains("[`C`]"),
        "[C] outside fence should tick:\n{out}"
    );
}

#[test]
fn safe_idiom_path_does_not_treat_marker_in_string_literal_as_marker() {
    let td = tempfile::tempdir().unwrap();
    let original = r#"pub const BEGIN_MARKER: &str = "// AUTO-TRAIT-POLICY-BEGIN";
pub const END_MARKER: &str = "// AUTO-TRAIT-POLICY-END";
pub fn f() {}
"#;
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "marker text inside string literal must not be treated as a real marker region; safe idiom path must preserve every non-doc byte. got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_does_not_corrupt_fixture_with_marker_and_anchor_in_string_literal() {
    let td = tempfile::tempdir().unwrap();
    let original = "fn build_fixture() -> &'static str {\n    \
                    \"// AUTO-TRAIT-POLICY-BEGIN\\n\\\n                     \
                    assert_auto_traits! {\\n    \\\n                         \
                        SendSync { Foo }\\n\\\n                     \
                    }\\n\\\n                     \
                    // AUTO-TRAIT-POLICY-END\\n\"\n\
                    }\n\
                    pub fn caller() { let _ = build_fixture(); }\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "marker text + assert_auto_traits! anchor *both* inside a string literal must round-trip byte-identical under safe idiom path (this is exactly the end_to_end.rs corruption class). got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_preserves_quote_macro_invocation_byte_identical() {
    let td = tempfile::tempdir().unwrap();
    let original = "fn build_tokens() -> proc_macro2::TokenStream {\n    \
                    let metas: proc_macro2::TokenStream = Default::default();\n    \
                    quote::quote!(#metas)\n\
                    }\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "quote!(#metas) outside any doc comment must round-trip byte-identical under safe idiom path. got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_preserves_preserved_markers_const_byte_identical() {
    let td = tempfile::tempdir().unwrap();
    let original = "pub struct PreservedMarkerPair {\n    \
                    pub begin_token: &'static str,\n    \
                    pub end_token: &'static str,\n    \
                    pub anchor_macro: &'static str,\n\
                    }\n\
                    pub const DEFAULT_PRESERVED_MARKERS: &[PreservedMarkerPair] = &[PreservedMarkerPair {\n    \
                        begin_token: \"AUTO-TRAIT-POLICY-BEGIN\",\n    \
                        end_token: \"AUTO-TRAIT-POLICY-END\",\n    \
                        anchor_macro: \"assert_auto_traits\",\n\
                    }];\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "DEFAULT_PRESERVED_MARKERS struct literal must round-trip byte-identical under safe idiom path. got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_preserves_non_doc_bytes_when_no_doc_changes() {
    let td = tempfile::tempdir().unwrap();
    let original = "use pardosa::store::{Event, FiberId};\n\
                    use pardosa::store::{ExtractError, FiberIndex, FiberLookup};\n\
                    fn _names_used() {\n    \
                        let _: FiberIndex<u64> = FiberIndex::empty();\n    \
                        let _: FiberLookup<FiberId> = FiberLookup::Empty;\n\
                    }\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "safe idiom path must not touch non-doc bytes when no doc-link idioms are present; got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_rewrites_outer_line_doc() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type) here\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, "/// see [`Type`] here\npub struct Type;\n",
        "outer /// doc-link idiom must be rewritten; got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_rewrites_inner_line_doc() {
    let td = tempfile::tempdir().unwrap();
    let original = "//! crate-level [Type](Type) doc\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, "//! crate-level [`Type`] doc\npub struct Type;\n",
        "inner //! doc-link idiom must be rewritten; got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_rewrites_explicit_doc_attr() {
    let td = tempfile::tempdir().unwrap();
    let original = "#[doc = \" see [Type](Type) here\"]\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, "#[doc = \" see [`Type`] here\"]\npub struct Type;\n",
        "#[doc=\"...\"] doc-link idiom must be rewritten; got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_rewrites_cfg_attr_doc() {
    let td = tempfile::tempdir().unwrap();
    let original = "#[cfg_attr(test, doc = \" see [Type](Type) here\")]\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, "#[cfg_attr(test, doc = \" see [`Type`] here\")]\npub struct Type;\n",
        "cfg_attr(_, doc=\"...\") doc-link idiom must be rewritten; got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_rewrites_nested_cfg_attr_doc() {
    let td = tempfile::tempdir().unwrap();
    let original =
        "#[cfg_attr(test, cfg_attr(unix, doc = \" see [Type](Type) here\"))]\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, "#[cfg_attr(test, cfg_attr(unix, doc = \" see [`Type`] here\"))]\npub struct Type;\n",
        "nested cfg_attr doc-link idiom must be rewritten; got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_is_idempotent() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type) and [`Other`]\npub struct Type;\npub struct Other;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let pass1 = read(td.path(), "a.rs");
    run_idioms(td.path());
    let pass2 = read(td.path(), "a.rs");
    assert_eq!(
        pass1, pass2,
        "safe idiom path must be idempotent; pass1:\n{pass1}\npass2:\n{pass2}"
    );
}

#[test]
fn safe_idiom_path_preserves_line_count() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// summary line 1\n\
                    /// see [Type](Type) here\n\
                    ///\n\
                    /// # Errors\n\
                    ///\n\
                    /// none\n\
                    pub struct Type;\n\
                    fn helper() {\n    \
                        let _ = 1;\n\
                    }\n";
    let lines_before = original.matches('\n').count();
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    let lines_after = out.matches('\n').count();
    assert_eq!(
        lines_before, lines_after,
        "safe idiom path must preserve line count; before={lines_before}, after={lines_after}\n--- BEFORE ---\n{original}--- AFTER ---\n{out}"
    );
}

#[test]
fn safe_idiom_path_preserves_block_doc_comment_unchanged() {
    let td = tempfile::tempdir().unwrap();
    let original = "/** see [Type](Type) here */\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.starts_with("/**"),
        "block /** ... */ doc must be left textually as a block doc by the safe path (no AST round-trip); got:\n{out}"
    );
    assert!(
        out.contains("[Type](Type)") || out.contains("[`Type`]"),
        "block doc payload should either be left verbatim (preferred) or be rewritten in place — but it must not be deleted; got:\n{out}"
    );
}

#[test]
fn safe_idiom_path_dry_run_emits_only_doc_line_changes() {
    let td = tempfile::tempdir().unwrap();
    let original = "use std::collections::HashMap;\n\
                    use std::collections::BTreeMap;\n\
                    /// see [Type](Type)\n\
                    pub struct Type;\n\
                    fn helper(m: HashMap<u32, u32>, b: BTreeMap<u32, u32>) -> usize {\n    \
                        m.len() + b.len()\n\
                    }\n";
    write(td.path(), "a.rs", original);
    let out = run_idioms_dry(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let plus_minus: Vec<&str> = stdout
        .lines()
        .filter(|l| {
            (l.starts_with('+') && !l.starts_with("+++"))
                || (l.starts_with('-') && !l.starts_with("---"))
        })
        .collect();
    for line in &plus_minus {
        let body = &line[1..];
        let trimmed = body.trim_start();
        assert!(
            trimmed.starts_with("///")
                || trimmed.starts_with("//!")
                || trimmed.starts_with("#[doc")
                || trimmed.starts_with("#![doc")
                || trimmed.starts_with("#[cfg_attr"),
            "diff line outside doc surface in safe idiom path: {line:?}\nfull stdout:\n{stdout}"
        );
    }
}

#[test]
fn safe_idiom_path_does_not_inject_auto_trait_markers() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type)\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        !out.contains("AUTO-TRAIT-POLICY"),
        "safe idiom path must not invoke marker restoration logic; got:\n{out}"
    );
}

#[test]
fn idioms_flag_preserves_a_shortcut_reference_with_a_definition() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type] here\n\
                    ///\n\
                    /// [Type]: https://example.com/t\n\
                    pub struct Type;\n";
    write(td.path(), "a.rs", original);
    let out = run_idioms(td.path());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        read(td.path(), "a.rs"),
        original,
        "shortcut reference bound to a definition must be byte-identical"
    );
}

#[test]
fn idioms_flag_still_ticks_a_shortcut_with_no_definition() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type] here\n\
                    ///\n\
                    /// [Other]: https://example.com/o\n\
                    pub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[`Type`]"),
        "an unbound shortcut must still be ticked, got:\n{out}"
    );
}

#[test]
fn idioms_flag_matches_definition_labels_case_insensitively() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [type] here\n\
                    ///\n\
                    /// [Type]: https://example.com/t\n\
                    pub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    assert_eq!(
        read(td.path(), "a.rs"),
        original,
        "CommonMark label normalisation must match [type] to [Type]:"
    );
}

#[test]
fn idioms_flag_preserves_multi_backtick_code_spans() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// two ``[Type]`` and three ```[Type]``` spans\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    assert_eq!(
        read(td.path(), "a.rs"),
        original,
        "multi-backtick code spans must be byte-identical"
    );
}

#[test]
fn idioms_flag_treats_an_unmatched_backtick_run_as_literal() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// dangling `` [Type] tail\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    let out = run_idioms(td.path());
    assert_eq!(
        out.status.code(),
        Some(0),
        "unmatched delimiter must not crash the run"
    );
    assert_eq!(
        read(td.path(), "a.rs"),
        "/// dangling `` [`Type`] tail\npub struct Type;\n",
        "rustdoc 1.98 treats an unmatched run as literal, so the link is eligible"
    );
}

#[test]
fn idioms_flag_preserves_a_four_backtick_fence_containing_a_three_backtick_line() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// ````\n/// ```\n/// [Type]\n/// ````\n/// after [Type]\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    let out = run_idioms(td.path());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        read(td.path(), "a.rs"),
        "/// ````\n/// ```\n/// [Type]\n/// ````\n/// after [`Type`]\npub struct Type;\n",
        "a shorter run must not close a longer fence"
    );
}

#[test]
fn idioms_flag_preserves_a_code_span_spanning_a_definition_looking_line() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// ``open\n/// [Type]: /type\n/// [Type]\n/// close``\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    let out = run_idioms(td.path());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        read(td.path(), "a.rs"),
        original,
        "a definition-looking line must not reset an open code span"
    );
}

#[test]
fn idioms_flag_rewrites_a_shortcut_whose_definition_is_indented_code() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type] here\n///\n///     [Type]: /type\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    let out = run_idioms(td.path());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        read(td.path(), "a.rs"),
        "/// see [`Type`] here\n///\n///     [Type]: /type\npub struct Type;\n",
        "four-space indentation is indented code, not a reference definition"
    );
}

use comment_free::{rewrite_rustdoc_link_idioms, single_line, strip_line_comments};
use proptest::collection::vec;
use proptest::prelude::{Just, Strategy, any, prop_oneof, proptest};
use proptest::test_runner::Config;

const CASES: u32 = 256;

fn config() -> Config {
    Config {
        cases: CASES,
        ..Config::default()
    }
}

const STRIP_MODEL_PIECES: &[(&str, Option<&str>)] = &[
    ("let x = 1;", Some("let x = 1;")),
    ("    let y = 2;", Some("    let y = 2;")),
    ("fn f() {}", Some("fn f() {}")),
    ("struct S { a: u32 }", Some("struct S { a: u32 }")),
    ("let s = \"a // b\";", Some("let s = \"a // b\";")),
    (
        "let t = \"/* not a comment */\";",
        Some("let t = \"/* not a comment */\";"),
    ),
    (
        "let u = r#\"raw // text\"#;",
        Some("let u = r#\"raw // text\"#;"),
    ),
    ("/// doc line", Some("/// doc line")),
    ("//! inner doc line", Some("//! inner doc line")),
    ("/** block doc */", Some("/** block doc */")),
    ("\tlet tabbed = 3;", Some("\tlet tabbed = 3;")),
    ("// solo note", None),
    ("    // indented note", None),
    ("/* block */", None),
    ("/* multi\n   still inside */", None),
    ("let z = 3; // trailing note", Some("let z = 3;")),
    ("let w = 4; /* inline block */", Some("let w = 4;")),
];

const STRIP_BLANK_PIECES: &[(&str, Option<&str>)] = &[("", Some("")), ("    ", Some("    "))];

const REWRITE_SPAN_PIECES: &[(&str, &str)] = &[
    ("", ""),
    (
        "Summary prose without any link.",
        "Summary prose without any link.",
    ),
    ("See [Type](Type) for details.", "See [`Type`] for details."),
    ("See [Type] for details.", "See [`Type`] for details."),
    (
        "See [label](Target) for details.",
        "See [`label`](Target) for details.",
    ),
    (
        "Already ticked [`Type`] stays put.",
        "Already ticked [`Type`] stays put.",
    ),
    (
        "A [link](https://example.com) target.",
        "A [link](https://example.com) target.",
    ),
    ("Use [ref][ref] here.", "Use [ref][ref] here."),
    (
        "Inline `code [Type](Type) span` here.",
        "Inline `code [Type](Type) span` here.",
    ),
    ("> quoted [Type] line", "> quoted [`Type`] line"),
    ("- list [Type] item", "- list [`Type`] item"),
    (
        "  indented prose without a link opener",
        "  indented prose without a link opener",
    ),
    (
        "See [module::Type] for details.",
        "See [`module::Type`] for details.",
    ),
    ("Escaped \\[Type] stays.", "Escaped \\[Type] stays."),
];

fn strip_piece_indices(len: usize) -> impl Strategy<Value = Vec<usize>> {
    vec(0..len, 0..12)
}

fn strip_source(indices: &[usize], table: &[(&str, Option<&str>)]) -> String {
    indices
        .iter()
        .map(|index| table[*index].0)
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_expected_output(indices: &[usize], table: &[(&str, Option<&str>)]) -> String {
    let mut out = String::new();
    let mut suppress_separator = false;
    for (position, index) in indices.iter().enumerate() {
        if position > 0 && !suppress_separator {
            out.push('\n');
        }
        suppress_separator = false;
        match table[*index].1 {
            Some(kept) => out.push_str(kept),
            None => {
                if position + 1 < indices.len() {
                    suppress_separator = true;
                }
            }
        }
    }
    out
}

fn non_whitespace(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn blanks_table() -> Vec<(&'static str, Option<&'static str>)> {
    let mut table = STRIP_MODEL_PIECES.to_vec();
    table.extend_from_slice(STRIP_BLANK_PIECES);
    table
}

fn rewrite_span_indices() -> impl Strategy<Value = Vec<usize>> {
    vec(0..REWRITE_SPAN_PIECES.len(), 0..14)
}

fn rewrite_source(indices: &[usize]) -> String {
    indices
        .iter()
        .map(|index| REWRITE_SPAN_PIECES[*index].0)
        .collect::<Vec<_>>()
        .join("\n")
}

fn rewrite_expected_output(indices: &[usize]) -> String {
    indices
        .iter()
        .map(|index| REWRITE_SPAN_PIECES[*index].1)
        .collect::<Vec<_>>()
        .join("\n")
}

fn comment_free_line() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("let x = 1;".to_owned()),
        Just("    let y = 2;".to_owned()),
        Just("fn f() {}".to_owned()),
        Just("struct S { a: u32 }".to_owned()),
        Just("let s = \"a // b\";".to_owned()),
        Just("let t = \"/* not a comment */\";".to_owned()),
        Just("let u = r#\"raw // text\"#;".to_owned()),
        Just("/// doc line".to_owned()),
        Just("//! inner doc line".to_owned()),
        Just("/** block doc */".to_owned()),
        Just("\tlet tabbed = 3;".to_owned()),
    ]
}

fn commented_line() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("// solo note".to_owned()),
        Just("    // indented note".to_owned()),
        Just("let z = 3; // trailing note".to_owned()),
        Just("/* block */".to_owned()),
        Just("let w = 4; /* inline block */".to_owned()),
        Just("/* multi".to_owned()),
        Just("   still inside */".to_owned()),
    ]
}

fn source_line() -> impl Strategy<Value = String> {
    prop_oneof![3 => comment_free_line(), 2 => commented_line()]
}

fn rust_source() -> impl Strategy<Value = String> {
    vec(source_line(), 0..24).prop_map(|lines| lines.join("\n"))
}

fn comment_free_source() -> impl Strategy<Value = String> {
    vec(comment_free_line(), 0..24).prop_map(|lines| lines.join("\n"))
}

fn doc_line() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("Summary prose without any link.".to_owned()),
        Just("See [Type](Type) for details.".to_owned()),
        Just("See [Type] for details.".to_owned()),
        Just("See [label](Target) for details.".to_owned()),
        Just("Already ticked [`Type`] stays put.".to_owned()),
        Just("A [link](https://example.com) target.".to_owned()),
        Just("[ref]: https://example.com".to_owned()),
        Just("Use [ref][ref] here.".to_owned()),
        Just("```".to_owned()),
        Just("```text".to_owned()),
        Just("~~~".to_owned()),
        Just("````".to_owned()),
        Just("  ```".to_owned()),
        Just("   ~~~rust".to_owned()),
        Just("  indented prose without a link opener".to_owned()),
        Just("    let indented = [Type](Type);".to_owned()),
        Just("Inline `code [Type](Type) span` here.".to_owned()),
        Just("> quoted [Type] line".to_owned()),
        Just("- list [Type] item".to_owned()),
    ]
}

fn doc_text() -> impl Strategy<Value = String> {
    vec(doc_line(), 0..20).prop_map(|lines| lines.join("\n"))
}

fn is_c0_or_del(ch: char) -> bool {
    ch < ' ' || ch == '\u{7f}'
}

fn fence_marker_lines(text: &str) -> Vec<(usize, String)> {
    text.split('\n')
        .enumerate()
        .filter(|(_, line)| {
            let indent = line.len() - line.trim_start_matches(' ').len();
            let body = line.trim_start_matches(' ');
            indent <= 3 && (body.starts_with("```") || body.starts_with("~~~"))
        })
        .map(|(index, line)| (index, line.to_owned()))
        .collect()
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn strip_never_panics_on_arbitrary_utf8(src in any::<String>()) {
        let _ = strip_line_comments(&src);
    }

    #[test]
    fn strip_never_panics_on_rust_shaped_source(src in rust_source()) {
        let _ = strip_line_comments(&src);
    }

    #[test]
    fn strip_preserves_every_byte_of_comment_free_source(src in comment_free_source()) {
        assert_eq!(strip_line_comments(&src), src);
    }

    #[test]
    fn strip_equals_independent_model_that_removes_only_comment_pieces(
        indices in strip_piece_indices(STRIP_MODEL_PIECES.len()),
    ) {
        let src = strip_source(&indices, STRIP_MODEL_PIECES);
        let expected = strip_expected_output(&indices, STRIP_MODEL_PIECES);
        assert_eq!(strip_line_comments(&src), expected);
    }

    #[test]
    fn strip_preserves_every_non_comment_non_whitespace_byte_across_blank_lines(
        indices in strip_piece_indices(STRIP_MODEL_PIECES.len() + STRIP_BLANK_PIECES.len()),
    ) {
        let table = blanks_table();
        let src = strip_source(&indices, &table);
        let kept: String = indices
            .iter()
            .filter_map(|index| table[*index].1)
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(
            non_whitespace(&strip_line_comments(&src)),
            non_whitespace(&kept)
        );
    }

    #[test]
    fn strip_is_idempotent_on_rust_shaped_source(src in rust_source()) {
        let once = strip_line_comments(&src);
        assert_eq!(strip_line_comments(&once), once);
    }

    #[test]
    fn strip_is_idempotent_on_arbitrary_utf8(src in any::<String>()) {
        let once = strip_line_comments(&src);
        assert_eq!(strip_line_comments(&once), once);
    }

    #[test]
    fn rewrite_never_panics_on_arbitrary_utf8(doc in any::<String>()) {
        let _ = rewrite_rustdoc_link_idioms(&doc);
    }

    #[test]
    fn rewrite_equals_independent_model_of_allowed_link_span_edits(
        indices in rewrite_span_indices(),
    ) {
        let doc = rewrite_source(&indices);
        let expected = rewrite_expected_output(&indices);
        assert_eq!(rewrite_rustdoc_link_idioms(&doc), expected);
    }

    #[test]
    fn rewrite_is_idempotent(doc in doc_text()) {
        let once = rewrite_rustdoc_link_idioms(&doc);
        assert_eq!(rewrite_rustdoc_link_idioms(&once), once);
    }

    #[test]
    fn rewrite_preserves_line_count(doc in doc_text()) {
        let rewritten = rewrite_rustdoc_link_idioms(&doc);
        assert_eq!(rewritten.split('\n').count(), doc.split('\n').count());
    }

    #[test]
    fn rewrite_leaves_lines_without_a_link_opener_byte_identical(doc in doc_text()) {
        let rewritten = rewrite_rustdoc_link_idioms(&doc);
        let after: Vec<&str> = rewritten.split('\n').collect();
        for (index, line) in doc.split('\n').enumerate() {
            if !line.contains('[') {
                assert_eq!(after[index], line);
            }
        }
    }

    #[test]
    fn rewrite_preserves_fence_marker_lines(doc in doc_text()) {
        let rewritten = rewrite_rustdoc_link_idioms(&doc);
        assert_eq!(fence_marker_lines(&rewritten), fence_marker_lines(&doc));
    }

    #[test]
    fn single_line_never_emits_a_c0_control_or_del(
        text in "[\\x00-\\x1f\\x7f\\u{80}-\\u{9f}a-z ]{0,32}",
    ) {
        assert!(!single_line(&text).chars().any(is_c0_or_del));
    }
}

// PLANT: a non-doc comment inside tests/, which the pre-cf-13 dogfood scope never saw.
/// PLANT w01 w02 w03 w04 w05 w06 w07 w08 w09 w10 w11 w12 w13 w14 w15 w16
/// w17 w18 w19 w20 w21 w22 w23 w24 w25 w26 w27 w28 w29 w30 w31 w32 w33
/// w34 w35 w36 w37 w38 w39 w40 w41 w42 w43 w44 w45 w46 w47 w48 w49 w50
/// w51 w52 w53 w54 w55 w56 w57 w58 w59 w60 w61 w62 w63 w64 w65 w66 w67
/// w68 w69 w70 w71 w72 w73 w74 w75 w76 w77 w78 w79 w80 w81 w82 w83 w84
/// w85 w86 w87 w88 w89 w90 w91 w92 w93 w94 w95 w96 w97 w98 w99 w100
#[test]
fn planted_violation_for_the_cf_13_bite_proof() {}

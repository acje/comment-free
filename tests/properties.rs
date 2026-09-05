//! Property tests pinning the parser invariants the example suite asserts
//! only at fixed points: total-ness on arbitrary UTF-8, byte preservation
//! outside dropped comments and rewritten link spans, idempotence, and
//! fenced-block balance.

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

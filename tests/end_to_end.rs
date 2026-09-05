use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::Command;
const fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_comment-free")
}
fn run(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
fn run_dry(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
fn write(dir: &Path, name: &str, content: &str) {
    let src = dir.join("src");
    fs::create_dir_all(&src).expect("mkdir src");
    fs::write(src.join(name), content).expect("write fixture");
}
fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join("src").join(name)).expect("read fixture")
}

const MAX_RECORD_VERSION: u32 = 2;

#[derive(Debug, PartialEq, Eq)]
enum Field {
    Text(String),
    Number(u32),
    Bool(bool),
}

#[derive(Debug, PartialEq, Eq)]
enum RecordError {
    Malformed(String),
    DuplicateField(String),
    UnknownField(String),
    UnknownRecord(String),
    VersionTooNew(u32),
}

#[derive(Debug, PartialEq, Eq)]
struct Record {
    fields: Vec<(String, Field)>,
}

impl Record {
    fn get(&self, key: &str) -> &Field {
        self.fields.iter().find(|(k, _)| k == key).map_or_else(
            || panic!("record has no field `{key}`: {:?}", self.fields),
            |(_, v)| v,
        )
    }
    fn text(&self, key: &str) -> &str {
        match self.get(key) {
            Field::Text(t) => t,
            other => panic!("field `{key}` is not a string: {other:?}"),
        }
    }
    fn number(&self, key: &str) -> u32 {
        match self.get(key) {
            Field::Number(n) => *n,
            other => panic!("field `{key}` is not a number: {other:?}"),
        }
    }
    fn boolean(&self, key: &str) -> bool {
        match self.get(key) {
            Field::Bool(b) => *b,
            other => panic!("field `{key}` is not a boolean: {other:?}"),
        }
    }
    fn name(&self) -> &str {
        self.text("record")
    }
    fn keys(&self) -> Vec<&str> {
        self.fields.iter().map(|(k, _)| k.as_str()).collect()
    }
}

fn schema(record: &str, outcome: Option<&str>) -> Option<&'static [&'static str]> {
    match record {
        "doc_lint_finding" => Some(&[
            "record",
            "v",
            "outcome",
            "kind",
            "path",
            "line",
            "item",
            "words",
            "budget",
            "fail_closed",
        ]),
        "doc_lint_header" => Some(&["record", "v", "kind", "doctrine"]),
        "doc_lint_hint" => Some(&[
            "record", "v", "outcome", "kind", "path", "line", "item", "words", "budget",
        ]),
        "doc_lint_truncated" => Some(&["record", "v", "kind", "remaining"]),
        "doc_lint_undecided" => match outcome {
            Some("configuration_dependent") => Some(&[
                "record",
                "v",
                "outcome",
                "kind",
                "path",
                "line",
                "item",
                "words",
                "budget",
                "words_all_cfgs",
                "fail_closed",
            ]),
            Some("unreadable_doc_payload" | "uninspected_macro_body") => Some(&[
                "record", "v", "outcome", "kind", "path", "line", "item", "budget",
            ]),
            _ => None,
        },
        "rewrite_summary" => Some(&[
            "record",
            "v",
            "mode",
            "comments_removed",
            "inline_trimmed",
            "blank_lines_collapsed",
            "doc_links_rewritten",
        ]),
        "run_error" => Some(&["record", "v", "kind", "path", "message"]),
        "doc_file_warning" => Some(&["record", "v", "path"]),
        "rewrite_file" => Some(&["record", "v", "mode", "path"]),
        "strip_summary" => Some(&["record", "v", "mode", "rewritten", "unchanged", "errors"]),
        "lint_summary" => Some(&["record", "v", "files", "findings", "undecided", "errors"]),
        _ => None,
    }
}

struct Scanner<'a> {
    src: &'a str,
    pos: usize,
}

impl Scanner<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }
    fn expect(&mut self, want: char) -> Result<(), RecordError> {
        match self.bump() {
            Some(c) if c == want => Ok(()),
            other => Err(RecordError::Malformed(format!(
                "expected `{want}` at byte {}, found {other:?}",
                self.pos
            ))),
        }
    }
    fn string(&mut self) -> Result<String, RecordError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            let c = self
                .bump()
                .ok_or_else(|| RecordError::Malformed("unterminated string".to_string()))?;
            match c {
                '"' => return Ok(out),
                '\\' => out.push(self.escape()?),
                c if (c as u32) < 0x20 => {
                    return Err(RecordError::Malformed(format!(
                        "raw control character U+{:04X} inside string",
                        c as u32
                    )));
                }
                c => out.push(c),
            }
        }
    }
    fn escape(&mut self) -> Result<char, RecordError> {
        let esc = self
            .bump()
            .ok_or_else(|| RecordError::Malformed("unterminated escape".to_string()))?;
        match esc {
            '"' => Ok('"'),
            '\\' => Ok('\\'),
            '/' => Ok('/'),
            'b' => Ok('\u{08}'),
            'f' => Ok('\u{0c}'),
            'n' => Ok('\n'),
            'r' => Ok('\r'),
            't' => Ok('\t'),
            'u' => {
                let mut code = 0u32;
                for _ in 0..4 {
                    let digit = self.bump().and_then(|c| c.to_digit(16)).ok_or_else(|| {
                        RecordError::Malformed("truncated \\u escape".to_string())
                    })?;
                    code = code * 16 + digit;
                }
                char::from_u32(code)
                    .ok_or_else(|| RecordError::Malformed(format!("bad code point U+{code:04X}")))
            }
            other => Err(RecordError::Malformed(format!(
                "unknown escape `\\{other}`"
            ))),
        }
    }
    fn literal(&mut self, want: &str) -> Result<(), RecordError> {
        if self.src[self.pos..].starts_with(want) {
            self.pos += want.len();
            Ok(())
        } else {
            Err(RecordError::Malformed(format!(
                "expected literal `{want}` at byte {}",
                self.pos
            )))
        }
    }
    fn number(&mut self) -> Result<u32, RecordError> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        self.src[start..self.pos]
            .parse()
            .map_err(|_| RecordError::Malformed(format!("bad number at byte {start}")))
    }
    fn value(&mut self) -> Result<Field, RecordError> {
        match self.peek() {
            Some('"') => Ok(Field::Text(self.string()?)),
            Some('t') => self.literal("true").map(|()| Field::Bool(true)),
            Some('f') => self.literal("false").map(|()| Field::Bool(false)),
            Some(c) if c.is_ascii_digit() => self.number().map(Field::Number),
            other => Err(RecordError::Malformed(format!(
                "unsupported value at byte {}: {other:?}",
                self.pos
            ))),
        }
    }
}

fn parse_record(line: &str) -> Result<Record, RecordError> {
    let mut scanner = Scanner { src: line, pos: 0 };
    scanner.expect('{')?;
    let mut fields: Vec<(String, Field)> = Vec::new();
    loop {
        let key = scanner.string()?;
        scanner.expect(':')?;
        let value = scanner.value()?;
        if fields.iter().any(|(k, _)| *k == key) {
            return Err(RecordError::DuplicateField(key));
        }
        fields.push((key, value));
        match scanner.bump() {
            Some(',') => {}
            Some('}') => break,
            other => {
                return Err(RecordError::Malformed(format!(
                    "expected `,` or `}}` at byte {}, found {other:?}",
                    scanner.pos
                )));
            }
        }
    }
    if scanner.pos != line.len() {
        return Err(RecordError::Malformed(format!(
            "trailing bytes after record at byte {}",
            scanner.pos
        )));
    }
    let record = match fields.iter().find(|(k, _)| k == "record") {
        Some((_, Field::Text(name))) => name.clone(),
        _ => return Err(RecordError::Malformed("missing `record` field".to_string())),
    };
    let outcome = match fields.iter().find(|(k, _)| k == "outcome") {
        Some((_, Field::Text(name))) => Some(name.as_str()),
        _ => None,
    };
    let known =
        schema(&record, outcome).ok_or_else(|| RecordError::UnknownRecord(record.clone()))?;
    if let Some((key, _)) = fields.iter().find(|(k, _)| !known.contains(&k.as_str())) {
        return Err(RecordError::UnknownField(key.clone()));
    }
    match fields.iter().find(|(k, _)| k == "v") {
        Some((_, Field::Number(v))) if *v <= MAX_RECORD_VERSION => {}
        Some((_, Field::Number(v))) => return Err(RecordError::VersionTooNew(*v)),
        _ => return Err(RecordError::Malformed("missing `v` field".to_string())),
    }
    Ok(Record { fields })
}

fn records(stream: &str) -> Vec<Record> {
    stream
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| parse_record(l).unwrap_or_else(|e| panic!("unparseable record {l}: {e:?}")))
        .collect()
}

fn records_named(stream: &str, name: &str) -> Vec<Record> {
    records(stream)
        .into_iter()
        .filter(|r| r.name() == name)
        .collect()
}

fn one_record(stream: &str, name: &str) -> Record {
    let mut found = records_named(stream, name);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one `{name}` record:\n{stream}"
    );
    found.remove(0)
}

fn error_records(stream: &str, kind: &str) -> Vec<Record> {
    records_named(stream, "run_error")
        .into_iter()
        .filter(|r| r.text("kind") == kind)
        .collect()
}

fn one_error(stream: &str, kind: &str) -> Record {
    let mut found = error_records(stream, kind);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one `{kind}` run_error record:\n{stream}"
    );
    found.remove(0)
}

fn rewritten_paths(stream: &str) -> Vec<String> {
    records_named(stream, "rewrite_file")
        .into_iter()
        .map(|r| r.text("path").to_string())
        .collect()
}

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
fn doc_warn_emits_when_root_is_dot() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("README.md"), "hi\n").expect("write README");
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(".")
        .current_dir(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        one_record(&stderr, "doc_file_warning")
            .text("path")
            .ends_with("README.md"),
        "doc_file_warning missing when ROOT='.':\n{stderr}"
    );
}
#[test]
fn scan_doc_files_skips_vendor_dirs() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("README.md"), "root\n").expect("write README");
    for sub in ["node_modules", "vendor", "dist", "build", "target"] {
        std::fs::create_dir_all(td.path().join(sub)).expect("mkdir");
        std::fs::write(td.path().join(sub).join("README.md"), "nested\n").expect("write");
    }
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(".")
        .current_dir(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let warned = records_named(&stderr, "doc_file_warning");
    assert_eq!(
        warned.len(),
        1,
        "expected exactly 1 doc_file_warning (root README.md):\n{stderr}"
    );
    for sub in ["node_modules", "vendor", "dist", "build", "target"] {
        assert!(
            !warned[0].text("path").contains(sub),
            "doc_file_warning unexpectedly reported file under {sub}/:\n{stderr}"
        );
    }
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
fn run_lint(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
fn run_lint_budget(root: &Path, budget: usize) -> std::process::Output {
    Command::new(bin())
        .arg(format!("--doc-max-words={budget}"))
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
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
fn prose_words(prefix: &str, count: usize) -> String {
    let mut out = String::new();
    for n in 1..=count {
        out.push_str(prefix);
        out.push_str(&n.to_string());
        out.push(' ');
    }
    out
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
    assert_eq!(undecided.number("v"), 2, "record version drift");
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
    assert_eq!(undecided.number("v"), 2, "record version drift");
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
    assert_eq!(undecided.number("v"), 2, "record version drift");
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
    assert_eq!(hint.number("v"), 2, "hint must carry record-version v=2");
    assert!(
        records_named(&stdout, "doc_lint_truncated").is_empty(),
        "no truncation expected for single finding:\n{stdout}"
    );
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
    assert!(!hint.text("item").is_empty());
    assert_eq!(hint.number("words"), 6);
    assert_eq!(hint.number("budget"), 5);
    assert_eq!(hint.text("kind"), "overlong_doc");
    assert_eq!(hint.text("outcome"), "finding");
    assert_eq!(hint.number("v"), 2);
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
#[test]
fn dry_run_processes_crates_and_src_but_skips_target_and_docs() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("src/lib.rs"), "// removable\nfn s() {}\n").expect("write src");
    fs::create_dir_all(root.join("crates/foo/src")).expect("mkdir crates/foo/src");
    fs::write(
        root.join("crates/foo/src/lib.rs"),
        "// removable\nfn c() {}\n",
    )
    .expect("write crates");
    fs::create_dir_all(root.join("target/package/foo-0.1.0/src")).expect("mkdir target subtree");
    fs::write(
        root.join("target/package/foo-0.1.0/src/lib.rs"),
        "// removable\nfn t() {}\n",
    )
    .expect("write target");
    fs::create_dir_all(root.join("docs")).expect("mkdir docs");
    fs::write(root.join("docs/example.rs"), "// removable\nfn d() {}\n").expect("write docs");
    fs::create_dir_all(root.join("scripts")).expect("mkdir scripts");
    fs::write(root.join("scripts/helper.rs"), "// removable\nfn h() {}\n").expect("write scripts");
    let out = run_dry(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    let paths = rewritten_paths(&stdout);
    assert!(
        paths.iter().any(|p| p.ends_with("src/lib.rs")),
        "expected a src/lib.rs rewrite_file record:\n{stdout}"
    );
    assert!(
        paths.iter().any(|p| p.contains("crates/foo/src/lib.rs")),
        "expected a crates/foo/src/lib.rs rewrite_file record:\n{stdout}"
    );
    for forbidden in ["target/package", "docs/example.rs", "scripts/helper.rs"] {
        assert!(
            !stdout.contains(forbidden),
            "unexpected out-of-scope path `{forbidden}` in dry-run output:\n{stdout}"
        );
    }
}
#[test]
fn lint_processes_crates_and_src_but_skips_target_and_docs() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(
        root.join("src/lib.rs"),
        "/// one two three four five six\npub fn s() {}\n",
    )
    .expect("write src");
    fs::create_dir_all(root.join("target/package/foo-0.1.0/src")).expect("mkdir target subtree");
    fs::write(
        root.join("target/package/foo-0.1.0/src/lib.rs"),
        "/// one two three four five six\npub fn t() {}\n",
    )
    .expect("write target");
    fs::create_dir_all(root.join("docs")).expect("mkdir docs");
    fs::write(
        root.join("docs/example.rs"),
        "/// one two three four five six\npub fn d() {}\n",
    )
    .expect("write docs");
    let out = run_lint_budget(root, 5);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "expected exit 4 (one in-scope finding); stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let finding = one_record(&stdout, "doc_lint_finding");
    assert!(
        finding.text("path").ends_with("src/lib.rs"),
        "expected the finding to cite src/lib.rs:\n{stdout}"
    );
    for forbidden in ["target/package", "docs/example.rs"] {
        assert!(
            !stdout.contains(forbidden),
            "out-of-scope path `{forbidden}` leaked into lint output:\n{stdout}"
        );
    }
}
#[test]
fn non_rust_files_under_allowed_roots_are_ignored() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("src/lib.rs"), "// removable\nfn s() {}\n").expect("write rs");
    fs::write(root.join("src/script.py"), "# python\nprint('hi')\n").expect("write py");
    fs::write(root.join("src/notes.md"), "# notes\n").expect("write md");
    let out = run_dry(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "exit {:?}", out.status.code());
    assert!(
        stdout.contains("src/lib.rs"),
        "expected src/lib.rs in dry-run output:\n{stdout}"
    );
    for forbidden in ["script.py", "notes.md"] {
        assert!(
            !stdout.contains(forbidden),
            "non-.rs file `{forbidden}` must not be processed:\n{stdout}"
        );
    }
}
#[test]
fn root_without_crates_or_src_processes_nothing() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::create_dir_all(root.join("docs")).expect("mkdir docs");
    fs::write(root.join("docs/example.rs"), "// removable\nfn d() {}\n").expect("write docs");
    fs::write(root.join("stray.rs"), "// removable\nfn x() {}\n").expect("write stray");
    let out = run_dry(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        rewritten_paths(&stdout).is_empty(),
        "no rewrites expected when root has no allowed source tree:\n{stdout}"
    );
    let summary = one_record(&stderr, "strip_summary");
    assert_eq!(
        (
            summary.number("rewritten"),
            summary.number("unchanged"),
            summary.number("errors")
        ),
        (0, 0, 0),
        "summary should reflect zero work:\n{stderr}"
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
fn root_inside_crates_subtree_is_processed_directly() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    let scoped = root.join("crates/foo");
    fs::create_dir_all(scoped.join("src")).expect("mkdir crates/foo/src");
    fs::write(scoped.join("src/lib.rs"), "// removable\nfn c() {}\n").expect("write");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(&scoped)
        .output()
        .expect("failed to spawn comment-free");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "exit {:?}", out.status.code());
    assert!(
        rewritten_paths(&stdout)
            .iter()
            .any(|p| p.ends_with("src/lib.rs")),
        "expected a src/lib.rs rewrite_file record when ROOT is inside crates/:\n{stdout}"
    );
}
#[test]
fn ancestor_named_src_does_not_widen_the_supplied_root() {
    let td = tempfile::tempdir().unwrap();
    let scoped = td.path().join("src/checkout");
    fs::create_dir_all(scoped.join("src")).expect("mkdir src/checkout/src");
    fs::write(scoped.join("src/lib.rs"), "// removable\nfn c() {}\n").expect("write lib");
    fs::write(scoped.join("stray.rs"), "// removable\nfn s() {}\n").expect("write stray");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(&scoped)
        .output()
        .expect("failed to spawn comment-free");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "exit {:?}", out.status.code());
    let paths = rewritten_paths(&stdout);
    assert!(
        paths.iter().any(|p| p.ends_with("src/lib.rs")),
        "expected the supplied root's own src/lib.rs:\n{stdout}"
    );
    assert!(
        !paths.iter().any(|p| p.ends_with("stray.rs")),
        "an ancestor named src must not widen traversal to the whole supplied root:\n{stdout}"
    );
}
fn run_idioms(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg("--rustdoc-link-idioms")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
fn run_idioms_dry(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg("--rustdoc-link-idioms")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
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
        assert_eq!(hint.number("v"), 2, "record version drift");
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
fn doc_lint_record_version_is_two() {
    assert_eq!(
        comment_free::DOC_LINT_RECORD_VERSION,
        2,
        "v=2 is the escaped JSON Lines record format"
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
    assert_eq!(truncated.number("v"), 2);
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
    assert_eq!(header.number("v"), 2);
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
            summary.fields.iter().any(|(k, _)| k == field),
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
#[cfg(unix)]
fn make_unreadable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    assert!(
        fs::read_dir(dir).is_err(),
        "precondition: {} must be unreadable (are you running as root?)",
        dir.display()
    );
}
#[cfg(unix)]
fn make_readable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).expect("chmod 755");
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
#[cfg(unix)]
#[test]
fn doc_scan_counts_unreadable_directory_as_error_and_exits_5() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "fn f() {}\n");
    let locked = td.path().join("locked");
    fs::create_dir_all(&locked).expect("mkdir locked");
    make_unreadable(&locked);
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    make_readable(&locked);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "unreadable documentation-scan directory must exit 5, not clean:\n{stderr}"
    );
    assert!(
        one_error(&stderr, "walk").text("path").contains("locked"),
        "expected a walk run_error naming the unreadable path:\n{stderr}"
    );
    assert_eq!(
        one_record(&stderr, "strip_summary").number("errors"),
        1,
        "expected the run error count to include the doc-scan walk error:\n{stderr}"
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
fn diagnostic_record_version_constant_is_two() {
    assert_eq!(comment_free::DIAGNOSTIC_RECORD_VERSION, 2);
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

const FORGED: &str = "{\"record\":\"forged\",\"v\":2}";

fn hostile_root_name() -> String {
    format!("hostile\n{FORGED}")
}

fn assert_no_forged_record_line(stream: &str, label: &str) {
    for line in stream.lines().filter(|l| l.starts_with('{')) {
        assert!(
            parse_record(line).is_ok(),
            "a hostile ROOT forged a record line on {label}: {line}\n\
             full stream:\n{stream}"
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

fn run_argv(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("failed to spawn comment-free")
}

fn hostile_argv_fragment() -> String {
    format!("hostile\n{FORGED}\n")
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
    assert_eq!(undecided.number("v"), 2, "record version drift");
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

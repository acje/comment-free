#![expect(
    dead_code,
    reason = "each integration binary uses a subset of the shared fixture helpers"
)]

pub(crate) use std::fs;
pub(crate) use std::path::Path;
pub(crate) use std::process::Command;
pub(crate) const fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_comment-free")
}
pub(crate) fn run(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
pub(crate) fn run_dry(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
pub(crate) fn assert_pending_changes(out: &std::process::Output, context: &str) {
    assert_eq!(
        out.status.code(),
        Some(3),
        "{context}: a dry run holding pending changes has not shown the tree clean:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
pub(crate) fn write(dir: &Path, name: &str, content: &str) {
    let src = dir.join("src");
    fs::create_dir_all(&src).expect("mkdir src");
    fs::write(src.join(name), content).expect("write fixture");
}
pub(crate) fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join("src").join(name)).expect("read fixture")
}

pub(crate) const MAX_RECORD_VERSION: u32 = 3;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Field {
    Text(String),
    Number(u32),
    Bool(bool),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RecordError {
    Malformed(String),
    DuplicateField(String),
    UnknownField(String),
    UnknownRecord(String),
    VersionTooNew(u32),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Record {
    fields: Vec<(String, Field)>,
}

impl Record {
    pub(crate) fn get(&self, key: &str) -> &Field {
        self.fields.iter().find(|(k, _)| k == key).map_or_else(
            || panic!("record has no field `{key}`: {:?}", self.fields),
            |(_, v)| v,
        )
    }
    pub(crate) fn text(&self, key: &str) -> &str {
        match self.get(key) {
            Field::Text(t) => t,
            Field::Number(_) | Field::Bool(_) => panic!("field `{key}` is not a string"),
        }
    }
    pub(crate) fn number(&self, key: &str) -> u32 {
        match self.get(key) {
            Field::Number(n) => *n,
            Field::Text(_) | Field::Bool(_) => panic!("field `{key}` is not a number"),
        }
    }
    pub(crate) fn boolean(&self, key: &str) -> bool {
        match self.get(key) {
            Field::Bool(b) => *b,
            Field::Text(_) | Field::Number(_) => panic!("field `{key}` is not a boolean"),
        }
    }
    pub(crate) fn name(&self) -> &str {
        self.text("record")
    }
    pub(crate) fn keys(&self) -> Vec<&str> {
        self.fields.iter().map(|(k, _)| k.as_str()).collect()
    }
}

pub(crate) fn schema(record: &str, outcome: Option<&str>) -> Option<&'static [&'static str]> {
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
        "lint_summary" => Some(&[
            "record",
            "v",
            "files",
            "findings",
            "undecided",
            "errors",
            "root",
            "scope",
            "max_warning_files",
            "warning_files",
            "warning_files_shown",
            "warning_files_hidden",
            "findings_shown",
            "findings_hidden",
            "undecided_shown",
            "undecided_hidden",
            "overlong_doc_findings",
            "overlong_doc_undecided",
            "over_budget",
            "configuration_dependent",
            "unreadable_doc_payload",
            "uninspected_macro_body",
        ]),
        _ => None,
    }
}

pub(crate) struct Scanner<'a> {
    src: &'a str,
    pos: usize,
}

impl Scanner<'_> {
    pub(crate) fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }
    pub(crate) fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }
    pub(crate) fn expect(&mut self, want: char) -> Result<(), RecordError> {
        match self.bump() {
            Some(c) if c == want => Ok(()),
            other => Err(RecordError::Malformed(format!(
                "expected `{want}` at byte {}, found {other:?}",
                self.pos
            ))),
        }
    }
    pub(crate) fn string(&mut self) -> Result<String, RecordError> {
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
    pub(crate) fn escape(&mut self) -> Result<char, RecordError> {
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
    pub(crate) fn literal(&mut self, want: &str) -> Result<(), RecordError> {
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
    pub(crate) fn number(&mut self) -> Result<u32, RecordError> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        self.src[start..self.pos]
            .parse()
            .map_err(|_| RecordError::Malformed(format!("bad number at byte {start}")))
    }
    pub(crate) fn value(&mut self) -> Result<Field, RecordError> {
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

pub(crate) fn parse_record(line: &str) -> Result<Record, RecordError> {
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

pub(crate) fn records(stream: &str) -> Vec<Record> {
    stream
        .lines()
        .filter(|l| l.starts_with('{'))
        .map(|l| parse_record(l).unwrap_or_else(|e| panic!("unparseable record {l}: {e:?}")))
        .collect()
}

pub(crate) fn records_named(stream: &str, name: &str) -> Vec<Record> {
    records(stream)
        .into_iter()
        .filter(|r| r.name() == name)
        .collect()
}

pub(crate) fn one_record(stream: &str, name: &str) -> Record {
    let mut found = records_named(stream, name);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one `{name}` record:\n{stream}"
    );
    found.remove(0)
}

pub(crate) fn error_records(stream: &str, kind: &str) -> Vec<Record> {
    records_named(stream, "run_error")
        .into_iter()
        .filter(|r| r.text("kind") == kind)
        .collect()
}

pub(crate) fn one_error(stream: &str, kind: &str) -> Record {
    let mut found = error_records(stream, kind);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one `{kind}` run_error record:\n{stream}"
    );
    found.remove(0)
}

pub(crate) fn rewritten_paths(stream: &str) -> Vec<String> {
    records_named(stream, "rewrite_file")
        .into_iter()
        .map(|r| r.text("path").to_string())
        .collect()
}

pub(crate) fn run_lint(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
pub(crate) fn run_lint_budget(root: &Path, budget: usize) -> std::process::Output {
    Command::new(bin())
        .arg(format!("--doc-max-words={budget}"))
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}

pub(crate) fn prose_words(prefix: &str, count: usize) -> String {
    let mut out = String::new();
    for n in 1..=count {
        out.push_str(prefix);
        out.push_str(&n.to_string());
        out.push(' ');
    }
    out
}

pub(crate) fn doc_prose_lines(count: usize) -> String {
    let mut out = String::new();
    for i in 1..=count {
        out.push_str("/// w");
        out.push_str(&i.to_string());
        out.push('\n');
    }
    out
}

pub(crate) fn manifest_policy_fixture(td: &Path) -> std::path::PathBuf {
    let repo = td.join("repo");
    fs::create_dir_all(repo.join("src/module")).expect("mkdir repo/src/module");
    fs::write(repo.join("src/module/mod.rs"), "// removable\nfn m() {}\n").expect("write mod");
    repo
}

pub(crate) fn run_idioms(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg("--rustdoc-link-idioms")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
pub(crate) fn run_idioms_dry(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg("--rustdoc-link-idioms")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}

#[cfg(unix)]
pub(crate) fn make_unreadable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    assert!(
        fs::read_dir(dir).is_err(),
        "precondition: {} must be unreadable (are you running as root?)",
        dir.display()
    );
}
#[cfg(unix)]
pub(crate) fn make_readable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).expect("chmod 755");
}

pub(crate) const FORGED: &str = "{\"record\":\"forged\",\"v\":2}";

pub(crate) fn hostile_root_name() -> String {
    format!("hostile\n{FORGED}")
}

pub(crate) fn assert_no_forged_record_line(stream: &str, label: &str) {
    for line in stream.lines().filter(|l| l.starts_with('{')) {
        assert!(
            parse_record(line).is_ok(),
            "a hostile ROOT forged a record line on {label}: {line}\n\
             full stream:\n{stream}"
        );
    }
}

pub(crate) fn run_argv(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("failed to spawn comment-free")
}

pub(crate) fn hostile_argv_fragment() -> String {
    format!("hostile\n{FORGED}\n")
}

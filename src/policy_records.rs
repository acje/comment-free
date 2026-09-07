use super::{Accounting, BudgetTotals, Counts, Fault, Policy, Verdict};
use comment_free::{DocFinding, DocUndecided, ReportScope, UndecidedCause, WarningLimit};
use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;

#[derive(Clone, Copy)]
pub(crate) enum Threshold {
    Advisory,
    Enforced,
}

impl Threshold {
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Advisory => 0,
            Self::Enforced => 1,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Advisory => "advisory",
            Self::Enforced => "enforced",
        }
    }
}

struct Object(String);

fn quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0}'..='\u{1f}' => {
                write!(out, "\\u{:04x}", u32::from(c))
                    .expect("writing to an in-memory String is infallible");
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Object {
    fn new() -> Self {
        Self(String::from("{"))
    }

    fn field(&mut self, key: &str, value: &str) {
        if self.0.len() > 1 {
            self.0.push(',');
        }
        self.0.push_str(&quoted(key));
        self.0.push(':');
        self.0.push_str(value);
    }

    fn text(&mut self, key: &str, value: &str) {
        self.field(key, &quoted(value));
    }
    fn number(&mut self, key: &str, value: impl std::fmt::Display) {
        self.field(key, &value.to_string());
    }
    fn boolean(&mut self, key: &str, value: bool) {
        self.field(key, if value { "true" } else { "false" });
    }
    fn finish(mut self) -> String {
        self.0.push('}');
        self.0
    }

    fn counts(&mut self, name: &str, counts: Counts) {
        self.number(name, counts.total());
        self.number(&format!("{name}_shown"), counts.shown());
        self.number(&format!("{name}_hidden"), counts.hidden());
    }
}

fn budget(totals: &BudgetTotals, max_words: usize) -> String {
    let mut out = Object::new();
    out.number("max_words", max_words);
    for (name, counts) in [
        ("findings", totals.findings),
        ("undecided", totals.undecided),
        ("warning_files", totals.warning_files),
        ("configuration_dependent", totals.configuration_dependent),
        ("unreadable_doc_payload", totals.unreadable_doc_payload),
        ("uninspected_macro_body", totals.uninspected_macro_body),
    ] {
        out.counts(name, counts);
    }
    out.number("over_budget", totals.findings.total());
    out.finish()
}

pub(crate) fn summary(
    policy: &Policy,
    root: &Path,
    scope: ReportScope,
    limit: WarningLimit,
) -> Result<String, Fault> {
    let (files, errors) = match policy.accounting {
        Accounting::Exact { files, errors } => (files, errors),
        Accounting::Failed(fault) => return Err(fault),
    };
    let mut out = Object::new();
    out.text("kind", "policy_summary");
    out.number("version", 1);
    out.text("root", &root.to_string_lossy());
    out.text("scope", scope.as_str());
    out.number("files", files);
    out.number("errors", errors);
    out.text("max_warning_files", &limit.to_string());
    out.text(
        "verdict",
        match policy.verdict() {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Unknown => "unknown",
        },
    );
    out.field(
        "reasons",
        &format!(
            "[{}]",
            policy
                .reasons()
                .iter()
                .map(|r| quoted(r))
                .collect::<Vec<_>>()
                .join(",")
        ),
    );
    out.field(
        "advisory",
        &budget(&policy.advisory, policy.thresholds.advisory),
    );
    out.field(
        "enforced",
        &budget(&policy.enforced, policy.thresholds.enforced),
    );
    Ok(out.finish())
}

#[derive(Clone, Copy)]
pub(crate) enum Event<'a> {
    Finding(&'a Path, &'a DocFinding),
    Undecided(&'a Path, &'a DocUndecided),
    Hint(&'a Path, &'a DocFinding),
    Header,
    Truncated(u32),
}

pub(crate) fn detail(threshold: Threshold, event: Event<'_>) -> String {
    let mut out = Object::new();
    out.text("kind", "policy_detail");
    out.number("version", 1);
    out.text("threshold", threshold.name());
    out.text(
        "event",
        match event {
            Event::Finding(..) => "doc_lint_finding",
            Event::Undecided(..) => "doc_lint_undecided",
            Event::Hint(..) => "doc_lint_hint",
            Event::Header => "doc_lint_header",
            Event::Truncated(_) => "doc_lint_truncated",
        },
    );
    match event {
        Event::Finding(path, finding) | Event::Hint(path, finding) => {
            out.text("outcome", "finding");
            location(
                &mut out,
                path,
                finding.line(),
                finding.item_label(),
                finding.budget(),
            );
            out.number("words", finding.words().count());
            if matches!(event, Event::Finding(..)) {
                out.boolean("fail_closed", finding.words().is_fail_closed());
            }
        }
        Event::Undecided(path, item) => {
            out.text("outcome", item.outcome().as_str());
            location(
                &mut out,
                path,
                item.line(),
                item.item_label(),
                item.budget(),
            );
            if let UndecidedCause::ConfigurationDependent {
                unconditional,
                all_configurations,
            } = item.cause()
            {
                out.number("words", unconditional.count());
                out.number("words_all_cfgs", all_configurations.count());
                out.boolean("fail_closed", all_configurations.is_fail_closed());
            }
        }
        Event::Header => out.text("doctrine", comment_free::DOC_LINT_DOCTRINE_MSG),
        Event::Truncated(remaining) => out.number("remaining", remaining),
    }
    out.finish()
}

fn location(out: &mut Object, path: &Path, line: usize, item: &str, budget: usize) {
    out.text("path", &path.to_string_lossy());
    out.number("line", line);
    out.text("item", item);
    out.number("budget", budget);
}

pub(crate) fn line(output: &mut impl Write, record: &str) -> Result<(), Fault> {
    output
        .write_all(record.as_bytes())
        .and_then(|()| output.write_all(b"\n"))
        .map_err(|_| Fault::Output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_detail_variants_and_output_error() {
        let ast = syn::parse_file("#[doc = \"one two\"] fn f() {} #[cfg_attr(feature = \"x\", doc = \"extra\")] fn u() {}").unwrap();
        let report = comment_free::doc_lint_file(&ast, comment_free::DocBudget { max_words: 0 });
        for threshold in [Threshold::Advisory, Threshold::Enforced] {
            assert!(threshold.index() < 2);
            let path = Path::new("quote\"slash\\newline\n.rs");
            for event in [
                Event::Finding(path, &report.findings()[0]),
                Event::Hint(path, &report.findings()[0]),
                Event::Undecided(path, &report.undecided()[0]),
                Event::Header,
                Event::Truncated(1),
            ] {
                let record = detail(threshold, event);
                assert!(record.starts_with("{\"kind\":\"policy_detail\",\"version\":1,"));
                assert!(!record.contains('\n'));
                line(&mut Vec::new(), &record).unwrap();
                assert_eq!(line(&mut &mut [][..], &record), Err(Fault::Output));
            }
        }
    }
}

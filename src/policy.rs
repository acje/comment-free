#[cfg(test)]
use comment_free::{DocBudget, doc_lint_file};
use comment_free::{DocLintReport, UndecidedCause, WarningLimit};
#[path = "policy_records.rs"]
pub(super) mod records;

#[derive(Clone, Copy, Debug)]
pub(super) struct Thresholds {
    advisory: usize,
    enforced: usize,
}

impl Thresholds {
    pub(super) const fn new(advisory: usize, enforced: usize) -> Option<Self> {
        if advisory <= enforced {
            Some(Self { advisory, enforced })
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Pass,
    Fail,
    Unknown,
}

#[derive(Clone, Copy, Default)]
pub(super) struct Counts {
    shown: u32,
    hidden: u32,
}

impl Counts {
    fn add(&mut self, amount: usize, admitted: bool) -> Result<(), Fault> {
        let amount = u32::try_from(amount).map_err(|_| Fault::CounterOverflow)?;
        self.total()
            .checked_add(amount)
            .ok_or(Fault::CounterOverflow)?;
        if admitted {
            self.shown += amount;
        } else {
            self.hidden += amount;
        }
        Ok(())
    }

    pub(super) const fn total(self) -> u32 {
        self.shown + self.hidden
    }

    pub(super) const fn shown(self) -> u32 {
        self.shown
    }

    pub(super) const fn hidden(self) -> u32 {
        self.hidden
    }
}

#[derive(Clone, Default)]
pub(super) struct BudgetTotals {
    findings: Counts,
    undecided: Counts,
    warning_files: Counts,
    configuration_dependent: Counts,
    unreadable_doc_payload: Counts,
    uninspected_macro_body: Counts,
}

impl BudgetTotals {
    fn observe(&mut self, report: &DocLintReport, limit: WarningLimit) -> Result<(), Fault> {
        let admitted = limit.admits(self.warning_files.shown());
        let mut next = self.clone();
        next.findings.add(report.findings().len(), admitted)?;
        next.undecided.add(report.undecided().len(), admitted)?;
        if !report.findings().is_empty() || !report.undecided().is_empty() {
            next.warning_files.add(1, admitted)?;
        }
        for undecided in report.undecided() {
            let counts = match undecided.cause() {
                UndecidedCause::ConfigurationDependent { .. } => &mut next.configuration_dependent,
                UndecidedCause::UnreadableDocPayload => &mut next.unreadable_doc_payload,
                UndecidedCause::UninspectedMacroBody => &mut next.uninspected_macro_body,
                _ => return Err(Fault::UnsupportedCause),
            };
            counts.add(1, admitted)?;
        }
        *self = next;
        Ok(())
    }

    pub(super) const fn findings(&self) -> Counts {
        self.findings
    }

    #[cfg(test)]
    pub(super) const fn undecided(&self) -> Counts {
        self.undecided
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    CounterOverflow,
    UnsupportedCause,
    Output,
}

#[derive(Clone, Copy)]
enum Accounting {
    Exact { files: u32, errors: u32 },
    Failed(Fault),
}

pub(super) struct Policy {
    thresholds: Thresholds,
    advisory: BudgetTotals,
    enforced: BudgetTotals,
    accounting: Accounting,
}

impl Policy {
    pub(super) fn new(thresholds: Thresholds) -> Self {
        Self {
            thresholds,
            advisory: BudgetTotals::default(),
            enforced: BudgetTotals::default(),
            accounting: Accounting::Exact {
                files: 0,
                errors: 0,
            },
        }
    }

    #[cfg(test)]
    pub(super) fn observe(&mut self, ast: &syn::File, limit: WarningLimit) -> Result<(), Fault> {
        self.evaluate(
            limit,
            |max_words| doc_lint_file(ast, DocBudget { max_words }),
            |_, _, _| Ok(()),
        )
    }

    pub(super) fn evaluate(
        &mut self,
        limit: WarningLimit,
        mut lint: impl FnMut(usize) -> DocLintReport,
        mut emit: impl FnMut(records::Threshold, &DocLintReport, bool) -> Result<(), Fault>,
    ) -> Result<(), Fault> {
        let result = self.evaluate_exact(limit, &mut lint, &mut emit);
        if let Err(fault) = result {
            self.accounting = Accounting::Failed(fault);
        }
        result
    }

    fn evaluate_exact(
        &mut self,
        limit: WarningLimit,
        lint: &mut impl FnMut(usize) -> DocLintReport,
        emit: &mut impl FnMut(records::Threshold, &DocLintReport, bool) -> Result<(), Fault>,
    ) -> Result<(), Fault> {
        let (files, errors) = match self.accounting {
            Accounting::Exact { files, errors } => (files, errors),
            Accounting::Failed(fault) => return Err(fault),
        };
        let files = files.checked_add(1).ok_or(Fault::CounterOverflow)?;
        for (threshold, totals, max_words) in [
            (
                records::Threshold::Advisory,
                &mut self.advisory,
                self.thresholds.advisory,
            ),
            (
                records::Threshold::Enforced,
                &mut self.enforced,
                self.thresholds.enforced,
            ),
        ] {
            let report = lint(max_words);
            let admitted = limit.admits(totals.warning_files.shown());
            totals.observe(&report, limit)?;
            emit(threshold, &report, admitted)?;
        }
        self.accounting = Accounting::Exact { files, errors };
        Ok(())
    }

    pub(super) fn processing_error(&mut self, is_file: bool) -> Result<(), Fault> {
        let next = match self.accounting {
            Accounting::Exact { files, errors } => files
                .checked_add(u32::from(is_file))
                .zip(errors.checked_add(1))
                .map(|(files, errors)| Accounting::Exact { files, errors })
                .ok_or(Fault::CounterOverflow),
            Accounting::Failed(fault) => Err(fault),
        };
        match next {
            Ok(accounting) => {
                self.accounting = accounting;
                Ok(())
            }
            Err(fault) => {
                self.accounting = Accounting::Failed(fault);
                Err(fault)
            }
        }
    }

    pub(super) const fn advisory(&self) -> &BudgetTotals {
        &self.advisory
    }

    pub(super) const fn enforced(&self) -> &BudgetTotals {
        &self.enforced
    }

    pub(super) const fn verdict(&self) -> Verdict {
        match (
            self.accounting,
            self.advisory.undecided.total(),
            self.enforced.undecided.total(),
            self.enforced.findings.total(),
        ) {
            (
                Accounting::Failed(_)
                | Accounting::Exact { files: 0, .. }
                | Accounting::Exact { errors: 1.., .. },
                _,
                _,
                _,
            )
            | (_, 1.., _, _)
            | (_, _, 1.., _) => Verdict::Unknown,
            (_, 0, 0, 1..) => Verdict::Fail,
            (_, 0, 0, 0) => Verdict::Pass,
        }
    }

    pub(super) fn reasons(&self) -> Vec<&'static str> {
        let mut reasons = Vec::new();
        match self.accounting {
            Accounting::Exact { files, errors } => {
                if files == 0 {
                    reasons.push("empty_scope");
                }
                if errors != 0 {
                    reasons.push("processing_error");
                }
            }
            Accounting::Failed(Fault::CounterOverflow) => reasons.push("counter_overflow"),
            Accounting::Failed(Fault::UnsupportedCause) => reasons.push("processing_error"),
            Accounting::Failed(Fault::Output) => reasons.push("output_error"),
        }
        if self.advisory.undecided.total() != 0 {
            reasons.push("advisory_undecided");
        }
        if self.enforced.undecided.total() != 0 {
            reasons.push("enforced_undecided");
        }
        if self.enforced.findings.total() != 0 {
            reasons.push("enforced_violation");
        }
        reasons
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_processing_errors_and_failed_summary() {
        let mut policy = Policy::new(Thresholds::new(0, 1).unwrap());
        policy.processing_error(false).unwrap();
        policy.processing_error(true).unwrap();
        let summary = records::summary(
            &policy,
            std::path::Path::new("."),
            comment_free::ReportScope::File,
            WarningLimit::Limited(0),
        )
        .unwrap();
        assert!(summary.contains("\"files\":1,\"errors\":2"));
        assert_eq!(policy.verdict(), Verdict::Unknown);
        policy.accounting = Accounting::Exact {
            files: 1,
            errors: u32::MAX,
        };
        assert_eq!(policy.processing_error(false), Err(Fault::CounterOverflow));
        assert!(
            records::summary(
                &policy,
                std::path::Path::new("."),
                comment_free::ReportScope::File,
                WarningLimit::Unlimited
            )
            .is_err()
        );
    }

    #[test]
    fn policy_thresholds_allow_equal_and_zero_reject_reversal() {
        assert!(Thresholds::new(0, 0).is_some());
        assert!(Thresholds::new(120, 120).is_some());
        assert!(Thresholds::new(121, 120).is_none());
        assert!(Thresholds::new(0, usize::MAX).is_some());
    }

    #[test]
    fn policy_empty_and_docless_are_distinct() {
        let mut policy = Policy::new(Thresholds::new(0, 0).unwrap());
        assert_eq!(policy.verdict(), Verdict::Unknown);
        assert_eq!(policy.reasons(), ["empty_scope"]);
        policy
            .observe(
                &syn::parse_file("fn item() {}").unwrap(),
                WarningLimit::Unlimited,
            )
            .unwrap();
        assert_eq!(policy.verdict(), Verdict::Pass);
        assert!(policy.reasons().is_empty());
        assert_eq!(policy.advisory().findings().shown(), 0);
        assert_eq!(policy.enforced().undecided().hidden(), 0);
    }

    #[test]
    fn policy_counts_overflow_cannot_mutate_partition() {
        for admitted in [false, true] {
            let mut counts = Counts {
                shown: u32::MAX - 1,
                hidden: 1,
            };
            assert_eq!(counts.add(1, admitted), Err(Fault::CounterOverflow));
            assert_eq!(counts.total(), u32::MAX);
            assert_eq!(counts.shown(), u32::MAX - 1);
            assert_eq!(counts.hidden(), 1);
        }
    }

    #[test]
    fn policy_all_counter_families_poison_verdict_on_overflow() {
        let ast = syn::parse_file("#[doc = \"one two\"] fn finding() {} #[cfg_attr(feature = \"x\", doc = \"word\")] fn cfg() {} #[doc = concat!(\"word\")] fn unreadable() {} macro_rules! opaque { () => { #[doc = \"word\"] fn nested() {} }; }").unwrap();
        for threshold in [false, true] {
            for family in 0..6 {
                let mut policy = Policy::new(Thresholds::new(0, 0).unwrap());
                let totals = if threshold {
                    &mut policy.enforced
                } else {
                    &mut policy.advisory
                };
                let counts = match family {
                    0 => &mut totals.findings,
                    1 => &mut totals.undecided,
                    2 => &mut totals.warning_files,
                    3 => &mut totals.configuration_dependent,
                    4 => &mut totals.unreadable_doc_payload,
                    _ => &mut totals.uninspected_macro_body,
                };
                counts.hidden = u32::MAX;
                assert_eq!(
                    policy.observe(&ast, WarningLimit::Unlimited),
                    Err(Fault::CounterOverflow),
                    "family {family}"
                );
                assert_eq!(policy.verdict(), Verdict::Unknown);
                assert!(
                    records::summary(
                        &policy,
                        std::path::Path::new("."),
                        comment_free::ReportScope::File,
                        WarningLimit::Unlimited,
                    )
                    .is_err()
                );
                assert!(policy.reasons().contains(&"counter_overflow"));
                assert_eq!(
                    policy.observe(&ast, WarningLimit::Unlimited),
                    Err(Fault::CounterOverflow)
                );
            }
        }
        let mut policy = Policy::new(Thresholds::new(0, 0).unwrap());
        policy.accounting = Accounting::Exact {
            files: u32::MAX,
            errors: 0,
        };
        assert_eq!(
            policy.observe(&ast, WarningLimit::Unlimited),
            Err(Fault::CounterOverflow)
        );
        assert_eq!(policy.verdict(), Verdict::Unknown);
        assert!(
            records::summary(
                &policy,
                std::path::Path::new("."),
                comment_free::ReportScope::File,
                WarningLimit::Unlimited,
            )
            .is_err()
        );
    }

    #[test]
    fn policy_cap_keeps_independent_exact_cause_partitions() {
        let ast = syn::parse_file("#[doc = \"one two\"] fn finding() {} #[cfg_attr(feature = \"x\", doc = \"word\")] fn cfg() {} #[doc = concat!(\"word\")] fn unreadable() {} macro_rules! opaque { () => { #[doc = \"word\"] fn nested() {} }; }").unwrap();
        for (limit, shown) in [
            (WarningLimit::Limited(0), 0),
            (WarningLimit::Limited(1), 1),
            (WarningLimit::Unlimited, 3),
        ] {
            let mut policy = Policy::new(Thresholds::new(0, 1).unwrap());
            for _ in 0..3 {
                policy.observe(&ast, limit).unwrap();
            }
            for totals in [policy.advisory(), policy.enforced()] {
                for count in [
                    totals.findings,
                    totals.warning_files,
                    totals.configuration_dependent,
                    totals.unreadable_doc_payload,
                    totals.uninspected_macro_body,
                ] {
                    assert_eq!(count.total(), 3);
                    assert_eq!(count.shown(), shown);
                    assert_eq!(count.hidden(), 3 - shown);
                }
                assert_eq!(totals.undecided.total(), 9);
                assert_eq!(totals.undecided.shown(), shown * 3);
            }
        }
    }
}

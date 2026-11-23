use crate::cfg::{
    CfgPredicate, CfgTruth, cfg_predicate_args, fold_cfg_predicate, ident_is, is_cfg_attr, path_is,
};
use crate::markdown::{
    BlockScan, CodeSpanState, LineFrame, opens_or_closes_fence, scan_code_spans,
};
use syn::spanned::Spanned;
use syn::{Attribute, Meta};
/// Doc-comment word budget for the [`doc_lint_file`] linter.
///
/// Examples are defined mechanically as fenced code blocks (` ``` ` or
/// `~~~`) and do not count toward the prose word budget. The linter has
/// no semantic notion of an "example" — fence delimiters are the only
/// signal — and enforces no limit on how many a doc comment may carry.
#[derive(Debug, Clone, Copy)]
pub struct DocBudget {
    /// Maximum words allowed per doc comment, counting prose only.
    ///
    /// Code is not prose and does not count: fenced ` ``` ` and `~~~`
    /// blocks, four-column indented code blocks, inline code spans of
    /// any backtick run length, and reference definitions. The lint
    /// counts against the same block and span model the rewriter uses,
    /// so both paths agree on what is code.
    pub max_words: usize,
}
/// Result of counting prose words in a doc comment.
///
/// Not `#[non_exhaustive]`: an undecidable lint result is a
/// [`DocLintOutcome`] variant, not a third count.
#[derive(Debug, Clone, Copy)]
pub enum WordCount {
    /// Fence state was balanced; `count` excludes fenced code.
    Balanced(usize),
    /// Fence was opened but never closed; `count` is the fail-closed
    /// recount treating every line as prose.
    FailClosed(usize),
}
impl WordCount {
    /// Return the numeric count, regardless of balance state.
    #[must_use]
    pub const fn count(self) -> usize {
        match self {
            Self::Balanced(n) | Self::FailClosed(n) => n,
        }
    }
    /// True iff this count came from the fail-closed recount path.
    #[must_use]
    pub const fn is_fail_closed(self) -> bool {
        matches!(self, Self::FailClosed(_))
    }
}
/// A single doc-comment over-budget finding emitted by [`doc_lint_file`].
///
/// The count carries its own fail-closed provenance rather than sitting
/// beside a boolean that could contradict it. Has no public
/// constructor: a finding is evidence of a lint this crate ran.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DocFinding {
    item_label: String,
    line: usize,
    words: WordCount,
    budget: usize,
}
impl DocFinding {
    /// Human-readable label for the docced item, e.g. `"fn foo"`.
    #[must_use]
    pub fn item_label(&self) -> &str {
        &self.item_label
    }
    /// Approximate source line of the docced item.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }
    /// Prose word count of the item's doc comment, carrying whether it
    /// came from the fail-closed recount path.
    #[must_use]
    pub const fn words(&self) -> WordCount {
        self.words
    }
    /// The budget the count exceeded.
    #[must_use]
    pub const fn budget(&self) -> usize {
        self.budget
    }
}
/// Finding kind carried by the `kind` field of every `doc_lint_*` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocLintKind {
    /// Doc-comment prose exceeded the configured word budget.
    OverlongDoc,
}
impl DocLintKind {
    /// The `kind` field value carried by emitted records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OverlongDoc => "overlong_doc",
        }
    }
}
/// What a `doc_lint_*` record asserts about the item it names.
///
/// A pass that cannot decide an item — an unresolved `cfg` predicate, a
/// doc payload it cannot read — reports an explicit indeterminate
/// outcome. The outcome set is closed within a major release line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocLintOutcome {
    /// The item was inspected and violates the budget.
    Finding,
    /// The item's doc set depends on unresolved `cfg` predicates: it is
    /// within budget for at least one configuration and over budget for
    /// at least one other. Not a finding, and not clean.
    ConfigurationDependent,
    /// The item carries a doc payload this tool cannot read — a macro
    /// call such as `include_str!` or `concat!` in the doc-value
    /// position, resolved only by expansion. Not a finding, and not
    /// clean.
    UnreadableDocPayload,
    /// A macro token body carries a doc attribute. What the expansion
    /// attaches it to, and how many times, is resolved only by
    /// expansion. Not a finding, and not clean.
    UninspectedMacroBody,
}
impl DocLintOutcome {
    /// The `outcome` field value carried by emitted records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Finding => "finding",
            Self::ConfigurationDependent => "configuration_dependent",
            Self::UnreadableDocPayload => "unreadable_doc_payload",
            Self::UninspectedMacroBody => "uninspected_macro_body",
        }
    }
}
/// Why the linter could not decide an item, carrying the evidence that
/// cause — and only that cause — supports.
///
/// A word bound exists only for the `cfg` case: an unreadable payload
/// resolves to text this tool never sees, so no count of it is
/// representable here.
#[derive(Debug, Clone, Copy)]
pub enum UndecidedCause {
    /// Part of the doc set sits behind unresolved `cfg` predicates.
    /// Carries both bounds: the count present in every configuration,
    /// and the count with every `cfg_attr` doc payload active.
    ConfigurationDependent {
        /// Prose word count of the doc set present in every configuration.
        unconditional: WordCount,
        /// Prose word count with every `cfg_attr` doc payload active.
        all_configurations: WordCount,
    },
    /// The doc-value position holds an expression this tool cannot
    /// read — a macro call resolved only by expansion.
    UnreadableDocPayload,
    /// A macro token body carries a doc attribute this tool does not
    /// expand, so the item it documents does not exist to be counted.
    UninspectedMacroBody,
}
/// An item whose doc set the linter could not decide.
///
/// The cause carries its own evidence, so a count that no reading
/// produced cannot be attached to an item whose text was never read.
/// Has no public constructor.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DocUndecided {
    item_label: String,
    line: usize,
    budget: usize,
    cause: UndecidedCause,
}
impl DocUndecided {
    /// Human-readable label for the docced item, e.g. `"fn foo"`.
    #[must_use]
    pub fn item_label(&self) -> &str {
        &self.item_label
    }
    /// Approximate source line of the docced item.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }
    /// The budget in force when the item was found undecidable.
    #[must_use]
    pub const fn budget(&self) -> usize {
        self.budget
    }
    /// Why the item could not be decided, with the evidence for it.
    #[must_use]
    pub const fn cause(&self) -> UndecidedCause {
        self.cause
    }
    /// The outcome this item's record asserts.
    #[must_use]
    pub const fn outcome(&self) -> DocLintOutcome {
        match self.cause {
            UndecidedCause::ConfigurationDependent { .. } => DocLintOutcome::ConfigurationDependent,
            UndecidedCause::UnreadableDocPayload => DocLintOutcome::UnreadableDocPayload,
            UndecidedCause::UninspectedMacroBody => DocLintOutcome::UninspectedMacroBody,
        }
    }
}
/// What one [`doc_lint_file`] pass established about a file.
///
/// Findings and undecided items are separate sequences rather than one
/// sequence tagged by outcome, so a caller counting findings for an exit
/// code cannot count an item the linter never decided.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct DocLintReport {
    findings: Vec<DocFinding>,
    undecided: Vec<DocUndecided>,
}
impl DocLintReport {
    /// Items proven over budget in every configuration.
    #[must_use]
    pub fn findings(&self) -> &[DocFinding] {
        &self.findings
    }
    /// Items left undecided because of unresolved `cfg` predicates,
    /// unreadable doc payloads, or uninspected doc-bearing macro bodies.
    /// See [`UndecidedCause`] for each cause and its available evidence.
    #[must_use]
    pub fn undecided(&self) -> &[DocUndecided] {
        &self.undecided
    }
}
/// Lint `ast` for doc-comments whose prose word count exceeds `budget.max_words`.
///
/// `///`, `//!` and `#[doc=...]` payloads count as one document; fenced
/// lines are excluded. `cfg_attr` payloads, nested ones included, are
/// held separately: constant predicates fold, others stay unresolved.
/// A finding requires the unconditional set alone to be over budget.
///
/// Anything the pass could not read or could not decide is reported as
/// an explicit indeterminate — see [`UndecidedCause`] — never as clean.
#[must_use]
pub fn doc_lint_file(ast: &syn::File, budget: DocBudget) -> DocLintReport {
    let mut visitor = DocLintVisitor {
        budget,
        report: DocLintReport::default(),
        macro_name_hint: None,
        bounded_source: false,
    };
    visitor.lint_attrs(&ast.attrs, "file-level", None);
    syn::visit::Visit::visit_file(&mut visitor, ast);
    visitor.report
}

/// Evaluate source-visible documentation, including literal macro doc blocks.
/// Macro uncertainty remains in the report as coverage evidence, not expanded items.
#[must_use]
pub fn doc_budget_source_file(ast: &syn::File, budget: DocBudget) -> DocLintReport {
    let mut visitor = DocLintVisitor {
        budget,
        report: DocLintReport::default(),
        macro_name_hint: None,
        bounded_source: true,
    };
    visitor.lint_attrs(&ast.attrs, "file-level", None);
    syn::visit::Visit::visit_file(&mut visitor, ast);
    visitor.report
}
struct DocLintVisitor {
    budget: DocBudget,
    report: DocLintReport,
    macro_name_hint: Option<String>,
    bounded_source: bool,
}
impl DocLintVisitor {
    fn lint_macro_blocks(&mut self, tokens: proc_macro2::TokenStream, label: &str) {
        use syn::parse::Parser as _;
        let trees: Vec<_> = tokens.into_iter().collect();
        let mut attrs = Vec::new();
        let mut index = 0;
        while index < trees.len() {
            let width = match (&trees[index], trees.get(index + 1), trees.get(index + 2)) {
                (proc_macro2::TokenTree::Punct(p), Some(proc_macro2::TokenTree::Group(g)), _)
                    if p.as_char() == '#' && g.delimiter() == proc_macro2::Delimiter::Bracket =>
                {
                    2
                }
                (
                    proc_macro2::TokenTree::Punct(p),
                    Some(proc_macro2::TokenTree::Punct(b)),
                    Some(proc_macro2::TokenTree::Group(g)),
                ) if p.as_char() == '#'
                    && b.as_char() == '!'
                    && g.delimiter() == proc_macro2::Delimiter::Bracket =>
                {
                    3
                }
                _ => 0,
            };
            if width != 0 {
                let tokens = trees[index..index + width].iter().cloned().collect();
                let parser = if width == 3 {
                    Attribute::parse_inner
                } else {
                    Attribute::parse_outer
                };
                if let Ok(parsed) = parser.parse2(tokens)
                    && parsed.iter().all(|attr| matches!(&attr.meta, Meta::NameValue(value) if value.path.get_ident().is_some_and(|id| ident_is(id, "doc")) && matches!(&value.value, syn::Expr::Lit(value) if matches!(value.lit, syn::Lit::Str(_))))) {
                        attrs.extend(parsed);
                        index += width;
                        continue;
                }
            }
            self.lint_attrs(&attrs, &format!("{label} source block"), None);
            attrs.clear();
            if width == 0
                && let proc_macro2::TokenTree::Group(group) = &trees[index]
            {
                self.lint_macro_blocks(group.stream(), label);
            }
            index += width.max(1);
        }
        self.lint_attrs(&attrs, &format!("{label} source block"), None);
    }
    fn lint_attrs(&mut self, attrs: &[Attribute], label: &str, span_line: Option<usize>) {
        let Some(docs) = extract_doc_text(attrs) else {
            return;
        };
        let line = span_line.unwrap_or(docs.line);
        match decide_doc_budget(&docs, self.budget.max_words) {
            DocVerdict::Clean => {}
            DocVerdict::Overlong(words) => self.report.findings.push(DocFinding {
                item_label: label.to_string(),
                line,
                words,
                budget: self.budget.max_words,
            }),
            DocVerdict::Undecided(cause) => self.report.undecided.push(DocUndecided {
                item_label: label.to_string(),
                line,
                budget: self.budget.max_words,
                cause,
            }),
        }
    }
}
enum DocVerdict {
    Clean,
    Overlong(WordCount),
    Undecided(UndecidedCause),
}
fn decide_doc_budget(docs: &ItemDocs, budget: usize) -> DocVerdict {
    if docs.has_unreadable() {
        return DocVerdict::Undecided(UndecidedCause::UnreadableDocPayload);
    }
    let all = prose_word_count(&docs.all_configurations_text());
    if !docs.has_conditional() {
        return if all.count() > budget {
            DocVerdict::Overlong(all)
        } else {
            DocVerdict::Clean
        };
    }
    let unconditional = prose_word_count(&docs.unconditional_text());
    let fence_state_resolved = !unconditional.is_fail_closed() && !docs.conditional_toggles_fence();
    if fence_state_resolved && unconditional.count() > budget {
        return DocVerdict::Overlong(unconditional);
    }
    DocVerdict::Undecided(UndecidedCause::ConfigurationDependent {
        unconditional,
        all_configurations: all,
    })
}
#[derive(Debug, Clone)]
enum DocPayload {
    Text(String),
    Unreadable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocOrigin {
    Unconditional,
    Conditional,
}
#[derive(Debug, Clone)]
struct DocPart {
    origin: DocOrigin,
    payload: DocPayload,
}
#[derive(Debug, Clone)]
struct ItemDocs {
    line: usize,
    parts: Vec<DocPart>,
}
impl ItemDocs {
    fn has_conditional(&self) -> bool {
        self.parts
            .iter()
            .any(|p| p.origin == DocOrigin::Conditional)
    }
    fn has_unreadable(&self) -> bool {
        self.parts
            .iter()
            .any(|p| matches!(p.payload, DocPayload::Unreadable))
    }
    fn conditional_toggles_fence(&self) -> bool {
        self.parts
            .iter()
            .filter(|p| p.origin == DocOrigin::Conditional)
            .any(|p| self::payload_text(p).is_some_and(|t| t.lines().any(opens_or_closes_fence)))
    }
    fn unconditional_text(&self) -> String {
        self.text(|p| p.origin == DocOrigin::Unconditional)
    }
    fn all_configurations_text(&self) -> String {
        self.text(|_| true)
    }
    fn text(&self, keep: impl Fn(&DocPart) -> bool) -> String {
        self.parts
            .iter()
            .filter(|p| keep(p))
            .filter_map(self::payload_text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}
const fn payload_text(part: &DocPart) -> Option<&str> {
    match &part.payload {
        DocPayload::Text(t) => Some(t.as_str()),
        DocPayload::Unreadable => None,
    }
}
fn extract_doc_text(attrs: &[Attribute]) -> Option<ItemDocs> {
    let mut parts: Vec<DocPart> = Vec::new();
    let mut first_line: Option<usize> = None;
    for attr in attrs {
        let line = attr
            .path()
            .get_ident()
            .map_or_else(|| attr.span().start().line, |id| id.span().start().line);
        if let Some(payload) = doc_payload(attr) {
            if first_line.is_none() {
                first_line = Some(line);
            }
            parts.push(DocPart {
                origin: DocOrigin::Unconditional,
                payload,
            });
        } else if is_cfg_attr(attr) {
            let before = parts.len();
            collect_cfg_attr_doc_parts(attr, &mut parts);
            if parts.len() > before && first_line.is_none() {
                first_line = Some(line);
            }
        }
    }
    let line = first_line?;
    Some(ItemDocs { line, parts })
}
/// The doc payload of one `doc = <expr>` attribute.
///
/// `None` means the attribute carries no doc prose at all — it is not a
/// `doc` name-value attribute, or it is the `#[doc(...)]` metadata form.
/// An expression that is not a string literal is a payload this tool
/// cannot read, never an absent one: `#[doc = include_str!("x.md")]`
/// carries prose that only expansion resolves.
fn doc_payload(attr: &Attribute) -> Option<DocPayload> {
    let Meta::NameValue(nv) = &attr.meta else {
        return None;
    };
    if !path_is(&nv.path, "doc") {
        return None;
    }
    Some(doc_value_payload(&nv.value))
}
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "syn::Expr is a non-exhaustive external vocabulary; only a string-literal expression carries doc prose this tool can read"
)]
fn doc_value_payload(value: &syn::Expr) -> DocPayload {
    match value {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => DocPayload::Text(s.value()),
        _ => DocPayload::Unreadable,
    }
}
fn collect_cfg_attr_doc_parts(attr: &Attribute, out: &mut Vec<DocPart>) {
    let Meta::List(list) = &attr.meta else {
        return;
    };
    if !path_is(&list.path, "cfg_attr") {
        return;
    }
    collect_cfg_attr_list_doc_parts(list, CfgTruth::Always, out);
}
const fn doc_origin(truth: CfgTruth) -> Option<DocOrigin> {
    match truth {
        CfgTruth::Always => Some(DocOrigin::Unconditional),
        CfgTruth::Unresolved => Some(DocOrigin::Conditional),
        CfgTruth::Never => None,
    }
}
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "syn::Meta is an external vocabulary; only doc name-values and nested cfg_attr lists contribute parts"
)]
fn collect_cfg_attr_list_doc_parts(list: &syn::MetaList, outer: CfgTruth, out: &mut Vec<DocPart>) {
    let Ok(args) = cfg_predicate_args(list) else {
        return;
    };
    let mut args = args.into_iter();
    let Some(predicate) = args.next() else {
        return;
    };
    let truth = outer.and(fold_cfg_predicate(&predicate));
    let Some(origin) = doc_origin(truth) else {
        return;
    };
    for arg in args {
        let CfgPredicate::Meta(meta) = arg else {
            continue;
        };
        match &*meta {
            Meta::NameValue(nv) if path_is(&nv.path, "doc") => out.push(DocPart {
                origin,
                payload: doc_value_payload(&nv.value),
            }),
            Meta::List(inner) if path_is(&inner.path, "cfg_attr") => {
                collect_cfg_attr_list_doc_parts(inner, truth, out);
            }
            _ => {}
        }
    }
}
fn prose_word_count(doc_text: &str) -> WordCount {
    let lines: Vec<&str> = doc_text.split('\n').collect();
    let mut blocks = BlockScan::new();
    let mut span = CodeSpanState::Closed;
    let mut words = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let following = &lines[index + 1..];
        match blocks.classify(line) {
            LineFrame::Code | LineFrame::Definition => span = CodeSpanState::Closed,
            LineFrame::Prose => {
                let mut prose = String::with_capacity(line.len());
                span = scan_code_spans(line, following, span, |outside| prose.push_str(outside));
                words += prose.split_whitespace().count();
            }
        }
    }
    if blocks.fence_is_open() {
        let recount = lines.iter().map(|l| l.split_whitespace().count()).sum();
        return WordCount::FailClosed(recount);
    }
    WordCount::Balanced(words)
}
impl<'ast> syn::visit::Visit<'ast> for DocLintVisitor {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if let Some((label, attrs, line)) = item_label_and_attrs(node) {
            self.lint_attrs(attrs, &label, Some(line));
        }
        syn::visit::visit_item(self, node);
    }
    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        if let Some((label, attrs, line)) = trait_item_label_and_attrs(node) {
            self.lint_attrs(attrs, &label, Some(line));
        }
        syn::visit::visit_trait_item(self, node);
    }
    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        if let Some((label, attrs, line)) = impl_item_label_and_attrs(node) {
            self.lint_attrs(attrs, &label, Some(line));
        }
        syn::visit::visit_impl_item(self, node);
    }
    fn visit_item_macro(&mut self, node: &'ast syn::ItemMacro) {
        self.macro_name_hint = node.ident.as_ref().map(|id| format!("macro {id}"));
        syn::visit::visit_item_macro(self, node);
        self.macro_name_hint = None;
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let hint = self.macro_name_hint.take();
        if macro_tokens_carry_doc_attribute(node.tokens.clone()) {
            let item_label = hint.unwrap_or_else(|| macro_invocation_label(node));
            if self.bounded_source {
                self.lint_macro_blocks(node.tokens.clone(), &item_label);
            }
            self.report.undecided.push(DocUndecided {
                item_label,
                line: node.path.span().start().line,
                budget: self.budget.max_words,
                cause: UndecidedCause::UninspectedMacroBody,
            });
        }
        syn::visit::visit_macro(self, node);
    }
    fn visit_field(&mut self, node: &'ast syn::Field) {
        let line = node.span().start().line;
        let label = node
            .ident
            .as_ref()
            .map_or_else(|| "field (tuple)".to_string(), |id| format!("field {id}"));
        self.lint_attrs(&node.attrs, &label, Some(line));
        syn::visit::visit_field(self, node);
    }
    fn visit_variant(&mut self, node: &'ast syn::Variant) {
        let line = node.span().start().line;
        let label = format!("variant {}", node.ident);
        self.lint_attrs(&node.attrs, &label, Some(line));
        syn::visit::visit_variant(self, node);
    }
}
fn macro_invocation_label(mac: &syn::Macro) -> String {
    let path = mac
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");
    format!("macro {path}")
}
fn macro_tokens_carry_doc_attribute(tokens: proc_macro2::TokenStream) -> bool {
    let trees: Vec<proc_macro2::TokenTree> = tokens.into_iter().collect();
    trees.iter().enumerate().any(|(i, tree)| {
        let proc_macro2::TokenTree::Group(group) = tree else {
            return false;
        };
        let is_attribute_body = group.delimiter() == proc_macro2::Delimiter::Bracket
            && attribute_pound_precedes(&trees, i);
        (is_attribute_body && assigns_doc(group.stream()))
            || macro_tokens_carry_doc_attribute(group.stream())
    })
}
fn attribute_pound_precedes(trees: &[proc_macro2::TokenTree], index: usize) -> bool {
    let is_punct = |offset: usize, want: char| {
        index
            .checked_sub(offset)
            .and_then(|i| trees.get(i))
            .is_some_and(|t| matches!(t, proc_macro2::TokenTree::Punct(p) if p.as_char() == want))
    };
    is_punct(1, '#') || (is_punct(1, '!') && is_punct(2, '#'))
}
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "proc_macro2::TokenTree is an external vocabulary; only identifiers and nested groups can assign a doc attribute"
)]
fn assigns_doc(tokens: proc_macro2::TokenStream) -> bool {
    let trees: Vec<proc_macro2::TokenTree> = tokens.into_iter().collect();
    trees.iter().enumerate().any(|(i, tree)| match tree {
        proc_macro2::TokenTree::Ident(id) => {
            ident_is(id, "doc")
                && matches!(
                    trees.get(i + 1),
                    Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '='
                )
        }
        proc_macro2::TokenTree::Group(group) => assigns_doc(group.stream()),
        _ => false,
    })
}
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "syn::Item is a non-exhaustive external vocabulary; items outside this linter's label set carry no doc budget"
)]
fn item_label_and_attrs(item: &syn::Item) -> Option<(String, &[Attribute], usize)> {
    use syn::Item::{
        Const, Enum, ExternCrate, Fn, Impl, Macro, Mod, Static, Struct, Trait, Type, Union, Use,
    };
    let (label, attrs, line): (String, &[Attribute], usize) = match item {
        Fn(i) => (
            format!("fn {}", i.sig.ident),
            &i.attrs,
            i.sig.fn_token.span.start().line,
        ),
        Struct(i) => (
            format!("struct {}", i.ident),
            &i.attrs,
            i.struct_token.span.start().line,
        ),
        Enum(i) => (
            format!("enum {}", i.ident),
            &i.attrs,
            i.enum_token.span.start().line,
        ),
        Trait(i) => (
            format!("trait {}", i.ident),
            &i.attrs,
            i.trait_token.span.start().line,
        ),
        Mod(i) => (
            format!("mod {}", i.ident),
            &i.attrs,
            i.mod_token.span.start().line,
        ),
        Const(i) => (
            format!("const {}", i.ident),
            &i.attrs,
            i.const_token.span.start().line,
        ),
        Static(i) => (
            format!("static {}", i.ident),
            &i.attrs,
            i.static_token.span.start().line,
        ),
        Type(i) => (
            format!("type {}", i.ident),
            &i.attrs,
            i.type_token.span.start().line,
        ),
        Union(i) => (
            format!("union {}", i.ident),
            &i.attrs,
            i.union_token.span.start().line,
        ),
        Impl(i) => ("impl".to_string(), &i.attrs, i.impl_token.span.start().line),
        Use(i) => ("use".to_string(), &i.attrs, i.use_token.span.start().line),
        ExternCrate(i) => (
            format!("extern crate {}", i.ident),
            &i.attrs,
            i.extern_token.span.start().line,
        ),
        Macro(i) => (
            i.ident
                .as_ref()
                .map_or_else(|| "macro".to_string(), |id| format!("macro {id}")),
            &i.attrs,
            i.mac.span().start().line,
        ),
        _ => return None,
    };
    Some((label, attrs, line))
}
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "syn::TraitItem is a non-exhaustive external vocabulary; items outside this linter's label set carry no doc budget"
)]
fn trait_item_label_and_attrs(item: &syn::TraitItem) -> Option<(String, &[Attribute], usize)> {
    use syn::TraitItem::{Const, Fn, Type};
    let (label, attrs, line): (String, &[Attribute], usize) = match item {
        Fn(i) => (
            format!("trait fn {}", i.sig.ident),
            &i.attrs,
            i.sig.fn_token.span.start().line,
        ),
        Const(i) => (
            format!("trait const {}", i.ident),
            &i.attrs,
            i.const_token.span.start().line,
        ),
        Type(i) => (
            format!("trait type {}", i.ident),
            &i.attrs,
            i.type_token.span.start().line,
        ),
        _ => return None,
    };
    Some((label, attrs, line))
}
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "syn::ImplItem is a non-exhaustive external vocabulary; items outside this linter's label set carry no doc budget"
)]
fn impl_item_label_and_attrs(item: &syn::ImplItem) -> Option<(String, &[Attribute], usize)> {
    use syn::ImplItem::{Const, Fn, Type};
    let (label, attrs, line): (String, &[Attribute], usize) = match item {
        Fn(i) => (
            format!("impl fn {}", i.sig.ident),
            &i.attrs,
            i.sig.fn_token.span.start().line,
        ),
        Const(i) => (
            format!("impl const {}", i.ident),
            &i.attrs,
            i.const_token.span.start().line,
        ),
        Type(i) => (
            format!("impl type {}", i.ident),
            &i.attrs,
            i.type_token.span.start().line,
        ),
        _ => return None,
    };
    Some((label, attrs, line))
}
#[cfg(test)]
mod doc_lint_tests {
    use super::{DocBudget, WordCount, doc_lint_file, prose_word_count};
    fn file(src: &str) -> syn::File {
        syn::parse_file(src).expect("doc-lint fixture must parse as Rust source")
    }
    fn lint(file: &syn::File, max_words: usize) -> Vec<super::DocFinding> {
        doc_lint_file(file, DocBudget { max_words })
            .findings()
            .to_vec()
    }
    fn report(file: &syn::File, max_words: usize) -> super::DocLintReport {
        doc_lint_file(file, DocBudget { max_words })
    }
    fn cfg_bounds(u: &super::DocUndecided) -> (super::WordCount, super::WordCount) {
        match u.cause() {
            super::UndecidedCause::ConfigurationDependent {
                unconditional,
                all_configurations,
            } => (unconditional, all_configurations),
            other @ (super::UndecidedCause::UnreadableDocPayload
            | super::UndecidedCause::UninspectedMacroBody) => {
                panic!("expected a configuration-dependent cause, got {other:?}")
            }
        }
    }
    #[test]
    fn no_docs_yields_no_findings() {
        let f = file(
            r"
pub fn foo() {}
        ",
        );
        assert!(lint(&f, 40).is_empty());
    }
    #[test]
    fn short_doc_under_budget_yields_no_findings() {
        let f = file(
            r#"
#[doc = " one two three four five"] pub fn foo() {}
        "#,
        );
        assert!(lint(&f, 40).is_empty());
    }
    #[test]
    fn long_doc_over_budget_yields_one_finding() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
" w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
" w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
" w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
" w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].words.count(), 50);
        assert_eq!(findings[0].budget, 40);
        assert_eq!(findings[0].item_label, "fn foo");
    }
    #[test]
    fn fenced_code_excluded_brings_under_budget() {
        let f = file(
            r#"
#[doc = " p01 p02 p03 p04 p05 p06 p07 p08 p09 p10"] #[doc = " ```"] #[doc =
" c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
" c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
" c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
" c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
" c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] #[doc = " ```"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 40);
        assert!(findings.is_empty(), "{findings:?}");
    }
    #[test]
    fn multi_attr_docs_concatenate() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05"] #[doc = " w06 w07 w08 w09 w10"] #[doc =
"w11 w12 w13 w14 w15"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 10);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].words.count(), 15);
    }
    #[test]
    fn cfg_attr_doc_payload_counted_towards_the_undecided_upper_bound() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05"] #[cfg_attr(test, doc =
"w06 w07 w08 w09 w10")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 7);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1);
        assert_eq!(cfg_bounds(&r.undecided()[0]).0.count(), 5);
        assert_eq!(cfg_bounds(&r.undecided()[0]).1.count(), 10);
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::ConfigurationDependent
        );
    }
    #[test]
    fn mutually_exclusive_cfg_docs_are_not_summed_into_a_finding() {
        let f = file(
            r#"
#[cfg_attr(unix, doc = " w01 w02 w03 w04 w05 w06")] #[cfg_attr(windows, doc =
" w07 w08 w09 w10 w11 w12")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(cfg_bounds(&r.undecided()[0]).0.count(), 0);
        assert_eq!(cfg_bounds(&r.undecided()[0]).1.count(), 12);
        assert_eq!(r.undecided()[0].budget(), 8);
    }
    #[test]
    fn unconditional_docs_over_budget_stay_a_finding_beside_cfg_docs() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[cfg_attr(unix, doc =
" w11 w12 w13")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1);
        assert_eq!(
            r.findings()[0].words().count(),
            10,
            "a finding must report the count every configuration carries"
        );
    }
    #[test]
    fn cfg_docs_within_the_aggregate_budget_are_undecided_not_clean() {
        let f = file(
            r#"
#[doc = " w01 w02"] #[cfg_attr(unix, doc = " w03 w04")] #[cfg_attr(windows, doc =
" w05 w06")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 6);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(
            r.undecided().len(),
            1,
            "the all-payload aggregate is not an attainable configuration: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn a_conditional_fence_hiding_unconditional_prose_is_undecided() {
        let f = file(
            r#"
#[cfg_attr(unix, doc = " ```")] #[doc =
" w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[cfg_attr(unix, doc = " ```")]
pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(
            r.undecided().len(),
            1,
            "a conditional fence leaves the unconditional prose undecided: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn nested_cfg_attr_doc_payload_is_not_invisible() {
        let f = file(
            r#"
#[cfg_attr(unix, cfg_attr(windows, doc =
" w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"))] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(
            r.undecided().len(),
            1,
            "a nested cfg_attr doc payload must not vanish: {:?}",
            r.undecided()
        );
        assert_eq!(cfg_bounds(&r.undecided()[0]).1.count(), 10);
    }
    #[test]
    fn a_trivially_true_cfg_attr_doc_is_a_finding() {
        let f = file(
            r#"
#[cfg_attr(all(), doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")]
pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].words().count(), 10);
    }
    #[test]
    fn a_nested_trivially_true_cfg_attr_doc_is_a_finding() {
        let f = file(
            r#"
#[cfg_attr(all(), cfg_attr(all(), doc =
" w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"))] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].words().count(), 10);
    }
    #[test]
    fn a_trivially_false_cfg_attr_doc_carries_no_words() {
        let f = file(
            r#"
#[cfg_attr(any(), doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")]
pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
    }
    #[test]
    fn boolean_constant_predicates_fold_through_not_and_nesting() {
        let f = file(
            r#"
#[cfg_attr(not(any()), doc = " w01 w02 w03 w04 w05")] #[cfg_attr(any(all(), unix),
doc = " w06 w07 w08 w09 w10")] #[cfg_attr(all(any(), unix), doc =
" x01 x02 x03 x04 x05 x06 x07 x08 x09 x10")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].words().count(), 10);
    }
    #[test]
    fn a_literal_true_cfg_attr_doc_is_a_finding() {
        let f = file(
            r#"
#[cfg_attr(true, doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")]
pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].words().count(), 10);
    }
    #[test]
    fn a_literal_false_cfg_attr_doc_carries_no_words() {
        let f = file(
            r#"
#[cfg_attr(false, doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")]
pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
    }
    #[test]
    fn literal_boolean_predicates_fold_through_all_any_and_not() {
        let f = file(
            r#"
#[cfg_attr(all(true), doc = " w01 w02 w03 w04 w05")] #[cfg_attr(any(true, unix),
doc = " w06 w07 w08 w09 w10")] #[cfg_attr(not(false), cfg_attr(true, doc =
" x01 x02 x03 x04 x05"))] #[cfg_attr(any(false), doc =
" y01 y02 y03 y04 y05 y06 y07 y08 y09 y10")] pub fn foo() {}
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].words().count(), 15);
    }
    #[test]
    fn a_file_level_trivially_true_cfg_attr_doc_is_a_finding() {
        let f = file(
            r#"
#![cfg_attr(all(), doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10")]
        "#,
        );
        let r = report(&f, 8);
        assert!(r.undecided().is_empty(), "{:?}", r.undecided());
        assert_eq!(r.findings().len(), 1, "{:?}", r.findings());
        assert_eq!(r.findings()[0].item_label(), "file-level");
    }
    #[test]
    fn a_fence_split_across_a_cfg_boundary_is_undecided_not_a_finding() {
        let f = file(
            r#"
#[doc = " ```"] #[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[cfg_attr(unix,
doc = " ```")] #[doc = " w11 w12 w13 w14 w15"] pub fn foo() {}
        "#,
        );
        let r = report(&f, 3);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert!(cfg_bounds(&r.undecided()[0]).0.is_fail_closed());
    }
    #[test]
    fn limitation_contract_shadowed_concat_stays_unreadable_without_word_counts() {
        let f = file(
            r#"
macro_rules! concat { ($($t:tt)*) => { "one two three four" }; }
#[doc = concat!("short")] pub fn foo() {}
"#,
        );
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        let undecided = &r.undecided()[0];
        assert_eq!(undecided.item_label(), "fn foo");
        assert!(matches!(
            undecided.cause(),
            super::UndecidedCause::UnreadableDocPayload
        ));
        let record = crate::doc_lint_undecided_record(
            crate::DocLintKind::OverlongDoc,
            std::path::Path::new("shadowed.rs"),
            undecided,
        );
        assert!(record.contains("\"outcome\":\"unreadable_doc_payload\""));
        for field in ["words", "words_all_cfgs", "fail_closed"] {
            assert!(!record.contains(&format!("\"{field}\":")), "{record}");
        }
    }
    #[test]
    fn a_concat_doc_expression_is_undecided_not_clean() {
        let f = file(r#"#[doc = concat!(" w01", " w02 w03 w04 w05")] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::UnreadableDocPayload
        );
        assert_eq!(r.undecided()[0].item_label(), "fn foo");
    }
    #[test]
    fn an_include_str_doc_expression_is_undecided_not_clean() {
        let f = file(r#"#[doc = include_str!("prose.md")] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::UnreadableDocPayload
        );
    }
    #[test]
    fn a_file_level_unreadable_doc_expression_is_undecided_not_clean() {
        let f = file(r#"#![doc = include_str!("README.md")]"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(r.undecided()[0].item_label(), "file-level");
    }
    #[test]
    fn an_unreadable_cfg_attr_doc_expression_is_undecided_not_clean() {
        let f = file(r#"#[cfg_attr(all(), doc = concat!(" a b c"))] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::UnreadableDocPayload
        );
    }
    #[test]
    fn a_raw_spelled_doc_attribute_is_the_same_path_as_doc() {
        let f = file(r#"#[r#doc = concat!(" a b c")] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::UnreadableDocPayload
        );
    }
    #[test]
    fn a_raw_spelled_doc_attribute_in_a_macro_body_is_the_same_path_as_doc() {
        let f = file(r#"pub fn f() { generate! { #[r#doc = " a b c"] fn g() {} } }"#);
        let r = report(&f, 1);
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::UninspectedMacroBody
        );
    }
    #[test]
    fn a_raw_spelled_cfg_attr_is_the_same_path_as_cfg_attr() {
        let f = file(r#"#[r#cfg_attr(all(), doc = concat!(" a b c"))] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::UnreadableDocPayload
        );
    }
    #[test]
    fn raw_spelled_cfg_predicate_operators_fold_like_their_plain_spellings() {
        for src in [
            r#"#[cfg_attr(r#all(), doc = concat!(" a b c"))] pub fn foo() {}"#,
            r#"#[cfg_attr(r#not(r#any()), doc = concat!(" a b c"))] pub fn foo() {}"#,
        ] {
            let f = file(src);
            let r = report(&f, 1);
            assert_eq!(r.undecided().len(), 1, "{src}: {:?}", r.undecided());
            assert_eq!(
                r.undecided()[0].outcome(),
                crate::DocLintOutcome::UnreadableDocPayload,
                "{src}"
            );
        }
    }
    #[test]
    fn an_unresolved_cfg_attr_carrying_an_unreadable_doc_expression_is_unreadable() {
        let f = file(r#"#[cfg_attr(unix, doc = concat!(" a b c"))] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::UnreadableDocPayload,
            "an unreadable payload is the stronger indeterminate: no word bound holds"
        );
    }
    #[test]
    fn a_never_taken_cfg_attr_unreadable_doc_expression_is_dropped() {
        let f = file(r#"#[cfg_attr(any(), doc = concat!(" a b c"))] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(
            r.undecided().is_empty(),
            "a payload present in no configuration is not an indeterminate: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn an_unreadable_payload_beside_overlong_prose_is_undecided_not_a_finding() {
        let f = file(r#"#[doc = " w01 w02 w03 w04 w05"] #[doc = concat!(" x")] pub fn foo() {}"#);
        let r = report(&f, 2);
        assert!(
            r.findings().is_empty(),
            "an unreadable payload may open a code fence, so no word count is provable: {:?}",
            r.findings()
        );
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::UnreadableDocPayload
        );
    }
    #[test]
    fn a_non_string_literal_doc_payload_is_undecided_not_clean() {
        let f = file("#[doc = 1] pub fn foo() {}");
        let r = report(&f, 1);
        assert_eq!(
            r.undecided().len(),
            1,
            "rustc accepts this with a malformed-input warning and it carries no readable prose; \
             reporting it is a conservative over-report, never a false clean: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn a_doc_list_attribute_carries_no_prose_and_is_not_unreadable() {
        let f = file(r#"#[doc(hidden)] #[doc(alias = "bar")] #[doc = " w01"] pub fn foo() {}"#);
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(
            r.undecided().is_empty(),
            "`#[doc(...)]` is rustdoc metadata, not doc prose: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn an_overlong_doc_inside_macro_rules_is_undecided_not_silence() {
        let f = file(
            r#"
macro_rules! noisy { () => {
#[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"]
#[doc = " w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"]
pub fn inner() {} }; }
"#,
        );
        let r = report(&f, 5);
        assert!(
            r.findings().is_empty(),
            "the expansion is not performed, so no word count is provable: {:?}",
            r.findings()
        );
        assert_eq!(
            r.undecided().len(),
            1,
            "an uninspected macro body carrying doc attributes is a reported gap, \
             not silence: {:?}",
            r.undecided()
        );
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::UninspectedMacroBody
        );
        assert_eq!(r.undecided()[0].item_label(), "macro noisy");
    }
    #[test]
    fn a_doc_comment_inside_a_macro_invocation_body_is_undecided() {
        let f = file(
            r"
generate! {
    /// w01 w02 w03 w04 w05 w06 w07 w08 w09 w10
    pub fn inner() {}
}
",
        );
        let r = report(&f, 5);
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
        assert_eq!(
            r.undecided()[0].outcome(),
            crate::DocLintOutcome::UninspectedMacroBody
        );
        assert_eq!(r.undecided()[0].item_label(), "macro generate");
    }
    #[test]
    fn a_macro_body_without_doc_attributes_is_clean() {
        let f = file(
            r#"
macro_rules! quiet { ($x:expr) => { $x + 1 }; }
pub fn f() {
    let v = vec![1, 2, 3];
    println!("this text mentions doc and docs but carries no attribute");
    let _ = quiet!(v.len());
}
"#,
        );
        let r = report(&f, 1);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(
            r.undecided().is_empty(),
            "a macro body with no doc attribute carries no doc payload: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn a_doc_bearing_macro_body_is_reported_in_every_position() {
        let positions = [
            (
                "statement",
                "pub fn f() { generate! { #[doc = \" a b c\"] fn g() {} } }",
            ),
            (
                "expression",
                "pub fn f() -> u32 { generate!(#[doc = \" a b c\"] fn g() {}) }",
            ),
            (
                "impl item",
                "pub struct S; impl S { generate! { #[doc = \" a b c\"] fn g() {} } }",
            ),
            (
                "trait item",
                "pub trait T { generate! { #[doc = \" a b c\"] fn g() {} } }",
            ),
            (
                "extern block",
                "unsafe extern \"C\" { generate! { #[doc = \" a b c\"] fn g(); } }",
            ),
            (
                "type position",
                "pub type A = generate!(#[doc = \" a b c\"] fn g() {});",
            ),
            (
                "pattern position",
                "pub fn f() { let generate!(#[doc = \" a b c\"] x) = 1; }",
            ),
            ("inner doc comment", "generate! { //! w01 w02 w03\n }"),
        ];
        for (position, src) in positions {
            let r = report(&file(src), 1);
            assert_eq!(
                r.undecided().len(),
                1,
                "a doc-bearing macro body in {position} position must be reported: {:?}",
                r.undecided()
            );
            assert_eq!(
                r.undecided()[0].outcome(),
                crate::DocLintOutcome::UninspectedMacroBody,
                "{position}"
            );
        }
    }
    #[test]
    fn a_nested_doc_bearing_macro_body_is_reported_at_the_outermost_body() {
        let f = file(r#"outer! { inner! { #[doc = " a b c"] fn g() {} } }"#);
        let r = report(&f, 1);
        assert_eq!(
            r.undecided().len(),
            1,
            "the inner invocation is tokens inside the outer opaque body, not a second \
             item: one indeterminate covers the whole body: {:?}",
            r.undecided()
        );
        assert_eq!(r.undecided()[0].item_label(), "macro outer");
    }
    #[test]
    fn a_doc_list_attribute_inside_a_macro_body_carries_no_prose() {
        let f = file(r"generate! { #[doc(hidden)] fn g() {} }");
        let r = report(&f, 1);
        assert!(
            r.undecided().is_empty(),
            "`#[doc(...)]` is rustdoc metadata, not doc prose: {:?}",
            r.undecided()
        );
    }
    #[test]
    fn a_cfg_attr_doc_inside_a_macro_body_is_reported() {
        let f = file(r#"generate! { #[cfg_attr(unix, doc = " a b c")] fn g() {} }"#);
        let r = report(&f, 1);
        assert_eq!(r.undecided().len(), 1, "{:?}", r.undecided());
    }
    #[test]
    fn field_and_variant_docs_linted_independently() {
        let f = file(
            r#"
pub struct S { #[doc = " w01 w02 w03 w04 w05"] pub a : u32, #[doc =
" w01 w02 w03 w04 w05 w06"] pub b : u32, }
        "#,
        );
        let findings = lint(&f, 3);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(findings.iter().all(|f| f.item_label.starts_with("field ")));
        let f = file(
            r#"
pub enum E { #[doc = " w01 w02 w03 w04 w05"] One, #[doc =
" w01 w02 w03 w04 w05 w06"] Two, }
        "#,
        );
        let findings = lint(&f, 3);
        assert_eq!(findings.len(), 2, "{findings:?}");
        assert!(
            findings
                .iter()
                .all(|f| f.item_label.starts_with("variant "))
        );
    }
    #[test]
    fn closing_fence_returns_to_prose() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05"] #[doc = " ```"] #[doc = " c01 c02 c03"] #[doc
= " ```"] #[doc = " w06 w07 w08 w09 w10 w11"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 10);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].words.count(), 11);
    }
    #[test]
    fn equal_to_budget_does_not_trigger() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05"] pub fn foo() {}
        "#,
        );
        assert!(lint(&f, 5).is_empty());
    }
    #[test]
    fn tilde_fence_excludes_code() {
        let f = file(
            r#"
#[doc = " p01 p02 p03 p04 p05 p06 p07 p08 p09 p10"] #[doc = " ~~~"] #[doc =
" c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
" c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
" c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
" c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
" c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] #[doc = " ~~~"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 40);
        assert!(findings.is_empty(), "{findings:?}");
    }
    #[test]
    fn unclosed_fence_fails_closed() {
        let f = file(
            r#"
#[doc = " p01 p02 p03 p04 p05"] #[doc = " ```"] #[doc =
" c01 c02 c03 c04 c05 c06 c07 c08 c09 c10"] #[doc =
" c11 c12 c13 c14 c15 c16 c17 c18 c19 c20"] #[doc =
" c21 c22 c23 c24 c25 c26 c27 c28 c29 c30"] #[doc =
" c31 c32 c33 c34 c35 c36 c37 c38 c39 c40"] #[doc =
" c41 c42 c43 c44 c45 c46 c47 c48 c49 c50"] pub fn foo() {}
        "#,
        );
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].words.count(), 56);
        assert!(
            findings[0].words.is_fail_closed(),
            "unbalanced fence must set fail_closed=true: {:?}",
            findings[0]
        );
    }
    #[test]
    fn over_budget_doc_on_pub_use_is_linted() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
" w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
" w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
" w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
" w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] pub use crate ::foo::Bar;
        "#,
        );
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].item_label, "use");
        assert_eq!(findings[0].words.count(), 50);
    }
    #[test]
    fn over_budget_doc_on_extern_crate_is_linted() {
        let f = file(
            r#"
#[doc = " w01 w02 w03 w04 w05 w06 w07 w08 w09 w10"] #[doc =
" w11 w12 w13 w14 w15 w16 w17 w18 w19 w20"] #[doc =
" w21 w22 w23 w24 w25 w26 w27 w28 w29 w30"] #[doc =
" w31 w32 w33 w34 w35 w36 w37 w38 w39 w40"] #[doc =
" w41 w42 w43 w44 w45 w46 w47 w48 w49 w50"] extern crate alloc;
        "#,
        );
        let findings = lint(&f, 40);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].item_label, "extern crate alloc");
    }
    #[test]
    fn every_named_item_form_has_its_label() {
        let f = file(
            r"
/// w1 w2
struct S;
/// w1 w2
enum E { A }
/// w1 w2
trait T {
    /// w1 w2
    fn tf();
    /// w1 w2
    const TC: u32;
    /// w1 w2
    type TT;
}
/// w1 w2
mod m {}
/// w1 w2
const C: u32 = 0;
/// w1 w2
static ST: u32 = 0;
/// w1 w2
type Ty = u32;
/// w1 w2
union U { a: u32 }
/// w1 w2
impl S {
    /// w1 w2
    fn im() {}
    /// w1 w2
    const IC: u32 = 0;
}
/// w1 w2
impl T for S {
    /// w1 w2
    fn tf() {}
    /// w1 w2
    const TC: u32 = 0;
    /// w1 w2
    type TT = u32;
}
        ",
        );
        let found = report(&f, 1);
        let labels: Vec<&str> = found
            .findings()
            .iter()
            .map(super::DocFinding::item_label)
            .collect();
        for expected in [
            "struct S",
            "enum E",
            "trait T",
            "impl",
            "mod m",
            "const C",
            "static ST",
            "type Ty",
            "union U",
            "trait fn tf",
            "trait const TC",
            "trait type TT",
            "impl fn im",
            "impl const IC",
            "impl type TT",
        ] {
            assert!(
                labels.contains(&expected),
                "documented item form {expected:?} has no label; labels seen: {labels:?}"
            );
        }
    }
    #[test]
    fn g34_lint_path_fence_tracking_holds_for_long_and_indented_openers() {
        assert!(matches!(
            prose_word_count("````\n```\none two\n````\nthree"),
            WordCount::Balanced(1)
        ));
        assert!(matches!(
            prose_word_count("    ```\none two"),
            WordCount::Balanced(2)
        ));
    }
    #[test]
    fn g42_inline_code_spans_are_not_prose() {
        assert!(matches!(
            prose_word_count("one `two three four` five"),
            WordCount::Balanced(2)
        ));
        assert!(matches!(
            prose_word_count("a ``b ` c`` d"),
            WordCount::Balanced(2)
        ));
    }
    #[test]
    fn g43_an_unclosed_span_leaves_its_delimiter_literal_prose() {
        assert!(matches!(
            prose_word_count("one `two three"),
            WordCount::Balanced(3)
        ));
    }
    #[test]
    fn g44_a_span_open_across_lines_hides_both_lines_words() {
        assert!(matches!(
            prose_word_count("one `two\nthree` four"),
            WordCount::Balanced(2)
        ));
    }
    #[test]
    fn g45_an_indented_code_block_is_not_prose() {
        assert!(matches!(
            prose_word_count("para\n\n    let x = 1;\n    let y = 2;\nafter"),
            WordCount::Balanced(2)
        ));
    }
    #[test]
    fn g46_a_reference_definition_is_link_machinery_not_prose() {
        assert!(matches!(
            prose_word_count("[label]: https://example.com/a/b\ntext"),
            WordCount::Balanced(1)
        ));
    }
    #[test]
    fn g47_an_unbalanced_fence_still_recounts_every_line_as_prose() {
        assert!(matches!(
            prose_word_count("```\none `two` three"),
            WordCount::FailClosed(4)
        ));
    }
}

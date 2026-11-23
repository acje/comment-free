use crate::cfg::{CfgPredicate, cfg_predicate_args, is_cfg_attr, path_is};
use crate::markdown::{
    BlockScan, CodeSpanState, LineFrame, advance_span, backtick_run_len, find_matching_bracket,
    reference_definition_label, span_after_run,
};
use std::collections::BTreeSet;
use syn::{Attribute, Meta};
#[derive(Debug, Clone)]
pub(crate) struct DocSplice {
    range: std::ops::Range<usize>,
    replacement: String,
}
pub(crate) fn apply_splices(original: &str, mut splices: Vec<DocSplice>) -> String {
    splices.sort_by_key(|s| std::cmp::Reverse(s.range.start));
    let mut out = original.to_string();
    for splice in splices {
        out.replace_range(splice.range, &splice.replacement);
    }
    out
}
pub(crate) fn collect_doc_splices(ast: &syn::File, original: &str) -> Vec<DocSplice> {
    let mut out = Vec::new();
    collect_attr_run_splices(&ast.attrs, original, &mut out);
    collect_cfg_attr_doc_splices(&ast.attrs, original, &mut out);
    for item in &ast.items {
        collect_item_splices(item, original, &mut out);
    }
    out
}
fn collect_item_splices(item: &syn::Item, original: &str, out: &mut Vec<DocSplice>) {
    if let Some(attrs) = item_attrs(item) {
        collect_attr_run_splices(attrs, original, out);
        collect_cfg_attr_doc_splices(attrs, original, out);
    }
    match item {
        syn::Item::Struct(s) => {
            for field in &s.fields {
                collect_attr_run_splices(&field.attrs, original, out);
                collect_cfg_attr_doc_splices(&field.attrs, original, out);
            }
        }
        syn::Item::Enum(e) => {
            for v in &e.variants {
                collect_attr_run_splices(&v.attrs, original, out);
                collect_cfg_attr_doc_splices(&v.attrs, original, out);
                for f in &v.fields {
                    collect_attr_run_splices(&f.attrs, original, out);
                    collect_cfg_attr_doc_splices(&f.attrs, original, out);
                }
            }
        }
        syn::Item::Union(u) => {
            for f in &u.fields.named {
                collect_attr_run_splices(&f.attrs, original, out);
                collect_cfg_attr_doc_splices(&f.attrs, original, out);
            }
        }
        syn::Item::Trait(t) => {
            for ti in &t.items {
                if let Some(attrs) = trait_item_attrs(ti) {
                    collect_attr_run_splices(attrs, original, out);
                    collect_cfg_attr_doc_splices(attrs, original, out);
                }
            }
        }
        syn::Item::Impl(i) => {
            for ii in &i.items {
                if let Some(attrs) = impl_item_attrs(ii) {
                    collect_attr_run_splices(attrs, original, out);
                    collect_cfg_attr_doc_splices(attrs, original, out);
                }
            }
        }
        syn::Item::Mod(m) => {
            if let Some((_, items)) = &m.content {
                for inner in items {
                    collect_item_splices(inner, original, out);
                }
            }
        }
        _ => {}
    }
}
const fn item_attrs(item: &syn::Item) -> Option<&Vec<Attribute>> {
    use syn::Item::{
        Const, Enum, ExternCrate, Fn, Impl, Macro, Mod, Static, Struct, Trait, TraitAlias, Type,
        Union, Use,
    };
    Some(match item {
        Const(i) => &i.attrs,
        Enum(i) => &i.attrs,
        ExternCrate(i) => &i.attrs,
        Fn(i) => &i.attrs,
        Impl(i) => &i.attrs,
        Macro(i) => &i.attrs,
        Mod(i) => &i.attrs,
        Static(i) => &i.attrs,
        Struct(i) => &i.attrs,
        Trait(i) => &i.attrs,
        TraitAlias(i) => &i.attrs,
        Type(i) => &i.attrs,
        Union(i) => &i.attrs,
        Use(i) => &i.attrs,
        _ => return None,
    })
}
const fn trait_item_attrs(item: &syn::TraitItem) -> Option<&Vec<Attribute>> {
    use syn::TraitItem::{Const, Fn, Macro, Type};
    Some(match item {
        Const(i) => &i.attrs,
        Fn(i) => &i.attrs,
        Macro(i) => &i.attrs,
        Type(i) => &i.attrs,
        _ => return None,
    })
}
const fn impl_item_attrs(item: &syn::ImplItem) -> Option<&Vec<Attribute>> {
    use syn::ImplItem::{Const, Fn, Macro, Type};
    Some(match item {
        Const(i) => &i.attrs,
        Fn(i) => &i.attrs,
        Macro(i) => &i.attrs,
        Type(i) => &i.attrs,
        _ => return None,
    })
}
fn collect_attr_run_splices(attrs: &[Attribute], original: &str, out: &mut Vec<DocSplice>) {
    let mut i = 0;
    while i < attrs.len() {
        let Some(_) = doc_attr_literal_span(&attrs[i], original, DocShape::SafeLineOrAttr) else {
            i += 1;
            continue;
        };
        let start = i;
        while i < attrs.len()
            && doc_attr_literal_span(&attrs[i], original, DocShape::SafeLineOrAttr).is_some()
        {
            i += 1;
        }
        emit_run_splices(&attrs[start..i], original, out);
    }
}
#[derive(Debug, Clone, Copy)]
enum DocShape {
    SafeLineOrAttr,
}
fn doc_attr_literal_span(
    attr: &Attribute,
    original: &str,
    _shape: DocShape,
) -> Option<DocLiteralSite> {
    let Meta::NameValue(nv) = &attr.meta else {
        return None;
    };
    if !path_is(&nv.path, "doc") {
        return None;
    }
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &nv.value
    else {
        return None;
    };
    let range = s.span().byte_range();
    let body = original.get(range.clone())?;
    let kind = classify_doc_literal(body)?;
    Some(DocLiteralSite {
        range,
        kind,
        value: s.value(),
    })
}
#[derive(Debug, Clone)]
struct DocLiteralSite {
    range: std::ops::Range<usize>,
    kind: DocLiteralKind,
    value: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocLiteralKind {
    OuterLine,
    InnerLine,
    QuotedAttr,
}
fn classify_doc_literal(body: &str) -> Option<DocLiteralKind> {
    if body.starts_with("///") && !body.starts_with("////") {
        Some(DocLiteralKind::OuterLine)
    } else if body.starts_with("//!") {
        Some(DocLiteralKind::InnerLine)
    } else if body.starts_with('"') && body.ends_with('"') {
        Some(DocLiteralKind::QuotedAttr)
    } else {
        None
    }
}
fn emit_run_splices(run: &[Attribute], original: &str, out: &mut Vec<DocSplice>) {
    let sites: Vec<DocLiteralSite> = run
        .iter()
        .filter_map(|a| doc_attr_literal_span(a, original, DocShape::SafeLineOrAttr))
        .collect();
    if sites.is_empty() {
        return;
    }
    let joined = sites
        .iter()
        .map(|s| s.value.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let rewritten = rewrite_rustdoc_link_idioms(&joined);
    if rewritten == joined {
        return;
    }
    let parts: Vec<&str> = rewritten.split('\n').collect();
    if parts.len() != sites.len() {
        return;
    }
    for (site, new_payload) in sites.iter().zip(parts) {
        if new_payload == site.value {
            continue;
        }
        let Some(body) = original.get(site.range.clone()) else {
            continue;
        };
        let replacement = match site.kind {
            DocLiteralKind::OuterLine => render_line_doc(body, "///", new_payload),
            DocLiteralKind::InnerLine => render_line_doc(body, "//!", new_payload),
            DocLiteralKind::QuotedAttr => Some(render_quoted_doc_literal(new_payload)),
        };
        let Some(replacement) = replacement else {
            continue;
        };
        out.push(DocSplice {
            range: site.range.clone(),
            replacement,
        });
    }
}
fn render_line_doc(body: &str, marker: &str, new_payload: &str) -> Option<String> {
    if !body.starts_with(marker) {
        return None;
    }
    let mut out = String::with_capacity(marker.len() + new_payload.len());
    out.push_str(marker);
    out.push_str(new_payload);
    Some(out)
}
fn render_quoted_doc_literal(value: &str) -> String {
    proc_macro2::Literal::string(value).to_string()
}
fn collect_cfg_attr_doc_splices(attrs: &[Attribute], original: &str, out: &mut Vec<DocSplice>) {
    for attr in attrs {
        if !is_cfg_attr(attr) {
            continue;
        }
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        collect_cfg_attr_list_doc_splices(list, original, out);
    }
}
fn collect_cfg_attr_list_doc_splices(
    list: &syn::MetaList,
    original: &str,
    out: &mut Vec<DocSplice>,
) {
    let Ok(args) = cfg_predicate_args(list) else {
        return;
    };
    for arg in args.iter().skip(1) {
        let CfgPredicate::Meta(meta) = arg else {
            continue;
        };
        match &**meta {
            Meta::List(inner) if path_is(&inner.path, "cfg_attr") => {
                collect_cfg_attr_list_doc_splices(inner, original, out);
            }
            Meta::NameValue(nv) if path_is(&nv.path, "doc") => {
                push_doc_literal_splice(nv, original, out);
            }
            _ => {}
        }
    }
}
fn push_doc_literal_splice(nv: &syn::MetaNameValue, original: &str, out: &mut Vec<DocSplice>) {
    let syn::Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(s),
        ..
    }) = &nv.value
    else {
        return;
    };
    let range = s.span().byte_range();
    let Some(body) = original.get(range.clone()) else {
        return;
    };
    if !(body.starts_with('"') && body.ends_with('"')) {
        return;
    }
    let value = s.value();
    let rewritten = rewrite_rustdoc_link_idioms(&value);
    if rewritten == value {
        return;
    }
    let replacement = render_quoted_doc_literal(&rewritten);
    out.push(DocSplice { range, replacement });
}
/// Rewrite mechanically-safe Rust item links in `doc_text`.
///
/// Operates on the prose of a single doc-comment block (concatenated
/// payloads of one item, joined by `\n`). Block structure is resolved
/// before inline structure, as `CommonMark` requires:
///
/// ```text
/// - fenced code: 0-3 columns of indent, closed only by the same marker
///   with a run at least as long as the opener
/// - indented code: 4+ columns at a block start, tab counting to column 4
/// - reference definitions: recognised only at a block start
/// - inline code spans: tracked only within the remaining prose lines; a
///   run with no matching closing run inside its block is literal text
/// ```
///
/// Rules applied only when the label is a conservative Rust item
/// token:
///
/// ```text
/// [Type](Type)             -> [`Type`]              (redundant target collapsed)
/// [Type]                   -> [`Type`]              (shortcut form gets ticks)
/// [label](Target)          -> [`label`](Target)    (label ticked; target kept)
/// ```
///
/// Skipped (left verbatim):
///
/// ```text
/// - lines inside fenced code blocks and indented code blocks
/// - spans inside inline code, for any backtick run length
/// - URL targets (contain ://, or start with /, #, mailto:)
/// - reference definitions ([label]: <url>) and reference links ([label][ref])
/// - shortcut references whose label is defined anywhere in the block
/// - labels opened by an escaped bracket (\[label])
/// - targets with generics, disambiguators, or fragments (< > @ # ( ) ! ?)
/// - labels already wrapped in backticks (idempotent)
/// - prose labels — anything not matching is_codeish_path
/// - empty link bodies
/// ```
#[must_use]
pub fn rewrite_rustdoc_link_idioms(doc_text: &str) -> String {
    let labels = ReferenceLabels::index(doc_text);
    let lines: Vec<&str> = doc_text.split('\n').collect();
    let mut out = String::with_capacity(doc_text.len());
    let mut blocks = BlockScan::new();
    let mut span = CodeSpanState::Closed;
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let following = &lines[index + 1..];
        match blocks.classify(line) {
            LineFrame::Code | LineFrame::Definition => {
                span = CodeSpanState::Closed;
                out.push_str(line);
            }
            LineFrame::Prose => {
                span = rewrite_line_links(line, following, &labels, span, &mut out);
            }
        }
    }
    out
}
struct ReferenceLabels(BTreeSet<String>);
impl ReferenceLabels {
    fn index(doc_text: &str) -> Self {
        let lines: Vec<&str> = doc_text.split('\n').collect();
        let mut labels = BTreeSet::new();
        let mut blocks = BlockScan::new();
        let mut span = CodeSpanState::Closed;
        for (index, line) in lines.iter().enumerate() {
            let following = &lines[index + 1..];
            match blocks.classify(line) {
                LineFrame::Code => span = CodeSpanState::Closed,
                LineFrame::Prose => span = advance_span(line, following, span),
                LineFrame::Definition => {
                    span = CodeSpanState::Closed;
                    if let Some(label) = reference_definition_label(line) {
                        let normalised = normalise_link_label(label);
                        if !normalised.is_empty() {
                            labels.insert(normalised);
                        }
                    }
                }
            }
        }
        Self(labels)
    }
    fn defines(&self, label_src: &str) -> bool {
        self.0.contains(&normalise_link_label(label_src))
    }
}
fn normalise_link_label(label_src: &str) -> String {
    let mut normalised = String::with_capacity(label_src.len());
    let mut gap = false;
    for ch in label_src.trim().chars() {
        if ch.is_whitespace() {
            gap = true;
            continue;
        }
        if gap && !normalised.is_empty() {
            normalised.push(' ');
        }
        gap = false;
        normalised.extend(ch.to_lowercase());
    }
    normalised
}
fn rewrite_line_links(
    line: &str,
    following: &[&str],
    labels: &ReferenceLabels,
    entry: CodeSpanState,
    out: &mut String,
) -> CodeSpanState {
    let bytes = line.as_bytes();
    let mut span = entry;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = backtick_run_len(bytes, i);
            span = span_after_run(span, run, &line[i + run..], following);
            out.push_str(&line[i..i + run]);
            i += run;
            continue;
        }
        if !span.is_open() && bytes[i] == b'\\' {
            let escaped = escape_pair_len(line, i);
            out.push_str(&line[i..i + escaped]);
            i += escaped;
            continue;
        }
        if span.is_open() || bytes[i] != b'[' {
            let step = char_len_at(line, i);
            out.push_str(&line[i..i + step]);
            i += step;
            continue;
        }
        if let Some((shape, consumed)) = parse_link_at(line, i) {
            emit_link(out, &shape, labels);
            i += consumed;
        } else {
            out.push('[');
            i += 1;
        }
    }
    span
}
fn escape_pair_len(line: &str, start: usize) -> usize {
    let mut chars = line[start..].chars();
    let backslash = chars.next().map_or(0, char::len_utf8);
    backslash + chars.next().map_or(0, char::len_utf8)
}
fn char_len_at(line: &str, start: usize) -> usize {
    line[start..].chars().next().map_or(1, char::len_utf8)
}
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkShape {
    Inline {
        label_src: String,
        target_src: String,
    },
    Reference {
        raw: String,
    },
    Shortcut {
        label_src: String,
    },
}
fn parse_link_at(line: &str, start: usize) -> Option<(LinkShape, usize)> {
    let bytes = line.as_bytes();
    let label_end = find_matching_bracket(line, start)?;
    let label_src = line[start + 1..label_end].to_string();
    let after_label = label_end + 1;
    if after_label < bytes.len() && bytes[after_label] == b'(' {
        let paren_end = find_matching_paren(line, after_label)?;
        let target_src = line[after_label + 1..paren_end].to_string();
        return Some((
            LinkShape::Inline {
                label_src,
                target_src,
            },
            paren_end + 1 - start,
        ));
    }
    if after_label < bytes.len() && bytes[after_label] == b'[' {
        let ref_end = find_matching_bracket(line, after_label)?;
        let raw = line[start..=ref_end].to_string();
        return Some((LinkShape::Reference { raw }, ref_end + 1 - start));
    }
    Some((LinkShape::Shortcut { label_src }, after_label - start))
}
const fn find_matching_paren(line: &str, open: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b')' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}
fn emit_link(out: &mut String, shape: &LinkShape, labels: &ReferenceLabels) {
    match shape {
        LinkShape::Reference { raw } => {
            out.push_str(raw);
        }
        LinkShape::Inline {
            label_src,
            target_src,
        } => emit_inline_link(out, label_src, target_src),
        LinkShape::Shortcut { label_src } => emit_shortcut_link(out, label_src, labels),
    }
}
fn emit_inline_link(out: &mut String, label_src: &str, target_src: &str) {
    let target_trim = target_src.trim();
    if target_trim.is_empty() || !is_safe_intra_doc_target(target_trim) {
        write_inline(out, label_src, target_src);
        return;
    }
    if label_src == target_trim && is_codeish_path(label_src) {
        write_shortcut_ticked(out, label_src);
        return;
    }
    if is_codeish_path(label_src) && !label_src_has_backticks(label_src) {
        write_inline_label_ticked(out, label_src, target_src);
        return;
    }
    write_inline(out, label_src, target_src);
}
fn emit_shortcut_link(out: &mut String, label_src: &str, labels: &ReferenceLabels) {
    if is_codeish_path(label_src)
        && !label_src_has_backticks(label_src)
        && !labels.defines(label_src)
    {
        write_shortcut_ticked(out, label_src);
    } else {
        out.push('[');
        out.push_str(label_src);
        out.push(']');
    }
}
fn write_shortcut_ticked(out: &mut String, label: &str) {
    out.push('[');
    out.push('`');
    out.push_str(label);
    out.push('`');
    out.push(']');
}
fn write_inline_label_ticked(out: &mut String, label: &str, target: &str) {
    out.push('[');
    out.push('`');
    out.push_str(label);
    out.push('`');
    out.push(']');
    out.push('(');
    out.push_str(target);
    out.push(')');
}
fn write_inline(out: &mut String, label: &str, target: &str) {
    out.push('[');
    out.push_str(label);
    out.push(']');
    out.push('(');
    out.push_str(target);
    out.push(')');
}
fn label_src_has_backticks(label: &str) -> bool {
    label.contains('`')
}
fn is_safe_intra_doc_target(target: &str) -> bool {
    if target.is_empty() {
        return false;
    }
    if target.contains("://")
        || target.starts_with('#')
        || target.starts_with('/')
        || target.starts_with("mailto:")
    {
        return false;
    }
    for ch in target.chars() {
        match ch {
            '<' | '>' | '@' | '#' | '(' | ')' | '!' | '?' | ' ' | '\t' => return false,
            _ => {}
        }
    }
    is_codeish_path(target)
}
fn is_codeish_path(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let trimmed = s.strip_prefix("::").unwrap_or(s);
    if trimmed.is_empty() {
        return false;
    }
    for segment in trimmed.split("::") {
        if !is_rust_ident(segment) {
            return false;
        }
    }
    true
}
fn is_rust_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    if s.len() == 1 && first == '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
#[cfg(test)]
mod rustdoc_link_idiom_tests {
    use super::{is_codeish_path, rewrite_rustdoc_link_idioms};
    #[test]
    fn multibyte_utf8_survives_rewrite_pure() {
        let input = "see [Type] — also русский and 🦀";
        let out = rewrite_rustdoc_link_idioms(input);
        assert_eq!(out, "see [`Type`] — also русский and 🦀");
    }
    #[test]
    fn em_dash_survives_a_quoted_attr_doc_splice() {
        let original = "#[doc = \" see [Type] — русский 🦀\"]\npub fn f() {}\n";
        let ast: syn::File = syn::parse_file(original).unwrap();
        let splices = super::collect_doc_splices(&ast, original);
        let rewritten = super::apply_splices(original, splices);
        assert_eq!(
            rewritten, "#[doc = \" see [`Type`] — русский 🦀\"]\npub fn f() {}\n",
            "non-ASCII payload mangled by the quoted-attr splice: {rewritten:?}"
        );
    }
    fn rw(s: &str) -> String {
        rewrite_rustdoc_link_idioms(s)
    }
    #[test]
    fn redundant_explicit_link_collapses_and_ticks() {
        assert_eq!(rw("see [Type](Type) for"), "see [`Type`] for");
    }
    #[test]
    fn redundant_explicit_path_link_collapses_and_ticks() {
        assert_eq!(rw("via [foo::Bar](foo::Bar) here"), "via [`foo::Bar`] here");
    }
    #[test]
    fn explicit_target_retained_label_ticked() {
        assert_eq!(
            rw("call [begin](Self::begin) first"),
            "call [`begin`](Self::begin) first"
        );
        assert_eq!(
            rw("see [Reader](crate::Reader) docs"),
            "see [`Reader`](crate::Reader) docs"
        );
    }
    #[test]
    fn shortcut_camel_case_ticked() {
        assert_eq!(rw("the [Type] applies"), "the [`Type`] applies");
    }
    #[test]
    fn shortcut_path_ticked() {
        assert_eq!(rw("see [foo::Bar] above"), "see [`foo::Bar`] above");
    }
    #[test]
    fn shortcut_self_super_crate_ticked() {
        assert_eq!(rw("the [Self] of"), "the [`Self`] of");
        assert_eq!(rw("from [super::Foo]"), "from [`super::Foo`]");
        assert_eq!(rw("via [crate::Reader]"), "via [`crate::Reader`]");
    }
    #[test]
    fn shortcut_snake_case_ticked() {
        assert_eq!(rw("call [do_thing] next"), "call [`do_thing`] next");
    }
    #[test]
    fn prose_label_not_rewritten() {
        assert_eq!(
            rw("see [the writer](Writer) for"),
            "see [the writer](Writer) for"
        );
    }
    #[test]
    fn url_link_not_rewritten() {
        assert_eq!(
            rw("see [docs](https://example.com)"),
            "see [docs](https://example.com)"
        );
        assert_eq!(rw("the [home](/index.html)"), "the [home](/index.html)");
        assert_eq!(
            rw("mail [admin](mailto:a@example.com)"),
            "mail [admin](mailto:a@example.com)"
        );
    }
    #[test]
    fn fragment_target_not_rewritten() {
        assert_eq!(rw("see [Foo](#anchor)"), "see [Foo](#anchor)");
    }
    #[test]
    fn target_with_generics_not_rewritten() {
        assert_eq!(rw("see [Vec](Vec<u8>) usage"), "see [Vec](Vec<u8>) usage");
    }
    #[test]
    fn target_with_disambiguator_not_rewritten() {
        assert_eq!(rw("call [foo](foo()) here"), "call [foo](foo()) here");
        assert_eq!(rw("call [m](m!) macro"), "call [m](m!) macro");
        assert_eq!(
            rw("see [t](struct@Type) struct"),
            "see [t](struct@Type) struct"
        );
    }
    #[test]
    fn reference_style_link_not_rewritten() {
        assert_eq!(rw("see [Type][ref] later"), "see [Type][ref] later");
    }
    #[test]
    fn reference_definition_not_rewritten() {
        assert_eq!(
            rw("[ref]: https://example.com"),
            "[ref]: https://example.com"
        );
    }
    #[test]
    fn fenced_code_block_left_verbatim() {
        let input = "before\n```\nlet x: [Type] = foo();\n[Type](Type)\n```\nafter [Type]";
        let expected = "before\n```\nlet x: [Type] = foo();\n[Type](Type)\n```\nafter [`Type`]";
        assert_eq!(rw(input), expected);
    }
    #[test]
    fn tilde_fenced_code_block_left_verbatim() {
        let input = "before\n~~~\n[Type](Type)\n~~~\nafter [Type]";
        let expected = "before\n~~~\n[Type](Type)\n~~~\nafter [`Type`]";
        assert_eq!(rw(input), expected);
    }
    #[test]
    fn inline_code_span_left_verbatim() {
        assert_eq!(
            rw("use `[Type]` syntax for [Type]"),
            "use `[Type]` syntax for [`Type`]"
        );
    }
    #[test]
    fn already_ticked_shortcut_left_verbatim() {
        assert_eq!(rw("see [`Type`] above"), "see [`Type`] above");
    }
    #[test]
    fn already_ticked_inline_left_verbatim() {
        assert_eq!(
            rw("call [`foo`](Self::foo) now"),
            "call [`foo`](Self::foo) now"
        );
    }
    #[test]
    fn empty_link_body_not_rewritten() {
        assert_eq!(rw("an [] empty"), "an [] empty");
    }
    #[test]
    fn empty_target_not_rewritten() {
        assert_eq!(rw("a [Type]() blank"), "a [Type]() blank");
    }
    #[test]
    fn is_codeish_path_basic() {
        assert!(is_codeish_path("Type"));
        assert!(is_codeish_path("foo_bar"));
        assert!(is_codeish_path("foo::Bar"));
        assert!(is_codeish_path("Self"));
        assert!(is_codeish_path("self"));
        assert!(is_codeish_path("super::Foo"));
        assert!(is_codeish_path("crate::Reader"));
        assert!(is_codeish_path("::foo::Bar"));
        assert!(!is_codeish_path(""));
        assert!(!is_codeish_path("two words"));
        assert!(!is_codeish_path("foo()"));
        assert!(!is_codeish_path("Vec<u8>"));
        assert!(!is_codeish_path("foo!"));
        assert!(!is_codeish_path("_"));
        assert!(!is_codeish_path("9bad"));
        assert!(!is_codeish_path("foo:bar"));
        assert!(!is_codeish_path("foo::"));
    }
    #[test]
    fn idempotent_rewrite() {
        let inputs = [
            "see [Type](Type)",
            "call [begin](Self::begin)",
            "the [Type]",
            "see `[Type]` literal",
            "in a fence\n```\n[Type]\n```\nout",
            "see [the writer](Writer)",
        ];
        for input in inputs {
            let once = rw(input);
            let twice = rw(&once);
            assert_eq!(once, twice, "non-idempotent for: {input}");
        }
    }
    #[test]
    fn multiline_fence_state_persists() {
        let input = "p1\n```\n[A](A)\n```\np2 [B]\n```\n[C]\n```\np3 [D](D)";
        let expected = "p1\n```\n[A](A)\n```\np2 [`B`]\n```\n[C]\n```\np3 [`D`]";
        assert_eq!(rw(input), expected);
    }
    #[test]
    fn multiple_links_on_one_line() {
        assert_eq!(
            rw("see [A] and [B](B) then [C](Self::C)"),
            "see [`A`] and [`B`] then [`C`](Self::C)"
        );
    }
    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(rw(""), "");
    }
    #[test]
    fn no_links_passthrough() {
        let s = "plain prose with no links\nand a second line\n";
        assert_eq!(rw(s), s);
    }
}
#[cfg(test)]
mod markdown_framing_grammar_tests {
    use super::rewrite_rustdoc_link_idioms as rw;
    fn unchanged(input: &str) {
        assert_eq!(rw(input), input, "expected byte-identical for {input:?}");
    }
    #[test]
    fn g01_shortcut_with_later_definition_is_preserved() {
        unchanged("see [Type] here\n\n[Type]: https://example.com/t");
    }
    #[test]
    fn g02_shortcut_without_definition_is_still_ticked() {
        assert_eq!(
            rw("see [Type] here\n\n[Other]: https://example.com/o"),
            "see [`Type`] here\n\n[Other]: https://example.com/o"
        );
    }
    #[test]
    fn g03_definition_before_reference_is_preserved() {
        unchanged("[Type]: https://example.com/t\n\nsee [Type] here");
    }
    #[test]
    fn g04_label_match_is_case_and_whitespace_normalised() {
        unchanged("see [type] here\n\n[Type]: https://example.com/t");
        unchanged("see [Foo   Bar] here\n\n[foo bar]: https://example.com/f");
    }
    #[test]
    fn g05_collapsed_reference_is_preserved() {
        unchanged("see [Type][] here\n\n[Type]: https://example.com/t");
    }
    #[test]
    fn g06_full_reference_is_preserved() {
        unchanged("see [Type][ref] here\n\n[ref]: https://example.com/t");
    }
    #[test]
    fn g07_inline_link_still_collapses_even_with_a_definition_present() {
        assert_eq!(
            rw("see [Type](Type) here\n\n[Type]: https://example.com/t"),
            "see [`Type`] here\n\n[Type]: https://example.com/t"
        );
    }
    #[test]
    fn g08_definition_indented_up_to_three_spaces_is_indexed() {
        unchanged("see [Type] here\n\n   [Type]: https://example.com/t");
    }
    #[test]
    fn g10_definition_with_title_on_same_or_next_line_is_indexed() {
        unchanged("see [Type] here\n\n[Type]: https://example.com/t \"A title\"");
        unchanged("see [Type] here\n\n[Type]: https://example.com/t\n   \"A title\"");
    }
    #[test]
    fn g11_label_spanning_a_line_break_is_never_rewritten() {
        unchanged("see [Type\nName] here");
    }
    #[test]
    fn g12_escaped_open_bracket_is_not_a_link() {
        unchanged("literal \\[Type] stays");
        unchanged("literal \\[Type\\] stays");
    }
    #[test]
    fn g13_single_backtick_span_is_preserved() {
        unchanged("use `[Type]` verbatim");
    }
    #[test]
    fn g14_two_backtick_span_is_preserved() {
        unchanged("use ``[Type]`` verbatim");
    }
    #[test]
    fn g15_three_backtick_inline_span_is_preserved() {
        unchanged("use ```[Type]``` verbatim");
    }
    #[test]
    fn g16_opening_run_longer_than_closing_run_is_literal_and_rewrites() {
        assert_eq!(rw("use ```[Type]` verbatim"), "use ```[`Type`]` verbatim");
    }
    #[test]
    fn g17_closing_run_longer_than_opening_run_is_literal_and_rewrites() {
        assert_eq!(rw("use `[Type]``` verbatim"), "use `[`Type`]``` verbatim");
    }
    #[test]
    fn g18_backticks_inside_a_fenced_block_are_inert() {
        unchanged("before\n```\n``[Type]``\n```\nafter [Other][r]\n\n[r]: https://e.com/r");
    }
    #[test]
    fn g19_unmatched_run_does_not_panic_and_still_rewrites() {
        assert_eq!(rw("dangling `` [Type] tail"), "dangling `` [`Type`] tail");
        assert_eq!(rw("dangling ` [Type] tail"), "dangling ` [`Type`] tail");
    }
    #[test]
    fn g20_multi_line_code_span_is_preserved() {
        unchanged("open ``spanning\n[Type] still code`` closed");
    }
    #[test]
    fn g21_stray_backtick_does_not_disable_the_next_line() {
        assert_eq!(
            rw("stray ` tail\nthe [Type] applies"),
            "stray ` tail\nthe [`Type`] applies"
        );
    }
    #[test]
    fn g22_definition_inside_a_fenced_block_does_not_shield_a_shortcut() {
        assert_eq!(
            rw("see [Type] here\n```\n[Type]: https://example.com/t\n```\ndone"),
            "see [`Type`] here\n```\n[Type]: https://example.com/t\n```\ndone"
        );
    }
    #[test]
    fn g09_definition_indented_four_spaces_is_indented_code_not_a_definition() {
        assert_eq!(
            rw("see [Type] here\n\n    [Type]: https://example.com/t"),
            "see [`Type`] here\n\n    [Type]: https://example.com/t"
        );
    }
    #[test]
    fn g23_inline_span_opening_with_three_ticks_is_not_a_fence() {
        assert_eq!(
            rw("```[Type]``` tail\n[Other] applies"),
            "```[Type]``` tail\n[`Other`] applies"
        );
    }
    #[test]
    fn g24_three_tick_line_does_not_close_a_four_tick_fence() {
        assert_eq!(
            rw("````\n```\n[Type]\n````\nafter [Other]"),
            "````\n```\n[Type]\n````\nafter [`Other`]"
        );
    }
    #[test]
    fn g25_open_code_span_survives_a_definition_looking_line() {
        unchanged("``open\n[Type]: /type\n[Other]\nclose``");
    }
    #[test]
    fn g26_tab_indented_definition_is_indented_code_not_a_definition() {
        assert_eq!(
            rw("see [Type] here\n\n\t[Type]: https://example.com/t"),
            "see [`Type`] here\n\n\t[Type]: https://example.com/t"
        );
    }
    #[test]
    fn g27_fence_opener_indented_four_spaces_is_not_a_fence() {
        assert_eq!(rw("    ```\n[Type] here"), "    ```\n[`Type`] here");
    }
    #[test]
    fn g28_backtick_line_does_not_close_a_tilde_fence() {
        assert_eq!(
            rw("~~~\n[Type]\n```\n[Other]\n~~~\ndone [Third]"),
            "~~~\n[Type]\n```\n[Other]\n~~~\ndone [`Third`]"
        );
    }
    #[test]
    fn g29_shorter_closer_does_not_close_and_fence_runs_to_end() {
        unchanged("````\n[Type]\n```\n[Other]");
    }
    #[test]
    fn g40_invalid_backtick_opener_does_not_interrupt_a_code_span() {
        unchanged("``open [Type]\n```foo`bar\nclose``");
    }
    #[test]
    fn g41_valid_backtick_opener_with_info_interrupts_a_code_span() {
        assert_eq!(
            rw("``open [Type]\n```foo\nclose``"),
            "``open [`Type`]\n```foo\nclose``"
        );
    }
    #[test]
    fn g30_definition_cannot_interrupt_a_paragraph() {
        assert_eq!(
            rw("text paragraph\n[Type]: https://example.com/t\n\nsee [Type] here"),
            "text paragraph\n[`Type`]: https://example.com/t\n\nsee [`Type`] here"
        );
    }
    #[test]
    fn g31_blank_line_terminates_an_open_code_span() {
        assert_eq!(
            rw("open ``run\n\n[Type] applies\nclose``"),
            "open ``run\n\n[`Type`] applies\nclose``"
        );
    }
    #[test]
    fn g32_span_closing_mid_line_leaves_the_tail_rewritable() {
        assert_eq!(
            rw("``open close`` and [Type]"),
            "``open close`` and [`Type`]"
        );
    }
    #[test]
    fn g33_fence_closer_with_trailing_text_does_not_close() {
        unchanged("```\n[Type]\n``` tail\n[Other]");
    }
    #[test]
    fn g35_indented_code_block_at_block_start_is_preserved() {
        unchanged("para\n\n    [Type]\n\nnope");
    }
    #[test]
    fn g37_a_fence_interrupts_an_open_code_span() {
        unchanged("``open\n```\nclose``\nafter [Type]");
    }
    #[test]
    fn g39_a_span_cannot_close_past_a_fence_so_its_delimiter_is_literal() {
        assert_eq!(
            rw("``open\ntext [Type] here\n```\nclose``"),
            "``open\ntext [`Type`] here\n```\nclose``"
        );
    }
    #[test]
    fn g38_rewriting_resumes_after_a_fence_that_interrupted_a_span() {
        assert_eq!(
            rw("``open\n```\ncode\n```\nafter [Type]"),
            "``open\n```\ncode\n```\nafter [`Type`]"
        );
    }
    #[test]
    fn g36_four_space_indent_interrupting_a_paragraph_is_prose() {
        assert_eq!(rw("para\n    [Type]"), "para\n    [`Type`]");
    }
}

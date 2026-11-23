use crate::rewrite_rustdoc_link_idioms;
use crate::{CfgPredicate, cfg_predicate_args};
use syn::ext::IdentExt as _;
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
pub(crate) fn ident_is(id: &syn::Ident, name: &str) -> bool {
    id.unraw() == name
}
pub(crate) fn path_is(path: &syn::Path, name: &str) -> bool {
    path.get_ident().is_some_and(|id| ident_is(id, name))
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
#[must_use]
pub(crate) fn is_cfg_attr(attr: &Attribute) -> bool {
    match &attr.meta {
        Meta::List(list) => path_is(&list.path, "cfg_attr"),
        _ => false,
    }
}

#[cfg(test)]
mod rustdoc_link_idiom_tests {
    use crate::{is_codeish_path, rewrite_rustdoc_link_idioms};
    #[test]
    fn multibyte_utf8_survives_rewrite_pure() {
        let input = "see [Type] — also русский and 🦀";
        let out = crate::rewrite_rustdoc_link_idioms(input);
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

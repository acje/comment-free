use syn::ext::IdentExt as _;
use syn::punctuated::Punctuated;
use syn::{Attribute, Meta, Token};
pub(crate) fn ident_is(id: &syn::Ident, name: &str) -> bool {
    id.unraw() == name
}
pub(crate) fn path_is(path: &syn::Path, name: &str) -> bool {
    path.get_ident().is_some_and(|id| ident_is(id, name))
}
/// Truth value of a `cfg` predicate under a build configuration this
/// crate has not been told, folded as far as the predicate's own
/// boolean constants allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CfgTruth {
    Always,
    Never,
    Unresolved,
}
impl CfgTruth {
    pub(crate) const fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Never, _) | (_, Self::Never) => Self::Never,
            (Self::Always, Self::Always) => Self::Always,
            (Self::Unresolved, Self::Unresolved | Self::Always)
            | (Self::Always, Self::Unresolved) => Self::Unresolved,
        }
    }
    const fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::Always, _) | (_, Self::Always) => Self::Always,
            (Self::Never, Self::Never) => Self::Never,
            (Self::Never, Self::Unresolved)
            | (Self::Unresolved, Self::Never | Self::Unresolved) => Self::Unresolved,
        }
    }
    const fn negate(self) -> Self {
        match self {
            Self::Always => Self::Never,
            Self::Never => Self::Always,
            Self::Unresolved => Self::Unresolved,
        }
    }
}
/// One argument of a `cfg`-style predicate list, which rustc accepts as
/// either a boolean literal or a `Meta` (path, `key = "value"`, or a
/// `all`/`any`/`not` list). `syn::Meta` alone cannot hold `true` or
/// `false`, because they are keywords rather than paths.
pub(crate) enum CfgPredicate {
    Bool(bool),
    Meta(Box<Meta>),
}
impl syn::parse::Parse for CfgPredicate {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(syn::LitBool) {
            return Ok(Self::Bool(input.parse::<syn::LitBool>()?.value()));
        }
        Ok(Self::Meta(Box::new(input.parse()?)))
    }
}
pub(crate) fn fold_cfg_predicate(predicate: &CfgPredicate) -> CfgTruth {
    let meta = match predicate {
        CfgPredicate::Bool(true) => return CfgTruth::Always,
        CfgPredicate::Bool(false) => return CfgTruth::Never,
        CfgPredicate::Meta(meta) => &**meta,
    };
    let Meta::List(list) = meta else {
        return CfgTruth::Unresolved;
    };
    let Ok(inner) = cfg_predicate_args(list) else {
        return CfgTruth::Unresolved;
    };
    if path_is(&list.path, "all") {
        return inner
            .iter()
            .fold(CfgTruth::Always, |acc, m| acc.and(fold_cfg_predicate(m)));
    }
    if path_is(&list.path, "any") {
        return inner
            .iter()
            .fold(CfgTruth::Never, |acc, m| acc.or(fold_cfg_predicate(m)));
    }
    match (path_is(&list.path, "not"), inner.first()) {
        (true, Some(only)) if inner.len() == 1 => fold_cfg_predicate(only).negate(),
        _ => CfgTruth::Unresolved,
    }
}
pub(crate) fn cfg_predicate_args(
    list: &syn::MetaList,
) -> syn::Result<Punctuated<CfgPredicate, Token![,]>> {
    list.parse_args_with(Punctuated::parse_terminated)
}
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "syn::Meta is an external vocabulary; only a list meta spells cfg_attr"
)]
#[must_use]
pub(crate) fn is_cfg_attr(attr: &Attribute) -> bool {
    match &attr.meta {
        Meta::List(list) => path_is(&list.path, "cfg_attr"),
        _ => false,
    }
}

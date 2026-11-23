#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use syn::{Item, ItemEnum};

#[derive(Debug)]
struct Violation {
    path: PathBuf,
    line: Option<usize>,
    enum_name: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(line) => write!(
                formatter,
                "{}:{} {}",
                self.path.display(),
                line,
                self.enum_name
            ),
            None => write!(formatter, "{} {}", self.path.display(), self.enum_name),
        }
    }
}

fn format_violations(violations: &[Violation]) -> String {
    let rendered: Vec<String> = violations.iter().map(ToString::to_string).collect();
    rendered.join("; ")
}

#[derive(Debug)]
struct ScanReport {
    files: usize,
    pub_enums: usize,
    pub_error_enums: usize,
    manual_error_impls: usize,
    violations: Vec<Violation>,
}

fn scan(root: &Path) -> Result<ScanReport, String> {
    if !root.is_dir() {
        return Err(format!("root is not a directory: {}", root.display()));
    }
    let mut files: Vec<PathBuf> = Vec::new();
    collect_rs_files(root, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("no .rs files under {}", root.display()));
    }

    let mut enums: Vec<(PathBuf, ItemEnum)> = Vec::new();
    let mut manual_error_types: BTreeSet<String> = BTreeSet::new();
    for file in &files {
        let source = fs::read_to_string(file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let parsed = syn::parse_file(&source)
            .map_err(|error| format!("failed to parse {}: {error}", file.display()))?;
        let mut file_enums: Vec<&ItemEnum> = Vec::new();
        collect_enums(&parsed.items, &mut file_enums);
        for item_enum in file_enums {
            enums.push((file.clone(), item_enum.clone()));
        }
        collect_error_impls(&parsed.items, &mut manual_error_types);
    }

    let mut pub_enums = 0;
    let mut pub_error_enums = 0;
    let mut violations = Vec::new();
    for (path, item_enum) in &enums {
        if !is_literal_pub(item_enum) || item_enum.attrs.iter().any(is_cfg_disabled_without_test) {
            continue;
        }
        pub_enums += 1;
        let is_error =
            derives_error(item_enum) || manual_error_types.contains(&item_enum.ident.to_string());
        if is_error {
            pub_error_enums += 1;
        }
        if is_violation(item_enum, &manual_error_types) {
            violations.push(Violation {
                path: path.clone(),
                line: enum_span_line(item_enum),
                enum_name: item_enum.ident.to_string(),
            });
        }
    }
    violations
        .sort_by(|left, right| (&left.path, &left.enum_name).cmp(&(&right.path, &right.enum_name)));

    Ok(ScanReport {
        files: files.len(),
        pub_enums,
        pub_error_enums,
        manual_error_impls: manual_error_types.len(),
        violations,
    })
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(dir)
        .map_err(|error| format!("failed to read dir {}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("failed to read dir entry in {}: {error}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            collect_rs_files(&path, out)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "syn::Item has dozens of AST variants; only Enum and Mod can contain enum declarations"
)]
fn collect_enums<'a>(items: &'a [Item], out: &mut Vec<&'a ItemEnum>) {
    for item in items {
        match item {
            Item::Enum(item_enum) => out.push(item_enum),
            Item::Mod(module) => {
                if module.attrs.iter().any(is_cfg_disabled_without_test) {
                    continue;
                }
                if let Some((_, inner_items)) = &module.content {
                    collect_enums(inner_items, out);
                }
            }
            _ => {}
        }
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "syn::Item has dozens of AST variants; only Impl and Mod can carry a manual Error impl"
)]
fn collect_error_impls(items: &[Item], out: &mut BTreeSet<String>) {
    for item in items {
        match item {
            Item::Impl(item_impl) => {
                if item_impl.attrs.iter().any(is_cfg_disabled_without_test) {
                    continue;
                }
                let Some((_, trait_path, _)) = &item_impl.trait_ else {
                    continue;
                };
                if trait_path
                    .segments
                    .last()
                    .is_none_or(|segment| segment.ident != "Error")
                {
                    continue;
                }
                let syn::Type::Path(type_path) = &*item_impl.self_ty else {
                    continue;
                };
                if let Some(segment) = type_path.path.segments.last() {
                    out.insert(segment.ident.to_string());
                }
            }
            Item::Mod(module) => {
                if module.attrs.iter().any(is_cfg_disabled_without_test) {
                    continue;
                }
                if let Some((_, inner_items)) = &module.content {
                    collect_error_impls(inner_items, out);
                }
            }
            _ => {}
        }
    }
}

fn attr_path_is(attr: &syn::Attribute, name: &str) -> bool {
    attr.path().is_ident(name)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CfgTruth {
    False,
    True,
    Unknown,
}

impl syn::parse::Parse for CfgTruth {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        use syn::ext::IdentExt;

        if input.peek(syn::LitBool) {
            return Ok(if input.parse::<syn::LitBool>()?.value {
                Self::True
            } else {
                Self::False
            });
        }
        let name = input.call(syn::Ident::parse_any)?.unraw().to_string();
        if input.peek(syn::Token![=]) {
            input.parse::<syn::Token![=]>()?;
            input.parse::<syn::LitStr>()?;
            return Ok(Self::Unknown);
        }
        if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            let values = content.parse_terminated(Self::parse, syn::Token![,])?;
            return match name.as_str() {
                "all" | "any" => {
                    let decisive = if name == "all" {
                        Self::False
                    } else {
                        Self::True
                    };
                    Ok(if values.iter().any(|value| *value == decisive) {
                        decisive
                    } else if values.iter().any(|value| *value == Self::Unknown) {
                        Self::Unknown
                    } else if name == "all" {
                        Self::True
                    } else {
                        Self::False
                    })
                }
                "not" if values.len() == 1 => Ok(match values[0] {
                    Self::False => Self::True,
                    Self::True => Self::False,
                    Self::Unknown => Self::Unknown,
                }),
                _ => Err(input.error("expected all(...), any(...) or not(one predicate)")),
            };
        }
        Ok(if name == "test" {
            Self::False
        } else {
            Self::Unknown
        })
    }
}

fn is_cfg_disabled_without_test(attr: &syn::Attribute) -> bool {
    attr_path_is(attr, "cfg")
        && matches!(
            attr.parse_args_with(|input: syn::parse::ParseStream<'_>| {
                let value = input.parse::<CfgTruth>()?;
                if input.peek(syn::Token![,]) {
                    input.parse::<syn::Token![,]>()?;
                }
                Ok(value)
            }),
            Ok(CfgTruth::False)
        )
}

fn derive_idents(attr: &syn::Attribute) -> Vec<String> {
    if !attr_path_is(attr, "derive") {
        return Vec::new();
    }
    let mut names = Vec::new();
    let parsed = attr.parse_args_with(
        syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
    );
    if let Ok(paths) = parsed {
        for path in paths {
            if let Some(segment) = path.segments.last() {
                names.push(segment.ident.to_string());
            }
        }
    }
    names
}

fn is_literal_pub(item_enum: &ItemEnum) -> bool {
    matches!(item_enum.vis, syn::Visibility::Public(_))
}

fn derives_error(item_enum: &ItemEnum) -> bool {
    item_enum
        .attrs
        .iter()
        .flat_map(derive_idents)
        .any(|name| name == "Error")
}

fn is_violation(item_enum: &ItemEnum, manual_error_types: &BTreeSet<String>) -> bool {
    if !is_literal_pub(item_enum) || item_enum.attrs.iter().any(is_cfg_disabled_without_test) {
        return false;
    }
    let has_non_exhaustive = item_enum
        .attrs
        .iter()
        .any(|attr| attr_path_is(attr, "non_exhaustive"));
    has_non_exhaustive
        && (derives_error(item_enum) || manual_error_types.contains(&item_enum.ident.to_string()))
}

fn enum_span_line(item_enum: &ItemEnum) -> Option<usize> {
    let line = item_enum.ident.span().start().line;
    (line != 0).then_some(line)
}

fn first_enum(source: &str) -> ItemEnum {
    let file = syn::parse_file(source).expect("fixture must parse");
    for item in file.items {
        if let Item::Enum(item_enum) = item {
            return item_enum;
        }
    }
    panic!("fixture must contain an enum");
}

fn collect_from(source: &str) -> usize {
    let file = syn::parse_file(source).expect("fixture must parse");
    let mut out = Vec::new();
    collect_enums(&file.items, &mut out);
    out.len()
}

fn manual_error_types(source: &str) -> BTreeSet<String> {
    let file = syn::parse_file(source).expect("fixture must parse");
    let mut out = BTreeSet::new();
    collect_error_impls(&file.items, &mut out);
    out
}

fn flags(source: &str) -> bool {
    is_violation(&first_enum(source), &BTreeSet::new())
}

fn flags_with_manual(source: &str, manual_source: &str) -> bool {
    is_violation(&first_enum(source), &manual_error_types(manual_source))
}

#[test]
fn public_error_enums_are_closed() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let report = scan(&source_root).expect("src/ must scan cleanly");
    let files = report.files;
    let pub_enums = report.pub_enums;
    let pub_error_enums = report.pub_error_enums;
    let manual_impls = report.manual_error_impls;
    println!(
        "closed-error scan: files={files} pub_enums={pub_enums} pub_error_enums={pub_error_enums} manual_error_impls={manual_impls} violations={}",
        report.violations.len()
    );
    assert!(
        files >= 10,
        "expected at least 10 Rust files under src/, scanned {files}"
    );
    assert!(
        pub_enums >= 10,
        "expected at least 10 pub enums under src/, scanned {pub_enums}"
    );
    assert!(
        pub_error_enums >= 2,
        "expected at least 2 pub error enums, scanned {pub_error_enums}"
    );
    assert!(
        report.violations.is_empty(),
        "closed-error violations: {}",
        format_violations(&report.violations)
    );
    let fixture_manual = manual_error_types("impl std::error::Error for FixtureError {}").len();
    assert!(
        fixture_manual >= 1,
        "manual Error detection found no impl in its own fixture"
    );
}

#[test]
fn skips_cfg_test_module_bodies() {
    let source =
        "#[cfg(test)] mod tests { #[derive(thiserror::Error)] pub enum TestOnlyError { A } }";
    assert_eq!(collect_from(source), 0);
}

#[test]
fn collects_non_test_module_bodies() {
    let source = "mod inner { #[derive(thiserror::Error)] pub enum InnerError { A } }";
    assert_eq!(collect_from(source), 1);
}

#[test]
fn positive_flags_non_exhaustive_error_enum() {
    assert!(flags(
        "#[non_exhaustive] #[derive(Debug, thiserror::Error)] pub enum FooError { A }"
    ));
}

#[test]
fn positive_flags_reversed_attr_order() {
    assert!(flags(
        "#[derive(thiserror::Error)] #[non_exhaustive] pub enum BarError { A }"
    ));
}

#[test]
fn negative_compliant_closed_error_enum() {
    assert!(!flags(
        "#[derive(Debug, thiserror::Error)] pub enum FooError { A }"
    ));
}

#[test]
fn negative_repr_dto() {
    assert!(!flags(
        "#[repr(u8)] #[derive(Debug)] pub enum CollectionFailureReason { A = 0 }"
    ));
}

#[test]
fn negative_serde_dto() {
    assert!(!flags(
        "#[derive(Serialize, Deserialize)] pub enum SchedulerEvent { A }"
    ));
}

#[test]
fn negative_serde_repr_dto() {
    assert!(!flags(
        "#[repr(u8)] #[derive(Serialize, Deserialize)] pub enum ExclusionReason { A = 0 }"
    ));
}

#[test]
fn negative_non_pub_error_enum() {
    assert!(!flags(
        "#[derive(thiserror::Error)] #[non_exhaustive] enum PrivErr { A }"
    ));
}

#[test]
fn manual_std_error_impl_flags_non_exhaustive_pub_enum() {
    assert!(flags_with_manual(
        "#[non_exhaustive] pub enum ManualError { A }",
        "impl std::error::Error for ManualError {}"
    ));
}

#[test]
fn manual_core_error_impl_flags_non_exhaustive_pub_enum() {
    assert!(flags_with_manual(
        "#[non_exhaustive] pub enum CoreError { A }",
        "impl core::error::Error for CoreError {}"
    ));
}

#[test]
fn manual_bare_error_impl_flags_non_exhaustive_pub_enum() {
    assert!(flags_with_manual(
        "#[non_exhaustive] pub enum ImportedError { A }",
        "impl Error for ImportedError {}"
    ));
}

#[test]
fn cross_file_manual_impl_flags_enum() {
    let dir = tempfile::tempdir().expect("temp dir must exist");
    fs::write(
        dir.path().join("enum_site.rs"),
        "#[non_exhaustive]\npub enum CrossFileError { One }\n",
    )
    .expect("enum fixture must write");
    fs::write(
        dir.path().join("impl_site.rs"),
        "impl std::error::Error for CrossFileError {}\n",
    )
    .expect("impl fixture must write");
    let report = scan(dir.path()).expect("fixture tree must scan");
    assert_eq!(report.files, 2);
    assert_eq!(report.pub_enums, 1);
    assert_eq!(report.pub_error_enums, 1);
    assert_eq!(report.manual_error_impls, 1);
    assert_eq!(report.violations.len(), 1);
    let violation = &report.violations[0];
    assert_eq!(violation.enum_name, "CrossFileError");
    assert_eq!(violation.path.file_name(), Some(OsStr::new("enum_site.rs")));
    assert_eq!(violation.line, Some(2));
}

#[test]
fn non_error_non_exhaustive_enum_is_silent() {
    assert!(!flags_with_manual(
        "#[non_exhaustive] pub enum DomainEvent { A }",
        "impl std::error::Error for OtherError {}"
    ));
}

#[test]
fn manual_impl_in_cfg_test_module_does_not_taint_production_enum() {
    let production = "#[non_exhaustive]\npub enum ProductionError { A }\n";
    let test_only = "#[cfg(test)]\nmod tests { impl std::error::Error for ProductionError {} }\n";
    let manual = manual_error_types(test_only);
    assert!(
        manual.is_empty(),
        "cfg(test) impl must stay invisible: {manual:?}"
    );
    assert!(!is_violation(&first_enum(production), &manual));

    let dir = tempfile::tempdir().expect("temp dir must exist");
    fs::write(dir.path().join("production.rs"), production).expect("production fixture must write");
    fs::write(dir.path().join("test_impl.rs"), test_only).expect("test impl fixture must write");
    let report = scan(dir.path()).expect("fixture tree must scan");
    assert_eq!(report.manual_error_impls, 0);
    assert!(
        report.violations.is_empty(),
        "cfg(test) impl must not taint production: {}",
        format_violations(&report.violations)
    );
}

#[test]
fn compound_cfg_test_only_is_skipped() {
    assert_eq!(
        collect_from(
            "#[cfg(all(test, feature = \"x\"))] mod tests { #[derive(thiserror::Error)] #[non_exhaustive] pub enum E { A } }"
        ),
        0
    );
}

#[test]
fn cfg_truth_controls_enum_and_inline_module_scanning() {
    for (predicate, skipped) in [
        ("test", true),
        ("test,", true),
        ("r#test", true),
        ("not(test)", false),
        ("not(not(test))", true),
        ("all(test, feature = \"x\")", true),
        ("any(test, feature = \"x\")", false),
        ("not(any(not(test), feature = \"x\"))", true),
        ("all()", false),
        ("any()", true),
        ("true", false),
        ("false", true),
        ("feature = \"test\"", false),
        ("test = \"x\"", false),
        ("all(test, broken())", false),
        ("test, broken()", false),
        ("not(test, test)", false),
        ("not()", false),
        ("test = 1", false),
        ("test extra", false),
    ] {
        let attrs = format!("#[cfg({predicate})]");
        let declaration = "#[derive(thiserror::Error)] #[non_exhaustive] pub enum E { A }";
        let item_enum = first_enum(&format!("{attrs} {declaration}"));
        assert_eq!(
            is_violation(&item_enum, &BTreeSet::new()),
            !skipped,
            "enum: {predicate}"
        );
        assert_eq!(
            collect_from(&format!("{attrs} mod m {{ {declaration} }}")),
            usize::from(!skipped),
            "module: {predicate}"
        );
    }
}

#[test]
fn cfg_attr_is_not_expanded_and_derive_identity_is_not_resolved() {
    for source in [
        "#[cfg_attr(not(test), cfg(test))] #[derive(thiserror::Error)] #[non_exhaustive] pub enum E { A }",
        "#[derive(unrelated::Error)] #[non_exhaustive] pub enum E { A }",
    ] {
        assert!(flags(source));
    }
    for source in [
        "#[cfg_attr(feature = \"x\", non_exhaustive)] #[derive(thiserror::Error)] pub enum E { A }",
        "#[derive(AliasedError)] #[non_exhaustive] pub enum E { A }",
        "#[derive(Debug)] #[non_exhaustive] pub enum E { A }",
        "#[derive(thiserror::Error)] #[non_exhaustive] pub(crate) enum E { A }",
        "#[cfg(feature = \"x\")] #[cfg(test)] #[derive(thiserror::Error)] #[non_exhaustive] pub enum E { A }",
    ] {
        assert!(!flags(source));
    }
    assert_eq!(
        collect_from("#[cfg_attr(not(test), cfg(test))] mod m { pub enum E { A } }"),
        1
    );
}

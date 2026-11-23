use crate::{ReportScope, WalkError};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
/// Result of [`scan_doc_files`]: the documentation files found, plus
/// every traversal failure encountered. Callers must count
/// [`DocScan::errors`] towards the run error total; a scan that could
/// not read part of the tree has not established that the tree is clean.
///
/// Has no public constructor: a scan is evidence of a traversal this
/// crate performed, not a value a caller can assert.
#[derive(Debug)]
#[non_exhaustive]
pub struct DocScan {
    files: Vec<PathBuf>,
    errors: Vec<WalkError>,
}
impl DocScan {
    /// Documentation files the scan found.
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
    /// Traversal failures the scan could not resolve into an entry.
    #[must_use]
    pub fn errors(&self) -> &[WalkError] {
        &self.errors
    }
}
/// Walk `root` and report every file that looks like documentation,
/// together with every traversal failure.
///
/// Skips dotfiles/dotdirs, `target/`, and common vendor/build directories
/// (`node_modules`, `vendor`, `dist`, `build`) to avoid polyglot-repo
/// noise, whatever case those directories are spelled in.
/// Unreadable entries are reported in [`DocScan::errors`], never dropped.
#[must_use]
pub fn scan_doc_files(root: &Path) -> DocScan {
    let mut scan = DocScan {
        files: Vec::new(),
        errors: Vec::new(),
    };
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| prune::traversal(e, prune::Pruned::Directories).descends());
    for entry in walker {
        match entry {
            Err(e) => scan.errors.push(WalkError::rooted_at(root, &e)),
            Ok(entry) if entry.file_type().is_file() && is_doc_path(entry.path(), root) => {
                scan.files.push(entry.into_path());
            }
            Ok(_) => {}
        }
    }
    scan
}
mod prune;
const ALLOWED_ROOT_DIRS: &[&str] = &["benches", "crates", "examples", "src", "tests"];
const ALLOWED_ROOT_FILES: &[&str] = &["build.rs"];
const CARGO_MANIFEST: &str = "Cargo.toml";
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootScope {
    SourceTree,
    ProjectRoot,
}
fn is_named_source_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| ALLOWED_ROOT_DIRS.contains(&n))
}
fn source_dir_anchor(path: &Path) -> Result<ManifestAnchor, WalkError> {
    match path.parent() {
        Some(parent) if is_named_source_dir(path) => manifest_anchor(parent),
        _ => Ok(ManifestAnchor::Absent),
    }
}
const MANIFEST_NOT_A_FILE: &str =
    "Cargo.toml is not a regular file, so source-root anchoring cannot be decided";
#[derive(Debug, Clone, Copy)]
enum ManifestAnchor {
    Anchored,
    Absent,
}
fn manifest_anchor(dir: &Path) -> Result<ManifestAnchor, WalkError> {
    let manifest = dir.join(CARGO_MANIFEST);
    match std::fs::symlink_metadata(&manifest) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ManifestAnchor::Absent),
        Err(e) => Err(WalkError::at(&manifest, &e.to_string())),
        Ok(node) if node.is_file() => Ok(ManifestAnchor::Anchored),
        Ok(node) if node.is_symlink() => resolve_manifest_link(&manifest),
        Ok(_) => Err(WalkError::at(&manifest, MANIFEST_NOT_A_FILE)),
    }
}
fn resolve_manifest_link(manifest: &Path) -> Result<ManifestAnchor, WalkError> {
    match std::fs::metadata(manifest) {
        Ok(target) if target.is_file() => Ok(ManifestAnchor::Anchored),
        Ok(_) => Err(WalkError::at(manifest, MANIFEST_NOT_A_FILE)),
        Err(e) => Err(WalkError::at(manifest, &e.to_string())),
    }
}
fn classify_root(root: &Path) -> Result<RootScope, WalkError> {
    if is_named_source_dir(root) {
        return Ok(RootScope::SourceTree);
    }
    for ancestor in root.ancestors() {
        match source_dir_anchor(ancestor)? {
            ManifestAnchor::Anchored => return Ok(RootScope::SourceTree),
            ManifestAnchor::Absent => {}
        }
    }
    Ok(RootScope::ProjectRoot)
}
#[derive(Debug, Clone, Copy)]
enum ChildRoot {
    Present,
    Absent,
}
#[derive(Debug, Clone, Copy)]
enum RootNode {
    Dir,
    File,
}
impl RootNode {
    fn matches(self, node: &std::fs::Metadata) -> bool {
        match self {
            Self::Dir => node.is_dir(),
            Self::File => node.is_file(),
        }
    }
}
fn child_source_root(child: &Path, want: RootNode) -> Result<ChildRoot, WalkError> {
    match std::fs::symlink_metadata(child) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ChildRoot::Absent),
        Err(e) => Err(WalkError::at(child, &e.to_string())),
        Ok(node) if node.is_symlink() => resolve_child_root_link(child, want),
        Ok(node) if want.matches(&node) => Ok(ChildRoot::Present),
        Ok(_) => Ok(ChildRoot::Absent),
    }
}
fn resolve_child_root_link(child: &Path, want: RootNode) -> Result<ChildRoot, WalkError> {
    match std::fs::metadata(child) {
        Ok(target) if want.matches(&target) => Ok(ChildRoot::Present),
        Ok(_) => Ok(ChildRoot::Absent),
        Err(e) => Err(WalkError::at(child, &e.to_string())),
    }
}
fn resolve_walk_roots(root: &Path) -> Result<Vec<PathBuf>, WalkError> {
    Ok(match classify_root(root)? {
        RootScope::SourceTree => vec![root.to_path_buf()],
        RootScope::ProjectRoot => project_walk_roots(root)?,
    })
}
fn project_walk_roots(root: &Path) -> Result<Vec<PathBuf>, WalkError> {
    let mut bases = Vec::new();
    let dirs = ALLOWED_ROOT_DIRS
        .iter()
        .map(|d| (root.join(d), RootNode::Dir));
    let files = ALLOWED_ROOT_FILES
        .iter()
        .map(|f| (root.join(f), RootNode::File));
    for (child, want) in dirs.chain(files) {
        match child_source_root(&child, want)? {
            ChildRoot::Present => bases.push(child),
            ChildRoot::Absent => {}
        }
    }
    Ok(bases)
}
const SYMLINK_NOT_TRAVERSED: &str =
    "symbolic link is not followed, so what it points at cannot be reported clean";
fn is_rust_source(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("rs")
}
#[derive(Debug, Clone, Copy)]
enum LinkedEntry {
    HidesSource,
    Ignorable,
}
fn linked_entry(path: &Path) -> Result<LinkedEntry, WalkError> {
    match std::fs::metadata(path) {
        Err(e) => Err(WalkError::at(path, &e.to_string())),
        Ok(target) if target.is_dir() || is_rust_source(path) => Ok(LinkedEntry::HidesSource),
        Ok(_) => Ok(LinkedEntry::Ignorable),
    }
}
fn walked_link(path: &Path) -> Option<Result<PathBuf, WalkError>> {
    match linked_entry(path) {
        Ok(LinkedEntry::HidesSource) => Some(Err(WalkError::at(path, SYMLINK_NOT_TRAVERSED))),
        Ok(LinkedEntry::Ignorable) => None,
        Err(e) => Some(Err(e)),
    }
}
/// Iterate every `.rs` file under `root`, yielding traversal failures
/// rather than discarding them.
///
/// Traversal covers `.rs` under what cargo compiles as crate source:
/// `benches`, `crates`, `examples`, `src`, `tests`, and a root
/// `build.rs`. A `root` so named, or anchored by a sibling
/// `Cargo.toml`, is walked directly; otherwise its allowlisted children
/// are. An ancestor merely *spelled* `src` never widens traversal.
/// Anything unresolvable, and any unfollowed link that could hide
/// source, surfaces as a [`WalkError`], never as scope. Build output is
/// pruned case-insensitively.
pub fn walk_rs_files(root: &Path) -> impl Iterator<Item = Result<PathBuf, WalkError>> + use<'_> {
    walk_resolved_roots(resolve_walk_roots(root))
}

/// How a CLI directory root was selected; neither choice discovers ancestors.
#[derive(Debug, Clone, Copy)]
pub enum DirectorySelection {
    /// Omitted ROOT: recursively scan cwd.
    DefaultCwd,
    /// Supplied ROOT: recursively scan the selected directory.
    Explicit,
}

/// Iterate Rust sources in a CLI directory scope, preserving pruning and link policy.
/// All directory roots recurse, regardless of manifests; no ancestors are consulted.
/// Traversal failures and nested source-hiding links yield [`WalkError`].
pub fn walk_cli_directory(
    root: &Path,
    selection: DirectorySelection,
) -> impl Iterator<Item = Result<PathBuf, WalkError>> + use<'_> {
    plan_cli_directory(root, selection).1
}

/// Resolve a directory scan once, returning its selected policy and traversal.
/// Policy is always [`ReportScope::RecursiveDirectory`]. The iterator preserves
/// traversal failures as [`WalkError`]; manifests do not select CLI scope.
pub fn plan_cli_directory(
    root: &Path,
    _selection: DirectorySelection,
) -> (
    ReportScope,
    impl Iterator<Item = Result<PathBuf, WalkError>> + use<'_>,
) {
    (
        ReportScope::RecursiveDirectory,
        walk_resolved_roots(Ok(vec![root.to_path_buf()])),
    )
}

fn walk_resolved_roots(
    roots: Result<Vec<PathBuf>, WalkError>,
) -> impl Iterator<Item = Result<PathBuf, WalkError>> {
    let (bases, undecided) = match roots {
        Ok(bases) => (bases, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    undecided
        .into_iter()
        .map(Err)
        .chain(bases.into_iter().flat_map(|base| {
            WalkDir::new(&base)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| prune::traversal(e, prune::Pruned::NonFiles).descends())
                .filter_map(move |entry| match entry {
                    Err(e) => Some(Err(WalkError::rooted_at(&base, &e))),
                    Ok(e) if e.file_type().is_file() => {
                        let path = e.into_path();
                        is_rust_source(&path).then_some(Ok(path))
                    }
                    Ok(e) if e.depth() > 0 && e.file_type().is_symlink() => walked_link(e.path()),
                    Ok(_) => None,
                })
        }))
}
#[must_use]
fn is_doc_path(path: &Path, root: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        let ext_lc = ext.to_ascii_lowercase();
        if matches!(
            ext_lc.as_str(),
            "md" | "markdown" | "rst" | "adoc" | "asciidoc" | "txt"
        ) {
            return true;
        }
    }
    if let Ok(rel) = path.strip_prefix(root)
        && let Some(first) = rel.components().next()
        && let Some(s) = first.as_os_str().to_str()
    {
        let s_lc = s.to_ascii_lowercase();
        if s_lc == "docs" || s_lc == "doc" {
            return true;
        }
    }
    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
        let name_uc = name.to_ascii_uppercase();
        for known in BARE_DOC_STEMS {
            if name_uc == *known {
                return true;
            }
        }
    }
    false
}
const BARE_DOC_STEMS: &[&str] = &[
    "LICENSE",
    "LICENCE",
    "NOTICE",
    "COPYING",
    "README",
    "CHANGELOG",
    "AUTHORS",
    "CONTRIBUTORS",
];

#[cfg(test)]
mod walk_error_tests {
    use super::walk_rs_files;
    use std::error::Error as _;
    use std::path::Path;
    fn a_walk_error() -> crate::WalkError {
        walk_rs_files(Path::new("/comment-free-no-such-root/src"))
            .next()
            .expect("an unreadable root yields one traversal failure")
            .expect_err("the entry cannot be traversed")
    }
    #[test]
    fn the_error_chain_does_not_leak_the_walker_error_type() {
        let err = a_walk_error();
        let source = err.source().expect("a walk error carries its source");
        assert!(
            source.downcast_ref::<walkdir::Error>().is_none(),
            "walkdir::Error is reachable through WalkError::source"
        );
    }
    #[test]
    fn the_rendered_message_survives_the_seal() {
        let err = a_walk_error();
        assert!(!err.message().is_empty(), "missing error message");
        assert!(
            err.to_string().contains(&err.message()),
            "display drops the source text: {err}"
        );
    }
}
#[cfg(test)]
mod doc_path_tests {
    use super::is_doc_path;
    use std::path::Path;
    #[test]
    fn bare_doc_stem_requires_exact_match() {
        let root = Path::new("");
        assert!(is_doc_path(Path::new("README"), root));
        assert!(is_doc_path(Path::new("README.md"), root));
        assert!(is_doc_path(Path::new("LICENSE"), root));
        assert!(!is_doc_path(Path::new("READMEISH"), root));
        assert!(!is_doc_path(Path::new("READMEISH.rs"), root));
        assert!(!is_doc_path(Path::new("LICENSEABLE.rs"), root));
        assert!(!is_doc_path(Path::new("NOTICED.rs"), root));
    }
    #[test]
    fn docs_dir_matches_only_at_top_level_under_root() {
        let root = Path::new("/proj");
        assert!(is_doc_path(Path::new("/proj/docs/guide.rs"), root));
        assert!(is_doc_path(Path::new("/proj/doc/inner.rs"), root));
        assert!(!is_doc_path(Path::new("/proj/src/docs/mod.rs"), root));
        assert!(!is_doc_path(
            Path::new("/proj/crates/foo/doc/inner.rs"),
            root
        ));
        assert!(!is_doc_path(Path::new("/proj/src/doc/util.rs"), root));
    }
    #[test]
    fn bare_doc_stem_does_not_match_a_non_doc_extension() {
        let root = Path::new("");
        assert!(is_doc_path(Path::new("README"), root));
        assert!(is_doc_path(Path::new("LICENSE"), root));
        assert!(is_doc_path(Path::new("NOTICE"), root));
        assert!(!is_doc_path(Path::new("README.rs"), root));
        assert!(!is_doc_path(Path::new("LICENSE.rs"), root));
        assert!(!is_doc_path(Path::new("NOTICE.toml"), root));
        assert!(!is_doc_path(Path::new("COPYING.rs"), root));
        assert!(!is_doc_path(Path::new("CHANGELOG.rs"), root));
        assert!(is_doc_path(Path::new("README.md"), root));
        assert!(is_doc_path(Path::new("CHANGELOG.markdown"), root));
    }
}
#[cfg(test)]
mod walk_root_tests {
    use super::{ManifestAnchor, RootScope, classify_root, resolve_walk_roots};
    use std::path::{Path, PathBuf};
    fn roots(path: &Path) -> Vec<PathBuf> {
        resolve_walk_roots(path).expect("classification must be decidable")
    }
    #[test]
    fn supplied_root_named_as_a_source_dir_is_walked_directly() {
        assert_eq!(
            roots(Path::new("/comment-free-no-such-root/proj/src")),
            vec![PathBuf::from("/comment-free-no-such-root/proj/src")]
        );
        assert_eq!(
            roots(Path::new("/comment-free-no-such-root/proj/crates")),
            vec![PathBuf::from("/comment-free-no-such-root/proj/crates")]
        );
    }
    #[test]
    fn ancestor_named_as_a_source_dir_does_not_put_the_root_in_scope() {
        assert!(
            roots(Path::new("/comment-free-no-such-root/src/checkout")).is_empty(),
            "an ancestor named src must not make the supplied root a source root"
        );
        assert!(
            roots(Path::new("/comment-free-no-such-root/crates/checkout")).is_empty(),
            "an ancestor named crates must not make the supplied root a source root"
        );
    }
    #[test]
    fn manifest_anchored_ancestor_makes_the_supplied_subtree_a_source_root() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
        let module = repo.join("src/module");
        std::fs::create_dir_all(&module).unwrap();
        assert_eq!(
            roots(&module),
            vec![module.clone()],
            "a subtree under a manifest-anchored src must be walked directly"
        );
        assert!(matches!(classify_root(&module), Ok(RootScope::SourceTree)));
    }
    #[test]
    fn unanchored_ancestor_leaves_the_supplied_root_a_project_root() {
        let td = tempfile::tempdir().unwrap();
        let checkout = td.path().join("src/checkout");
        std::fs::create_dir_all(checkout.join("src")).unwrap();
        assert!(
            matches!(classify_root(&checkout), Ok(RootScope::ProjectRoot)),
            "an ancestor named src with no sibling Cargo.toml is ambient spelling, not intent"
        );
        assert_eq!(
            roots(&checkout),
            vec![checkout.join("src")],
            "an unanchored root must fall back to its own allowlisted children"
        );
    }
    #[test]
    fn an_absent_manifest_is_a_decided_non_anchor() {
        let td = tempfile::tempdir().unwrap();
        assert!(
            matches!(
                super::manifest_anchor(td.path()),
                Ok(ManifestAnchor::Absent)
            ),
            "nothing at the manifest path decides the question: unanchored"
        );
    }
    #[test]
    fn a_manifest_that_is_a_directory_leaves_anchoring_undecided() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir(td.path().join(super::CARGO_MANIFEST)).unwrap();
        let e = super::manifest_anchor(td.path())
            .expect_err("a non-file manifest node must not yield a verdict");
        assert!(
            e.path().ends_with(super::CARGO_MANIFEST) && e.message() == super::MANIFEST_NOT_A_FILE,
            "the error must name the undecidable manifest, got {e:?}"
        );
    }
    #[cfg(unix)]
    #[test]
    fn a_manifest_symlink_to_a_regular_file_anchors() {
        let td = tempfile::tempdir().unwrap();
        let real = td.path().join("real-manifest.toml");
        std::fs::write(&real, "[package]\nname = \"p\"\n").unwrap();
        std::os::unix::fs::symlink(&real, td.path().join(super::CARGO_MANIFEST)).unwrap();
        assert!(matches!(
            super::manifest_anchor(td.path()),
            Ok(ManifestAnchor::Anchored)
        ));
    }
    #[cfg(unix)]
    #[test]
    fn a_dangling_manifest_symlink_leaves_anchoring_undecided() {
        let td = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(
            td.path().join("gone.toml"),
            td.path().join(super::CARGO_MANIFEST),
        )
        .unwrap();
        assert!(
            super::manifest_anchor(td.path()).is_err(),
            "a deliberate link to a missing target is not the absence of a manifest"
        );
    }
    #[cfg(unix)]
    #[test]
    fn an_unresolvable_allowlisted_child_is_not_a_decided_absence() {
        let td = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("src", td.path().join("src")).unwrap();
        let e = resolve_walk_roots(td.path())
            .expect_err("a child that cannot be stat-ed must not be dropped as absent");
        assert!(
            e.path().ends_with("src"),
            "the error must name the undecidable child, got {e:?}"
        );
    }
    #[test]
    fn an_absent_allowlisted_child_is_a_decided_absence() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir(td.path().join("src")).unwrap();
        assert_eq!(
            roots(td.path()),
            vec![td.path().join("src")],
            "an absent crates/ is a decided absence, not an error"
        );
    }
    #[test]
    fn every_cargo_standard_source_dir_is_a_walk_root() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        for dir in ["benches", "crates", "examples", "src", "tests"] {
            std::fs::create_dir(repo.join(dir)).unwrap();
        }
        assert_eq!(
            roots(repo),
            vec![
                repo.join("benches"),
                repo.join("crates"),
                repo.join("examples"),
                repo.join("src"),
                repo.join("tests"),
            ],
            "cargo compiles benches/, examples/ and tests/ as crate source; a check that \
             skips them is narrower than the claim made for it"
        );
    }
    #[test]
    fn a_supplied_root_named_as_a_cargo_test_dir_is_walked_directly() {
        for dir in ["benches", "examples", "tests"] {
            let supplied = PathBuf::from("/comment-free-no-such-root/proj").join(dir);
            assert_eq!(
                roots(&supplied),
                vec![supplied.clone()],
                "a root named {dir} must be walked directly, as src is"
            );
        }
    }
    #[test]
    fn a_root_build_script_is_a_walk_root() {
        let td = tempfile::tempdir().unwrap();
        let repo = td.path();
        std::fs::create_dir(repo.join("src")).unwrap();
        std::fs::write(repo.join("build.rs"), "fn main() {}\n").unwrap();
        assert_eq!(
            roots(repo),
            vec![repo.join("src"), repo.join("build.rs")],
            "a root build script is crate source and sits in no allowlisted directory"
        );
    }
    #[test]
    fn an_absent_root_build_script_is_a_decided_absence() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir(td.path().join("src")).unwrap();
        assert_eq!(
            roots(td.path()),
            vec![td.path().join("src")],
            "no build.rs is a decided absence, not an error"
        );
    }
    #[test]
    fn a_root_build_script_that_is_a_directory_is_not_a_walk_root() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir(td.path().join("build.rs")).unwrap();
        assert!(
            roots(td.path()).is_empty(),
            "a directory spelled build.rs is not a build script"
        );
    }
    #[cfg(unix)]
    #[test]
    fn an_unresolvable_root_build_script_is_not_a_decided_absence() {
        let td = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("build.rs", td.path().join("build.rs")).unwrap();
        let e = resolve_walk_roots(td.path())
            .expect_err("a build script that cannot be stat-ed must not be dropped as absent");
        assert!(
            e.path().ends_with("build.rs"),
            "the error must name the undecidable build script, got {e:?}"
        );
    }
}
#[cfg(test)]
mod walk_symlink_tests {
    use super::walk_rs_files;
    use std::path::{Path, PathBuf};
    fn walk(root: &Path) -> (Vec<PathBuf>, Vec<String>) {
        let mut files = Vec::new();
        let mut errors = Vec::new();
        for entry in walk_rs_files(root) {
            match entry {
                Ok(p) => files.push(p),
                Err(e) => errors.push(e.path().display().to_string()),
            }
        }
        (files, errors)
    }
    #[cfg(unix)]
    #[test]
    fn a_symlinked_rust_file_is_surfaced_not_skipped() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(td.path().join("real.rs"), "fn a() {}\n").unwrap();
        std::os::unix::fs::symlink(td.path().join("real.rs"), src.join("linked.rs")).unwrap();
        let (files, errors) = walk(td.path());
        assert!(
            files.is_empty(),
            "a symlinked source file must not be handed to the rewrite path: {files:?}"
        );
        assert!(
            errors.iter().any(|p| p.ends_with("linked.rs")),
            "a symlinked source file must surface as a traversal failure, got {errors:?}"
        );
    }
    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_under_a_walk_root_is_surfaced_not_skipped() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("src");
        let real = td.path().join("elsewhere");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("f.rs"), "fn f() {}\n").unwrap();
        std::os::unix::fs::symlink(&real, src.join("linkdir")).unwrap();
        let (_, errors) = walk(td.path());
        assert!(
            errors.iter().any(|p| p.ends_with("linkdir")),
            "an undescended symlinked directory hides every file beneath it, got {errors:?}"
        );
    }
    #[cfg(unix)]
    #[test]
    fn a_symlinked_build_directory_is_pruned_not_reported() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("src");
        let real = td.path().join("elsewhere");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, src.join("target")).unwrap();
        std::os::unix::fs::symlink(&real, src.join(".hidden")).unwrap();
        let (_, errors) = walk(td.path());
        assert!(
            errors.is_empty(),
            "a link named as build output or a dotdir is pruned by name, got {errors:?}"
        );
    }
    #[test]
    fn a_regular_rust_file_is_still_yielded() {
        let td = tempfile::tempdir().unwrap();
        let src = td.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.rs"), "fn a() {}\n").unwrap();
        let (files, errors) = walk(td.path());
        assert!(
            errors.is_empty(),
            "no traversal failure expected: {errors:?}"
        );
        assert_eq!(files.len(), 1, "the regular source file must be walked");
    }
}

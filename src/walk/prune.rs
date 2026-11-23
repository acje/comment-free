use std::ffi::OsStr;
const BUILD_OUTPUT_DIRS: &[&str] = &["target", "node_modules", "vendor", "dist", "build"];
struct FoldedName(String);
impl FoldedName {
    fn of(name: &OsStr) -> Self {
        Self(name.to_string_lossy().to_ascii_lowercase())
    }
    fn is_hidden(&self) -> bool {
        self.0.starts_with('.')
    }
    fn is_build_output(&self) -> bool {
        BUILD_OUTPUT_DIRS.contains(&self.0.as_str())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Traversal {
    Prune,
    Descend,
}
impl Traversal {
    pub(super) const fn descends(self) -> bool {
        matches!(self, Self::Descend)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pruned {
    Directories,
    NonFiles,
}
impl Pruned {
    fn covers(self, kind: std::fs::FileType) -> bool {
        match self {
            Self::Directories => kind.is_dir(),
            Self::NonFiles => !kind.is_file(),
        }
    }
}
pub(super) fn traversal(entry: &walkdir::DirEntry, pruned: Pruned) -> Traversal {
    let name = FoldedName::of(entry.file_name());
    let prunable = entry.depth() > 0 && pruned.covers(entry.file_type());
    if prunable && (name.is_hidden() || name.is_build_output()) {
        Traversal::Prune
    } else {
        Traversal::Descend
    }
}
#[cfg(test)]
mod tests {
    use super::{BUILD_OUTPUT_DIRS, FoldedName};
    use std::ffi::OsStr;
    #[test]
    fn every_build_output_literal_is_already_folded() {
        for known in BUILD_OUTPUT_DIRS {
            assert_eq!(
                *known,
                known.to_ascii_lowercase(),
                "a build-output literal that is not already folded can never match a folded name"
            );
        }
    }
    #[test]
    fn a_build_output_name_is_recognised_in_every_case_variant() {
        for spelling in ["target", "Target", "TARGET", "tArGeT"] {
            assert!(
                FoldedName::of(OsStr::new(spelling)).is_build_output(),
                "{spelling} must be recognised as build output"
            );
        }
    }
    #[test]
    fn folding_does_not_widen_the_build_output_set() {
        for spelling in ["targets", "src", "targe", "buildx", "distinct"] {
            assert!(
                !FoldedName::of(OsStr::new(spelling)).is_build_output(),
                "{spelling} must not be treated as build output"
            );
        }
    }
}

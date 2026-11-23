use crate::FileError;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
struct TempFileGuard {
    path: PathBuf,
    armed: bool,
}
impl TempFileGuard {
    const fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.armed {
            drop(fs::remove_file(&self.path));
        }
    }
}
pub(crate) fn write_atomically(
    path: &Path,
    expected: &str,
    rewritten: &str,
) -> Result<(), FileError> {
    use std::io::Write as _;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(FileError::SymlinkDestination);
    }
    let permissions = fs::metadata(path)?.permissions();
    let (mut file, mut guard) = create_sibling_temp(path, dir)?;
    file.write_all(rewritten.as_bytes())?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::set_permissions(&guard.path, permissions)?;
    if !destination_still_holds(path, expected)? {
        return Err(FileError::Conflict);
    }
    fs::rename(&guard.path, path)?;
    guard.disarm();
    Ok(())
}
fn destination_still_holds(path: &Path, expected: &str) -> Result<bool, FileError> {
    Ok(fs::read_to_string(path)? == expected)
}
fn create_sibling_temp(path: &Path, dir: &Path) -> Result<(fs::File, TempFileGuard), FileError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stem = path.file_name().map_or_else(
        || String::from("source"),
        |n| n.to_string_lossy().into_owned(),
    );
    let pid = std::process::id();
    for _ in 0..TEMP_NAME_ATTEMPTS {
        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = dir.join(format!(".{stem}.{pid}.{nonce}.comment-free-tmp"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                let guard = TempFileGuard {
                    path: candidate,
                    armed: true,
                };
                return Ok((file, guard));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(FileError::Write(e)),
        }
    }
    Err(FileError::Write(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "exhausted temporary file name attempts",
    )))
}
const TEMP_NAME_ATTEMPTS: u32 = 1024;
#[cfg(test)]
mod atomic_write_tests {
    use super::{create_sibling_temp, write_atomically};
    use crate::{
        FileError, FileOutcome, RewriteMode, RunErrorKind, process_file, run_error_record,
    };
    use std::fs;
    const fn write_opts() -> RewriteMode {
        RewriteMode::Write
    }
    fn dir_entry_names(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
    #[test]
    fn every_file_error_class_maps_to_its_run_error_kind() {
        let io = || std::io::Error::other("boom");
        let parse = syn::parse_file("fn f( {").unwrap_err();
        assert_eq!(FileError::Parse(parse).kind(), RunErrorKind::Parse);
        assert_eq!(FileError::Read(io()).kind(), RunErrorKind::Io);
        assert_eq!(FileError::Write(io()).kind(), RunErrorKind::Io);
        assert_eq!(FileError::SymlinkDestination.kind(), RunErrorKind::Io);
        assert_eq!(FileError::Conflict.kind(), RunErrorKind::Conflict);
    }
    #[test]
    fn an_unreadable_file_is_a_read_failure_not_a_parse_failure() {
        let td = tempfile::tempdir().unwrap();
        let missing = td.path().join("nope.rs");
        match process_file(&missing, write_opts()) {
            FileOutcome::Failed(e @ FileError::Read(_)) => {
                assert_eq!(e.kind(), RunErrorKind::Io);
            }
            other @ (FileOutcome::Rewritten(_)
            | FileOutcome::WouldRewrite(_)
            | FileOutcome::Unchanged(_)
            | FileOutcome::Failed(_)) => panic!("expected Failed(Read), got {other:?}"),
        }
    }
    #[test]
    fn a_conflict_reports_itself_as_a_conflict_run_error() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "concurrent editor won\n").unwrap();
        let err = write_atomically(&path, "bytes we read earlier\n", "our rewrite\n").unwrap_err();
        assert_eq!(err.kind(), RunErrorKind::Conflict);
        assert_eq!(err.to_string(), "destination changed since it was read");
        let record = run_error_record(err.kind(), &path, &err.to_string());
        assert!(record.contains("\"kind\":\"conflict\""), "{record}");
        assert!(
            record.contains("\"message\":\"destination changed since it was read\""),
            "{record}"
        );
    }
    #[test]
    fn destination_changed_since_read_is_a_conflict_and_leaves_bytes_untouched() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "concurrent editor won\n").unwrap();
        let err = write_atomically(&path, "bytes we read earlier\n", "our rewrite\n").unwrap_err();
        assert!(matches!(err, FileError::Conflict), "got {err:?}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "concurrent editor won\n"
        );
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
    #[test]
    fn a_conflicted_destination_is_still_rewritable_afterwards() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "// drop me\nfn f() {}\n").unwrap();
        assert!(matches!(
            write_atomically(&path, "stale bytes\n", "never lands\n"),
            Err(FileError::Conflict)
        ));
        match process_file(&path, write_opts()) {
            FileOutcome::Rewritten(_) => {}
            other @ (FileOutcome::WouldRewrite(_)
            | FileOutcome::Unchanged(_)
            | FileOutcome::Failed(_)) => {
                panic!("expected Rewritten after a conflict, got {other:?}")
            }
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn f() {}\n");
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
    #[test]
    fn successful_write_leaves_no_temp_files_behind() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "// drop me\nfn f() {}\n").unwrap();
        match process_file(&path, write_opts()) {
            FileOutcome::Rewritten(_) => {}
            other @ (FileOutcome::WouldRewrite(_)
            | FileOutcome::Unchanged(_)
            | FileOutcome::Failed(_)) => panic!("expected Rewritten, got {other:?}"),
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "fn f() {}\n");
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
    #[cfg(unix)]
    #[test]
    fn limitation_contract_hard_link_alias_keeps_original_contents() {
        use std::os::unix::fs::MetadataExt as _;
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        let alias = td.path().join("alias.rs");
        let original = "// drop me\nfn f() {}\n";
        fs::write(&path, original).unwrap();
        fs::hard_link(&path, &alias).unwrap();
        let original_inode = fs::metadata(&path).unwrap().ino();
        assert_eq!(fs::metadata(&alias).unwrap().ino(), original_inode);

        assert!(matches!(
            process_file(&path, write_opts()),
            FileOutcome::Rewritten(_)
        ));

        assert_eq!(fs::read_to_string(&path).unwrap(), "fn f() {}\n");
        assert_eq!(fs::read_to_string(&alias).unwrap(), original);
        assert_eq!(fs::metadata(&alias).unwrap().ino(), original_inode);
        assert_ne!(fs::metadata(&path).unwrap().ino(), original_inode);
    }
    #[cfg(unix)]
    #[test]
    fn rewrite_preserves_the_destination_file_mode() {
        use std::os::unix::fs::PermissionsExt as _;
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "// drop me\nfn f() {}\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        match process_file(&path, write_opts()) {
            FileOutcome::Rewritten(_) => {}
            other @ (FileOutcome::WouldRewrite(_)
            | FileOutcome::Unchanged(_)
            | FileOutcome::Failed(_)) => panic!("expected Rewritten, got {other:?}"),
        }
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "mode was {mode:o}");
    }
    #[cfg(unix)]
    #[test]
    fn a_symlink_destination_is_refused_and_its_target_is_left_alone() {
        let td = tempfile::tempdir().unwrap();
        let target = td.path().join("real.rs");
        let link = td.path().join("link.rs");
        fs::write(&target, "// drop me\nfn f() {}\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        match process_file(&link, write_opts()) {
            FileOutcome::Failed(e @ FileError::SymlinkDestination) => {
                assert_eq!(e.kind(), RunErrorKind::Io);
                assert!(e.to_string().contains("symbolic link"), "got {e}");
            }
            other @ (FileOutcome::Rewritten(_)
            | FileOutcome::WouldRewrite(_)
            | FileOutcome::Unchanged(_)
            | FileOutcome::Failed(_)) => {
                panic!("expected Failed(SymlinkDestination), got {other:?}")
            }
        }
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "// drop me\nfn f() {}\n"
        );
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            dir_entry_names(td.path()),
            vec!["link.rs".to_string(), "real.rs".to_string()]
        );
    }
    #[test]
    fn a_panic_after_temp_creation_unwinds_through_the_guard_and_removes_the_temp() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::write(&path, "fn f() {}\n").unwrap();
        let dir = td.path().to_path_buf();
        let panicked = std::panic::catch_unwind(|| {
            let (_file, _guard) = create_sibling_temp(&path, &dir).unwrap();
            assert_eq!(dir_entry_names(&dir).len(), 2, "temp should exist here");
            panic!("simulated failure after the temp file exists");
        });
        assert!(panicked.is_err(), "expected the closure to unwind");
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
    #[test]
    fn an_io_failure_after_temp_creation_leaves_no_residue() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("a.rs");
        fs::create_dir(&path).unwrap();
        let err = write_atomically(&path, "bytes we read earlier\n", "our rewrite\n").unwrap_err();
        assert!(matches!(err, FileError::Write(_)), "got {err:?}");
        assert_eq!(dir_entry_names(td.path()), vec!["a.rs".to_string()]);
    }
}

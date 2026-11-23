mod common;

use common::*;

#[test]
fn explicit_rust_file_is_the_entire_scope_in_every_mode() {
    let td = tempfile::tempdir().unwrap();
    let selected = td.path().join("selected.rs");
    let sibling = td.path().join("sibling.rs");
    let original = "// removable\n/// one two three four five six\npub fn selected() {}\n";
    fs::write(&selected, original).unwrap();
    fs::write(&sibling, original).unwrap();
    fs::write(td.path().join("README.md"), "untouched").unwrap();
    for (args, expected) in [
        (vec!["--doc-max-words", "5"], 4),
        (vec!["--rewrite", "--dry-run"], 3),
        (vec!["--rewrite"], 0),
    ] {
        let out = Command::new(bin())
            .args(args)
            .arg(&selected)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(expected), "{out:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("doc_file_warning"), "{stderr}");
        assert_eq!(fs::read_to_string(&sibling).unwrap(), original);
        if expected != 0 {
            assert_eq!(fs::read_to_string(&selected).unwrap(), original);
        }
    }
    assert_eq!(
        fs::read_to_string(selected).unwrap(),
        original.replace("// removable\n", "")
    );
}

#[cfg(unix)]
#[test]
fn explicit_leaf_symlinks_are_usage_errors_in_every_mode() {
    let td = tempfile::tempdir().unwrap();
    let target = td.path().join("target.rs");
    let link = td.path().join("linked.rs");
    for content in ["fn clean() {}\n", "// removable\nfn dirty() {}\n"] {
        fs::write(&target, content).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        for args in [vec![], vec!["--rewrite", "--dry-run"], vec!["--rewrite"]] {
            let out = Command::new(bin()).args(args).arg(&link).output().unwrap();
            assert_eq!(out.status.code(), Some(2), "{out:?}");
            assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
            assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
            assert_eq!(fs::read_to_string(&target).unwrap(), content);
        }
        fs::remove_file(&link).unwrap();
    }
    std::os::unix::fs::symlink(td.path().join("missing.rs"), &link).unwrap();
    for args in [vec![], vec!["--rewrite", "--dry-run"], vec!["--rewrite"]] {
        let out = Command::new(bin()).args(args).arg(&link).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{out:?}");
    }
}

#[test]
fn explicit_non_rust_and_missing_files_are_usage_errors() {
    let td = tempfile::tempdir().unwrap();
    for name in ["input.txt", "input.RS", "missing.rs"] {
        let path = td.path().join(name);
        if name != "missing.rs" {
            fs::write(&path, "fn untouched() {}\n").unwrap();
        }
        for args in [vec![], vec!["--rewrite", "--dry-run"], vec!["--rewrite"]] {
            let out = Command::new(bin()).args(args).arg(&path).output().unwrap();
            assert_eq!(out.status.code(), Some(2), "{out:?}");
            assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
        }
    }
}

#[cfg(unix)]
#[test]
fn explicit_source_directory_link_retains_traversal() {
    let td = tempfile::tempdir().unwrap();
    let real = td.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("lib.rs"), "// removable\nfn item() {}\n").unwrap();
    let link = td.path().join("src");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let out = run_dry(&link);
    assert_pending_changes(&out, "explicit directory-root links retain traversal");
    assert_eq!(
        fs::read_to_string(real.join("lib.rs")).unwrap(),
        "// removable\nfn item() {}\n"
    );
}

#[test]
fn explicit_and_default_directories_recurse_without_manifest_dependency() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("custom");
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("nested/item.rs"), "// removable\nfn item() {}\n").unwrap();
    let explicit = run_dry(&root);
    assert_pending_changes(&explicit, "explicit arbitrary directory must recurse");
    let default = Command::new(bin())
        .args(["--rewrite", "--dry-run"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_pending_changes(&default, "default cwd must recurse");
    assert_eq!(
        one_record(&String::from_utf8_lossy(&default.stderr), "strip_summary").number("rewritten"),
        1
    );
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    assert_pending_changes(&run_dry(&root), "manifest must not narrow recursion");
}

#[test]
fn explicit_manifest_root_named_src_includes_root_sources() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path().join("src");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::write(root.join("stray.rs"), "// removable\nfn stray() {}\n").unwrap();
    fs::write(root.join("src/lib.rs"), "// removable\nfn item() {}\n").unwrap();
    let out = run_dry(&root);
    assert_pending_changes(&out, "manifest root must scan src/lib.rs");
    assert_eq!(
        rewritten_paths(&String::from_utf8_lossy(&out.stdout)).len(),
        2
    );
}

#[test]
fn arbitrary_directory_scope_is_shared_by_lint_and_write_without_ancestor_discovery() {
    let td = tempfile::tempdir().unwrap();
    fs::create_dir(td.path().join("Cargo.toml")).unwrap();
    let root = td.path().join("custom");
    fs::create_dir_all(root.join("nested")).unwrap();
    let selected = root.join("nested/item.rs");
    let original = "// removable\n/// one two three four five six\npub fn item() {}\n";
    fs::write(&selected, original).unwrap();
    let lint = run_lint_budget(&root, 5);
    assert_eq!(lint.status.code(), Some(4), "{lint:?}");
    assert_eq!(fs::read_to_string(&selected).unwrap(), original);
    let rewritten = run(&root);
    assert_eq!(rewritten.status.code(), Some(0), "{rewritten:?}");
    assert_eq!(
        fs::read_to_string(selected).unwrap(),
        original.replace("// removable\n", "")
    );
}

#[test]
fn recursive_scope_includes_custom_sources_and_prunes_hidden_and_build_directories() {
    for manifest in [false, true] {
        for explicit in [false, true] {
            let td = tempfile::tempdir().unwrap();
            let root = td.path().join("checkout");
            fs::create_dir(&root).unwrap();
            let original = "// removable\n/// one two three four five six\npub fn item() {}\n";
            fs::write(td.path().join("outside.rs"), "fn invalid( {").unwrap();
            for name in ["main.rs", "tools/task.rs", "scripts/check.rs", "src/lib.rs"] {
                let file = root.join(name);
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(file, original).unwrap();
            }
            for name in [
                ".git",
                ".beads",
                "target",
                "BUILD",
                "vendor",
                "dist",
                "node_modules",
            ] {
                fs::create_dir(root.join(name)).unwrap();
                fs::write(root.join(name).join("ignored.rs"), "fn invalid( {").unwrap();
            }
            if manifest {
                fs::write(root.join("Cargo.toml"), "not a valid manifest").unwrap();
            }
            for (args, expected) in [
                (vec!["--doc-max-words", "5", "--max-warning-files", "0"], 4),
                (vec!["--rewrite", "--dry-run"], 3),
                (vec!["--rewrite"], 0),
            ] {
                let mut command = Command::new(bin());
                command.current_dir(&root).args(args);
                if explicit {
                    command.arg(".");
                }
                let out = command.output().unwrap();
                assert_eq!(
                    out.status.code(),
                    Some(expected),
                    "{manifest}/{explicit}: {out:?}"
                );
                let stderr = String::from_utf8_lossy(&out.stderr);
                if expected == 4 {
                    let summary = one_record(&stderr, "lint_summary");
                    assert_eq!(summary.text("scope"), "recursive-directory");
                    assert_eq!(summary.number("files"), 4);
                    assert_eq!(summary.number("findings_hidden"), 4);
                    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
                } else {
                    assert_eq!(one_record(&stderr, "strip_summary").number("rewritten"), 4);
                }
                for name in ["main.rs", "tools/task.rs", "scripts/check.rs", "src/lib.rs"] {
                    let wanted = if expected == 0 {
                        original.replace("// removable\n", "")
                    } else {
                        original.to_owned()
                    };
                    assert_eq!(fs::read_to_string(root.join(name)).unwrap(), wanted);
                }
            }
        }
    }
}

#[test]
fn doc_warn_emits_when_root_is_dot() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("README.md"), "hi\n").expect("write README");
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(".")
        .current_dir(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        one_record(&stderr, "doc_file_warning")
            .text("path")
            .ends_with("README.md"),
        "doc_file_warning missing when ROOT='.':\n{stderr}"
    );
}

#[test]
fn scan_doc_files_skips_vendor_dirs() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("README.md"), "root\n").expect("write README");
    for sub in ["node_modules", "vendor", "dist", "build", "target"] {
        std::fs::create_dir_all(td.path().join(sub)).expect("mkdir");
        std::fs::write(td.path().join(sub).join("README.md"), "nested\n").expect("write");
    }
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(".")
        .current_dir(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let warned = records_named(&stderr, "doc_file_warning");
    assert_eq!(
        warned.len(),
        1,
        "expected exactly 1 doc_file_warning (root README.md):\n{stderr}"
    );
    for sub in ["node_modules", "vendor", "dist", "build", "target"] {
        assert!(
            !warned[0].text("path").contains(sub),
            "doc_file_warning unexpectedly reported file under {sub}/:\n{stderr}"
        );
    }
}

#[test]
fn build_output_dirs_are_pruned_in_every_case_variant() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("Cargo.toml"), "[package]\nname = \"p\"\n").expect("write manifest");
    std::fs::create_dir_all(td.path().join("src")).expect("mkdir src");
    write(td.path().join("src").as_path(), "lib.rs", "fn f() {}\n");
    for variant in [
        "Target",
        "TARGET",
        "Node_Modules",
        "Dist",
        "BUILD",
        "Vendor",
    ] {
        let dir = td.path().join("src").join(variant);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("gen.rs"), "// generated\nfn g() {}\n").expect("write");
    }
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(".")
        .current_dir(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        records_named(&stdout, "rewrite_file").is_empty(),
        "a build-output directory spelled in a different case was traversed and rewritten:\n{stdout}"
    );
    assert_eq!(
        one_record(&stderr, "strip_summary").number("rewritten"),
        0,
        "expected nothing rewritten:\n{stderr}"
    );
}

#[test]
fn scan_doc_files_skips_vendor_dirs_in_every_case_variant() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("README.md"), "root\n").expect("write README");
    for sub in ["Node_Modules", "Vendor", "Dist", "BUILD", "Target"] {
        std::fs::create_dir_all(td.path().join(sub)).expect("mkdir");
        std::fs::write(td.path().join(sub).join("GUIDE.md"), "nested\n").expect("write");
    }
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(".")
        .current_dir(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let warned = records_named(&stderr, "doc_file_warning");
    assert_eq!(
        warned.len(),
        1,
        "expected exactly 1 doc_file_warning (root README.md):\n{stderr}"
    );
    assert!(
        !warned[0].text("path").contains("GUIDE"),
        "doc scan descended into a build directory spelled in a different case:\n{stderr}"
    );
}

#[test]
fn a_rust_file_named_in_a_different_case_is_not_rewritten() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("Cargo.toml"), "[package]\nname = \"p\"\n").expect("write manifest");
    std::fs::create_dir_all(td.path().join("src")).expect("mkdir src");
    let src = td.path().join("src");
    std::fs::write(src.join("lib.rs"), "fn f() {}\n").expect("write");
    std::fs::write(src.join("Notes.RS"), "// not compiled\nfn g() {}\n").expect("write");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(".")
        .current_dir(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        records_named(&stdout, "rewrite_file").is_empty(),
        "a file whose extension is not exactly `rs` was rewritten:\n{stdout}"
    );
    assert_eq!(
        one_record(&stderr, "strip_summary").number("rewritten"),
        0,
        "expected nothing rewritten:\n{stderr}"
    );
}

#[test]
fn dry_run_processes_all_rust_directories_but_skips_target() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("src/lib.rs"), "// removable\nfn s() {}\n").expect("write src");
    fs::create_dir_all(root.join("crates/foo/src")).expect("mkdir crates/foo/src");
    fs::write(
        root.join("crates/foo/src/lib.rs"),
        "// removable\nfn c() {}\n",
    )
    .expect("write crates");
    fs::create_dir_all(root.join("target/package/foo-0.1.0/src")).expect("mkdir target subtree");
    fs::write(
        root.join("target/package/foo-0.1.0/src/lib.rs"),
        "// removable\nfn t() {}\n",
    )
    .expect("write target");
    fs::create_dir_all(root.join("docs")).expect("mkdir docs");
    fs::write(root.join("docs/example.rs"), "// removable\nfn d() {}\n").expect("write docs");
    fs::create_dir_all(root.join("scripts")).expect("mkdir scripts");
    fs::write(root.join("scripts/helper.rs"), "// removable\nfn h() {}\n").expect("write scripts");
    let out = run_dry(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_pending_changes(&out, "crates and src both hold removable comments");
    let paths = rewritten_paths(&stdout);
    assert!(
        paths.iter().any(|p| p.ends_with("src/lib.rs")),
        "expected a src/lib.rs rewrite_file record:\n{stdout}"
    );
    assert!(
        paths.iter().any(|p| p.contains("crates/foo/src/lib.rs")),
        "expected a crates/foo/src/lib.rs rewrite_file record:\n{stdout}"
    );
    for included in ["docs/example.rs", "scripts/helper.rs"] {
        assert!(paths.iter().any(|p| p.ends_with(included)), "{stdout}");
    }
    assert!(!stdout.contains("target/package"));
}

#[test]
fn lint_processes_docs_rust_sources_but_skips_target() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(
        root.join("src/lib.rs"),
        "/// one two three four five six\npub fn s() {}\n",
    )
    .expect("write src");
    fs::create_dir_all(root.join("target/package/foo-0.1.0/src")).expect("mkdir target subtree");
    fs::write(
        root.join("target/package/foo-0.1.0/src/lib.rs"),
        "/// one two three four five six\npub fn t() {}\n",
    )
    .expect("write target");
    fs::create_dir_all(root.join("docs")).expect("mkdir docs");
    fs::write(
        root.join("docs/example.rs"),
        "/// one two three four five six\npub fn d() {}\n",
    )
    .expect("write docs");
    let out = run_lint_budget(root, 5);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "expected exit 4 (two findings, one shown); stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let finding = one_record(&stdout, "doc_lint_finding");
    assert!(
        finding.text("path").ends_with("docs/example.rs"),
        "expected first finding in native path order:\n{stdout}"
    );
    assert!(!stdout.contains("target/package"));
    let summary = one_record(&stderr, "lint_summary");
    assert_eq!(summary.number("findings"), 2);
    assert_eq!(summary.number("findings_hidden"), 1);
}

#[test]
fn non_rust_files_under_allowed_roots_are_ignored() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("src/lib.rs"), "// removable\nfn s() {}\n").expect("write rs");
    fs::write(root.join("src/script.py"), "# python\nprint('hi')\n").expect("write py");
    fs::write(root.join("src/notes.md"), "# notes\n").expect("write md");
    let out = run_dry(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_pending_changes(&out, "src/lib.rs holds a removable comment");
    assert!(
        stdout.contains("src/lib.rs"),
        "expected src/lib.rs in dry-run output:\n{stdout}"
    );
    for forbidden in ["script.py", "notes.md"] {
        assert!(
            !stdout.contains(forbidden),
            "non-.rs file `{forbidden}` must not be processed:\n{stdout}"
        );
    }
}

#[test]
fn root_without_conventional_source_directories_still_processes_rust() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir_all(root.join("docs")).expect("mkdir docs");
    fs::write(root.join("docs/example.rs"), "// removable\nfn d() {}\n").expect("write docs");
    fs::write(root.join("stray.rs"), "// removable\nfn x() {}\n").expect("write stray");
    let out = run_dry(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_pending_changes(&out, "root and docs Rust sources are in scope");
    assert_eq!(rewritten_paths(&stdout).len(), 2);
    let summary = one_record(&stderr, "strip_summary");
    assert_eq!(
        (
            summary.number("rewritten"),
            summary.number("unchanged"),
            summary.number("errors")
        ),
        (2, 0, 0),
        "summary should reflect recursive scope:\n{stderr}"
    );
}

#[test]
fn every_cargo_standard_source_location_is_processed() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    for dir in ["benches", "examples", "src", "tests"] {
        fs::create_dir_all(root.join(dir)).expect("mkdir source dir");
        fs::write(root.join(dir).join("unit.rs"), "// removable\nfn u() {}\n")
            .expect("write source");
    }
    fs::write(root.join("build.rs"), "// removable\nfn main() {}\n").expect("write build script");
    let out = run_dry(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let rewritten = rewritten_paths(&stdout);
    for expected in [
        "benches/unit.rs",
        "examples/unit.rs",
        "src/unit.rs",
        "tests/unit.rs",
        "build.rs",
    ] {
        assert!(
            rewritten.iter().any(|p| p.ends_with(expected)),
            "cargo compiles {expected} as crate source, so a self-check that skips it is \
             narrower than the claim made for it:\n{stdout}"
        );
    }
    let summary = one_record(&stderr, "strip_summary");
    assert_eq!(
        summary.number("rewritten"),
        5,
        "every standard source location must be reported:\n{stderr}"
    );
}

#[test]
fn root_inside_crates_subtree_is_processed_directly() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    let scoped = root.join("crates/foo");
    fs::create_dir_all(scoped.join("src")).expect("mkdir crates/foo/src");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/*\"]\n",
    )
    .expect("write workspace manifest");
    fs::write(scoped.join("Cargo.toml"), "[package]\nname = \"foo\"\n")
        .expect("write crate manifest");
    fs::write(scoped.join("src/lib.rs"), "// removable\nfn c() {}\n").expect("write");
    fs::write(scoped.join("build.rs"), "// removable\nfn main() {}\n").expect("write build");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(&scoped)
        .output()
        .expect("failed to spawn comment-free");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_pending_changes(&out, "the supplied subtree holds a removable comment");
    let paths = rewritten_paths(&stdout);
    assert!(
        paths.iter().any(|p| p.ends_with("src/lib.rs")),
        "expected a src/lib.rs rewrite_file record when ROOT is inside crates/:\n{stdout}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("build.rs")),
        "a supplied crate root must be walked directly, including root-level build.rs:\n{stdout}"
    );
}

#[test]
fn supplied_subtree_below_src_is_processed_while_ambient_ancestry_is_not() {
    let td = tempfile::tempdir().unwrap();
    let checkout = td.path().join("src/checkout");
    let repo = checkout.join("repo");
    fs::create_dir_all(repo.join("src/module")).expect("mkdir repo/src/module");
    fs::write(repo.join("Cargo.toml"), "[package]\nname = \"p\"\n").expect("write manifest");
    fs::write(repo.join("src/module/mod.rs"), "// removable\nfn m() {}\n").expect("write mod");
    fs::write(checkout.join("stray.rs"), "// removable\nfn s() {}\n").expect("write stray");

    let supplied = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(repo.join("src/module"))
        .output()
        .expect("failed to spawn comment-free");
    let supplied_out = String::from_utf8_lossy(&supplied.stdout);
    assert_pending_changes(&supplied, "the supplied module holds a removable comment");
    assert!(
        rewritten_paths(&supplied_out)
            .iter()
            .any(|p| p.ends_with("module/mod.rs")),
        "an intentionally supplied subtree under a manifest-anchored src must be walked directly:\n{supplied_out}"
    );

    let ambient = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .current_dir(&checkout)
        .output()
        .expect("failed to spawn comment-free");
    let ambient_out = String::from_utf8_lossy(&ambient.stdout);
    assert_pending_changes(&ambient, "cwd includes all descendants");
    assert!(
        rewritten_paths(&ambient_out)
            .iter()
            .any(|p| p.ends_with("stray.rs")),
        "cwd root sources must be included without a manifest:\n{ambient_out}"
    );
}

#[test]
fn ancestor_named_src_does_not_widen_the_supplied_root() {
    let td = tempfile::tempdir().unwrap();
    let scoped = td.path().join("src/checkout");
    fs::create_dir_all(scoped.join("src")).expect("mkdir src/checkout/src");
    fs::write(scoped.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::write(scoped.join("src/lib.rs"), "// removable\nfn c() {}\n").expect("write lib");
    fs::write(scoped.join("stray.rs"), "// removable\nfn s() {}\n").expect("write stray");
    fs::write(
        td.path().join("src/outside.rs"),
        "// removable\nfn outside() {}\n",
    )
    .unwrap();
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(&scoped)
        .output()
        .expect("failed to spawn comment-free");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_pending_changes(
        &out,
        "the supplied root's own src holds a removable comment",
    );
    let paths = rewritten_paths(&stdout);
    assert!(
        paths.iter().any(|p| p.ends_with("src/lib.rs")),
        "expected the supplied root's own src/lib.rs:\n{stdout}"
    );
    assert!(
        paths.iter().any(|p| p.ends_with("stray.rs")),
        "root sources must be included:\n{stdout}"
    );
    assert!(!paths.iter().any(|p| p.ends_with("outside.rs")));
}

#[test]
fn absent_manifest_does_not_hide_an_explicit_subtree() {
    let td = tempfile::tempdir().unwrap();
    let repo = manifest_policy_fixture(td.path());
    let out = run_dry(&repo.join("src/module"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(3),
        "an explicit subtree needs no ancestor manifest:\n{stderr}"
    );
    assert!(
        !rewritten_paths(&stdout).is_empty(),
        "an explicit subtree must be walked directly:\n{stdout}"
    );
    assert_eq!(
        one_record(&stderr, "strip_summary").number("errors"),
        0,
        "an absent manifest must not be reported as a walk error:\n{stderr}"
    );
}

#[test]
fn manifest_that_is_a_directory_does_not_select_cli_scope() {
    let td = tempfile::tempdir().unwrap();
    let repo = manifest_policy_fixture(td.path());
    fs::create_dir(repo.join("Cargo.toml")).expect("mkdir Cargo.toml");
    let out = run_dry(&repo);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(3),
        "manifest type must not select CLI scope:\n{stderr}"
    );
    assert!(
        records_named(&stderr, "run_error").is_empty(),
        "manifest directory is an ordinary directory:\n{stderr}"
    );
    assert!(
        !rewritten_paths(&stdout).is_empty(),
        "manifest type must not hide source:\n{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn manifest_symlink_to_an_inaccessible_target_is_an_error_not_a_clean_run() {
    let td = tempfile::tempdir().unwrap();
    let repo = manifest_policy_fixture(td.path());
    let vault = td.path().join("vault");
    fs::create_dir(&vault).expect("mkdir vault");
    fs::write(vault.join("Cargo.toml"), "[package]\nname = \"p\"\n").expect("write manifest");
    std::os::unix::fs::symlink(vault.join("Cargo.toml"), repo.join("Cargo.toml"))
        .expect("symlink manifest");
    make_unreadable(&vault);
    let out = run_dry(&repo);
    make_readable(&vault);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "an unresolvable manifest symlink is indeterminate, never a clean run:\n{stderr}"
    );
    assert!(
        one_error(&stderr, "walk")
            .text("path")
            .contains("Cargo.toml"),
        "expected a walk run_error naming the unresolvable manifest:\n{stderr}"
    );
    assert!(
        !rewritten_paths(&stdout).is_empty(),
        "a failing nested link must not hide independent source:\n{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn manifest_symlink_to_a_readable_file_anchors_the_supplied_subtree() {
    let td = tempfile::tempdir().unwrap();
    let repo = manifest_policy_fixture(td.path());
    fs::write(repo.join("real-manifest.toml"), "[package]\nname = \"p\"\n").expect("write real");
    std::os::unix::fs::symlink(repo.join("real-manifest.toml"), repo.join("Cargo.toml"))
        .expect("symlink manifest");
    let out = run_dry(&repo.join("src/module"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_pending_changes(
        &out,
        "a manifest symlink resolving to a regular file is a decided anchor",
    );
    assert!(
        rewritten_paths(&stdout)
            .iter()
            .any(|p| p.ends_with("module/mod.rs")),
        "a symlinked manifest must anchor the supplied subtree:\n{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn an_unresolvable_allowlisted_child_is_an_error_not_a_clean_run() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path();
    std::os::unix::fs::symlink("src", repo.join("src")).expect("self-referential src link");
    let out = run_dry(repo);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "an allowlisted child that cannot be stat-ed is undecidable, never a clean run:\n{stderr}"
    );
    assert!(
        one_error(&stderr, "walk").text("path").ends_with("src"),
        "expected a walk run_error naming the unresolvable child:\n{stderr}"
    );
    assert!(
        rewritten_paths(&stdout).is_empty(),
        "an unresolvable child must not silently vanish from the walk roots:\n{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn a_nested_src_symlink_is_refused_in_every_mode() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path();
    fs::write(repo.join("Cargo.toml"), "[workspace]\n").unwrap();
    let external = tempfile::tempdir().unwrap();
    let real = external.path().join("inner");
    fs::create_dir_all(&real).expect("mkdir realsrc/inner");
    fs::write(real.join("m.rs"), "// removable\nfn m() {}\n").expect("write fixture");
    std::os::unix::fs::symlink(&real, repo.join("src")).expect("src link");
    for explicit in [false, true] {
        for args in [vec![], vec!["--rewrite", "--dry-run"], vec!["--rewrite"]] {
            let mut command = Command::new(bin());
            command.current_dir(repo).args(args);
            if explicit {
                command.arg(".");
            }
            let out = command.output().unwrap();
            assert_eq!(out.status.code(), Some(5), "{out:?}");
            assert!(
                one_error(&String::from_utf8_lossy(&out.stderr), "walk")
                    .text("path")
                    .ends_with("src")
            );
            assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
            assert_eq!(
                fs::read_to_string(real.join("m.rs")).unwrap(),
                "// removable\nfn m() {}\n"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_source_file_is_refused_not_silently_skipped() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path();
    fs::write(repo.join("Cargo.toml"), "[workspace]\n").unwrap();
    let external = tempfile::tempdir().unwrap();
    let store = external.path().join("store");
    fs::create_dir_all(&store).expect("mkdir store");
    fs::create_dir_all(repo.join("src")).expect("mkdir src");
    fs::write(store.join("real.rs"), "// removable\nfn a() {}\n").expect("write target");
    std::os::unix::fs::symlink(store.join("real.rs"), repo.join("src/linked.rs"))
        .expect("symlink source file");
    let out = run(repo);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "a source file the rewrite path refuses is not a clean run:\n{stderr}"
    );
    assert!(
        one_error(&stderr, "walk")
            .text("path")
            .ends_with("linked.rs"),
        "expected a walk run_error naming the symlinked source file:\n{stderr}"
    );
    assert!(
        rewritten_paths(&stdout).is_empty(),
        "nothing was rewritten, so nothing may be reported rewritten:\n{stdout}"
    );
    assert_eq!(
        fs::read_to_string(store.join("real.rs")).expect("read target"),
        "// removable\nfn a() {}\n",
        "the link target must be left byte-identical"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_directory_under_a_source_root_is_refused_not_silently_skipped() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path();
    let real = repo.join("elsewhere/deep");
    fs::create_dir_all(&real).expect("mkdir elsewhere/deep");
    fs::create_dir_all(repo.join("src")).expect("mkdir src");
    fs::write(real.join("f.rs"), "// removable\nfn f() {}\n").expect("write hidden file");
    std::os::unix::fs::symlink(&real, repo.join("src/linkdir")).expect("symlink dir");
    let out = run_dry(repo);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "a subtree the walk cannot enter is not a clean run:\n{stderr}"
    );
    assert!(
        one_error(&stderr, "walk").text("path").ends_with("linkdir"),
        "expected a walk run_error naming the undescended directory link:\n{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_build_directory_is_pruned_not_reported() {
    let td = tempfile::tempdir().unwrap();
    let repo = td.path();
    let real = repo.join("elsewhere");
    fs::create_dir_all(&real).expect("mkdir elsewhere");
    write(repo, "a.rs", "// removable\nfn a() {}\n");
    std::os::unix::fs::symlink(&real, repo.join("src/target")).expect("symlink target");
    let out = run_dry(repo);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_pending_changes(&out, "a link named as build output is pruned, not reported");
    assert!(
        rewritten_paths(&stdout).iter().any(|p| p.ends_with("a.rs")),
        "pruning the link must not stop the rest of the walk:\n{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn doc_scan_counts_unreadable_directory_as_error_and_exits_5() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    write(td.path(), "a.rs", "fn f() {}\n");
    let locked = td.path().join("locked");
    fs::create_dir_all(&locked).expect("mkdir locked");
    make_unreadable(&locked);
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    make_readable(&locked);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "unreadable directory in both scans must exit 5, not clean:\n{stderr}"
    );
    let errors = records_named(&stderr, "run_error");
    assert_eq!(errors.len(), 2);
    assert!(
        errors
            .iter()
            .all(|e| e.text("kind") == "walk" && e.text("path").contains("locked"))
    );
    assert_eq!(
        one_record(&stderr, "strip_summary").number("errors"),
        2,
        "both Rust and documentation walks report the inaccessible directory:\n{stderr}"
    );
}

#[test]
fn doc_warn_covers_extensions_and_bare_stems() {
    let td = tempfile::tempdir().unwrap();
    for name in [
        "README",
        "LICENCE",
        "NOTICE",
        "COPYING",
        "CHANGELOG",
        "AUTHORS",
        "CONTRIBUTORS",
    ] {
        fs::write(td.path().join(name), "text\n").expect("write bare stem");
    }
    for name in [
        "notes.txt",
        "guide.rst",
        "manual.adoc",
        "book.asciidoc",
        "doc.md",
        "doc2.markdown",
    ] {
        fs::write(td.path().join(name), "text\n").expect("write extension");
    }
    fs::create_dir_all(td.path().join("docs")).expect("mkdir docs");
    fs::write(td.path().join("docs").join("asset.bin"), "text\n").expect("write docs asset");
    fs::write(td.path().join("data.json"), "{}\n").expect("write non-doc data");
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(".")
        .current_dir(td.path())
        .output()
        .expect("failed to spawn comment-free");
    assert_eq!(
        out.status.code(),
        Some(0),
        "the fixture holds no pending rewrite:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut warned: Vec<String> = records_named(&stderr, "doc_file_warning")
        .iter()
        .map(|record| {
            Path::new(record.text("path"))
                .file_name()
                .expect("a warned path names a file")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    warned.sort();
    let mut expected = vec![
        "README",
        "LICENCE",
        "NOTICE",
        "COPYING",
        "CHANGELOG",
        "AUTHORS",
        "CONTRIBUTORS",
        "notes.txt",
        "guide.rst",
        "manual.adoc",
        "book.asciidoc",
        "doc.md",
        "doc2.markdown",
        "asset.bin",
    ];
    expected.sort_unstable();
    assert_eq!(
        warned, expected,
        "the doc scan missed or over-claimed a documented form:\n{stderr}"
    );
}

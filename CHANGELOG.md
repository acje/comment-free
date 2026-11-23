# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Closed public enum vocabulary: `RewriteMode`, `DocLintKind`, `DocLintOutcome`,
  `UndecidedCause` and `RunErrorKind` no longer carry `#[non_exhaustive]`.
  New variants require a breaking release, and consumers still treat unknown
  wire values from a newer producer as indeterminate, never clean.
- The closed-enum guard now scans public state/outcome enums in addition to
  error enums, with zero name exemptions.
- Breaking API change: `doc_lint_finding_record` takes `WordCount` instead of a
  separate `usize` and `fail_closed` bool, so contradictory fail-closed state is
  unrepresentable. The emitted JSON key set is unchanged.

## [0.2.0] - 2026-11-23

First release with a recorded changelog; the entries describe the capabilities
this release provides, not a diff against an earlier tagged release.

### Added

- Read-only default mode: recursive `.rs` traversal with build/hidden pruning
  and a doc-comment prose word budget (`--doc-max-words`, default 80). Fenced
  blocks, indented code blocks, inline code spans, and reference definitions
  are excluded from the count.
- Opt-in two-threshold policy gate (`--check-doc-budget`,
  `--doc-advisory-words`, `--doc-max-words`) with bounded macro-source coverage
  and its own policy records v2.
- `--rewrite`: byte-preserving rustdoc-link idiom canonicalisation followed by
  lexer-based removal of every non-doc `//` and `/* */` comment. `--dry-run`
  prints a unified diff and exits 3 when changes are pending.
- Bounded warning details via `--max-warning-files N|unlimited`, with
  total/shown/hidden partitions in `lint_summary`.
- JSON Lines records: `doc_lint_*` and run diagnostics v3, `rewrite_summary` v2,
  with one-line grammar constants and `docs/record-format.md` as the schema.
- Atomic replacement through a sibling temporary file with a pre-rename
  byte-conflict check, destination permission preservation, and documented
  limits for symlinks, hard links, concurrent writers, crash durability, and
  temporary-file cleanup.

### Changed

- Indeterminate doc-lint outcomes are first-class: `configuration_dependent`,
  `unreadable_doc_payload`, and `uninspected_macro_body` are reported and never
  counted clean.
- Traversal reports a walk failure as indeterminate, never as an absent file.
  Nested source-hiding links are traversal errors; explicit directory roots
  recurse.
- `--rustdoc-link-idioms` is retained as a deprecated alias for `--rewrite` for
  one release.

[Unreleased]: https://github.com/acje/comment-free/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/acje/comment-free/releases/tag/v0.2.0

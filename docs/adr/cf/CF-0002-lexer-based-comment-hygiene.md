# CF-0002. Lexer-Based Comment Hygiene

Date: 2026-09-06
Last-reviewed: 2026-09-06
Tier: B
Status: Accepted
Crates: comment-free

## Related

References: CF-0001

## Context

Comment-like text occurs inside Rust strings and macro tokens, so textual
marker removal cannot distinguish comments from program data. The existing
tool uses Rust syntax for doc-link edits and lexer tokens for stripping.
This record documents that boundary, not a new documentation policy.
Sources: [README](../../../README.md#comment-free),
[AGENTS](../../../AGENTS.md#what-this-repo-is), and
[implementation](../../../src/lib.rs) (`process_file`,
`strip_line_comments_with_counts`).

## Decision

Retain the two-pass rewrite rather than pretty-printing the parsed file or
maintaining a comment-marker allowlist.

R1 [5]: Strip line and block comment tokens whose doc style is absent, including safety annotations and policy markers; preserve doc comments and comment-looking string contents.

R2 [5]: Canonicalise supported rustdoc link idioms through doc-payload splices before stripping comments; preserve bytes outside those edits and the stripping pass's adjacent-whitespace normalization.

R3 [5]: Keep default mode read-only, linting doc prose against its configured word budget while excluding fenced code, indented code, inline code spans, and reference definitions.

The stripping pass trims horizontal whitespace before removed comments and
collapses blank-line scars. Preservation does not mean that every byte
outside a comment token is unchanged. Repository documentation files are
reported in rewrite mode but never rewritten.

## Consequences

+ becomes easier: Comment classification follows Rust tokens, not prose conventions.
− becomes harder: Non-doc rationale has no marker-based exemption from removal.
risks/migration: Acceptance of this record makes no behavior change. The
existing regression tests in [end_to_end.rs](../../../tests/end_to_end.rs)
include `safety_line_comment_is_stripped`, `safety_block_comment_is_stripped`,
and `string_literal_with_double_slash_marker_text_round_trips_byte_identical`.
Doc preservation does not imply every doc syntax supports link rewriting.

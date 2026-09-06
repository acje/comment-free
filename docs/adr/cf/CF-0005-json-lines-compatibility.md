# CF-0005. JSON Lines Compatibility

Date: 2026-09-06
Last-reviewed: 2026-09-06
Tier: B
Status: Draft
Crates: comment-free

## Related

References: CF-0001, CF-0003

## Context

Paths and item labels can contain characters that forge boundaries in
unescaped tab-separated output. JSON Lines provides structured records
while keeping dry-run diffs readable. The authoritative schema remains
[record-format.md](../../record-format.md), not this Draft or the help
templates. [Record emitters](../../../src/lib.rs) include
`push_json_string`, `doc_lint_undecided_record`, and the three record-family
version constants; [CLI output](../../../src/main.rs) supplies line endings.

## Decision

Retain the versioned record protocol and its distinction between extensible
consumption and strict schema validation.

R1 [6]: Emit structured records as newline-terminated JSON objects with escaped free-text fields; preserve independently versioned doc-lint, rewrite-summary, and run-diagnostic families according to docs/record-format.md.

R2 [5]: Parse fields by key and outcome, reject unsupported future versions and duplicate keys, and treat unknown record, kind, or outcome values as indeterminate rather than clean.

R3 [5]: Ignore unknown object keys when consuming records, but reject keys outside the schema when validating; bump a family version only for changes a conforming previous-version consumer cannot survive.

R4 [6]: Keep dry-run diff bodies human-facing rather than machine-parsed; use the documented opening-brace line filter to separate records from prefixed diff lines and non-record diagnostics.

All three families currently carry version two. The specification's seven
compatibility rules and outcome-specific evidence fields remain the
contract. Paths use `Path::display`, so non-UTF-8 paths are lossy; JSON
escaping does not create byte-exact filesystem path identity.

## Consequences

+ becomes easier: Unusual UTF-8 paths round-trip without forging record boundaries.
− becomes harder: Consumers must preserve uncertainty when extensions are not understood.
risks/migration: This record adds no schema fields or compatibility rules.
Existing [tests](../../../tests/end_to_end.rs) cover
`record_survives_a_path_containing_a_tab_and_a_newline`,
`every_emitted_record_line_parses_strictly`,
`parser_rejects_a_duplicate_field`, and
`parser_rejects_a_record_version_it_does_not_understand`.

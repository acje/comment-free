# CF ADR Authoring Template

This is non-policy authoring guidance, not an ADR or an index. It adapts
the useful metadata, parent relationship, and human/machine section split
of the gh-report template without importing that repository's decisions.
Discover the actual corpus with `adr-fmt --tree CF` from the repository root.

Copy the fenced skeleton to `cf/CF-NNNN-decision-title.md`, replace its
placeholders, and keep retrospective records Draft pending explicit review.
Use `adr-fmt.toml` for discovery rather than maintaining a second index.
The first `References` target is the structural parent; CF-0001 is the
existing root. Do not combine `Root` and `References`.

```markdown
# CF-NNNN. Decision Title

Date: YYYY-MM-DD
Last-reviewed: YYYY-MM-DD
Tier: B
Status: Draft
Crates: comment-free

## Related

References: CF-0001

## Context

Explain the local problem and rejected alternative. Link the actual
repository specification and implementation that support this record.

## Decision

Summarise the existing decision without strengthening its guarantees.

R1 [5]: Replace this sentence with a source-backed rule describing the existing contract.

## Consequences

+ becomes easier: Describe the benefit of the recorded choice.
− becomes harder: Describe its cost or rejected capability.
risks/migration: Preserve limitations and name relevant regression tests.
```

Tier B fits rules and information-flow contracts (layers 5 and 6); choose
metadata for the decision actually recorded, not to avoid diagnostics.
Number rules sequentially from R1, keep each between 7 and 60 words, and
keep Tier B records within ten rules. Write concise Context and
Consequences sections for humans; tagged rules are the extraction surface.
Link local authorities and name implementation symbols and tests so a
reviewer can check each guarantee. Do not copy upstream policy citations
as if they governed this crate.

Run `adr-fmt --lint`, `adr-fmt --tree CF`, and
`adr-fmt --context comment-free`. Inspect diagnostics: lint warnings are
advisory at the pinned revision, even with exit zero. Draft records appear
in the tree but contribute no accepted rules to context. Lint success does
not accept a decision. Existing repository specifications remain authoritative.

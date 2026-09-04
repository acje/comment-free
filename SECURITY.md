# Security Policy

## Reporting a Vulnerability

If you discover a security issue in comment-free, please report it
privately rather than filing a public issue.

Send a report to the repository owner via GitHub:
<https://github.com/acje>. Include:

- a description of the issue,
- reproduction steps or a minimal proof of concept,
- any known mitigations.

We will acknowledge receipt within a reasonable window, investigate, and
coordinate disclosure once a fix is available.

## Scope

In scope:

- supply-chain concerns flagged by `cargo audit` / `cargo deny` (policy
  in `deny.toml`),
- input handling in the Rust source lexer/parser when run over untrusted
  source trees,
- incorrect rewriting in `--rewrite` mode: comment-free edits files in
  place, so a rewrite that deletes a doc comment, drops a `// SAFETY:`
  line, or otherwise changes program semantics is a security-relevant
  defect, not merely a correctness one.

Out of scope:

- issues in third-party dependencies for which an upstream advisory
  already exists — please file upstream first.

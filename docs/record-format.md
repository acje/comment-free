# comment-free machine-readable record format

This is the authoritative reference for the structured records
`comment-free` emits. The rustdoc constants
`comment_free::DOC_LINT_RECORD_GRAMMAR` and
`comment_free::REWRITE_RECORD_GRAMMAR` carry one-line templates for
`--help`; this document carries the contract.

## Encoding

Records are **JSON Lines**: one JSON object per line, terminated by
`\n`, with no line ever containing a raw tab, newline, or control
character. Every free-text value — paths and item labels — is JSON
escaped, so a filename containing a tab or a newline cannot break record
boundaries. This replaces the tab-separated `v=1` grammar, in which a
hostile or merely unusual filename could forge a field separator.

Field order is fixed as documented below, but consumers must not depend
on it; parse by key.

`path` is rendered with `Path::display`, so a path that is not valid
UTF-8 is reported lossily.

## Versions

Two independent version constants, each carried as the `v` field of its
own record family:

| Constant | Records | Current |
|---|---|---|
| `DOC_LINT_RECORD_VERSION` | `doc_lint_*` | `2` |
| `REWRITE_RECORD_VERSION` | `rewrite_summary` | `2` |
| `DIAGNOSTIC_RECORD_VERSION` | `run_error`, `doc_file_warning`, `rewrite_file`, `strip_summary`, `lint_summary` | `2` |

The run-diagnostic family was previously emitted as the unversioned
tab-separated lines `SUMMARY`, `REWRITE`, `WOULD_REWRITE`, `DOC_WARN`,
`WALK_ERROR`, `IO_ERROR`, `PARSE_ERROR` and `CONFLICT_ERROR`, carrying raw
unescaped paths. Those lines are machine-consumed — this repository's own
exit-5 tests parse them — so a filename containing a tab could make a
consumer attribute a finding to the wrong file. They join the JSON Lines
contract at the same version the other two families bumped to; the
tab-separated spellings are gone and are not emitted in any mode.

A consumer **rejects** a record whose `v` exceeds the version it
understands. A version is bumped only for a change that a correct
consumer of the previous version cannot survive — see the compatibility
rules below, which are designed so that most evolution needs no bump.

## Records

### `doc_lint_finding`

One per finding, on stdout.

```json
{"record":"doc_lint_finding","v":2,"outcome":"finding","kind":"overlong_doc","path":"src/lib.rs","line":42,"item":"fn f","words":90,"budget":80,"fail_closed":false}
```

`fail_closed` is `true` when `words` came from the fail-closed recount
path (an unbalanced fence at EOF); the number is then inflated and is
not the real prose count.

### `doc_lint_header`

One per finding kind, naming the doctrine once, on stdout.

```json
{"record":"doc_lint_header","v":2,"kind":"overlong_doc","doctrine":"Rust docs must contain a concise summary, ..."}
```

The doctrine string states no numeric limit on fenced examples. Nothing
in the tool counts examples, and the record does not promise a count it
does not enforce.

### `doc_lint_hint`

Up to 50 per kind, on stdout, sorted by `words - budget` descending. A
`doc_lint_truncated` record follows when the kind exceeds the cap.

```json
{"record":"doc_lint_hint","v":2,"outcome":"finding","kind":"overlong_doc","path":"src/lib.rs","line":42,"item":"fn f","words":90,"budget":80}
```

### `doc_lint_truncated`

```json
{"record":"doc_lint_truncated","v":2,"kind":"overlong_doc","remaining":10}
```

### `doc_lint_undecided`

One per item whose doc set the linter could not decide, on stdout.

```json
{"record":"doc_lint_undecided","v":2,"outcome":"configuration_dependent","kind":"overlong_doc","path":"src/lib.rs","line":42,"item":"fn f","words":40,"budget":80,"words_all_cfgs":95,"fail_closed":false}
```

`cfg_attr` doc payloads — nested `cfg_attr` included — are held apart
from the unconditional doc set, because their predicates are not
resolved by this tool and two of them may be mutually exclusive — a
`unix` and a `windows` doc set are never both present in one build.
Summing them would manufacture a word count no build exposes.

A predicate built only from the boolean constants `all()` and `any()`
is folded: `cfg_attr(all(), doc = ...)` applies in every configuration
and counts as unconditional, `cfg_attr(any(), doc = ...)` applies in
none and is dropped. Composition through `not(...)` and through nesting
is folded the same way. Anything naming a real `cfg` key stays
unresolved.

`words` is therefore the count of the unconditional doc set alone, which
every configuration carries, and `words_all_cfgs` the count with every
`cfg_attr` doc payload active. The `words_all_cfgs` text is an upper
bound, **not** an attainable build: it may carry fence markers from
payloads that are never both present. It therefore never establishes a
clean verdict. This record is emitted whenever an unresolved payload
remains and no finding is provable from the unconditional set.

A finding requires that the unconditional set alone be over budget in
every configuration. When a conditional payload opens or closes a code
fence, the fence state at the unconditional prose is itself unresolved,
so the item is reported here rather than as a finding.

It is **not** a finding. It does not increment the `findings` counter and
does not drive exit code 4; it increments `undecided` in `lint_summary`.
A run with `undecided` above zero has not established that the tree is
clean, even at exit 0.

`fail_closed` reports the balance state of the `words_all_cfgs` count.

### `rewrite_summary`

One per `--rewrite` run including `--dry-run`, on stderr, aggregating
every processed file.

```json
{"record":"rewrite_summary","v":2,"mode":"write","comments_removed":3,"inline_trimmed":1,"blank_lines_collapsed":0,"doc_links_rewritten":2}
```

`mode` is `write` or `dry-run`.

### `run_error`

One per failed path, on stderr. Every occurrence increments the `errors`
counter of the run's summary record and drives exit code 5.

```json
{"record":"run_error","v":2,"kind":"walk","path":"src/locked","message":"cannot traverse src/locked: Permission denied (os error 13)"}
```

`kind` is one of:

| `kind` | Meaning |
|---|---|
| `walk` | a directory entry could not be read during traversal |
| `io` | the file could not be read or written |
| `parse` | the file could not be parsed as Rust |
| `conflict` | the destination changed between read and write; the file was left exactly as found |

An unknown `kind` is indeterminate, never clean — rule 4 below.

### `doc_file_warning`

One per documentation file found under ROOT in `--rewrite` mode, on
stderr. These files are never modified; the record exists so a consumer
can see what was deliberately skipped.

```json
{"record":"doc_file_warning","v":2,"path":"README.md"}
```

### `rewrite_file`

One per changed file, on stdout. `mode` is `write` when the file was
replaced on disk and `dry-run` when it only would have been.

```json
{"record":"rewrite_file","v":2,"mode":"dry-run","path":"src/lib.rs"}
```

Under `--dry-run` the unified diff for that file follows immediately on
stdout as plain text — see "The `--dry-run` diff body" below.

### `strip_summary`

One per `--rewrite` run including `--dry-run`, on stderr, closing the run.

```json
{"record":"strip_summary","v":2,"mode":"write","rewritten":1,"unchanged":0,"errors":0}
```

### `lint_summary`

One per default-mode lint run, on stderr, closing the run.

```json
{"record":"lint_summary","v":2,"files":12,"findings":3,"undecided":1,"errors":0}
```

`undecided` counts `doc_lint_undecided` records. It is reported
separately from `findings` precisely because an item the linter could
not decide is neither a finding nor clean.

## Compatibility rules for consumers

These rules are what make the format extensible without a version bump.
A consumer that follows them keeps working as the tool grows; one that
does not is not a conforming consumer.

1. **Reject a `v` greater than you understand.** Do not attempt a
   best-effort parse of a future record.
2. **Ignore unknown object keys.** New optional fields may be added to
   an existing record within the same version.
3. **Treat an unknown `record` value as indeterminate, never as clean.**
   New record types may be added within the same version. A run whose
   output you did not fully understand has not been shown to be clean.
4. **Treat an unknown `kind` value as indeterminate, never as clean.**
   New finding kinds may be added within the same version.
5. **Treat an unknown `outcome` value as indeterminate, never as
   clean.** Two values are emitted today: `finding`, and
   `configuration_dependent` on the `doc_lint_undecided` record, where a
   doc set behind unresolved `cfg` predicates is over budget for some
   configurations only. A further indeterminate outcome is planned for
   doc comments generated inside uninspected macro bodies. Such
   additions arrive as an added `outcome` value with additional keys,
   under `v=2`.
6. **Reject duplicate keys.** A record with a repeated key is malformed;
   do not keep the last value.
7. **Reject a record carrying a key not in its schema** if you are
   validating rather than consuming. Rule 2 (ignore unknown keys) is for
   consumers reading fields they care about; a validator — such as this
   repository's own test suite — pins the exact schema so that an
   accidental field cannot ship unnoticed.

Rules 3 to 5 are the mechanism by which an indeterminate result is
additive: nothing about reporting "this item could not be decided"
requires a field-shape change to an existing record.

## The `--dry-run` diff body — human-only, never machine-parsed

`--dry-run` prints a unified diff to stdout after each `rewrite_file`
record. **The diff body is deliberately NOT a record and is deliberately
NOT escaped.** It is the only human-facing surface this tool has; wrapping
it in a JSON string would destroy it.

Consumers must not parse it. If you need to know which files changed, read
the `rewrite_file` records; if you need to know what changed within a
file, apply the diff with a diff tool, do not scrape it.

Separating the two streams is mechanical, not a matter of judgement:

- Every record line starts with `{`.
- Every diff line is prefixed by the unified-diff format itself — `---`,
  `+++`, `@@`, or a leading space, `-` or `+` for context, removed and
  added lines respectively.

The `---`/`+++` header lines carry a path, and a path may contain a
newline. Rendering one raw would split the header and put whatever
follows the newline at column zero — enough to forge a record. The header
therefore renders the path as a **single line**: `\n`, `\r`, `\t`, and
every other C0 control character (plus `DEL`) are escaped in place. This
affects the human-facing header only; the `path` field of a record is
JSON-escaped separately and round-trips the original bytes.

So a diff body line can never begin with `{`, and **filtering stdout to
lines starting with `{` yields exactly the records**. This property is
pinned by a test.

The only other non-record output is free-text prose on stderr: the
`--rustdoc-link-idioms` deprecation note, the fatal `error:` line, and
the CLI-argument diagnostics rendered by `clap` (including `--help`).
The deprecation note is a static string. The fatal `error:` line renders
its message through the same single-line escaper the diff header uses, so
no message text can introduce a column-zero line. Argument diagnostics
interpolate caller-controlled argv, so argv is screened first: if any
argument contains a C0 control character or `DEL`, the diagnostic is
replaced by a static prose line that reproduces none of the offending
text, and the process exits 2. Control-free argv keeps `clap`'s normal
rendering, which then cannot span a line break. All of these are
human-facing, none carries a machine-readable contract, and all are
excluded by the same `{`-prefix filter.

A prose `warning:` line formerly accompanied the `doc_file_warning`
records, interpolating ROOT raw. It has been removed: it duplicated what
the records already carry, and interpolating a path into unescaped prose
is the one shape that can forge a record line. The `doc_file_warning`
records remain the contract for "these documentation files were found and
not modified".

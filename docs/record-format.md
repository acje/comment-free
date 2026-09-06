# comment-free machine-readable record format

This is the authoritative reference for the structured records
`comment-free` emits. The rustdoc constants
`comment_free::DOC_LINT_RECORD_GRAMMAR`,
`comment_free::REWRITE_RECORD_GRAMMAR`, and
`comment_free::DIAGNOSTIC_RECORD_GRAMMAR` carry one-line templates for
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

Three independent version constants, each carried as the `v` field of its
own record family:

| Constant | Records | Current |
|---|---|---|
| `DOC_LINT_RECORD_VERSION` | `doc_lint_*` | `3` |
| `REWRITE_RECORD_VERSION` | `rewrite_summary` | `2` |
| `DIAGNOSTIC_RECORD_VERSION` | `run_error`, `doc_file_warning`, `rewrite_file`, `strip_summary`, `lint_summary` | `3` |

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

Version 3 permits bounded detail output: `--max-warning-files` defaults to 1,
accepts ASCII decimal digits (leading zeros allowed) or exactly `unlimited`,
and conflicts with rewrite/its alias. Zero emits no stdout lint records.
Every scoped file is scanned; errors always remain on stderr. A warning file
contains findings or undecided items (or both); clean/error-only files consume
no slots. All details from an admitted file are emitted together. Admission is
native PathBuf order, before lossy display, so equal displayed paths need not
identify equal files. The CLI cap never selects which files rewrite may modify.

Record counts on stdout no longer equal corpus totals. Use `lint_summary` for
full totals and exit status for the verdict. This breaks a v2 consumer's
one-detail-per-item assumption and requires the family bump. The deprecated
library `lint_summary_record` explicitly retains its v2 payload/version; new
callers use `LintTotals::record`. The CLI emits only the complete v3 summary.
The v3 public helper takes `ReportScope` and `WarningLimit`, not raw metadata
strings. `WarningLimit` parses the same strict syntax as the CLI; its numeric
variant formats normalized decimal. These types constrain schema values, not
the truth of caller-supplied scope or relationships between caller-owned totals.

### `doc_lint_finding`

One per admitted finding, on stdout.

```json
{"record":"doc_lint_finding","v":3,"outcome":"finding","kind":"overlong_doc","path":"src/lib.rs","line":42,"item":"fn f","words":90,"budget":80,"fail_closed":false}
```

`fail_closed` is `true` when `words` came from the fail-closed recount
path (an unbalanced fence at EOF); the number is then inflated and is
not the real prose count.

### `doc_lint_header`

One per finding kind, naming the doctrine once, on stdout.

```json
{"record":"doc_lint_header","v":3,"kind":"overlong_doc","doctrine":"Rust docs must contain a concise summary, ..."}
```

The doctrine string states no numeric limit on fenced examples. Nothing
in the tool counts examples, and the record does not promise a count it
does not enforce.

### `doc_lint_hint`

Up to 50 admitted findings, on stdout, sorted by `words - budget` descending.
Equal overshoots retain native path order then report order. A
`doc_lint_truncated` record follows when admitted findings exceed 50; its
remaining count excludes hidden-file findings. No admitted findings means no
header, hint, or truncated record.

```json
{"record":"doc_lint_hint","v":3,"outcome":"finding","kind":"overlong_doc","path":"src/lib.rs","line":42,"item":"fn f","words":90,"budget":80}
```

### `doc_lint_truncated`

```json
{"record":"doc_lint_truncated","v":3,"kind":"overlong_doc","remaining":10}
```

### `doc_lint_undecided`

One per admitted item whose doc set the linter could not decide, on stdout. The
evidence fields are **keyed on `outcome`**, because each cause supports
different evidence; a consumer reads `outcome` before reading any
numeric field.

#### `outcome`: `configuration_dependent`

```json
{"record":"doc_lint_undecided","v":3,"outcome":"configuration_dependent","kind":"overlong_doc","path":"src/lib.rs","line":42,"item":"fn f","words":40,"budget":80,"words_all_cfgs":95,"fail_closed":false}
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

It is **not** a finding. It does not increment the `findings` counter;
it increments `undecided` in `lint_summary`. A run with `undecided`
above zero has not established that the tree is clean, and does not exit
`0` — see "Exit codes and the meaning of clean" below.

`fail_closed` reports the balance state of the `words_all_cfgs` count.

#### `outcome`: `unreadable_doc_payload`

```json
{"record":"doc_lint_undecided","v":3,"outcome":"unreadable_doc_payload","kind":"overlong_doc","path":"src/lib.rs","line":42,"item":"fn f","budget":80}
```

The item carries a doc payload that is not a string literal — a macro
call in the doc-value position, such as `#[doc = include_str!("x.md")]`,
`#[doc = concat!(...)]`, or either of those inside a `cfg_attr`. The text
resolves only by macro expansion, which this tool does not perform.

This outcome carries **no `words`, `words_all_cfgs` or `fail_closed`
key**. Those fields report a count produced by reading the doc text; no
reading happened, so no count exists, and emitting `0` would assert prose
this tool never saw. `budget` is the budget that was in force.

An unreadable payload outranks an unresolved `cfg` predicate: an item
with both is reported here, not as `configuration_dependent`. It also
suppresses what would otherwise be a finding on the item's readable
prose, because an unread payload may open or close a code fence, leaving
the fence state — and therefore the word count — unresolved.

A payload behind a predicate that folds to false is dropped, exactly as a
readable one is: a payload present in no configuration is not an
indeterminate.

Two residual gaps this outcome does **not** cover, stated so they are not
mistaken for coverage: doc text synthesised by a procedural macro from
tokens that never spell `doc` is invisible to a syntactic tool, and is
not detected. Doc attributes inside a macro token body are reported
under `uninspected_macro_body` instead.

#### `outcome`: `uninspected_macro_body`

```json
{"record":"doc_lint_undecided","v":3,"outcome":"uninspected_macro_body","kind":"overlong_doc","path":"src/lib.rs","line":42,"item":"macro noisy","budget":80}
```

A macro token body — a `macro_rules!` definition, or the tokens passed to
any macro invocation — carries a doc attribute: `#[doc = ...]`,
`#![doc = ...]`, `#[cfg_attr(_, doc = ...)]`, or a `///` or `//!` comment,
which the lexer presents as the same attribute tokens. This tool does not
expand macros, so it does not know what item the expansion documents, or
how many times.

The key set is the same as `unreadable_doc_payload`, and for the same
reason: no reading produced a word count. `item` names the macro —
`macro noisy` for `macro_rules! noisy`, otherwise the invoked path, as in
`macro generate`.

The report is made at the **outermost** opaque body. A nested invocation
inside that body is tokens within it, not a second item, and one
indeterminate covers the whole body. A `macro_rules!` definition carrying
the doc attribute is reported at its definition; an invocation such as
`noisy!()` that passes no tokens is not itself a doc payload and is not
reported again.

A macro body that carries no doc attribute is **not** reported. The tool
would otherwise report every `println!` and `vec!` in a tree, which is
noise, not coverage. The precision rule is mechanical: an `attribute`
token group containing the ident `doc` immediately followed by `=`. This
means `#[doc(hidden)]` inside a macro body is not reported, because it is
rustdoc metadata and carries no prose.

### Exit codes and the meaning of clean

Default lint mode exits `4` when the run produced at least one finding
**or** at least one undecided item, and `0` only when it produced
neither. One rule governs both indeterminates. Exit `0` is the assertion
"this tree is clean", and a run that could not read or could not decide
part of the tree has not established that.

Exit `4` therefore does not mean "findings observed". The two remain
distinguishable in the records: `lint_summary` counts `findings` and
`undecided` separately, and every `doc_lint_*` record names its
`outcome`.

The same rule governs `--rewrite --dry-run`: it exits `3` when
`strip_summary` reports `rewritten` above zero, because a preview
holding pending changes has not shown the tree clean either. Exit `0`
from a dry run therefore means "nothing to do", which is what makes it
usable as a check. Write mode is not a check — it was asked to change
the tree — so it exits `0` whatever `rewritten` reports. `errors` above
zero outranks both and exits `5`: a tree that could not be fully read is
a stronger signal than a pending change. No record grammar changes; the
counters that drive this retain their meaning in `v3` `strip_summary` fields.
Exact lint-total overflow aborts with exit 1 and no final summary; it never
saturates or wraps into a clean verdict.

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
{"record":"run_error","v":3,"kind":"walk","path":"src/locked","message":"cannot traverse src/locked: Permission denied (os error 13)"}
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
{"record":"doc_file_warning","v":3,"path":"README.md"}
```

### `rewrite_file`

One per changed file, on stdout. `mode` is `write` when the file was
replaced on disk and `dry-run` when it only would have been.

```json
{"record":"rewrite_file","v":3,"mode":"dry-run","path":"src/lib.rs"}
```

Under `--dry-run` the unified diff for that file follows immediately on
stdout as plain text — see "The `--dry-run` diff body" below.

### `strip_summary`

One per `--rewrite` run including `--dry-run`, on stderr, closing the run.

```json
{"record":"strip_summary","v":3,"mode":"write","rewritten":1,"unchanged":0,"errors":0}
```

### `lint_summary`

One per default-mode lint run, on stderr, closing the run.

```json
{"record":"lint_summary","v":3,"root":".","scope":"project-allowlist","max_warning_files":"1","files":12,"errors":0,"warning_files":2,"warning_files_shown":1,"warning_files_hidden":1,"findings":3,"findings_shown":2,"findings_hidden":1,"undecided":1,"undecided_shown":0,"undecided_hidden":1,"overlong_doc_findings":3,"overlong_doc_undecided":1,"over_budget":3,"configuration_dependent":0,"unreadable_doc_payload":1,"uninspected_macro_body":0}
```

`undecided` counts all undecided items, including suppressed records. It is reported
separately from `findings` precisely because an item the linter could
not decide is neither a finding nor clean.

`root` preserves the supplied spelling (or `.` for omitted ROOT), JSON escaped.
`scope` is the policy selected once for this run: `file`, `project-allowlist`,
`recursive-directory`, or `unresolved` for a failed manifest probe. Project
allowlist covers benches/crates/examples/src/tests/build.rs. Default cwd and
explicit manifest roots use it; other explicit directories recurse. No upward
discovery occurs. Rewrite uses the same selection rules, without a warning cap.
`max_warning_files` is normalized decimal text or `unlimited`. `files` counts
enumerated Rust paths, including read/parse failures. Warning-file and item
totals split exactly into shown plus hidden; no per-hidden-file list is emitted.
`overlong_doc_findings`/`overlong_doc_undecided` are kind totals; `over_budget`
counts outcome `finding`, with the three other outcome totals named verbatim.
All these are exact u32 run totals. Per-item word/line counts and rewrite counts
retain their pre-existing representation limits, outside this run-total contract.

The lint implementation retains sorted paths (O(P items plus path bytes and
capacities)), a current source/AST/report, and at most 50 owned hint candidates.
Per-file bytes/items and path count are unbounded. Output blocks synchronously;
there are no spawned tasks, queues, or retries. Runtime/allocator/stack/kernel
memory and huge single-file/error output are not bounded by this contract.

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
   clean.** Four values are emitted today: `finding`; and, on the
   `doc_lint_undecided` record, `configuration_dependent` for a doc set
   behind unresolved `cfg` predicates that is over budget for some
   configurations only, `unreadable_doc_payload` for a doc payload that
   is not a string literal, and `uninspected_macro_body` for a doc
   attribute inside a macro token body. Further outcomes arrive as an
   added `outcome` value, under `v=3`, and may carry a different key set
   from the outcomes already defined — which is why the evidence fields
   of `doc_lint_undecided` are read only after its `outcome`.
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

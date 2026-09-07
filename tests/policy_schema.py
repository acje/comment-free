import json
import pathlib
import subprocess
import sys
import tempfile
import copy
import struct


def unique(pairs):
    result = {}
    for key, value in pairs:
        assert key not in result, f"duplicate {key}"
        result[key] = value
    return result


def keys(value, expected):
    assert set(value) == set(expected.split()), (set(value), expected)


families = "findings undecided warning_files configuration_dependent unreadable_doc_payload uninspected_macro_body".split()
causes = families[3:]
usize_max = 2 ** (8 * struct.calcsize("P")) - 1


def unsigned(value, maximum=2**32 - 1):
    assert type(value) is int and 0 <= value <= maximum


def validate_exit(record, code):
    assert type(code) is int
    assert code == {"pass": 0, "fail": 1, "unknown": 2}[record["verdict"]]


def validate(record):
    assert type(record) is dict
    if record.get("record") == "run_error":
        keys(record, "record v kind path message")
        assert type(record["v"]) is int and record["v"] == 3
        assert record["kind"] in ("walk", "io", "parse", "conflict")
        assert all(type(record[k]) is str for k in ("path", "message"))
        return
    assert type(record["version"]) is int and record["version"] == 1
    if record["kind"] == "policy_summary":
        keys(record, "kind version root scope files errors max_warning_files verdict reasons advisory enforced")
        assert record["verdict"] in ("pass", "fail", "unknown")
        assert record["scope"] in ("file", "recursive-directory")
        assert type(record["root"]) is str
        for name in ("files", "errors"):
            unsigned(record[name])
        limit = record["max_warning_files"]
        assert type(limit) is str
        assert limit == "unlimited" or (limit.isascii() and limit.isdecimal() and str(int(limit)) == limit and int(limit) <= usize_max)
        assert type(record["reasons"]) is list and all(type(r) is str for r in record["reasons"])
        assert len(set(record["reasons"])) == len(record["reasons"])
        order = "empty_scope processing_error advisory_undecided enforced_undecided enforced_violation counter_overflow output_error".split()
        assert record["reasons"] == [r for r in order if r in record["reasons"]]
        for threshold in ("advisory", "enforced"):
            budget = record[threshold]
            assert type(budget) is dict
            unsigned(budget["max_words"], usize_max)
            expected = ["max_words", "over_budget"]
            for name in families:
                expected += [name, name + "_shown", name + "_hidden"]
                assert budget[name] == budget[name + "_shown"] + budget[name + "_hidden"]
            keys(budget, " ".join(expected))
            assert all(type(v) is int and v >= 0 for v in budget.values())
            assert all(v <= 2**32 - 1 for k, v in budget.items() if k != "max_words")
            assert budget["over_budget"] == budget["findings"]
            for suffix in ("", "_shown", "_hidden"):
                assert budget["undecided" + suffix] == sum(budget[c + suffix] for c in causes)
            assert budget["warning_files"] <= record["files"]
            if limit != "unlimited":
                assert budget["warning_files_shown"] <= int(limit)
            if limit == "0":
                assert all(budget[name + "_shown"] == 0 for name in families)
        assert record["advisory"]["max_words"] <= record["enforced"]["max_words"]
        expected_reasons = [name for name, applies in (
            ("empty_scope", record["files"] == 0),
            ("processing_error", record["errors"] > 0),
            ("advisory_undecided", record["advisory"]["undecided"] > 0),
            ("enforced_undecided", record["enforced"]["undecided"] > 0),
            ("enforced_violation", record["enforced"]["findings"] > 0),
        ) if applies]
        assert record["reasons"] == expected_reasons
        unknown = any(r != "enforced_violation" for r in expected_reasons)
        assert record["verdict"] == ("unknown" if unknown else "fail" if expected_reasons else "pass")
    else:
        assert record["kind"] == "policy_detail"
        assert record["threshold"] in ("advisory", "enforced")
        base = "kind version threshold event"
        event = record["event"]
        if event == "doc_lint_header":
            extra = "doctrine"
        elif event == "doc_lint_truncated":
            extra = "remaining"
        else:
            extra = "outcome path line item budget"
            if event in ("doc_lint_finding", "doc_lint_hint"):
                assert record["outcome"] == "finding"
                extra += " words"
                if event == "doc_lint_finding":
                    extra += " fail_closed"
            else:
                assert event == "doc_lint_undecided"
                assert record["outcome"] in causes
                if record["outcome"] == "configuration_dependent":
                    extra += " words words_all_cfgs fail_closed"
        keys(record, base + " " + extra)
        for name in ("line", "budget", "words", "words_all_cfgs", "remaining"):
            if name in record:
                unsigned(record[name], 2**32 - 1 if name == "remaining" else usize_max)
        for name in ("path", "item", "doctrine", "outcome"):
            if name in record:
                assert type(record[name]) is str
        if "fail_closed" in record:
            assert type(record["fail_closed"]) is bool


def records(data, policy=True):
    result = []
    for line in data.decode().splitlines():
        if not policy and not line.startswith("{"):
            assert line.startswith("error:"), line
            continue
        record = json.loads(line, object_pairs_hook=unique)
        assert json.loads(json.dumps(record), object_pairs_hook=unique) == record
        if policy:
            validate(record)
        result.append(record)
    return result


def run(path, a=80, e=120, cap="unlimited"):
    output = subprocess.run([sys.argv[1], "--check-doc-budget", "--doc-advisory-words", str(a), "--doc-max-words", str(e), "--max-warning-files", cap, str(path)], capture_output=True, check=False)
    details, meta = records(output.stdout), records(output.stderr)
    summary = meta[-1]
    assert summary["kind"] == "policy_summary"
    validate_exit(summary, output.returncode)
    return output.returncode, details, summary


def doc(n, name="item"):
    return f'#[doc = {json.dumps("word " * n)}] pub fn {name}() {{}}\n'


with tempfile.TemporaryDirectory() as directory:
    root = pathlib.Path(directory)
    path = root / 'quote"slash\\tab\t.rs'
    samples = [doc(n) for n in (0, 80, 81, 120, 121)] + [
        '#[cfg_attr(feature="x", doc="extra")]' + doc(90),
        '#[cfg_attr(feature="x", doc="extra")]' + doc(90) + doc(121, "breach"),
        '#[doc=include_str!("absent")] pub fn item() {}',
        'macro_rules! concat { () => { "words" }; } #[doc=concat!()] fn item() {}',
        'macro_rules! opaque { () => { #[doc="words"] fn f() {} }; }',
        '#[cfg_attr(all(), cfg_attr(not(any()), doc="words"))] fn item() {}',
        '#[cfg_attr(any(), doc=include_str!("absent"))] fn item() {}',
        '#[cfg_attr(feature="x", cfg_attr(feature="y", doc="words"))] fn item() {}',
        '#[doc="```\n' + 'word ' * 140 + '\n```"] fn item() {}',
        '#[doc="```\n' + 'word ' * 140 + '"] fn item() {}',
        'generate!(tokens_without_doc);',
    ]
    for sample in samples:
        path.write_text(sample)
        for a, e in ((80, 120), (0, 0), (120, 120)):
            for cap in ("0", "1", "unlimited"):
                code, details, summary = run(path, a, e, cap)
                if cap == "0":
                    assert not details
                for threshold, words in (("advisory", a), ("enforced", e)):
                    legacy = subprocess.run([sys.argv[1], "--doc-max-words", str(words), "--max-warning-files", cap, str(path)], capture_output=True, check=False)
                    total = records(legacy.stderr, False)[-1]
                    for name in families + ["over_budget"]:
                        assert summary[threshold][name] == total[name], (sample, threshold, name)
                    for name in families[:3]:
                        for suffix in ("_shown", "_hidden"):
                            assert summary[threshold][name + suffix] == total[name + suffix]
                    expected = records(legacy.stdout, False)
                    actual = [d for d in details if d["threshold"] == threshold]
                    assert len(expected) == len(actual)
                    for old, new in zip(expected, actual):
                        old = dict(old)
                        event = old.pop("record")
                        old.pop("v")
                        old.pop("kind")
                        assert new == dict(old, kind="policy_detail", version=1, threshold=threshold, event=event)
    path.unlink()
    assert run(root)[0] == 2
    path.write_text("fn docless() {}")
    assert run(root)[0] == 0
    path.write_text("fn invalid( {")
    assert run(root)[2]["errors"] == 1
    path.write_bytes(b"\xff")
    assert run(root)[2]["errors"] == 1
    path.unlink()
    (root / "hidden.rs").symlink_to(root / "missing.rs")
    assert run(root)[2]["errors"] == 1
    (root / "hidden.rs").unlink()
    path.write_text(doc(120))
    assert run(path)[0] == 0
    path.write_text(doc(121))
    compiled = subprocess.run(["rustc", "--crate-name", "policy_plant", "--crate-type", "lib", str(path), "--out-dir", str(root)], capture_output=True, check=False)
    assert compiled.returncode == 0, compiled.stderr
    assert run(path)[0] == 1
    path.write_text(doc(120))
    assert run(path)[0] == 0
    clean = run(path)[2]
    for field, bad in [("version", True), ("files", -1), ("errors", "0"), ("root", 0), ("max_warning_files", "01"), ("verdict", "fail"), ("reasons", ["enforced_violation"])]:
        mutant = dict(clean, **{field: bad})
        try:
            validate(mutant)
        except (AssertionError, KeyError, TypeError):
            pass
        else:
            raise AssertionError(f"validator accepted {field}={bad!r}")
    text = json.dumps(clean)
    mutants = [text.replace('"version": 1', '"version": 2'), text[:-1] + ',"extra":0}', text[:-1] + ',"version":1}']
    nested = text.replace('"max_words": 80', '"max_words":80,"max_words":80')
    for mutation in mutants + [nested]:
        try:
            validate(json.loads(mutation, object_pairs_hook=unique))
        except (AssertionError, KeyError):
            pass
        else:
            raise AssertionError("validator accepted planted schema violation")
    validate(json.loads(text, object_pairs_hook=unique))
    for field, value in [("kind", "bogus"), ("scope", "bogus"), ("files", 2**32), ("reasons", ""), ("version", "1")]:
        mutant = dict(clean, **{field: value})
        try:
            validate(mutant)
        except (AssertionError, KeyError, TypeError):
            pass
        else:
            raise AssertionError(field)
    for field, value in [("max_words", 121), ("findings", -1), ("findings_shown", True), ("over_budget", 999), ("undecided", 1)]:
        mutant = copy.deepcopy(clean)
        mutant["advisory"][field] = value
        try:
            validate(mutant)
        except (AssertionError, KeyError, TypeError):
            pass
        else:
            raise AssertionError(field)
    for code in (-1, 1, 2, 5, True):
        try:
            validate_exit(clean, code)
        except AssertionError:
            pass
        else:
            raise AssertionError("exit mismatch accepted")
    path.unlink()
    for i in range(3):
        (root / f"{i}.rs").write_text("".join(doc(121 + j, f"f{j}") for j in range(60)) + samples[7] + samples[9] + samples[12])
    for cap, selected in (("0", 0), ("1", 1), ("unlimited", 3)):
        code, details, summary = run(root, cap=cap)
        assert code == 2
        for threshold in ("advisory", "enforced"):
            budget = summary[threshold]
            assert budget["findings"] == 180
            assert budget["warning_files_shown"] == selected
            assert budget["findings_shown"] == selected * 60
            for cause in causes:
                assert budget[cause] == 3
                assert budget[cause + "_shown"] == selected
            events = [d for d in details if d["threshold"] == threshold]
            hints = [d for d in events if d["event"] == "doc_lint_hint"]
            assert len(hints) == (50 if selected else 0)
            overshoots = [d["words"] - d["budget"] for d in hints]
            assert overshoots == sorted(overshoots, reverse=True)
            truncated = [d for d in events if d["event"] == "doc_lint_truncated"]
            assert [d["remaining"] for d in truncated] == ([selected * 60 - 50] if selected else [])
            if events:
                for mutation in (dict(events[0], version=2), dict(events[0], extra=0), dict(events[0], threshold="bogus"), dict(events[0], event="bogus"), dict(events[0], words=-1), dict(events[0], path=0), dict(events[0], item=False), dict(events[0], fail_closed=0)):
                    try:
                        validate(mutation)
                    except AssertionError:
                        pass
                    else:
                        raise AssertionError("detail validator accepted mutation")
    for file in root.glob("*.rs"):
        file.unlink()
    (root / "a.rs").write_text("fn clean() {}")
    (root / "b.rs").write_text("fn invalid( {")
    (root / "c.rs").write_text(doc(121))
    for cap in ("0", "1", "unlimited"):
        code, details, summary = run(root, cap=cap)
        assert code == 2 and summary["files"] == 3 and summary["errors"] == 1
        for threshold in ("advisory", "enforced"):
            assert summary[threshold]["warning_files"] == 1
            assert summary[threshold]["warning_files_shown"] == int(cap != "0")
        assert all(d.get("path", str(root / "c.rs")) == str(root / "c.rs") for d in details)
print("policy schema/differential/IO/compiling plant acceptance passed")

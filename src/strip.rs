use crate::RewriteCounts;
use ra_ap_rustc_lexer::{FrontmatterAllowed, TokenKind, tokenize};
use similar::{ChangeTag, TextDiff};
use std::fmt::Write as _;
use std::path::Path;
/// Strip non-doc line and block comments from `src` using
/// [`ra_ap_rustc_lexer`], including adjacent-whitespace trimming and
/// blank-line scar collapsing. Delegates to [`strip_line_comments_with_counts`]
/// and discards the [`RewriteCounts`] tally; see its stripping algorithm.
/// Other bytes are preserved, including doc comments and string literals.
#[must_use]
pub fn strip_line_comments(src: &str) -> String {
    strip_line_comments_with_counts(src).0
}
/// Like [`strip_line_comments`] but also returns a [`RewriteCounts`]
/// tally. `doc_links_rewritten` stays 0; [`process_file`](crate::process_file) fills it.
///
/// Drops a token iff it is a `LineComment` or `BlockComment` with
/// `doc_style: None`. Comment-looking text inside a string literal is
/// structurally unreachable here and round-trips byte-identical.
///
/// A solo-line drop collapses its trailing blank-line scar; an inline
/// (post-code) drop trims the preceding horizontal whitespace instead.
/// A contiguous run of solo-line drops with blanks on both sides emits
/// `max(blanks_above, blanks_below)` rather than their sum.
#[must_use]
pub fn strip_line_comments_with_counts(src: &str) -> (String, RewriteCounts) {
    let mut out = String::with_capacity(src.len());
    let mut counts = RewriteCounts::default();
    let mut cursor = 0usize;
    let mut pending_blank_collapse = false;
    let mut drop_run: Option<DropRun> = None;
    for token in tokenize(src, FrontmatterAllowed::Yes) {
        let end = cursor + token.len as usize;
        let text = &src[cursor..end];
        cursor = end;
        let is_comment = matches!(
            token.kind,
            TokenKind::LineComment { .. } | TokenKind::BlockComment { .. }
        );
        let drop = matches!(
            token.kind,
            TokenKind::LineComment { doc_style: None }
                | TokenKind::BlockComment {
                    doc_style: None,
                    ..
                }
        );
        if drop {
            let before_comment = &src[..end - text.len()];
            let was_line_alone = line_was_blank_before(before_comment);
            let trimmed_horizontal = trim_trailing_whitespace_to_last_newline(&mut out);
            counts.comments_removed = counts.comments_removed.saturating_add(1);
            if !was_line_alone && trimmed_horizontal > 0 {
                counts.inline_trimmed = counts.inline_trimmed.saturating_add(1);
            }
            if was_line_alone {
                if drop_run.is_none() {
                    drop_run = Some(DropRun {
                        blanks_above: trailing_blank_lines(&out),
                        blanks_below_emitted: 0,
                    });
                }
                pending_blank_collapse = true;
            }
            continue;
        }
        if pending_blank_collapse && matches!(token.kind, TokenKind::Whitespace) {
            let trimmed = text.strip_prefix('\n').unwrap_or(text);
            if let Some(run) = drop_run.as_mut() {
                run.blanks_below_emitted += count_newlines(trimmed);
            }
            out.push_str(trimmed);
            pending_blank_collapse = false;
        } else {
            if !matches!(token.kind, TokenKind::Whitespace)
                && let Some(run) = drop_run.take()
                && run.blanks_above >= 1
                && run.blanks_below_emitted >= 1
            {
                pop_one_trailing_newline(&mut out);
                counts.blank_lines_collapsed = counts.blank_lines_collapsed.saturating_add(1);
            }
            out.push_str(text);
            if !matches!(token.kind, TokenKind::Whitespace) || !is_comment {
                pending_blank_collapse = false;
            }
        }
    }
    if let Some(run) = drop_run
        && run.blanks_above >= 1
        && run.blanks_below_emitted >= 1
    {
        pop_one_trailing_newline(&mut out);
        counts.blank_lines_collapsed = counts.blank_lines_collapsed.saturating_add(1);
    }
    (out, counts)
}
struct DropRun {
    blanks_above: usize,
    blanks_below_emitted: usize,
}
fn trailing_blank_lines(s: &str) -> usize {
    let trailing = s.bytes().rev().take_while(|b| *b == b'\n').count();
    trailing.saturating_sub(1)
}
fn count_newlines(s: &str) -> usize {
    s.bytes().filter(|b| *b == b'\n').count()
}
fn pop_one_trailing_newline(s: &mut String) {
    let bytes = s.as_bytes();
    let mut indent_start = bytes.len();
    while indent_start > 0 {
        match bytes[indent_start - 1] {
            b' ' | b'\t' => indent_start -= 1,
            _ => break,
        }
    }
    if indent_start == 0 || bytes[indent_start - 1] != b'\n' {
        return;
    }
    let newline_pos = indent_start - 1;
    s.remove(newline_pos);
}
fn line_was_blank_before(prefix: &str) -> bool {
    let line_start = prefix.rfind('\n').map_or(0, |p| p + 1);
    prefix[line_start..].chars().all(|c| c == ' ' || c == '\t')
}
fn trim_trailing_whitespace_to_last_newline(s: &mut String) -> usize {
    let mut popped = 0;
    while matches!(s.chars().last(), Some(' ' | '\t')) {
        s.pop();
        popped += 1;
    }
    popped
}
fn push_single_line(out: &mut String, path: &Path) {
    push_single_line_str(out, &path.display().to_string());
}
fn push_single_line_str(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                write!(out, "\\u{:04x}", c as u32).expect("Write for String never fails");
            }
            c => out.push(c),
        }
    }
}
/// Collapse `text` to a single line for a human-facing stream.
///
/// Escapes LF, CR, TAB, every other C0 code point and DEL, so rendered
/// prose can never introduce a column-zero line a record consumer would
/// mistake for a JSON Lines record.
#[must_use]
pub fn single_line(text: &str) -> String {
    let mut out = String::new();
    push_single_line_str(&mut out, text);
    out
}
#[must_use]
pub(crate) fn unified_diff(path: &Path, original: &str, rewritten: &str, context: usize) -> String {
    let diff = TextDiff::from_lines(original, rewritten);
    let mut out = String::new();
    out.push_str("--- a/");
    push_single_line(&mut out, path);
    out.push('\n');
    out.push_str("+++ b/");
    push_single_line(&mut out, path);
    out.push('\n');
    for hunk in diff.unified_diff().context_radius(context).iter_hunks() {
        writeln!(out, "{}", hunk.header()).expect("Write for String never fails");
        for change in hunk.iter_changes() {
            let sign = match change.tag() {
                ChangeTag::Equal => ' ',
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
            };
            let value = change.value();
            out.push(sign);
            out.push_str(value);
            if !value.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

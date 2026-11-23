const fn leading_columns(line: &str) -> (usize, usize) {
    let bytes = line.as_bytes();
    let mut col = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b' ' => col += 1,
            b'\t' => col = (col / 4 + 1) * 4,
            _ => return (col, i),
        }
        i += 1;
    }
    (col, i)
}
fn fence_delimiter_run(line: &str) -> Option<(u8, usize, &str)> {
    let (col, offset) = leading_columns(line);
    if col > 3 {
        return None;
    }
    let rest = &line[offset..];
    let marker = *rest.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let run = rest.bytes().take_while(|b| *b == marker).count();
    if run < 3 {
        return None;
    }
    Some((marker, run, &rest[run..]))
}
pub(crate) fn opens_or_closes_fence(line: &str) -> bool {
    fence_delimiter_run(line).is_some()
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FenceState {
    Closed,
    Open { marker: u8, run: usize },
}
impl FenceState {
    fn advance(self, line: &str) -> (Self, bool) {
        match (self, fence_delimiter_run(line)) {
            (Self::Closed, Some((marker, run, info))) if marker == b'~' || !info.contains('`') => {
                (Self::Open { marker, run }, true)
            }
            (Self::Open { marker, run }, Some((found, found_run, tail)))
                if found == marker && found_run >= run && tail.trim().is_empty() =>
            {
                (Self::Closed, true)
            }
            (state, _) => (state, false),
        }
    }
    const fn is_open(self) -> bool {
        matches!(self, Self::Open { .. })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineFrame {
    Code,
    Definition,
    Prose,
}
pub(crate) struct BlockScan {
    fence: FenceState,
    indented_code: bool,
    block_start: bool,
}
impl BlockScan {
    pub(crate) const fn new() -> Self {
        Self {
            fence: FenceState::Closed,
            indented_code: false,
            block_start: true,
        }
    }
    pub(crate) const fn fence_is_open(&self) -> bool {
        self.fence.is_open()
    }
    pub(crate) fn classify(&mut self, line: &str) -> LineFrame {
        if self.fence.is_open() {
            let (next, _) = self.fence.advance(line);
            self.fence = next;
            self.block_start = !next.is_open();
            return LineFrame::Code;
        }
        let blank = line.trim().is_empty();
        let (col, _) = leading_columns(line);
        if self.indented_code {
            if blank || col >= 4 {
                return LineFrame::Code;
            }
            self.indented_code = false;
            self.block_start = true;
        }
        if blank {
            self.block_start = true;
            return LineFrame::Prose;
        }
        let (next, delimiter) = self.fence.advance(line);
        if delimiter {
            self.fence = next;
            self.block_start = false;
            return LineFrame::Code;
        }
        if self.block_start && col >= 4 {
            self.indented_code = true;
            return LineFrame::Code;
        }
        if self.block_start && is_reference_definition(line) {
            return LineFrame::Definition;
        }
        self.block_start = false;
        LineFrame::Prose
    }
}
pub(crate) fn reference_definition_label(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('[') {
        return None;
    }
    let close = find_matching_bracket(trimmed, 0)?;
    if !trimmed[close + 1..].starts_with(':') {
        return None;
    }
    Some(&trimmed[1..close])
}
fn is_reference_definition(line: &str) -> bool {
    reference_definition_label(line).is_some()
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeSpanState {
    Closed,
    Open { run: usize },
}
impl CodeSpanState {
    pub(crate) const fn is_open(self) -> bool {
        matches!(self, Self::Open { .. })
    }
}
pub(crate) fn span_after_run(
    span: CodeSpanState,
    run: usize,
    rest: &str,
    following: &[&str],
) -> CodeSpanState {
    match span {
        CodeSpanState::Open { run: open } if open == run => CodeSpanState::Closed,
        open @ CodeSpanState::Open { .. } => open,
        CodeSpanState::Closed
            if line_has_backtick_run(rest, run) || closing_run_follows(following, run) =>
        {
            CodeSpanState::Open { run }
        }
        CodeSpanState::Closed => CodeSpanState::Closed,
    }
}
pub(crate) fn scan_code_spans(
    line: &str,
    following: &[&str],
    entry: CodeSpanState,
    mut outside: impl FnMut(&str),
) -> CodeSpanState {
    let bytes = line.as_bytes();
    let mut span = entry;
    let mut i = 0;
    let mut plain = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            if !span.is_open() {
                outside(&line[plain..i]);
            }
            let run = backtick_run_len(bytes, i);
            span = span_after_run(span, run, &line[i + run..], following);
            i += run;
            plain = i;
            continue;
        }
        i += 1;
    }
    if !span.is_open() {
        outside(&line[plain..]);
    }
    span
}
pub(crate) fn advance_span(line: &str, following: &[&str], entry: CodeSpanState) -> CodeSpanState {
    scan_code_spans(line, following, entry, |_| {})
}
fn closing_run_follows(following: &[&str], run: usize) -> bool {
    for line in following {
        if line.trim().is_empty() || FenceState::Closed.advance(line).1 {
            return false;
        }
        if line_has_backtick_run(line, run) {
            return true;
        }
    }
    false
}
const fn line_has_backtick_run(line: &str, run: usize) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let found = backtick_run_len(bytes, i);
            if found == run {
                return true;
            }
            i += found;
            continue;
        }
        i += 1;
    }
    false
}
pub(crate) const fn backtick_run_len(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end] == b'`' {
        end += 1;
    }
    end - start
}
pub(crate) const fn find_matching_bracket(line: &str, open: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b']' => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

use unicode_general_category::{GeneralCategory, get_general_category};

use crate::{Span, Text};

// Non-ambiguous CJK bases from the pinned markdown-it-cjk-friendly width and
// emoji rules. Ambiguous quotation marks with variation selectors are handled separately.
const CJK: &[(u32, u32)] = &[
    (0x1100, 0x11ff),
    (0x20a9, 0x20a9),
    (0x2329, 0x232a),
    (0x2630, 0x2637),
    (0x268a, 0x268f),
    (0x2e80, 0x2e99),
    (0x2e9b, 0x2ef3),
    (0x2f00, 0x2fd5),
    (0x2ff0, 0x303e),
    (0x3041, 0x3096),
    (0x3099, 0x30ff),
    (0x3105, 0x312f),
    (0x3131, 0x318e),
    (0x3190, 0x31e5),
    (0x31ef, 0x321e),
    (0x3220, 0x3247),
    (0x3250, 0xa48c),
    (0xa490, 0xa4c6),
    (0xa960, 0xa97c),
    (0xac00, 0xd7a3),
    (0xd7b0, 0xd7c6),
    (0xd7cb, 0xd7fb),
    (0xf900, 0xfaff),
    (0xfe10, 0xfe19),
    (0xfe30, 0xfe52),
    (0xfe54, 0xfe66),
    (0xfe68, 0xfe6b),
    (0xff01, 0xffbe),
    (0xffc2, 0xffc7),
    (0xffca, 0xffcf),
    (0xffd2, 0xffd7),
    (0xffda, 0xffdc),
    (0xffe0, 0xffe6),
    (0xffe8, 0xffee),
    (0x16fe0, 0x16fe4),
    (0x16ff0, 0x16ff6),
    (0x17000, 0x18cda),
    (0x18cff, 0x18d20),
    (0x18d80, 0x18df2),
    (0x18e00, 0x19191),
    (0x191a0, 0x191d2),
    (0x1aff0, 0x1aff3),
    (0x1aff5, 0x1affb),
    (0x1affd, 0x1affe),
    (0x1b000, 0x1b128),
    (0x1b132, 0x1b132),
    (0x1b150, 0x1b152),
    (0x1b155, 0x1b155),
    (0x1b164, 0x1b168),
    (0x1b170, 0x1b2fb),
    (0x1d300, 0x1d356),
    (0x1d360, 0x1d376),
    (0x1f1ae, 0x1f1ae),
    (0x1f200, 0x1f200),
    (0x1f202, 0x1f202),
    (0x1f210, 0x1f219),
    (0x1f21b, 0x1f22e),
    (0x1f230, 0x1f231),
    (0x1f237, 0x1f237),
    (0x1f23b, 0x1f23b),
    (0x1f240, 0x1f248),
    (0x1f260, 0x1f265),
    (0x1f6d8, 0x1f6d9),
    (0x1f7da, 0x1f7da),
    (0x1fa8a, 0x1fa8e),
    (0x1fac8, 0x1fac8),
    (0x1facc, 0x1facd),
    (0x1fadd, 0x1fadd),
    (0x1faea, 0x1faeb),
    (0x1faef, 0x1faef),
    (0x1faf9, 0x1fafa),
    (0x20000, 0x2fffd),
    (0x30000, 0x3fffd),
];

pub(super) fn is_cjk(ch: char) -> bool {
    let code = u32::from(ch);
    code >= 0x1100
        && CJK
            .get(CJK.partition_point(|&(_, end)| end < code))
            .is_some_and(|&(start, _)| code >= start)
}

pub(super) fn preceding(source: &str, last: char) -> (char, bool) {
    if ('\u{fe00}'..='\u{fe0e}').contains(&last) {
        let base = source.chars().rev().nth(1).unwrap_or(' ');
        let main = if get_general_category(base) == GeneralCategory::SpaceSeparator {
            last
        } else {
            base
        };
        let cjk = is_cjk(base) || last == '\u{fe01}' && matches!(base, '‘' | '’' | '“' | '”');
        (main, cjk)
    } else {
        (
            last,
            is_cjk(last) || ('\u{e0100}'..='\u{e01ef}').contains(&last),
        )
    }
}

fn is_han(ch: char) -> bool {
    matches!(ch, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{20000}'..='\u{323af}')
}

fn han_number_suffix(prefix: &str) -> bool {
    let mut chars = prefix.chars().rev();
    if !chars.next().is_some_and(is_han) {
        return false;
    }
    for _ in 0..2 {
        match chars.next() {
            Some(ch) if ch.is_ascii_digit() => return true,
            Some(ch) if is_han(ch) => {}
            _ => return false,
        }
    }
    chars.next().is_some_and(|ch| ch.is_ascii_digit())
}

pub(super) fn script_pair(source: &str, start: usize, end: usize) -> Option<(usize, Text)> {
    let bytes = source.as_bytes();
    let marker = bytes[start];
    let body_start = start + 1;
    if marker == b'~'
        && start > 0
        && bytes[start - 1].is_ascii_digit()
        && bytes.get(body_start).is_some_and(u8::is_ascii_digit)
    {
        return None;
    }
    let mut pos = body_start;
    let mut unescaped = None;
    let mut copied = body_start;
    let mut backticks = None;
    while pos < end {
        let ch = source[pos..end].chars().next().unwrap();
        if ch == char::from(marker) {
            break;
        }
        if ch.is_whitespace() {
            return None;
        }
        if ch == '\\'
            && pos + 1 < end
            && (bytes[pos + 1].is_ascii_punctuation() || bytes[pos + 1] == b' ')
        {
            let output = unescaped.get_or_insert_with(String::new);
            output.push_str(&source[copied..pos]);
            output.push(char::from(bytes[pos + 1]));
            pos += 2;
            copied = pos;
            continue;
        }
        if ch == '`' {
            let body = super::run_end(bytes, pos, end);
            if let Some(close) = super::end_code_span(bytes, body, end, body - pos, &mut backticks)
            {
                let stop = close + body - pos;
                if source[pos..stop].chars().any(char::is_whitespace) {
                    return None;
                }
                pos = stop;
                continue;
            }
            pos = body;
            continue;
        }
        pos += ch.len_utf8();
    }
    if pos == end || pos == body_start {
        return None;
    }
    if marker == b'~'
        && bytes[body_start].is_ascii_digit()
        && bytes.get(pos + 1).is_some_and(u8::is_ascii_digit)
        && (bytes[pos - 1].is_ascii_digit()
            || han_number_suffix(&source[..start])
            || source[body_start..pos].contains([
                ',', '.', '!', '?', ';', ':', '，', '。', '；', '、', '！', '？', '：',
            ]))
    {
        return None;
    }
    let text = if let Some(mut output) = unescaped {
        output.push_str(&source[copied..pos]);
        Text::Decoded(output.into_boxed_str())
    } else {
        Text::Source(Span::new(body_start, pos))
    };
    Some((pos + 1, text))
}

enum Part<'a> {
    Text(&'a mut Text, bool),
    Opaque(&'a Text),
    Raw(Span),
    Break,
    Boundary,
}

impl Part<'_> {
    fn as_str<'a>(&'a self, source: &'a str) -> &'a str {
        match self {
            Self::Text(text, _) => text.as_str(source),
            Self::Opaque(text) => text.as_str(source),
            Self::Raw(span) => &source[span.start..span.end],
            Self::Break => " ",
            Self::Boundary => "",
        }
    }
}

fn collect_parts<'a>(
    nodes: &'a mut [super::Inline],
    level: usize,
    parts: &mut Vec<(usize, Part<'a>)>,
) {
    use super::InlineKind;

    for node in nodes {
        let part = match &mut node.kind {
            InlineKind::Text(text @ Text::Decoded(_)) => Part::Opaque(text),
            InlineKind::Text(text) => {
                let Text::Source(source) = text else {
                    unreachable!()
                };
                let escaped_first = source.start != node.span.start;
                Part::Text(text, escaped_first)
            }
            InlineKind::Code(text)
            | InlineKind::Math { body: text, .. }
            | InlineKind::Subscript(text)
            | InlineKind::Superscript(text) => Part::Opaque(text),
            InlineKind::SoftBreak | InlineKind::HardBreak => Part::Break,
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children)
            | InlineKind::Highlight(children)
            | InlineKind::Insert(children) => {
                collect_parts(children, level + 1, parts);
                Part::Boundary
            }
            InlineKind::Link(link) => {
                collect_parts(&mut link.children, level + 1, parts);
                Part::Boundary
            }
            _ => Part::Raw(node.span),
        };
        parts.push((level, part));
    }
}

pub(super) fn needs_quotes(text: &str) -> bool {
    memchr::memchr2(b'\'', b'"', text.as_bytes()).is_some()
}

pub(super) fn smart_quotes(source: &str, nodes: &mut [super::Inline]) {
    let mut parts = Vec::new();
    collect_parts(nodes, 0, &mut parts);
    let mut stack: Vec<(usize, usize, usize, bool)> = Vec::new();
    let mut edits = Vec::new();

    for (i, (level, part)) in parts.iter().enumerate() {
        while stack
            .last()
            .is_some_and(|&(quote_level, ..)| quote_level > *level)
        {
            stack.pop();
        }
        let Part::Text(text, escaped_first) = part else {
            continue;
        };
        let text = text.as_str(source);
        for pos in memchr::memchr2_iter(b'\'', b'"', text.as_bytes()) {
            if pos == 0 && *escaped_first {
                continue;
            }
            let single = text.as_bytes()[pos] == b'\'';
            let last = text[..pos]
                .chars()
                .next_back()
                .or_else(|| {
                    parts[..i]
                        .iter()
                        .rev()
                        .find_map(|(_, p)| p.as_str(source).chars().next_back())
                })
                .unwrap_or(' ');
            let next = text[pos + 1..]
                .chars()
                .next()
                .or_else(|| {
                    parts[i + 1..]
                        .iter()
                        .find_map(|(_, p)| p.as_str(source).chars().next())
                })
                .unwrap_or(' ');
            let last_punct = super::is_punctuation(last);
            let next_punct = super::is_punctuation(next);
            let last_space = super::is_whitespace(last);
            let next_space = super::is_whitespace(next);
            let mut opens = !next_space && (!next_punct || last_space || last_punct);
            let mut closes = !last_space && (!last_punct || next_space || next_punct);
            if !single && next == '"' && last.is_ascii_digit() {
                opens = false;
                closes = false;
            }
            if opens && closes {
                opens = last_punct;
                closes = next_punct;
            }
            if closes
                && let Some(index) = stack.iter().rposition(|&(quote_level, _, _, is_single)| {
                    quote_level == *level && is_single == single
                })
            {
                let (_, opener, at, _) = stack[index];
                stack.truncate(index);
                edits.push((opener, at, if single { '‘' } else { '“' }));
                edits.push((i, pos, if single { '’' } else { '”' }));
            } else if opens {
                stack.push((*level, i, pos, single));
            } else if single {
                edits.push((i, pos, '’'));
            }
        }
    }

    edits.sort_unstable_by_key(|&(part, pos, _)| (part, pos));
    let mut edits = edits.into_iter().peekable();
    for (i, (_, part)) in parts.into_iter().enumerate() {
        if !edits.peek().is_some_and(|&(part, ..)| part == i) {
            continue;
        }
        let Part::Text(text, _) = part else {
            unreachable!()
        };
        let original = text.as_str(source);
        let mut rewritten = String::with_capacity(original.len() + 2);
        let mut cursor = 0;
        while let Some((_, pos, quote)) = edits.next_if(|&(part, ..)| part == i) {
            rewritten.push_str(&original[cursor..pos]);
            rewritten.push(quote);
            cursor = pos + 1;
        }
        rewritten.push_str(&original[cursor..]);
        *text = Text::Decoded(rewritten.into_boxed_str());
    }
}

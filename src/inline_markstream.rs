use unicode_general_category::{GeneralCategory, get_general_category};

use crate::{Span, Text};

// The inline `://` rule reads the preceding ASCII scheme, allowing a link
// to follow prose in scripts that do not put spaces between adjacent words.
pub(super) fn embedded_web_url(text: &str) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    for pos in memchr::memchr_iter(b':', bytes) {
        if !bytes[pos + 1..].starts_with(b"//") {
            continue;
        }
        let (offset, length) = if super::scheme_suffix::<true, 5>(&bytes[..pos], b"https") {
            (pos - 5, 8)
        } else if super::scheme_suffix::<true, 4>(&bytes[..pos], b"http") {
            (pos - 4, 7)
        } else {
            continue;
        };
        if !bytes[..offset]
            .last()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == b'+')
        {
            return Some((offset, length));
        }
    }
    None
}

pub(super) fn unicode_url_separator(ch: char) -> bool {
    !ch.is_ascii()
        && (ch == '｜'
            || matches!(
                get_general_category(ch),
                GeneralCategory::SpaceSeparator
                    | GeneralCategory::LineSeparator
                    | GeneralCategory::ParagraphSeparator
                    | GeneralCategory::ClosePunctuation
                    | GeneralCategory::ConnectorPunctuation
                    | GeneralCategory::DashPunctuation
                    | GeneralCategory::FinalPunctuation
                    | GeneralCategory::InitialPunctuation
                    | GeneralCategory::OpenPunctuation
                    | GeneralCategory::OtherPunctuation
            ))
}

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
    Text {
        text: &'a mut Text,
        escaped_first: bool,
        ellipsis: bool,
    },
    Opaque(&'a Text),
    Raw(Span),
    Break,
    Boundary,
}

impl Part<'_> {
    fn as_str<'a>(&'a self, source: &'a str) -> &'a str {
        match self {
            Self::Text { text, .. } => text.as_str(source),
            Self::Opaque(text) => text.as_str(source),
            Self::Raw(span) => &source[span.start..span.end],
            Self::Break => " ",
            Self::Boundary => "",
        }
    }
}

fn collect_parts<'a>(
    source: &str,
    nodes: &'a mut [super::Inline],
    level: usize,
    ellipsis: bool,
    parts: &mut Vec<(usize, Part<'a>)>,
) {
    use super::InlineKind;

    for node in nodes {
        let part = match &mut node.kind {
            InlineKind::Text(text @ Text::Decoded(_)) => Part::Opaque(text),
            InlineKind::Text(text) => {
                let Text::Source(span) = text else {
                    unreachable!()
                };
                let escaped_first = span.start != node.span.start;
                Part::Text {
                    text,
                    escaped_first,
                    ellipsis,
                }
            }
            InlineKind::Code(text)
            | InlineKind::CodePreview(text)
            | InlineKind::Math { body: text, .. }
            | InlineKind::Subscript(text)
            | InlineKind::Superscript(text) => Part::Opaque(text),
            InlineKind::SoftBreak | InlineKind::HardBreak => Part::Break,
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children)
            | InlineKind::Highlight(children)
            | InlineKind::Insert(children) => {
                collect_parts(source, children, level + 1, ellipsis, parts);
                Part::Boundary
            }
            InlineKind::Link(link) | InlineKind::LinkPreview(link) => {
                let autolink = matches!(&link.children[..], [child]
                    if child.span == node.span || source.as_bytes()[node.span.start] == b'<');
                collect_parts(
                    source,
                    &mut link.children,
                    level + 1,
                    ellipsis && !autolink,
                    parts,
                );
                Part::Boundary
            }
            _ => Part::Raw(node.span),
        };
        parts.push((level, part));
    }
}

pub(super) fn typography(source: &str, input: Span, nodes: &mut [super::Inline]) {
    let raw = &source[input.start..input.end];
    let ellipsis = memchr::memmem::find(raw.as_bytes(), b"..").is_some();
    let quotes = memchr::memchr2(b'\'', b'"', raw.as_bytes()).is_some();
    if !ellipsis && !quotes {
        return;
    }
    let mut parts = Vec::new();
    collect_parts(source, nodes, 0, ellipsis, &mut parts);
    if ellipsis {
        for (_, part) in &mut parts {
            if let Part::Text {
                text,
                escaped_first,
                ellipsis: true,
            } = part
                && let Some(rewritten) = replace_ellipsis(text.as_str(source), *escaped_first)
            {
                **text = Text::Decoded(rewritten.into_boxed_str());
            }
        }
    }
    if quotes {
        smart_quotes(source, parts);
    }
}

fn replace_ellipsis(original: &str, escaped_first: bool) -> Option<String> {
    let start = usize::from(escaped_first);
    let bytes = original.as_bytes();
    memchr::memmem::find(&bytes[start..], b"..")?;
    let mut rewritten = String::with_capacity(original.len() + 1);
    let mut cursor = 0;
    for pos in memchr::memchr_iter(b'.', &bytes[start..]).map(|pos| pos + start) {
        if pos < cursor || bytes.get(pos + 1) != Some(&b'.') {
            continue;
        }
        rewritten.push_str(&original[cursor..pos]);
        rewritten.push_str(if pos > start && matches!(bytes[pos - 1], b'?' | b'!') {
            ".."
        } else {
            "…"
        });
        cursor = pos + 2;
        while bytes.get(cursor) == Some(&b'.') {
            cursor += 1;
        }
    }
    rewritten.push_str(&original[cursor..]);
    Some(rewritten)
}

fn smart_quotes(source: &str, parts: Vec<(usize, Part<'_>)>) {
    let mut stack: Vec<(usize, usize, usize, bool)> = Vec::new();
    let mut edits = Vec::new();

    for (i, (level, part)) in parts.iter().enumerate() {
        while stack
            .last()
            .is_some_and(|&(quote_level, ..)| quote_level > *level)
        {
            stack.pop();
        }
        let Part::Text {
            text,
            escaped_first,
            ..
        } = part
        else {
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
        let Part::Text { text, .. } = part else {
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

pub(super) fn strip_preview_tail(source: &str, nodes: &mut Vec<super::Inline>, math: bool) {
    let Some(last) = nodes.last_mut() else { return };
    let super::InlineKind::Text(Text::Source(text)) = &mut last.kind else {
        return;
    };
    let bytes = source.as_bytes();
    let end = text.end;
    if end == text.start {
        return;
    }
    let marker = bytes[end - 1];
    let single_dollar = math && marker == b'$' && (end < 2 || bytes[end - 2] != b'$');
    if !single_dollar && !matches!(marker, b'*' | b'(' | b'\\') {
        return;
    }
    let mut stop = end - 1;
    if marker == b'*' {
        while stop > text.start && bytes[stop - 1] == b'*' {
            stop -= 1;
        }
    }
    if escaped(bytes, stop) {
        stop += 1;
    }
    if stop == text.start {
        nodes.pop();
    } else {
        text.end = stop;
        last.span.end = stop;
    }
}

fn escaped(bytes: &[u8], at: usize) -> bool {
    bytes[..at]
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count()
        % 2
        != 0
}

pub(super) fn escaped_code(source: &str, start: usize, end: usize) -> Option<(usize, Text)> {
    let bytes = source.as_bytes();
    let body = start + 2;
    if bytes.get(body) == Some(&b'`') {
        return None;
    }
    let line = memchr::memchr2(b'\n', b'\r', &bytes[body..end]).map_or(end, |offset| body + offset);
    for offset in memchr::memmem::find_iter(&bytes[body..line], b"\\`") {
        let close = body + offset;
        if !escaped(bytes, close) && close > body {
            return Some((close + 2, super::code_content(source, body, close)));
        }
    }
    (body < line).then(|| (line, Text::Source(Span::new(body, line))))
}

const HTML_TAGS: &[&str] = &[
    "a",
    "abbr",
    "area",
    "article",
    "aside",
    "b",
    "base",
    "bdi",
    "bdo",
    "blockquote",
    "br",
    "button",
    "cite",
    "code",
    "col",
    "data",
    "del",
    "details",
    "dfn",
    "div",
    "em",
    "embed",
    "figcaption",
    "figure",
    "font",
    "footer",
    "g",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "i",
    "img",
    "input",
    "ins",
    "kbd",
    "label",
    "li",
    "link",
    "main",
    "mark",
    "meta",
    "nav",
    "ol",
    "p",
    "param",
    "path",
    "pre",
    "q",
    "s",
    "samp",
    "section",
    "small",
    "source",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "svg",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "time",
    "tr",
    "track",
    "u",
    "ul",
    "var",
    "wbr",
];

fn name_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    left.bytes()
        .cmp(right.bytes().map(|byte| byte.to_ascii_lowercase()))
}

fn configured_html(name: &str, custom: &[String]) -> bool {
    HTML_TAGS
        .binary_search_by(|candidate| name_cmp(candidate, name))
        .is_ok()
        || custom.iter().any(|tag| tag.eq_ignore_ascii_case(name))
}

fn configured_html_prefix(name: &str, custom: &[String]) -> bool {
    let ascii_prefix = |tag: &str| {
        tag.get(..name.len())
            .is_some_and(|part| part.eq_ignore_ascii_case(name))
    };
    let index = HTML_TAGS.partition_point(|candidate| name_cmp(candidate, name).is_lt());
    HTML_TAGS.get(index).is_some_and(|tag| ascii_prefix(tag))
        || custom.iter().any(|tag| ascii_prefix(tag))
}

fn tag_name(value: &str) -> Option<(&str, bool, &str)> {
    let tail = value.strip_prefix('<')?;
    let (closing, tail) = tail
        .strip_prefix('/')
        .map_or((false, tail), |rest| (true, rest));
    if !tail.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let end = tail
        .bytes()
        .take_while(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
        })
        .count();
    Some((&tail[..end], closing, &tail[end..]))
}

fn tag_space(input: &str) -> &str {
    input.trim_start_matches(|ch: char| ch.is_ascii_whitespace())
}

pub(super) fn pending_html_tail(
    source: &str,
    leaf_start: usize,
    start: usize,
    end: usize,
    custom: &[String],
) -> bool {
    let raw = &source[start..end];
    if matches!(raw, "<" | "</") {
        return true;
    }
    let Some((name, closing, mut rest)) = tag_name(raw) else {
        return false;
    };
    let prefix = &source[leaf_start..start];
    let separated = start == leaf_start
        || source.as_bytes()[start - 1].is_ascii_whitespace()
        || prefix.ends_with(r"\n")
        || prefix.ends_with(r"\r");
    if rest.is_empty() {
        return separated || configured_html_prefix(name, custom);
    }
    if !separated && !configured_html(name, custom) {
        return false;
    }
    if closing {
        return tag_space(rest).is_empty();
    }
    if rest == "/" {
        return true;
    }
    if !rest.as_bytes()[0].is_ascii_whitespace() {
        return false;
    }
    loop {
        rest = tag_space(rest);
        if matches!(rest, "" | "/") {
            return true;
        }
        let mut bytes = rest.bytes();
        let Some(first) = bytes.next() else {
            unreachable!()
        };
        if !(first.is_ascii_alphabetic() || matches!(first, b':' | b'_')) {
            return false;
        }
        let name_end = 1 + bytes
            .take_while(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'-' | b'_')
            })
            .count();
        rest = &rest[name_end..];
        if let Some(value) = tag_space(rest).strip_prefix('=') {
            let value = tag_space(value);
            let Some(first) = value.as_bytes().first().copied() else {
                return true;
            };
            if matches!(first, b'\'' | b'"') {
                let Some(close) = memchr::memchr(first, &value.as_bytes()[1..]) else {
                    return true;
                };
                rest = &value[close + 2..];
            } else {
                let finish = value
                    .bytes()
                    .take_while(|byte| {
                        !byte.is_ascii_whitespace()
                            && !matches!(byte, b'\'' | b'"' | b'=' | b'<' | b'>' | b'`')
                    })
                    .count();
                if finish == 0 {
                    return false;
                }
                rest = &value[finish..];
            }
        }
        if !rest.is_empty() && !rest.as_bytes()[0].is_ascii_whitespace() {
            return rest == "/";
        }
    }
}

pub(super) fn normalize_html(source: &str, nodes: &mut [super::Inline], custom: &[String]) {
    use super::InlineKind;
    use std::{
        borrow::Cow,
        collections::{HashMap, HashSet},
    };

    let unnamed = |node: &super::Inline| {
        if !matches!(node.kind, InlineKind::Html) {
            return None;
        }
        let raw = &source[node.span.start..node.span.end];
        tag_name(raw).filter(|&(name, _, _)| !configured_html(name, custom))
    };
    let mut paired = HashSet::new();
    if nodes
        .iter()
        .any(|node| unnamed(node).is_some_and(|(_, closing, _)| closing))
    {
        let mut openings = HashMap::<Cow<str>, Vec<usize>>::new();
        for (index, node) in nodes.iter().enumerate() {
            let Some((name, closing, rest)) = unnamed(node) else {
                continue;
            };
            let key = if name.bytes().any(|byte| byte.is_ascii_uppercase()) {
                Cow::Owned(name.to_ascii_lowercase())
            } else {
                Cow::Borrowed(name)
            };
            if closing {
                if let Some(opening) = openings.get_mut(&key).and_then(Vec::pop) {
                    paired.insert(opening);
                    paired.insert(index);
                }
            } else if !rest.ends_with("/>") {
                openings.entry(key).or_default().push(index);
            }
        }
    }

    for (index, node) in nodes.iter_mut().enumerate() {
        if unnamed(node).is_some() && !paired.contains(&index) {
            node.kind = InlineKind::Text(Text::Source(node.span));
        }
        match &mut node.kind {
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children)
            | InlineKind::Highlight(children)
            | InlineKind::Insert(children) => normalize_html(source, children, custom),
            InlineKind::Link(link)
            | InlineKind::LinkPreview(link)
            | InlineKind::Image(link)
            | InlineKind::ImagePreview(link) => normalize_html(source, &mut link.children, custom),
            _ => {}
        }
    }
}

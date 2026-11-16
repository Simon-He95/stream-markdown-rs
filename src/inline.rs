use std::{borrow::Cow, collections::HashMap};

use unicode_casefold::UnicodeCaseFold;
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::{Options, References, Span, Syntax, Text};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inline {
    pub span: Span,
    pub kind: InlineKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineKind {
    Text(Text),
    Code(Text),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    Link(Box<Link>),
    Image(Box<Link>),
    Html,
    Extension(u32),
    SoftBreak,
    HardBreak,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub destination: Text,
    pub title: Option<Text>,
    pub children: Vec<Inline>,
}

#[derive(Clone, Copy)]
struct Delimiter {
    marker: u8,
    length_class: u8,
    opens: bool,
    closes: bool,
}

#[derive(Clone, Copy)]
enum TokenKind {
    Plain,
    Escape,
    Delimiter(Delimiter),
    SoftBreak,
    HardBreak,
    Html,
    Rich(usize),
}

#[derive(Clone, Copy)]
struct Token {
    span: Span,
    kind: TokenKind,
    next: usize,
}

impl Token {
    fn new(span: Span, kind: TokenKind) -> Self {
        Self {
            span,
            kind,
            next: usize::MAX,
        }
    }

    fn from_inline(
        span: Span,
        kind: InlineKind,
        delimiter: Option<Delimiter>,
        payloads: &mut Vec<Option<InlineKind>>,
    ) -> Self {
        let kind = match kind {
            InlineKind::Text(Text::Source(text)) if text == span => {
                delimiter.map_or(TokenKind::Plain, TokenKind::Delimiter)
            }
            InlineKind::Text(Text::Source(_)) => TokenKind::Escape,
            InlineKind::SoftBreak => TokenKind::SoftBreak,
            InlineKind::HardBreak => TokenKind::HardBreak,
            InlineKind::Html => TokenKind::Html,
            other => {
                let index = payloads.len();
                payloads.push(Some(other));
                TokenKind::Rich(index)
            }
        };
        Self::new(span, kind)
    }

    fn delimiter(self) -> Option<Delimiter> {
        if let TokenKind::Delimiter(value) = self.kind {
            Some(value)
        } else {
            None
        }
    }

    fn into_inline(self, payloads: &mut [Option<InlineKind>]) -> Inline {
        let kind = match self.kind {
            TokenKind::Plain | TokenKind::Delimiter(_) => InlineKind::Text(Text::Source(self.span)),
            TokenKind::Escape => {
                InlineKind::Text(Text::Source(span(self.span.start + 1, self.span.end)))
            }
            TokenKind::SoftBreak => InlineKind::SoftBreak,
            TokenKind::HardBreak => InlineKind::HardBreak,
            TokenKind::Html => InlineKind::Html,
            TokenKind::Rich(index) => payloads[index].take().unwrap(),
        };
        Inline {
            span: self.span,
            kind,
        }
    }
}

struct Bracket {
    token: usize,
    image: bool,
    link_generation: usize,
}

fn span(start: usize, end: usize) -> Span {
    Span { start, end }
}

fn original(start: usize, end: usize) -> Inline {
    Inline {
        span: span(start, end),
        kind: InlineKind::Text(Text::Source(span(start, end))),
    }
}

fn content(source: &str, span: Span) -> &str {
    &source[span.start..span.end]
}

pub(crate) fn normalize_label(label: &str) -> Cow<'_, str> {
    let mut previous_space = true;
    if label.bytes().all(|byte| {
        let canonical = if byte == b' ' {
            !previous_space
        } else {
            !byte.is_ascii_uppercase() && !byte.is_ascii_whitespace() && byte.is_ascii()
        };
        previous_space = byte == b' ';
        canonical
    }) && !previous_space
    {
        return Cow::Borrowed(label);
    }

    let mut result = String::with_capacity(label.len());
    for part in label
        .split([' ', '\t', '\r', '\n'])
        .filter(|part| !part.is_empty())
    {
        if !result.is_empty() {
            result.push(' ');
        }
        result.extend(part.case_fold());
    }
    Cow::Owned(result)
}

pub(crate) fn parse(
    source: &str,
    input: Span,
    options: &Options,
    refs: &References,
) -> Vec<Inline> {
    parse_inner(source, input, options, refs, None, None)
}

pub(crate) fn parse_tracked(
    source: &str,
    input: Span,
    options: &Options,
    refs: &References,
    unresolved: &mut Vec<String>,
) -> Vec<Inline> {
    parse_inner(source, input, options, refs, Some(unresolved), None)
}

pub(crate) fn parse_fragments(
    source: &str,
    fragments: &[Span],
    options: &Options,
    refs: &References,
    unresolved: Option<&mut Vec<String>>,
) -> Vec<Inline> {
    let mut nonempty = fragments
        .iter()
        .copied()
        .filter(|part| part.start < part.end);
    let Some(mut contiguous) = nonempty.next() else {
        return Vec::new();
    };
    if nonempty.all(|part| {
        let next = contiguous.end == part.start;
        contiguous.end = part.end;
        next
    }) {
        return parse_inner(source, contiguous, options, refs, unresolved, None);
    }

    let capacity = fragments.iter().map(|part| part.end - part.start).sum();
    let mut joined = String::with_capacity(capacity);
    let mut mapping: Vec<Fragment> = Vec::with_capacity(fragments.len());
    for part in fragments
        .iter()
        .copied()
        .filter(|part| part.start < part.end)
    {
        if let Some(previous) = mapping.last_mut()
            && previous.source.end == part.start
        {
            previous.source.end = part.end;
        } else {
            mapping.push(Fragment {
                local_start: joined.len(),
                source: part,
            });
        }
        joined.push_str(content(source, part));
    }

    let mut references = Vec::new();
    let mut nodes = parse_inner(
        &joined,
        span(0, joined.len()),
        options,
        refs,
        unresolved,
        Some((source, &mut references)),
    );
    references.sort_unstable();
    remap_nodes(&mut nodes, &joined, &mapping, &references);
    nodes
}

struct Fragment {
    local_start: usize,
    source: Span,
}

fn map_byte(offset: usize, parts: &[Fragment]) -> usize {
    let part = &parts[parts.partition_point(|part| part.local_start <= offset) - 1];
    part.source.start + offset - part.local_start
}

fn map_span(value: Span, parts: &[Fragment]) -> Span {
    let start = map_byte(value.start, parts);
    if value.start == value.end {
        span(start, start)
    } else {
        span(start, map_byte(value.end - 1, parts) + 1)
    }
}

fn remap_text(value: &mut Text, joined: &str, parts: &[Fragment]) {
    if let Text::Source(local) = *value {
        let original = map_span(local, parts);
        *value = if original.end - original.start == local.end - local.start {
            Text::Source(original)
        } else {
            Text::Decoded(content(joined, local).into())
        };
    }
}

fn remap_nodes(nodes: &mut [Inline], joined: &str, parts: &[Fragment], references: &[usize]) {
    for node in nodes {
        match &mut node.kind {
            InlineKind::Text(text) | InlineKind::Code(text) => remap_text(text, joined, parts),
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children) => {
                remap_nodes(children, joined, parts, references)
            }
            InlineKind::Link(link) | InlineKind::Image(link) => {
                if references.binary_search(&node.span.start).is_err() {
                    remap_text(&mut link.destination, joined, parts);
                    if let Some(title) = &mut link.title {
                        remap_text(title, joined, parts);
                    }
                }
                remap_nodes(&mut link.children, joined, parts, references);
            }
            _ => {}
        }
        node.span = map_span(node.span, parts);
    }
}

#[inline]
fn special(byte: u8, gfm: bool) -> bool {
    matches!(
        byte,
        b'\\' | b'`' | b'*' | b'_' | b'[' | b']' | b'!' | b'&' | b'<' | b'\n' | b'\r' | 0
    ) || (gfm && byte == b'~')
}

pub(crate) fn needs_markup(text: &str, syntax: Syntax) -> bool {
    let gfm = syntax == Syntax::Gfm;
    first_special(text.as_bytes(), gfm).is_some() || (gfm && has_gfm_trigger(text))
}

#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
const fn marker_table(gfm: bool) -> [u8; 16] {
    let mut table = [0; 16];
    let markers = b"\0\n\r!&*[\\]_`<";
    let mut i = 0;
    while i < markers.len() {
        let byte = markers[i];
        table[(byte & 15) as usize] |= 1 << (byte >> 4);
        i += 1;
    }
    if gfm {
        table[(b'~' & 15) as usize] |= 1 << (b'~' >> 4);
    }
    table
}

#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
fn first_special(bytes: &[u8], gfm: bool) -> Option<usize> {
    use std::arch::aarch64::{vandq_u8, vdupq_n_u8, vld1q_u8, vmaxvq_u8, vqtbl1q_u8, vshrq_n_u8};

    const COMMONMARK: [u8; 16] = marker_table(false);
    const GFM: [u8; 16] = marker_table(true);
    const HIGH_BITS: [u8; 16] = [1, 2, 4, 8, 16, 32, 64, 128, 0, 0, 0, 0, 0, 0, 0, 0];
    let (chunks, tail) = bytes.as_chunks::<16>();
    if !chunks.is_empty() {
        // The lookup tables and each exact chunk contain 16 readable bytes.
        unsafe {
            let table = vld1q_u8(if gfm {
                GFM.as_ptr()
            } else {
                COMMONMARK.as_ptr()
            });
            let high_bits = vld1q_u8(HIGH_BITS.as_ptr());
            let mask = vdupq_n_u8(15);
            for (index, chunk) in chunks.iter().enumerate() {
                let bytes = vld1q_u8(chunk.as_ptr());
                let low = vandq_u8(bytes, mask);
                let high = vshrq_n_u8::<4>(bytes);
                let candidates = vandq_u8(vqtbl1q_u8(table, low), vqtbl1q_u8(high_bits, high));
                if vmaxvq_u8(candidates) != 0 {
                    return chunk
                        .iter()
                        .position(|&byte| special(byte, gfm))
                        .map(|n| index * 16 + n);
                }
            }
        }
    }
    tail.iter()
        .position(|&byte| special(byte, gfm))
        .map(|n| bytes.len() - tail.len() + n)
}

#[cfg(not(all(target_arch = "aarch64", target_feature = "neon")))]
fn first_special(bytes: &[u8], gfm: bool) -> Option<usize> {
    bytes.iter().position(|&byte| special(byte, gfm))
}

fn parse_inner(
    source: &str,
    input: Span,
    options: &Options,
    refs: &References,
    mut unresolved: Option<&mut Vec<String>>,
    reference_origins: Option<(&str, &mut Vec<usize>)>,
) -> Vec<Inline> {
    let (reference_source, mut reference_offsets) = reference_origins
        .map_or((source, None), |(reference_source, offsets)| {
            (reference_source, Some(offsets))
        });
    let bytes = source.as_bytes();
    let raw = content(source, input);
    let leading = raw.trim_start_matches([' ', '\t', '\r', '\n']);
    let start = input.end - leading.len();
    let end = start + leading.trim_end_matches([' ', '\t', '\r', '\n']).len();
    let gfm = options.syntax == Syntax::Gfm;
    let gfm_autolink = gfm && has_gfm_trigger(&source[start..end]);
    let Some(first) = first_special(&bytes[start..end], gfm) else {
        return if start == end {
            Vec::new()
        } else if gfm_autolink {
            let mut output = Vec::new();
            append_gfm_text(source, start, end, &mut output);
            output
        } else {
            vec![original(start, end)]
        };
    };

    let mut tokens = Vec::new();
    let mut payloads = Vec::new();
    let mut brackets: Vec<Bracket> = Vec::new();
    let mut link_generation = 0;
    let mut backticks = None;
    push_original(&mut tokens, start, start + first);
    let mut i = start + first;
    while i < end {
        if !special(bytes[i], gfm) {
            let next = first_special(&bytes[i..end], gfm).map_or(end, |offset| i + offset);
            push_original(&mut tokens, i, next);
            i = next;
            continue;
        }
        let begin = i;
        let mut delimiter = None;
        let kind = match bytes[i] {
            b'\\' if i + 1 < end && bytes[i + 1].is_ascii_punctuation() => {
                i += 2;
                InlineKind::Text(Text::Source(span(begin + 1, i)))
            }
            b'\\' if i + 1 < end && matches!(bytes[i + 1], b'\n' | b'\r') => {
                i = break_end(bytes, i + 1, end);
                InlineKind::HardBreak
            }
            b'\n' | b'\r' => {
                let trim = source[start..i].trim_end_matches([' ', '\t']).len() + start;
                let hard = i - trim >= 2 && bytes[i - 2..i] == *b"  ";
                if let Some(last) = tokens.last_mut()
                    && last.span.end > trim
                    && matches!(last.kind, TokenKind::Plain)
                {
                    last.span.end = trim;
                }
                i = break_end(bytes, i, end);
                if hard {
                    InlineKind::HardBreak
                } else {
                    InlineKind::SoftBreak
                }
            }
            0 => {
                i += 1;
                InlineKind::Text(Text::Decoded("�".into()))
            }
            b'&' => {
                if let Some((end, decoded)) = entity(source, i, end) {
                    i = end;
                    InlineKind::Text(Text::Decoded(decoded))
                } else {
                    i += 1;
                    InlineKind::Text(Text::Source(span(begin, i)))
                }
            }
            b'`' => {
                let n = run_end(bytes, i, end);
                if let Some(close) = end_code_span(bytes, n, end, n - begin, &mut backticks) {
                    i = close + n - begin;
                    InlineKind::Code(code_content(source, n, close))
                } else {
                    i = n;
                    InlineKind::Text(Text::Source(span(begin, i)))
                }
            }
            b'*' | b'_' | b'~' if bytes[i] != b'~' || gfm => {
                i = run_end(bytes, i, end);
                if bytes[begin] != b'~' || i - begin <= 2 {
                    delimiter = Some(flanking(source, start, begin, i, end));
                }
                InlineKind::Text(Text::Source(span(begin, i)))
            }
            b'[' | b'!' if bytes[i] == b'[' || (i + 1 < end && bytes[i + 1] == b'[') => {
                let image = bytes[i] == b'!';
                i += if image { 2 } else { 1 };
                brackets.push(Bracket {
                    token: tokens.len(),
                    image,
                    link_generation,
                });
                InlineKind::Text(Text::Source(span(begin, i)))
            }
            b']' => {
                i += 1;
                if let Some(bracket) = brackets.pop()
                    && (bracket.image || bracket.link_generation == link_generation)
                {
                    let open = tokens[bracket.token].span;
                    if let Some(target) = link_target(
                        source,
                        span(open.end, begin),
                        i,
                        end,
                        refs,
                        unresolved.as_deref_mut(),
                    ) {
                        let child_start = bracket.token + 1;
                        let linkify_image = gfm_autolink && bracket.image;
                        let children = if tokens.len() == child_start + 1 && !linkify_image {
                            singleton(tokens.pop().unwrap().into_inline(&mut payloads))
                        } else {
                            finish_tokens(
                                source,
                                tokens.split_off(child_start),
                                &mut payloads,
                                linkify_image,
                            )
                        };
                        tokens.pop();
                        let target_source = if target.is_reference {
                            if let Some(offsets) = &mut reference_offsets {
                                offsets.push(open.start);
                            }
                            reference_source
                        } else {
                            source
                        };
                        let link = Box::new(Link {
                            destination: decode_span(target_source, target.destination),
                            title: target.title.map(|value| decode_span(target_source, value)),
                            children,
                        });
                        let kind = if bracket.image {
                            InlineKind::Image(link)
                        } else {
                            link_generation += 1;
                            InlineKind::Link(link)
                        };
                        tokens.push(Token::from_inline(
                            span(open.start, target.end),
                            kind,
                            None,
                            &mut payloads,
                        ));
                        i = target.end;
                        continue;
                    }
                }
                InlineKind::Text(Text::Source(span(begin, i)))
            }
            b'<' => {
                if let Some((finish, email)) = autolink_end(source, i, end) {
                    let label = span(i + 1, finish - 1);
                    i = finish;
                    link_generation += 1;
                    InlineKind::Link(Box::new(Link {
                        destination: if email {
                            Text::Decoded(
                                format!("mailto:{}", content(source, label)).into_boxed_str(),
                            )
                        } else {
                            Text::Source(label)
                        },
                        title: None,
                        children: vec![original(label.start, label.end)],
                    }))
                } else if let Some(finish) = html_end(source, i, end) {
                    i = finish;
                    InlineKind::Html
                } else {
                    i += 1;
                    InlineKind::Text(Text::Source(span(begin, i)))
                }
            }
            _ => {
                i += 1;
                InlineKind::Text(Text::Source(span(begin, i)))
            }
        };
        tokens.push(Token::from_inline(
            span(begin, i),
            kind,
            delimiter,
            &mut payloads,
        ));
        let next = first_special(&bytes[i..end], gfm).map_or(end, |offset| i + offset);
        push_original(&mut tokens, i, next);
        i = next;
    }

    finish_tokens(source, tokens, &mut payloads, gfm_autolink)
}

fn push_original(tokens: &mut Vec<Token>, start: usize, end: usize) {
    if start < end {
        tokens.push(Token::new(span(start, end), TokenKind::Plain));
    }
}

fn break_end(bytes: &[u8], mut i: usize, end: usize) -> usize {
    i += if bytes[i] == b'\r' && i + 1 < end && bytes[i + 1] == b'\n' {
        2
    } else {
        1
    };
    while i < end && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    i
}

fn run_end(bytes: &[u8], mut i: usize, end: usize) -> usize {
    let marker = bytes[i];
    i += 1;
    while i < end && bytes[i] == marker {
        i += 1;
    }
    i
}

fn backtick_runs(bytes: &[u8], mut i: usize, end: usize) -> impl Iterator<Item = (usize, usize)> {
    std::iter::from_fn(move || {
        let start = i + memchr::memchr(b'`', &bytes[i..end])?;
        i = run_end(bytes, start, end);
        Some((start, i - start))
    })
}

fn end_code_span(
    bytes: &[u8],
    start: usize,
    end: usize,
    count: usize,
    index: &mut Option<HashMap<usize, Vec<usize>>>,
) -> Option<usize> {
    if let Some(index) = index {
        let positions = index.get(&count)?;
        return positions
            .get(positions.partition_point(|position| *position < start))
            .copied();
    }

    if let Some((start, _)) = backtick_runs(bytes, start, end).find(|(_, len)| *len == count) {
        return Some(start);
    }

    // A failed lookup reached the end. Index that suffix once for all later unmatched runs.
    let mut positions = HashMap::<usize, Vec<usize>>::new();
    for (start, len) in backtick_runs(bytes, start, end) {
        positions.entry(len).or_default().push(start);
    }
    *index = Some(positions);
    None
}

fn code_content(source: &str, mut start: usize, mut end: usize) -> Text {
    let raw = &source.as_bytes()[start..end];
    let contains_line_end = raw.iter().any(|b| matches!(b, b'\r' | b'\n'));
    let first_space = raw
        .first()
        .is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n'));
    let last_space = raw
        .last()
        .is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n'));
    if first_space && last_space && raw.iter().any(|b| !matches!(b, b' ' | b'\r' | b'\n')) {
        start += if raw.starts_with(b"\r\n") { 2 } else { 1 };
        end -= if raw.ends_with(b"\r\n") { 2 } else { 1 };
    }
    let slice = &source[start..end];
    if contains_line_end || slice.as_bytes().contains(&0) {
        let mut decoded = String::with_capacity(slice.len());
        let mut chars = slice.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    decoded.push(' ');
                }
                '\n' => decoded.push(' '),
                '\0' => decoded.push('�'),
                ch => decoded.push(ch),
            }
        }
        Text::Decoded(decoded.into_boxed_str())
    } else {
        Text::Source(span(start, end))
    }
}

fn is_whitespace(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\x0c' | '\r' | ' ')
        || (!ch.is_ascii() && get_general_category(ch) == GeneralCategory::SpaceSeparator)
}

fn is_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation()
        || (!ch.is_ascii()
            && matches!(
                get_general_category(ch),
                GeneralCategory::ClosePunctuation
                    | GeneralCategory::ConnectorPunctuation
                    | GeneralCategory::DashPunctuation
                    | GeneralCategory::FinalPunctuation
                    | GeneralCategory::InitialPunctuation
                    | GeneralCategory::OpenPunctuation
                    | GeneralCategory::OtherPunctuation
                    | GeneralCategory::CurrencySymbol
                    | GeneralCategory::MathSymbol
                    | GeneralCategory::ModifierSymbol
                    | GeneralCategory::OtherSymbol
            ))
}

fn flanking(source: &str, start: usize, begin: usize, next: usize, end: usize) -> Delimiter {
    let before = source[start..begin].chars().next_back().unwrap_or(' ');
    let after = source[next..end].chars().next().unwrap_or(' ');
    let before_ws = is_whitespace(before);
    let after_ws = is_whitespace(after);
    let (left, right, before_p, after_p) = if before_ws || after_ws {
        (!after_ws, !before_ws, false, false)
    } else {
        let before_p = is_punctuation(before);
        let after_p = is_punctuation(after);
        (
            !after_p || before_p,
            !before_p || after_p,
            before_p,
            after_p,
        )
    };
    let marker = source.as_bytes()[begin];
    Delimiter {
        marker,
        length_class: if marker == b'~' {
            (next - begin) as u8
        } else {
            ((next - begin) % 3) as u8
        },
        opens: left && (marker != b'_' || !right || before_p),
        closes: right && (marker != b'_' || !left || after_p),
    }
}

fn matching(open: Delimiter, close: Delimiter) -> bool {
    open.marker == close.marker
        && if open.marker == b'~' {
            open.length_class == close.length_class
        } else {
            !(open.closes || close.opens) || (open.length_class + close.length_class) != 3
        }
}

fn finish_tokens(
    source: &str,
    mut tokens: Vec<Token>,
    payloads: &mut Vec<Option<InlineKind>>,
    gfm_autolink: bool,
) -> Vec<Inline> {
    if tokens.len() == 1 && !gfm_autolink {
        return singleton(tokens.pop().unwrap().into_inline(payloads));
    }
    let mut output = Vec::new();
    if !tokens
        .iter()
        .any(|token| token.delimiter().is_some_and(|d| d.opens || d.closes))
    {
        for token in tokens {
            push_inline(&mut output, token.into_inline(payloads));
        }
    } else {
        let head = tokens.len();
        for (i, token) in tokens[..head - 1].iter_mut().enumerate() {
            token.next = i + 1;
        }
        tokens[head - 1].next = usize::MAX;
        let mut openers: Vec<usize> = Vec::new();
        let mut lower_bound = [0; 15];

        for i in 0..head {
            let Some(close) = tokens[i].delimiter() else {
                continue;
            };
            if close.closes {
                let length = usize::from(close.length_class);
                let category = match close.marker {
                    b'*' => 3 * usize::from(close.opens) + length,
                    b'_' => 6 + 3 * usize::from(close.opens) + length,
                    _ => 11 + length,
                };
                loop {
                    let close_span = tokens[i].span;
                    if close_span.start == close_span.end {
                        break;
                    }
                    let found = openers
                        .iter()
                        .copied()
                        .enumerate()
                        .rev()
                        .take_while(|(_, index)| *index >= lower_bound[category])
                        .find(|(_, index)| matching(tokens[*index].delimiter().unwrap(), close));
                    let Some((position, opener)) = found else {
                        if let Some(last) = openers.last() {
                            lower_bound[category] = last + 1;
                        }
                        break;
                    };
                    let open_span = tokens[opener].span;
                    let used = if close.marker == b'~' {
                        length
                    } else {
                        (open_span.end - open_span.start)
                            .min(close_span.end - close_span.start)
                            .min(2)
                    };
                    let mut cursor = tokens[opener].next;
                    let children = if cursor != i && tokens[cursor].next == i {
                        singleton(tokens[cursor].into_inline(payloads))
                    } else {
                        let mut children = Vec::new();
                        while cursor != i {
                            let token = tokens[cursor];
                            push_inline(&mut children, token.into_inline(payloads));
                            cursor = token.next;
                        }
                        children
                    };

                    let kind = match close.marker {
                        b'~' => InlineKind::Strikethrough(children),
                        _ if used == 2 => InlineKind::Strong(children),
                        _ => InlineKind::Emphasis(children),
                    };
                    tokens[opener].span.end -= used;
                    tokens[i].span.start += used;
                    tokens[opener].next = tokens.len();
                    let mut group = Token::from_inline(
                        span(open_span.end - used, close_span.start + used),
                        kind,
                        None,
                        payloads,
                    );
                    group.next = i;
                    tokens.push(group);
                    openers
                        .truncate(position + usize::from(open_span.end - open_span.start > used));
                }
            }
            let rem = tokens[i].span;
            if close.opens && rem.start < rem.end {
                openers.push(i);
            }
        }

        let mut cursor = 0;
        while cursor != usize::MAX {
            let token = tokens[cursor];
            push_inline(&mut output, token.into_inline(payloads));
            cursor = token.next;
        }
    }
    if gfm_autolink {
        return gfm_linkify(source, output);
    }
    output
}

fn singleton(inline: Inline) -> Vec<Inline> {
    if inline.span.start == inline.span.end {
        Vec::new()
    } else {
        vec![inline]
    }
}

fn push_inline(output: &mut Vec<Inline>, inline: Inline) {
    if inline.span.start == inline.span.end {
        return;
    }
    if let InlineKind::Text(Text::Source(current)) = inline.kind
        && let Some(Inline {
            span: previous_span,
            kind: InlineKind::Text(Text::Source(previous)),
        }) = output.last_mut()
        && previous.end == current.start
        && previous_span.end == inline.span.start
    {
        previous.end = current.end;
        previous_span.end = inline.span.end;
        return;
    }
    output.push(inline);
}

fn entity(source: &str, start: usize, end: usize) -> Option<(usize, Box<str>)> {
    let bytes = source.as_bytes();
    let mut i = start + 1;
    if i >= end {
        return None;
    }
    if bytes[i] == b'#' {
        i += 1;
        let hex = i < end && matches!(bytes[i], b'x' | b'X');
        if hex {
            i += 1;
        }
        let digits = i;
        let (radix, max_digits) = if hex { (16, 6) } else { (10, 7) };
        let mut code = 0;
        while i < end && i - digits < max_digits {
            let Some(digit) = (bytes[i] as char).to_digit(radix) else {
                break;
            };
            code = code * radix + digit;
            i += 1;
        }
        if i == digits || bytes.get(i) != Some(&b';') || i >= end {
            return None;
        }
        let ch = char::from_u32(code).filter(|ch| *ch != '\0').unwrap_or('�');
        let mut buffer = [0; 4];
        return Some((i + 1, ch.encode_utf8(&mut buffer).into()));
    }
    if !bytes[i].is_ascii_alphabetic() {
        return None;
    }
    while i < end && i - start <= 32 && bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    if i >= end || bytes[i] != b';' {
        return None;
    }
    let candidate = &source[start..i + 1];
    match html_escape::decode_html_entities(candidate) {
        Cow::Borrowed(_) => None,
        Cow::Owned(decoded) => Some((i + 1, decoded.into_boxed_str())),
    }
}

fn decode_span(source: &str, input: Span) -> Text {
    let bytes = source.as_bytes();
    let mut consumed = input.start;
    let mut i = consumed;
    let mut result = String::new();
    while i < input.end {
        match bytes[i] {
            b'\\' if i + 1 < input.end && bytes[i + 1].is_ascii_punctuation() => {
                result.push_str(&source[consumed..i]);
                result.push(bytes[i + 1] as char);
                i += 2;
                consumed = i;
            }
            b'&' => {
                if let Some((end, decoded)) = entity(source, i, input.end) {
                    result.push_str(&source[consumed..i]);
                    result.push_str(&decoded);
                    consumed = end;
                    i = end;
                } else {
                    i += 1;
                }
            }
            0 => {
                result.push_str(&source[consumed..i]);
                result.push('�');
                i += 1;
                consumed = i;
            }
            _ => i += 1,
        }
    }
    if consumed == input.start {
        Text::Source(input)
    } else {
        result.push_str(&source[consumed..input.end]);
        Text::Decoded(result.into_boxed_str())
    }
}

fn skip_link_space(bytes: &[u8], mut i: usize, end: usize) -> Option<usize> {
    let mut line_end = false;
    while i < end {
        match bytes[i] {
            b' ' | b'\t' => i += 1,
            b'\n' | b'\r' if !line_end => {
                line_end = true;
                i += if bytes[i] == b'\r' && i + 1 < end && bytes[i + 1] == b'\n' {
                    2
                } else {
                    1
                };
            }
            b'\n' | b'\r' => return None,
            _ => break,
        }
    }
    Some(i)
}

pub(crate) fn link_destination(source: &str, mut i: usize, end: usize) -> Option<(Span, usize)> {
    let bytes = source.as_bytes();
    if i == end {
        return None;
    }
    if bytes[i] == b'<' {
        i += 1;
        let start = i;
        while i < end {
            match bytes[i] {
                b'<' | b'\n' | b'\r' => return None,
                b'>' => return Some((span(start, i), i + 1)),
                b'\\' if i + 1 < end && bytes[i + 1].is_ascii_punctuation() => i += 2,
                _ => i += 1,
            }
        }
        return None;
    }

    let start = i;
    let mut depth = 0;
    while i < end {
        match bytes[i] {
            b')' if depth == 0 => break,
            b')' => depth -= 1,
            b'(' => depth += 1,
            b' ' | 0..=31 | 127 => break,
            b'\\' if i + 1 < end && bytes[i + 1].is_ascii_punctuation() => i += 1,
            _ => {}
        }
        i += 1;
    }
    if depth == 0 {
        Some((span(start, i), i))
    } else {
        None
    }
}

pub(crate) fn link_title(source: &str, i: usize, end: usize) -> Option<(Span, usize)> {
    if i >= end {
        return None;
    }
    let bytes = source.as_bytes();
    let opener = bytes[i];
    let close = match opener {
        b'\'' | b'"' => opener,
        b'(' => b')',
        _ => return None,
    };
    let start = i + 1;
    let mut cursor = start;
    while cursor < end {
        match bytes[cursor] {
            c if c == close => return Some((span(start, cursor), cursor + 1)),
            b'(' if close == b')' => return None,
            b'\\' if cursor + 1 < end && bytes[cursor + 1].is_ascii_punctuation() => cursor += 2,
            b'\n' | b'\r' => cursor = skip_link_space(bytes, cursor, end)?,
            _ => cursor += 1,
        }
    }
    None
}

fn link_inline(source: &str, i: usize, end: usize) -> Option<(usize, Span, Option<Span>)> {
    let bytes = source.as_bytes();
    let inner = skip_link_space(bytes, i + 1, end)?;
    if inner >= end {
        return None;
    }
    if bytes[inner] == b')' {
        return Some((inner + 1, span(inner, inner), None));
    }
    let (destination, tail) = link_destination(source, inner, end)?;
    let after_destination = skip_link_space(bytes, tail, end)?;
    if after_destination >= end {
        return None;
    }
    if bytes[after_destination] == b')' {
        return Some((after_destination + 1, destination, None));
    }
    if after_destination == tail {
        return None;
    }
    let (title, after_title) = link_title(source, after_destination, end)?;
    let close = skip_link_space(bytes, after_title, end)?;
    (close < end && bytes[close] == b')').then_some((close + 1, destination, Some(title)))
}

fn label_end(source: &str, mut cursor: usize, end: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let start = cursor;
    while cursor < end && cursor - start <= 4 * 999 {
        match bytes[cursor] {
            b'[' => return None,
            b']' => {
                return (cursor - start <= 999
                    || source[start..cursor].chars().take(1000).count() <= 999)
                    .then_some(cursor);
            }
            b'\\' if cursor + 1 < end && bytes[cursor + 1].is_ascii_punctuation() => cursor += 2,
            b'\r' | b'\n' => cursor = skip_link_space(bytes, cursor, end)?,
            _ => cursor += 1,
        }
    }
    None
}

struct Target {
    end: usize,
    destination: Span,
    title: Option<Span>,
    is_reference: bool,
}

fn link_target(
    source: &str,
    label: Span,
    next: usize,
    end: usize,
    refs: &References,
    unresolved: Option<&mut Vec<String>>,
) -> Option<Target> {
    let bytes = source.as_bytes();
    if next < end
        && bytes[next] == b'('
        && let Some((end, destination, title)) = link_inline(source, next, end)
    {
        return Some(Target {
            end,
            destination,
            title,
            is_reference: false,
        });
    }
    let suffix = if next < end && bytes[next] == b'[' {
        label_end(source, next + 1, end)
    } else {
        None
    };
    let (label, finish) = match suffix {
        Some(close) if close > next + 1 => (span(next + 1, close), close + 1),
        _ => {
            if label_end(source, label.start, label.end + 1) != Some(label.end) {
                return None;
            }
            (label, suffix.map_or(next, |close| close + 1))
        }
    };
    let label = normalize_label(content(source, label));
    if label.is_empty() {
        return None;
    }
    if let Some(reference) = refs.get(label.as_ref()) {
        Some(Target {
            end: finish,
            destination: reference.destination,
            title: reference.title,
            is_reference: true,
        })
    } else {
        if let Some(unresolved) = unresolved {
            unresolved.push(label.into_owned());
        }
        None
    }
}

fn autolink_end(source: &str, i: usize, end: usize) -> Option<(usize, bool)> {
    let bytes = source.as_bytes();
    let start = i + 1;
    let mut cursor = start;
    while cursor < end && !matches!(bytes[cursor], b'>' | b'<' | b' ' | 0..=31 | 127) {
        cursor += 1;
    }
    if cursor >= end || bytes[cursor] != b'>' {
        return None;
    }
    let inner = &bytes[start..cursor];
    if let Some(colon) = inner.iter().position(|b| *b == b':')
        && (2..=32).contains(&colon)
        && inner[0].is_ascii_alphabetic()
        && inner[1..colon]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-'))
    {
        return Some((cursor + 1, false));
    }
    let at = inner.iter().position(|b| *b == b'@')?;
    if at == 0
        || !inner[..at]
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b".!#$%&'*+/=?^_`{|}~-".contains(b))
    {
        return None;
    }
    let domain = &inner[at + 1..];
    if domain.split(|b| *b == b'.').all(valid_email_domain_label) {
        Some((cursor + 1, true))
    } else {
        None
    }
}

fn valid_email_domain_label(label: &[u8]) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && label.first().unwrap().is_ascii_alphanumeric()
        && label.last().unwrap().is_ascii_alphanumeric()
        && label
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

fn html_end(source: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let after_angle = &source[start + 1..end];
    if let Some(comment) = after_angle.strip_prefix("!--") {
        if comment.starts_with('>') {
            return Some(start + 5);
        }
        if comment.starts_with("->") {
            return Some(start + 6);
        }
        return comment.find("-->").map(|position| start + 4 + position + 3);
    }
    if let Some(body) = after_angle.strip_prefix('?') {
        return body.find("?>").map(|position| start + 2 + position + 2);
    }
    if let Some(cdata) = after_angle.strip_prefix("![CDATA[") {
        return cdata.find("]]>").map(|position| start + 9 + position + 3);
    }
    if let Some(declaration) = after_angle.strip_prefix('!')
        && declaration
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
    {
        return declaration
            .find('>')
            .map(|position| start + 2 + position + 1);
    }

    let mut i = start + 1;
    let closing = i < end && bytes[i] == b'/';
    i += usize::from(closing);
    if i >= end || !bytes[i].is_ascii_alphabetic() {
        return None;
    }
    i += 1;
    while i < end && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    loop {
        let before_space = i;
        i = skip_html_space(bytes, i, end);
        if i == end {
            return None;
        }
        if bytes[i] == b'>' {
            return Some(i + 1);
        }
        if !closing && bytes[i] == b'/' && i + 1 < end && bytes[i + 1] == b'>' {
            return Some(i + 2);
        }
        if closing
            || i == before_space
            || !(bytes[i].is_ascii_alphabetic() || matches!(bytes[i], b'_' | b':'))
        {
            return None;
        }

        i += 1;
        while i < end
            && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b':' | b'.' | b'-'))
        {
            i += 1;
        }
        let after_name = skip_html_space(bytes, i, end);
        if after_name < end && bytes[after_name] == b'=' {
            i = skip_html_space(bytes, after_name + 1, end);
            if i == end {
                return None;
            }
            if matches!(bytes[i], b'\'' | b'"') {
                let quote = bytes[i];
                i += 1 + bytes[i + 1..end].iter().position(|b| *b == quote)? + 1;
            } else {
                let from = i;
                while i < end
                    && !matches!(
                        bytes[i],
                        b' ' | b'\r'
                            | b'\n'
                            | b'\t'
                            | b'\x0c'
                            | b'\''
                            | b'"'
                            | b'='
                            | b'<'
                            | b'>'
                            | b'`'
                    )
                {
                    i += 1;
                }
                if i == from {
                    return None;
                }
            }
        }
    }
}

fn skip_html_space(bytes: &[u8], mut i: usize, end: usize) -> usize {
    while i < end && matches!(bytes[i], b' ' | b'\r' | b'\n' | b'\t' | b'\x0c') {
        i += 1;
    }
    i
}

fn has_gfm_trigger(text: &str) -> bool {
    let bytes = text.as_bytes();
    memchr::memchr3_iter(b':', b'@', b'.', bytes).any(|pos| match bytes[pos] {
        b'@' => true,
        b'.' => bytes[..pos].ends_with(b"www"),
        _ => {
            bytes[pos + 1..].starts_with(b"//")
                && (bytes[..pos].ends_with(b"http")
                    || bytes[..pos].ends_with(b"https")
                    || bytes[..pos].ends_with(b"ftp"))
        }
    })
}

fn gfm_linkify(source: &str, nodes: Vec<Inline>) -> Vec<Inline> {
    let mut nodes = nodes;
    let mut needs_linkification = false;
    for node in &mut nodes {
        match &mut node.kind {
            InlineKind::Text(Text::Source(s)) if node.span == *s => {
                needs_linkification |= has_gfm_trigger(content(source, *s));
            }
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children) => {
                *children = gfm_linkify(source, std::mem::take(children));
            }
            _ => {}
        }
    }
    if !needs_linkification {
        return nodes;
    }

    let mut output = Vec::with_capacity(nodes.len());
    for node in nodes {
        if let InlineKind::Text(Text::Source(s)) = node.kind
            && node.span == s
        {
            append_gfm_text(source, s.start, s.end, &mut output);
        } else {
            output.push(node);
        }
    }
    output
}

fn append_gfm_text(source: &str, start: usize, end: usize, output: &mut Vec<Inline>) {
    let raw = &source[start..end];
    if !has_gfm_trigger(raw) {
        push_inline(output, original(start, end));
        return;
    }
    let bytes = source.as_bytes();
    let mut cursor = start;
    let mut unlinked = start;
    while cursor < end {
        let word = cursor;
        while cursor < end
            && !matches!(
                bytes[cursor],
                b' ' | b'\n' | b'\r' | b'\t' | b'(' | b'<' | b'>' | b'\'' | b'"' | b',' | b';'
            )
        {
            cursor += 1;
        }
        let mut word_end = cursor;
        cursor += usize::from(cursor < end);
        if word == word_end {
            continue;
        }
        let value = &source[word..word_end];
        let scheme_len = ["http://", "https://", "ftp://", "www."]
            .into_iter()
            .find(|scheme| value.starts_with(scheme))
            .map(str::len);
        let prefix = if let Some(scheme_len) = scheme_len {
            let link_tail = bytes[word_end..end]
                .iter()
                .position(|b| b.is_ascii_whitespace() || matches!(b, b'<' | b'>'))
                .map_or(end, |offset| word_end + offset);
            word_end = autolink_url_end(source, word, link_tail);
            cursor = cursor.max(word_end);
            let Some(domain) = bytes.get(word + scheme_len..word_end) else {
                continue;
            };

            if domain.is_empty() || !valid_url_domain(domain, scheme_len == 4) {
                continue;
            }
            if scheme_len == 4 {
                Some("http://")
            } else {
                None
            }
        } else {
            let scheme = ["mailto:", "xmpp:"]
                .into_iter()
                .find(|scheme| value.starts_with(scheme));
            let address_start = word + scheme.map_or(0, str::len);
            let Some(finish) =
                trailing_email_end(source, address_start, word_end, scheme == Some("xmpp:"))
            else {
                continue;
            };
            word_end = finish;
            if scheme.is_none() {
                Some("mailto:")
            } else {
                None
            }
        };
        push_inline(output, original(unlinked, word));
        let label = span(word, word_end);
        let destination = if let Some(prefix) = prefix {
            let mut value = String::with_capacity(prefix.len() + word_end - word);
            value.push_str(prefix);
            value.push_str(content(source, label));
            Text::Decoded(value.into_boxed_str())
        } else {
            Text::Source(label)
        };
        output.push(Inline {
            span: label,
            kind: InlineKind::Link(Box::new(Link {
                destination,
                title: None,
                children: vec![original(word, word_end)],
            })),
        });
        unlinked = word_end;
    }
    push_inline(output, original(unlinked, end));
}

fn valid_url_domain(raw: &[u8], www: bool) -> bool {
    let end = raw
        .iter()
        .position(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'-' | b'.'))
        .unwrap_or(raw.len());
    if end == 0 {
        return false;
    }
    let mut parts = raw[..end].rsplit(|byte| *byte == b'.');
    let Some(last) = parts.next() else {
        return false;
    };
    let middle = parts.next();
    !(last.is_empty()
        || last.contains(&b'_')
        || middle.is_none()
        || middle.is_some_and(|segment| segment.is_empty() || segment.contains(&b'_')))
        && (!www || raw[..end].contains(&b'.'))
}

fn autolink_url_end(source: &str, start: usize, mut end: usize) -> usize {
    let bytes = source.as_bytes();
    // The first whitespace or angle bracket is the end of the link candidate.
    end = bytes[start..end]
        .iter()
        .position(|b| b.is_ascii_whitespace() || matches!(b, b'<' | b'>'))
        .map_or(end, |offset| start + offset);
    while end > start
        && matches!(
            bytes[end - 1],
            b'?' | b'!' | b'.' | b',' | b':' | b'*' | b'_' | b'~'
        )
    {
        end -= 1;
    }
    if end > start
        && bytes[end - 1] == b';'
        && let Some(position) = bytes[start..end - 1].iter().rposition(|byte| *byte == b'&')
        && bytes[start + position + 1..end - 1]
            .iter()
            .all(u8::is_ascii_alphanumeric)
    {
        end = start + position;
    }
    if end > start && bytes[end - 1] == b')' {
        let opens = bytes[start..end].iter().filter(|b| **b == b'(').count();
        let closes = bytes[start..end].iter().filter(|b| **b == b')').count();
        let mut extra = closes.saturating_sub(opens);
        while extra > 0 && end > start && bytes[end - 1] == b')' {
            end -= 1;
            extra -= 1;
        }
    }
    end
}

fn trailing_email_end(source: &str, start: usize, end: usize, xmpp: bool) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = start;
    while i < end
        && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'.' | b'-' | b'_' | b'+'))
    {
        i += 1;
    }
    if i == start || i == end || bytes[i] != b'@' {
        return None;
    }
    let domain = i + 1;
    i += 1;
    while i < end && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'.' | b'-' | b'_')) {
        i += 1;
    }
    let host_finish = source[domain..i].trim_end_matches('.').len() + domain;
    if host_finish == domain
        || !bytes[domain..host_finish].contains(&b'.')
        || bytes[domain..host_finish]
            .split(|b| *b == b'.')
            .any(|s| s.is_empty())
        || matches!(bytes[host_finish - 1], b'-' | b'_')
    {
        return None;
    }
    if xmpp && host_finish < end && bytes[host_finish] == b'/' {
        let resource = host_finish + 1;
        let mut cursor = resource;
        while cursor < end
            && (bytes[cursor].is_ascii_alphanumeric() || matches!(bytes[cursor], b'@' | b'.'))
        {
            cursor += 1;
        }
        let resource_end = source[resource..cursor].trim_end_matches('.').len() + resource;
        if resource_end > resource {
            return Some(resource_end);
        }
    }
    Some(host_finish)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Reference;

    #[test]
    fn markup_search_preserves_byte_offsets_at_chunk_boundaries() {
        for gfm in [false, true] {
            for offset in [0, 15, 16, 31, 32, 49] {
                for byte in u8::MIN..=u8::MAX {
                    let mut bytes = [b'c'; 50];
                    bytes[offset] = byte;
                    let expected = special(byte, gfm).then_some(offset);
                    assert_eq!(
                        first_special(&bytes, gfm),
                        expected,
                        "{gfm} {offset} {byte}"
                    );
                }
            }
        }

        assert!(!needs_markup("普通文本と。 No formatting.", Syntax::Gfm));
        assert!(!needs_markup("www.example.com", Syntax::CommonMark));
        assert!(needs_markup("www.example.com", Syntax::Gfm));
        assert!(needs_markup("普通中文\r\n正文", Syntax::CommonMark));
    }

    #[test]
    fn tracks_only_unresolved_labels_and_retains_original_coordinates() {
        let source = "[标签][Straße\t  例]中 [未定义] `[code]` /dest";
        let end = source.len() - 6;
        let options = Options {
            syntax: Syntax::CommonMark,
            ..Options::default()
        };
        let mut unresolved = Vec::new();
        let mut refs = References::default();
        assert!(refs.insert(
            normalize_label("STRASSE 例"),
            Reference {
                destination: span(end + 1, source.len()),
                title: None,
            }
        ));
        let actual = parse_tracked(source, span(0, end), &options, &refs, &mut unresolved);
        assert_eq!(unresolved, ["未定义"]);
        let InlineKind::Link(link) = &actual[0].kind else {
            panic!("expected link")
        };
        assert_eq!(link.destination.as_str(source), "/dest");
        assert_eq!(content(source, link.children[0].span), "标签");
        assert_eq!(content(source, actual[1].span), "中 [未定义] ");
        assert!(matches!(actual[2].kind, InlineKind::Code(_)));
    }

    #[test]
    fn recognizes_links_inside_emphasis_and_balanced_url_paths() {
        let source = "**www.google.com/search?q=(business))+ok**";
        let options = Options::default();
        let nodes = parse(
            source,
            span(0, source.len()),
            &options,
            &References::default(),
        );
        let InlineKind::Strong(children) = &nodes[0].kind else {
            panic!("expected strong emphasis")
        };
        let InlineKind::Link(link) = &children[0].kind else {
            panic!("expected autolink")
        };
        assert_eq!(
            link.destination.as_str(source),
            "http://www.google.com/search?q=(business))+ok"
        );
    }

    #[test]
    fn disjoint_fragments_remap_labels_but_borrow_reference_destinations() {
        let source = "before [la<ignored>bel][KEY] [unknown] after /dest title";
        let start = source.find("[la").unwrap();
        let gap = source.find("<ignored>").unwrap();
        let end = source.find(" after").unwrap();
        let destination = source.find("/dest").unwrap();
        let title = source.find("title").unwrap();
        let options = Options {
            syntax: Syntax::CommonMark,
            ..Options::default()
        };
        let mut refs = References::default();
        refs.insert(
            normalize_label("KEY"),
            Reference {
                destination: span(destination, destination + 5),
                title: Some(span(title, source.len())),
            },
        );
        let mut unresolved = Vec::new();
        let nodes = parse_fragments(
            source,
            &[span(start, gap), span(gap + "<ignored>".len(), end)],
            &options,
            &refs,
            Some(&mut unresolved),
        );
        let InlineKind::Link(link) = &nodes[0].kind else {
            panic!("expected reference link")
        };
        assert_eq!(content(source, nodes[0].span), "[la<ignored>bel][KEY]");
        let InlineKind::Text(text) = &link.children[0].kind else {
            panic!("expected text label")
        };
        assert_eq!(*text, Text::Decoded("label".into()));
        assert_eq!(
            link.destination,
            Text::Source(span(destination, destination + 5))
        );
        assert_eq!(link.title, Some(Text::Source(span(title, source.len()))));
        assert_eq!(unresolved, ["unknown"]);
    }

    #[test]
    fn emphasis_can_cross_disjoint_container_lines() {
        let source = "> *begin\n> end*";
        let options = Options::default();
        let nodes = parse_fragments(
            source,
            &[span(2, 9), span(11, source.len())],
            &options,
            &References::default(),
            None,
        );
        let InlineKind::Emphasis(children) = &nodes[0].kind else {
            panic!("expected emphasis")
        };
        assert_eq!(content(source, children[0].span), "begin");
        assert!(matches!(children[1].kind, InlineKind::SoftBreak));
        assert_eq!(content(source, children[2].span), "end");
    }

    #[test]
    fn unmatched_backticks_preserve_later_matched_runs() {
        let source = "``` text ``two`` ```` unmatched `one`";
        let options = Options::default();
        let nodes = parse(
            source,
            span(0, source.len()),
            &options,
            &References::default(),
        );
        let code: Vec<_> = nodes
            .iter()
            .filter_map(|node| match &node.kind {
                InlineKind::Code(value) => Some(value.as_str(source)),
                _ => None,
            })
            .collect();
        assert_eq!(code, ["two", "one"]);
    }

    #[test]
    fn github_extension_autolinks_ftp() {
        let source = "Mirror at ftp://foo.bar.baz.";
        let options = Options::default();
        let nodes = parse(
            source,
            span(0, source.len()),
            &options,
            &References::default(),
        );
        let InlineKind::Link(link) = &nodes[1].kind else {
            panic!("expected FTP autolink")
        };
        assert_eq!(link.destination.as_str(source), "ftp://foo.bar.baz");
        assert_eq!(content(source, nodes[2].span), ".");
    }

    #[test]
    fn decodes_the_complete_unicode_entity() {
        let source = "&ngE;";
        let options = Options::default();
        let nodes = parse(
            source,
            span(0, source.len()),
            &options,
            &References::default(),
        );
        let InlineKind::Text(text) = &nodes[0].kind else {
            panic!("expected decoded text")
        };
        assert_eq!(text.as_str(source), "≧̸");
    }
}

use std::collections::HashMap;

use crate::{Span, Text};

#[derive(Default)]
pub(super) struct Scanner {
    parentheses: Option<Parentheses>,
    literal_dollar_close: Option<usize>,
}

#[derive(Default)]
struct Parentheses {
    pairs: HashMap<usize, usize>,
    last_open: Option<usize>,
}

pub(super) struct Formula {
    pub end: usize,
    pub body: Text,
    pub loading: bool,
}

impl Scanner {
    pub fn read(
        &mut self,
        source: &str,
        start: usize,
        end: usize,
        allow_loading: bool,
    ) -> Option<Formula> {
        let bytes = source.as_bytes();
        let opener = bytes[start];
        if self.literal_dollar_close == Some(start) {
            self.literal_dollar_close = None;
            return None;
        }
        let (width, close, last_opener) = if opener == b'$' {
            if start > 0 && bytes[start - 1] == b'$' {
                return None;
            }
            let width = bytes[start..end]
                .iter()
                .take_while(|&&byte| byte == b'$')
                .count();
            if width > 2 {
                return None;
            }
            let mut cursor = start + width;
            let mut saw_other_opener = false;
            let close = loop {
                let Some(offset) = memchr::memchr2(b'$', b'`', &bytes[cursor..end]) else {
                    break None;
                };
                let pos = cursor + offset;
                if bytes[pos] == b'`' {
                    return None;
                }
                let run = bytes[pos..end]
                    .iter()
                    .take_while(|&&byte| byte == b'$')
                    .count();
                if !escaped(bytes, pos) {
                    if run == width {
                        break Some(pos);
                    }
                    if width == 1 {
                        return None;
                    }
                    saw_other_opener = true;
                }
                cursor = pos + run;
            };
            (width, close, !saw_other_opener)
        } else {
            let parens = self
                .parentheses
                .get_or_insert_with(|| parentheses(bytes, start, end));
            (
                2,
                parens.pairs.get(&start).copied(),
                parens.last_open == Some(start),
            )
        };

        let body_start = start + width;
        let body_end = close.unwrap_or(end);
        let body = &source[body_start..body_end];
        let trimmed = body.trim();
        if trimmed.is_empty() || opener == b'$' && width == 1 && placeholder(trimmed) {
            if opener == b'$' {
                self.literal_dollar_close = close;
            }
            return None;
        }
        if memchr::memchr(b'`', body.as_bytes()).is_some() {
            return None;
        }
        let loading = close.is_none();
        if loading && (!allow_loading || !last_opener) {
            return None;
        }
        if opener == b'$' && width == 1 && (likely_currency(body, !loading) || placeholder(trimmed))
        {
            return None;
        }
        if (loading || opener != b'$' || width == 2) && !math_like(trimmed) {
            return None;
        }
        let body =
            normalize(body).map_or(Text::Source(Span::new(body_start, body_end)), Text::Decoded);
        Some(Formula {
            end: close.map_or(end, |pos| pos + width),
            body,
            loading,
        })
    }
}

fn escaped(bytes: &[u8], at: usize) -> bool {
    bytes[..at]
        .iter()
        .rev()
        .take_while(|&&b| b == b'\\')
        .count()
        % 2
        != 0
}

fn parentheses(bytes: &[u8], start: usize, end: usize) -> Parentheses {
    let mut result = Parentheses::default();
    if memchr::memmem::find(&bytes[start..end], b"\\)").is_none() {
        result.last_open = memchr::memmem::rfind_iter(&bytes[start..end], b"\\(")
            .map(|pos| start + pos)
            .find(|&pos| !escaped(bytes, pos));
        return result;
    }
    let mut pending = Vec::new();
    let mut cursor = start;
    while let Some(offset) = memchr::memchr(b'\\', &bytes[cursor..end]) {
        let pos = cursor + offset;
        if pos + 1 == end {
            break;
        }
        match bytes[pos + 1] {
            b'(' => {
                pending.push(pos);
                result.last_open = Some(pos);
            }
            b')' => {
                if let Some(opener) = pending.pop() {
                    result.pairs.insert(opener, pos);
                }
            }
            _ => {}
        }
        // A pair of backslashes is consumed together, so its second byte cannot open a delimiter.
        cursor = match bytes[pos + 1] {
            b'\\' | b'(' | b')' => pos + 2,
            _ => pos + 1,
        };
    }
    result
}

fn likely_currency(body: &str, closed: bool) -> bool {
    // A following explicit formula is independent evidence.
    let body = body.split("\\(").next().unwrap().trim_start();
    let bytes = body.as_bytes();
    let mut amount_end = digit_run(bytes, 0);
    if amount_end == 0
        || memchr::memchr3(b'\\', b'^', b'_', bytes).is_some()
        || memchr::memchr2(b'{', b'}', bytes).is_some()
    {
        return false;
    }
    while bytes.get(amount_end) == Some(&b',')
        && bytes
            .get(amount_end + 1..amount_end + 4)
            .is_some_and(|group| group.iter().all(u8::is_ascii_digit))
    {
        amount_end += 4;
    }
    if bytes.get(amount_end) == Some(&b'.')
        && bytes.get(amount_end + 1).is_some_and(u8::is_ascii_digit)
    {
        amount_end = digit_run(bytes, amount_end + 1);
    }

    let rest = &body[amount_end..];
    let trimmed = rest.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return !closed;
    };
    if ascii_operator(first) {
        return rest.starts_with("**")
            || trimmed
                .chars()
                .all(|ch| ch.is_whitespace() || ascii_operator(ch));
    }
    rest.chars().next().is_some_and(|ch| {
        ch.is_whitespace()
            || matches!(
                ch,
                ')' | ',' | '.' | '!' | '?' | ';' | ':' | '~' | '～' | '–' | '—'
            )
    })
}

fn digit_run(bytes: &[u8], start: usize) -> usize {
    start
        + bytes[start..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
}

fn ascii_operator(ch: char) -> bool {
    matches!(ch, '+' | '-' | '*' | '/' | '=' | '<' | '>')
}

fn placeholder(trimmed: &str) -> bool {
    (trimmed.len() >= 3 && trimmed.bytes().all(|b| b == b'.'))
        || trimmed.chars().all(|ch| ch == '…')
}

fn math_like(body: &str) -> bool {
    let bytes = body.as_bytes();
    if memchr::memmem::find(bytes, b"**").is_some() || contains_date(bytes) {
        return false;
    }
    if body.len() > 2_000
        || bytes.len() == 1 && bytes[0].is_ascii_alphabetic()
        || body.eq_ignore_ascii_case("pi")
    {
        return true;
    }

    if let Some(inner) = body
        .strip_prefix('(')
        .and_then(|text| text.strip_suffix(')'))
        && inner.split(',').all(|part| {
            let text = part.trim();
            text.len() == 1 && text.as_bytes()[0].is_ascii_alphabetic()
        })
    {
        return true;
    }

    let mut has_operator = false;
    let mut compound_word = false;
    let mut word_end = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        match byte {
            b'\\' if bytes.get(i + 1).is_some_and(u8::is_ascii_alphabetic) => return true,
            b'\\'
                if bytes
                    .get(i + 1)
                    .is_some_and(|ch| matches!(ch, b'%' | b'{' | b'}')) =>
            {
                return true;
            }
            b'\x08' | b'\x0b' | b'\x0c'
                if bytes.get(i + 1).is_some_and(u8::is_ascii_alphabetic) =>
            {
                return true;
            }
            b'_' if i == 1
                && bytes[0].is_ascii_alphabetic()
                && bytes
                    .get(i + 1)
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'{' | b'\\')) =>
            {
                return true;
            }
            b'+' if bytes.get(i + 1) != Some(&b'+') && (i == 0 || bytes[i - 1] != b'+') => {
                has_operator = true
            }
            b'=' | b'*' | b'/' | b'^' | b'<' | b'>' => has_operator = true,
            b'-' => {
                has_operator = true;
                compound_word |= i >= 2
                    && i + 3 <= bytes.len()
                    && bytes[i - 2..i].iter().all(u8::is_ascii_alphabetic)
                    && bytes[i + 1..i + 3].iter().all(u8::is_ascii_alphabetic);
            }
            b'a'..=b'z' | b'A'..=b'Z' if i >= word_end => {
                word_end = i + bytes[i..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_alphabetic())
                    .count();
                let word = &body[i..word_end];
                let next = body[word_end..].trim_start();
                if matches!(
                    word,
                    "sin"
                        | "cos"
                        | "tan"
                        | "log"
                        | "ln"
                        | "exp"
                        | "sqrt"
                        | "frac"
                        | "sum"
                        | "lim"
                        | "int"
                        | "prod"
                ) || brace_command(word) && next.starts_with('{')
                    || next.starts_with('(')
                        && next
                            .strip_prefix('(')
                            .is_some_and(|tail| tail.find(')').is_some_and(|n| n != 0))
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    has_operator && !compound_word || chemical_like(bytes)
}

fn contains_date(bytes: &[u8]) -> bool {
    for slash in memchr::memchr_iter(b'/', bytes) {
        if slash < 4
            || slash + 3 >= bytes.len()
            || !bytes[slash - 4..slash].iter().all(u8::is_ascii_digit)
            || slash > 4 && bytes[slash - 5].is_ascii_alphanumeric()
        {
            continue;
        }
        let month = digit_run(bytes, slash + 1);
        if !matches!(month - slash, 2 | 3) || bytes.get(month) != Some(&b'/') {
            continue;
        }
        let day = digit_run(bytes, month + 1);
        if matches!(day - month, 2 | 3) && !bytes.get(day).is_some_and(u8::is_ascii_alphanumeric) {
            return true;
        }
    }
    false
}

fn chemical_like(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_uppercase() {
            return false;
        }
        i += 1;
        if bytes.get(i).is_some_and(u8::is_ascii_lowercase) {
            i += 1;
        }
        if matches!(bytes.get(i), Some(b'_' | b'^')) {
            i += 1;
            let brace = bytes.get(i) == Some(&b'{');
            i += usize::from(brace);
            let end = digit_run(bytes, i);
            if end == i || brace && bytes.get(end) != Some(&b'}') {
                return false;
            }
            i = end + usize::from(brace);
        }
    }
    true
}

fn brace_command(word: &str) -> bool {
    matches!(
        word,
        "boldsymbol"
            | "mathbb"
            | "mathcal"
            | "mathfrak"
            | "mathrm"
            | "mathit"
            | "mathsf"
            | "vec"
            | "hat"
            | "bar"
            | "tilde"
            | "overline"
            | "underline"
            | "mathscr"
            | "mathnormal"
            | "operatorname"
            | "mathbf"
            | "text"
    )
}

fn tex_command(word: &str) -> bool {
    matches!(
        word,
        "ldots"
            | "cdots"
            | "quad"
            | "in"
            | "displaystyle"
            | "int_"
            | "lim"
            | "lim_"
            | "ce"
            | "pu"
            | "end"
            | "infty"
            | "perp"
            | "mid"
            | "operatorname"
            | "to"
            | "rightarrow"
            | "leftarrow"
            | "math"
            | "mathrm"
            | "mathit"
            | "mathbb"
            | "mathcal"
            | "mathfrak"
            | "implies"
            | "alpha"
            | "beta"
            | "gamma"
            | "delta"
            | "epsilon"
            | "lambda"
            | "sum"
            | "sum_"
            | "prod"
            | "sqrt"
            | "fbox"
            | "boxed"
            | "color"
            | "rule"
            | "edef"
            | "fcolorbox"
            | "hline"
            | "hdashline"
            | "cdot"
            | "times"
            | "pm"
            | "le"
            | "ge"
            | "neq"
            | "sin"
            | "cos"
            | "tan"
            | "log"
            | "ln"
            | "exp"
            | "frac"
            | "text"
            | "left"
            | "right"
    )
}

fn normalize(body: &str) -> Option<Box<str>> {
    let bytes = body.as_bytes();
    let mut result = String::new();
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let byte = bytes[i];
        i += 1;
        match byte {
            b'\t' | b'\r' | b'\x08' | b'\x0b' | b'\x0c' => {
                result.push_str(&body[copied..start]);
                result.push('\\');
                result.push(match byte {
                    b'\t' => 't',
                    b'\r' => 'r',
                    b'\x08' => 'b',
                    b'\x0b' => 'v',
                    _ => 'f',
                });
                copied = i;
            }
            b'a'..=b'z'
                if start == 0
                    || (!bytes[start - 1].is_ascii_alphanumeric()
                        && !matches!(bytes[start - 1], b'\\' | b'_')) =>
            {
                i += bytes[i..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
                    .count();
                let word = &body[start..i];
                if tex_command(word)
                    || brace_command(word) && body[i..].trim_start().starts_with('{')
                {
                    result.push_str(&body[copied..start]);
                    result.push('\\');
                    copied = start;
                }
            }
            b'!' => {
                i += bytes[i..].iter().take_while(|&&byte| byte == b'!').count();
                let previous = body[..start].chars().next_back();
                if !previous.is_some_and(|ch| {
                    ch.is_alphanumeric()
                        || matches!(
                            ch,
                            '\\' | '\'' | ')' | ']' | '}' | '′' | '″' | '‴' | '|' | '‖'
                        )
                }) {
                    result.push_str(&body[copied..start]);
                    for _ in start..i {
                        result.push_str("\\!");
                    }
                    copied = i;
                }
            }
            b'\\'
                if !escaped(bytes, start) && (i == bytes.len() || bytes.get(i) == Some(&b'\n')) =>
            {
                result.push_str(&body[copied..i]);
                result.push('\\');
                copied = i;
            }
            _ => {}
        }
    }
    if result.is_empty() {
        None
    } else {
        result.push_str(&body[copied..]);
        Some(result.into_boxed_str())
    }
}

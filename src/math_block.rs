use crate::Span;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Delimiter {
    Dollars,
    Escaped,
    Brackets,
}

impl Delimiter {
    pub(crate) fn closing(self) -> &'static [u8] {
        match self {
            Self::Dollars => b"$$",
            Self::Escaped => b"\\]",
            Self::Brackets => b"]",
        }
    }

    pub(crate) fn width(self) -> usize {
        self.closing().len()
    }

    pub(crate) fn closes(self, source: &str, start: usize, end: usize) -> Option<Span> {
        if let Some(close) = self.explicit_close(source, start, start, end) {
            return Some(close);
        }
        if self == Self::Escaped && source[start..end].trim() == "]" {
            let offset = source[start..end].find(']').unwrap();
            return Some(Span::new(start + offset, start + offset + 1));
        }
        None
    }

    pub(crate) fn explicit_close(
        self,
        source: &str,
        start: usize,
        scan: usize,
        end: usize,
    ) -> Option<Span> {
        let bytes = source.as_bytes();
        let delimiter = self.closing();
        for offset in memchr::memchr_iter(delimiter[0], &bytes[scan..end]) {
            let index = scan + offset;
            let tail = &bytes[index..end];
            if !tail.starts_with(delimiter) {
                continue;
            }
            let escapes = bytes[start..index]
                .iter()
                .rev()
                .take_while(|&&byte| byte == b'\\')
                .count();
            let compatible_escape = self == Self::Brackets && escapes == 1;
            if escapes % 2 == 0 || compatible_escape {
                return Some(Span::new(
                    index - usize::from(compatible_escape),
                    index + delimiter.len(),
                ));
            }
        }
        None
    }

    pub(crate) fn accepts(self, body: &str, same_line: bool) -> bool {
        match self {
            Self::Dollars => !body.trim().is_empty(),
            Self::Escaped => evidence(body, !same_line),
            Self::Brackets => evidence(body, true) && !(body.contains('"') && body.contains(':')),
        }
    }
}

pub(crate) fn opening(line: &str) -> Option<Delimiter> {
    if let Some(tail) = line.strip_prefix("$$") {
        return (!tail.starts_with('$')).then_some(Delimiter::Dollars);
    }
    if let Some(tail) = line.strip_prefix("\\[") {
        let index = tail.find("\\]");
        return if let Some(index) = index {
            Delimiter::Escaped
                .accepts(&tail[..index], true)
                .then_some(Delimiter::Escaped)
        } else {
            (tail.trim().is_empty() || evidence(tail, false)).then_some(Delimiter::Escaped)
        };
    }
    if let Some(tail) = line.strip_prefix('[') {
        if tail.starts_with('^') {
            return None;
        }
        let trimmed = tail.trim();
        if trimmed.is_empty()
            || trimmed.strip_suffix(']').is_some_and(|body| {
                !body.contains([']', '[']) && Delimiter::Brackets.accepts(body, true)
            })
        {
            return Some(Delimiter::Brackets);
        }
    }
    None
}

pub(crate) fn evidence(body: &str, include_variable: bool) -> bool {
    let body = body.trim();
    if body.is_empty() || body.contains('`') || body.contains("**") {
        return false;
    }
    if include_variable
        && (body.len() == 1 && body.as_bytes()[0].is_ascii_alphabetic() || body == "pi")
    {
        return true;
    }
    let bytes = body.as_bytes();
    bytes
        .windows(2)
        .any(|pair| pair[0] == b'\\' && pair[1].is_ascii_alphabetic())
        || bytes
            .iter()
            .any(|byte| matches!(byte, b'=' | b'+' | b'*' | b'^' | b'<' | b'>'))
        || bytes.windows(3).any(|part| {
            matches!(part[1], b'_' | b'-' | b'/')
                && part[0].is_ascii_alphanumeric()
                && part[2].is_ascii_digit()
        })
}

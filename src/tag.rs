use crate::Span;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tag {
    name: Span,
    attributes: Span,
}

impl Tag {
    pub fn tag_name(self, source: &str) -> &str {
        &source[Range::from(self.name)]
    }

    pub fn attrs(self, source: &str) -> Attributes<'_> {
        Attributes {
            remaining: &source[Range::from(self.attributes)],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Attribute<'src> {
    pub name: &'src str,
    pub value: Option<&'src str>,
}

pub struct Attributes<'src> {
    remaining: &'src str,
}

impl<'src> Iterator for Attributes<'src> {
    type Item = Attribute<'src>;

    fn next(&mut self) -> Option<Self::Item> {
        let (attribute, rest) = attribute(self.remaining)?;
        self.remaining = rest;
        Some(attribute)
    }
}

pub(crate) struct Opening {
    pub tag: Tag,
    pub end: usize,
    pub self_closing: bool,
}

pub(crate) struct OpeningProbe {
    pub start: usize,
    checked: usize,
    quote: Option<u8>,
    rejected: bool,
}

impl OpeningProbe {
    pub fn new(start: usize, attributes: usize) -> Self {
        Self {
            start,
            checked: attributes,
            quote: None,
            rejected: false,
        }
    }

    pub fn update(&mut self, source: &str, end: usize) -> Option<usize> {
        if self.rejected {
            return None;
        }
        let bytes = source.as_bytes();
        while self.checked < end {
            let byte = bytes[self.checked];
            self.checked += 1;
            match self.quote {
                Some(quote) if quote == byte => self.quote = None,
                Some(_) => {}
                None if matches!(byte, b'\'' | b'"') => self.quote = Some(byte),
                None if byte == b'>' => return Some(self.checked),
                None if byte == b'<' => {
                    self.rejected = true;
                    return None;
                }
                None => {}
            }
        }
        None
    }

    pub fn reject(&mut self) {
        self.rejected = true;
    }
}

pub(crate) fn opening(source: &str, start: usize, end: usize) -> Option<Opening> {
    let input = &source[start..end];
    let after_lt = input.strip_prefix('<')?;
    if !after_lt
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
    {
        return None;
    }
    let name_len = after_lt
        .bytes()
        .take_while(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
        })
        .count();
    let name_end = start + 1 + name_len;
    let mut rest = &source[name_end..end];
    loop {
        let trimmed = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let attr_end = end - trimmed.len();
        if let Some(after) = trimmed.strip_prefix("/>") {
            return Some(Opening {
                tag: Tag {
                    name: Span::new(start + 1, name_end),
                    attributes: Span::new(name_end, attr_end),
                },
                end: end - after.len(),
                self_closing: true,
            });
        }
        if let Some(after) = trimmed.strip_prefix('>') {
            return Some(Opening {
                tag: Tag {
                    name: Span::new(start + 1, name_end),
                    attributes: Span::new(name_end, attr_end),
                },
                end: end - after.len(),
                self_closing: false,
            });
        }
        if trimmed.len() == rest.len() {
            return None;
        }
        (_, rest) = attribute(rest)?;
    }
}

fn attribute(input: &str) -> Option<(Attribute<'_>, &str)> {
    let input = input.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let first = *input.as_bytes().first()?;
    if !(first.is_ascii_alphabetic() || matches!(first, b'_' | b':')) {
        return None;
    }
    let name_len = input
        .bytes()
        .take_while(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'-' | b'.')
        })
        .count();
    let (name, tail) = input.split_at(name_len);
    let Some(value) = tail
        .trim_start_matches(|c: char| c.is_ascii_whitespace())
        .strip_prefix('=')
    else {
        return Some((Attribute { name, value: None }, tail));
    };
    let value = value.trim_start_matches(|c: char| c.is_ascii_whitespace());
    if let Some(quote @ (b'"' | b'\'')) = value.as_bytes().first().copied() {
        let end = 1 + memchr::memchr(quote, &value.as_bytes()[1..])?;
        return Some((
            Attribute {
                name,
                value: Some(&value[1..end]),
            },
            &value[end + 1..],
        ));
    }
    let width = value
        .bytes()
        .take_while(|byte| {
            !byte.is_ascii_whitespace() && !matches!(byte, b'>' | b'<' | b'=' | b'"' | b'\'' | b'`')
        })
        .count();
    (width != 0).then(|| {
        (
            Attribute {
                name,
                value: Some(&value[..width]),
            },
            &value[width..],
        )
    })
}

/// Publish body bytes until a possible close of the current element.
pub(crate) fn pending_close(source: &str, start: usize, end: usize, tag: Tag) -> usize {
    let bytes = source.as_bytes();
    let name = tag.tag_name(source).as_bytes();
    for relative in memchr::memchr_iter(b'<', &bytes[start..end]) {
        let offset = start + relative;
        let after_lt = &bytes[offset + 1..end];
        if after_lt.is_empty() {
            return offset;
        }
        let Some(after_slash) = after_lt.strip_prefix(b"/") else {
            continue;
        };
        if name.starts_with(after_slash)
            || after_slash.strip_prefix(name).is_some_and(|tail| {
                tail.first()
                    .is_some_and(|byte| matches!(byte, b'>' | b' ' | b'\t'))
            })
        {
            return offset;
        }
    }
    end
}

pub(crate) fn close(
    source: &str,
    start: usize,
    end: usize,
    tag: Tag,
    depth: &mut usize,
) -> Option<Span> {
    let name = tag.tag_name(source);
    let mut offset = start;
    while let Some(index) = memchr::memchr(b'<', &source.as_bytes()[offset..end]) {
        offset += index;
        let rest = &source[offset..end];
        if let Some(after) = rest
            .strip_prefix("</")
            .and_then(|rest| rest.strip_prefix(name))
        {
            if let Some(after) = after.trim_start_matches([' ', '\t']).strip_prefix('>') {
                *depth -= 1;
                if *depth == 0 {
                    return Some(Span::new(offset, end - after.len()));
                }
                offset = end - after.len();
                continue;
            }
        } else if let Some(inner) = opening(source, offset, end) {
            if inner.tag.tag_name(source) == name && !inner.self_closing {
                *depth += 1;
            }
            offset = inner.end;
            continue;
        }
        offset += 1;
    }
    None
}

use crate::{AdmonitionKind, BlockId};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Style {
    Bullet(u8),
    Ordered(u8),
}

#[derive(Clone, Copy)]
pub(crate) struct ListMarker {
    pub style: Style,
    pub ordinal: u64,
    pub indent: usize,
    pub content: usize,
}

impl ListMarker {
    pub fn interrupts(self, line: &str) -> bool {
        (!matches!(self.style, Style::Ordered(_)) || self.ordinal == 1)
            && !line[self.content..].trim_matches([' ', '\t']).is_empty()
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Quote,
    Admonition {
        header_end: usize,
        kind: AdmonitionKind,
        quoted: bool,
        closed: bool,
        preview_closed: bool,
    },
    Html {
        opening_end: usize,
        closed: bool,
        tag: crate::block::details::Name,
    },
    Vmr {
        header_end: usize,
        closed: bool,
        preview_closed: bool,
    },
    List {
        style: Style,
        ordinal: u64,
        tight: bool,
    },
    Item {
        indent: usize,
        blank: bool,
        has_content: bool,
        task: Option<bool>,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct Frame {
    pub id: BlockId,
    pub output_index: usize,
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

fn list_marker(bytes: &[u8], leading: usize) -> Option<(Style, usize, u64)> {
    Some(match bytes.get(leading).copied()? {
        marker @ (b'-' | b'+' | b'*') => (Style::Bullet(marker), 0, 1),
        b'0'..=b'9' => {
            let digits = bytes[leading..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .take(10)
                .count();
            if digits == 10 {
                return None;
            }
            let marker @ (b'.' | b')') = bytes.get(leading + digits).copied()? else {
                return None;
            };
            let ordinal = bytes[leading..leading + digits]
                .iter()
                .fold(0u64, |value, byte| value * 10 + u64::from(*byte - b'0'));
            (Style::Ordered(marker), digits, ordinal)
        }
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Prefix {
    pub bytes: usize,
    pub columns: usize,
    pub padding: usize,
}

pub(crate) fn whitespace(bytes: &[u8], column: usize, padding: usize, max: usize) -> Prefix {
    let mut prefix = Prefix {
        bytes: 0,
        columns: padding.min(max),
        padding: padding.saturating_sub(max),
    };
    for &byte in bytes {
        if prefix.columns == max {
            break;
        }
        let width = match byte {
            b' ' => 1,
            b'\t' => 4 - (column + prefix.columns) % 4,
            _ => break,
        };
        let consumed = width.min(max - prefix.columns);
        prefix.bytes += 1;
        prefix.columns += consumed;
        prefix.padding = width - consumed;
    }
    prefix
}

pub(crate) fn quote_at(line: &str, column: usize, padding: usize) -> Option<Prefix> {
    let bytes = line.as_bytes();
    let leading = whitespace(bytes, column, padding, 3);
    if leading.padding != 0 || bytes.get(leading.bytes) != Some(&b'>') {
        return None;
    }
    Some(quote_after_indent(bytes, column, leading))
}

fn quote_after_indent(bytes: &[u8], column: usize, leading: Prefix) -> Prefix {
    let marker_end = leading.bytes + 1;
    let columns = leading.columns + 1;
    let following = whitespace(&bytes[marker_end..], column + columns, 0, 1);
    Prefix {
        bytes: marker_end + following.bytes,
        columns: columns + following.columns,
        padding: following.padding,
    }
}

pub(crate) fn list_at(line: &str, column: usize, padding: usize) -> Option<(ListMarker, Prefix)> {
    let bytes = line.as_bytes();
    let leading = whitespace(bytes, column, padding, 3);
    if leading.padding != 0 {
        return None;
    }
    list_after_indent(bytes, column, leading)
}

pub(crate) enum Opening {
    Quote(Prefix),
    List(ListMarker, Prefix),
}

pub(crate) fn opening_at(line: &str, column: usize, padding: usize) -> Option<Opening> {
    let bytes = line.as_bytes();
    let leading = whitespace(bytes, column, padding, 3);
    opening_after_indent(bytes, column, leading)
}

pub(crate) fn opening_after_indent(
    bytes: &[u8],
    column: usize,
    leading: Prefix,
) -> Option<Opening> {
    if leading.columns > 3 || leading.padding != 0 {
        return None;
    }
    if bytes.get(leading.bytes) == Some(&b'>') {
        return Some(Opening::Quote(quote_after_indent(bytes, column, leading)));
    }
    list_after_indent(bytes, column, leading).map(|(marker, prefix)| Opening::List(marker, prefix))
}

fn list_after_indent(bytes: &[u8], column: usize, leading: Prefix) -> Option<(ListMarker, Prefix)> {
    let (style, digits, ordinal) = list_marker(bytes, leading.bytes)?;
    let width = digits + 1;
    let marker_end = leading.bytes + width;
    let columns = leading.columns + width;
    let rest = &bytes[marker_end..];
    let mut following = whitespace(rest, column + columns, 0, usize::MAX);
    let indent = if following.bytes == rest.len() {
        columns + 1
    } else {
        if following.bytes == 0 {
            return None;
        }
        if following.columns > 4 {
            following = whitespace(rest, column + columns, 0, 1);
        }
        columns + following.columns
    };
    let prefix = Prefix {
        bytes: marker_end + following.bytes,
        columns: columns + following.columns,
        padding: following.padding,
    };
    Some((
        ListMarker {
            style,
            ordinal,
            indent,
            content: prefix.bytes,
        },
        prefix,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_tab_preserves_indent_for_code() {
        let line = ">\t\tcode";
        let quote = quote_at(line, 0, 0).unwrap();
        assert_eq!(
            quote,
            Prefix {
                bytes: 2,
                columns: 2,
                padding: 2
            }
        );
        let code = whitespace(
            &line.as_bytes()[quote.bytes..],
            quote.columns,
            quote.padding,
            4,
        );
        assert_eq!(
            code,
            Prefix {
                bytes: 1,
                columns: 4,
                padding: 2
            }
        );
        assert_eq!(&line[quote.bytes + code.bytes..], "code");
        assert!(quote_at("\t> code", 0, 0).is_none());
    }

    #[test]
    fn nested_list_uses_absolute_tab_stops_and_preserves_leftover_padding() {
        let line = ">\t-\t\t中文";
        let quote = quote_at(line, 0, 0).unwrap();
        let (list, prefix) = list_at(&line[quote.bytes..], quote.columns, quote.padding).unwrap();
        assert!(matches!(list.style, Style::Bullet(b'-')));
        assert_eq!(list.indent, 4);
        assert_eq!(list.content, 2);
        assert_eq!(
            prefix,
            Prefix {
                bytes: 2,
                columns: 4,
                padding: 2
            }
        );

        let (list, prefix) = list_at("\t12.\tx", 1, 0).unwrap();
        assert!(matches!(list.style, Style::Ordered(b'.')));
        assert_eq!(list.ordinal, 12);
        assert_eq!(list.indent, 7);
        assert_eq!(
            prefix,
            Prefix {
                bytes: 5,
                columns: 7,
                padding: 0
            }
        );
    }

    #[test]
    fn blank_list_item_requires_one_column_after_the_marker() {
        let (list, prefix) = list_at("-\t  ", 0, 0).unwrap();
        assert_eq!(list.indent, 2);
        assert_eq!(list.content, 4);
        assert_eq!(
            prefix,
            Prefix {
                bytes: 4,
                columns: 6,
                padding: 0
            }
        );
    }
}

pub const CASES: [&str; 8] = [
    "plain",
    "mixed",
    "long-paragraph",
    "open-fence",
    "table",
    "list",
    "late-reference",
    "real-chat",
];

pub const SIZES: [(&str, usize); 5] = [
    ("small", 1 << 10),
    ("medium", 16 << 10),
    ("large", 128 << 10),
    ("huge", 1 << 20),
    ("very-huge", 8 << 20),
];

const MIXED: &str = "## Answer\n\n中文 **strong** and _emphasis_ [link](/docs) with `code`.\n\n```rust\nlet answer = 42;\n```\n\n---\n\n";
const REAL_CHAT: &str = include_str!("../../../fixtures/corpus/ai-chat-streaming.md");

pub fn source(case: &str, target: usize) -> Option<String> {
    let (prefix, unit, suffix) = match case {
        "plain" => (
            "",
            "This answer explains the result without markup. 中文也会出现在这里。\n\n",
            "",
        ),
        "mixed" => ("", MIXED, ""),
        "long-paragraph" => (
            "",
            "中文 **strong** and [link](/docs) with `code` 🙂. ",
            "\n",
        ),
        "open-fence" => ("```rust\n", "let result = (42, \"example 中文\");\n", ""),
        "table" => (
            "| Name | Value |\n| --- | :---: |\n",
            "| **text** | [link](/docs) |\n",
            "",
        ),
        "list" => ("", "- top **text**\n  - nested [link](/docs)\n", ""),
        "late-reference" => (
            "A [link][late].\n\n",
            "A **stable** paragraph with no dependency.\n\n",
            "[late]: https://example.com \"late title\"\n",
        ),
        "real-chat" => ("", REAL_CHAT, ""),
        _ => return None,
    };
    let mut source = String::with_capacity(target.max(unit.len()));
    source.push_str(prefix);
    if case == "real-chat" && target < unit.len() {
        let end = boundary_at_or_before(unit, target);
        source.push_str(&unit[..end]);
    } else {
        let body_bytes = target.saturating_sub(prefix.len() + suffix.len());
        let repeats = (body_bytes / unit.len()).max(1);
        for _ in 0..repeats {
            source.push_str(unit);
        }
    }
    source.push_str(suffix);
    Some(source)
}

pub fn boundary_at_or_before(source: &str, mut offset: usize) -> usize {
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub fn chunk_ends(source: &str, bytes: usize) -> Vec<usize> {
    let mut ends = Vec::with_capacity(source.len().div_ceil(bytes));
    let mut start = 0;
    while start < source.len() {
        let candidate = start + bytes.min(source.len() - start);
        let end = boundary_at_or_before(source, candidate);
        // A requested width below four bytes may still need to fit one code point.
        let end = if end == start {
            start + source[start..].chars().next().unwrap().len_utf8()
        } else {
            end
        };
        ends.push(end);
        start = end;
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_preserve_multibyte_input() {
        let source = "a中文😀x";
        for bytes in 1..=source.len() {
            let mut start = 0;
            let chunks = chunk_ends(source, bytes)
                .into_iter()
                .map(|end| {
                    let chunk = &source[start..end];
                    assert!(chunk.len() <= bytes || chunk.chars().count() == 1);
                    start = end;
                    chunk
                })
                .collect::<Vec<_>>();
            assert_eq!(chunks.concat(), source);
        }
    }
}

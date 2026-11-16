//! JSON-lines transport for the JavaScript compatibility tests.

use serde_json::{Value, json};
use std::{
    collections::{HashMap, hash_map::Entry},
    io::{self, BufRead, BufWriter, Write},
};
use stream_markdown_rs::{
    Alignment, Block, BlockData, BlockStatus, Change, HtmlOpening, Inline, InlineContent,
    InlineKind, Options, Parser, PluginField, PluginFieldValue, Preview, PreviewHint, RawLine,
    Span, Syntax, TableData, Text,
};

fn main() -> io::Result<()> {
    let mut sessions = HashMap::new();
    let input = io::stdin().lock();
    let mut output = BufWriter::new(io::stdout().lock());
    for line in input.lines() {
        let line = line?;
        let response = match serde_json::from_str(&line) {
            Ok(input) => match request(&mut sessions, &input) {
                Ok(value) => json!({ "value": value }),
                Err(message) => json!({ "error": message }),
            },
            Err(error) => json!({ "error": error.to_string() }),
        };
        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}

fn request(sessions: &mut HashMap<u64, Parser>, input: &Value) -> Result<Value, String> {
    let session = input["session"].as_u64().ok_or("invalid session ID")?;
    let operation = input["op"].as_str().ok_or("invalid operation")?;
    if operation == "create" {
        let Entry::Vacant(slot) = sessions.entry(session) else {
            return Err("session already exists".into());
        };
        let custom_tags = input["customTags"]
            .as_array()
            .ok_or("expected customTags array")?
            .iter()
            .map(|tag| tag.as_str().map(str::to_owned).ok_or("invalid custom tag"))
            .collect::<Result<_, _>>()?;
        slot.insert(Parser::new(Options {
            syntax: Syntax::Gfm,
            preview: Preview::Ai,
            custom_tags,
            math: input["math"].as_bool().unwrap_or(true),
            footnotes: input["footnotes"].as_bool().unwrap_or(true),
            markstream: input["markstream"].as_bool().unwrap_or(true),
            require_closing_strong: input["requireClosingStrong"].as_bool().unwrap_or(false),
        }));
        return Ok(Value::Null);
    }
    if operation == "dispose" {
        sessions.remove(&session);
        return Ok(Value::Null);
    }
    let parser = sessions.get_mut(&session).ok_or("unknown session")?;
    let mut changes = Vec::new();
    match operation {
        "append" => {
            let chunk = input["chunk"].as_str().ok_or("expected chunk string")?;
            parser.append(chunk, &mut changes)
        }
        "finish" => parser.finish(&mut changes),
        _ => return Err("unknown operation".into()),
    }
    .map_err(|error| error.to_string())?;
    let source = parser.source_text();
    let metrics = parser.metrics();
    Ok(json!({
        "changes": changes.iter().map(|change| change_value(change, source)).collect::<Vec<_>>(),
        "metrics": {
            "inputBytes": metrics.input_bytes,
            "scannedBytes": metrics.scanned_bytes,
            "inlineBytes": metrics.inline_bytes,
            "invalidatedBlocks": metrics.invalidated_blocks,
        },
    }))
}

fn span(value: Span) -> [usize; 2] {
    [value.start, value.end]
}

fn text(value: &Text) -> Value {
    match value {
        Text::Source(range) => json!(span(*range)),
        Text::Decoded(value) => json!(value.as_ref()),
    }
}

fn inline(value: &Inline) -> Value {
    let (kind, data) = match &value.kind {
        InlineKind::Text(value) => ("text", text(value)),
        InlineKind::Code(value) => ("code", text(value)),
        InlineKind::Math { body, loading } => (
            "math_inline",
            json!({ "body": text(body), "loading": loading }),
        ),
        InlineKind::NumericReference(label) => ("reference", json!({ "label": span(*label) })),
        InlineKind::FootnoteReference { label, .. } => {
            ("footnote_reference", json!({ "label": span(*label) }))
        }
        InlineKind::Emphasis(children) => ("emphasis", json!(inlines(children))),
        InlineKind::Strong(children) => ("strong", json!(inlines(children))),
        InlineKind::Strikethrough(children) => ("strikethrough", json!(inlines(children))),
        InlineKind::Subscript(body) => ("subscript", text(body)),
        InlineKind::Superscript(body) => ("superscript", text(body)),
        InlineKind::Highlight(children) => ("highlight", json!(inlines(children))),
        InlineKind::Insert(children) => ("insert", json!(inlines(children))),
        InlineKind::Link(link)
        | InlineKind::LinkPreview(link)
        | InlineKind::Image(link)
        | InlineKind::ImagePreview(link) => {
            let kind = if matches!(value.kind, InlineKind::Link(_) | InlineKind::LinkPreview(_)) {
                "link"
            } else {
                "image"
            };
            (
                kind,
                json!({
                    "destination": text(&link.destination),
                    "title": link.title.as_ref().map(text),
                    "loading": matches!(value.kind, InlineKind::ImagePreview(_) | InlineKind::LinkPreview(_)),
                    "children": inlines(&link.children),
                }),
            )
        }
        InlineKind::Html => ("html", Value::Null),
        InlineKind::SoftBreak => ("softbreak", Value::Null),
        InlineKind::HardBreak => ("hardbreak", Value::Null),
        InlineKind::Extension(kind) => ("extension", json!({ "kind": kind })),
        InlineKind::ExtensionWithFields { kind, fields } => (
            "extension",
            json!({
                "kind": kind,
                "fields": fields_value(fields, value.span),
            }),
        ),
    };
    json!({ "kind": kind, "span": span(value.span), "data": data })
}

fn inlines(values: &[Inline]) -> Vec<Value> {
    values.iter().map(inline).collect()
}

fn content(value: &InlineContent) -> Value {
    match value {
        InlineContent::Plain(range) => json!({ "plain": span(*range) }),
        InlineContent::Fragments(ranges) => {
            json!({ "fragments": ranges.iter().copied().map(span).collect::<Vec<_>>() })
        }
        InlineContent::Parsed(values) => json!({ "parsed": inlines(values) }),
    }
}

fn raw_line(value: RawLine) -> Value {
    json!({ "span": span(value.span), "leading": value.leading_spaces })
}

fn block(value: &Block, source: &str) -> Value {
    let (kind, data) = match &value.data {
        BlockData::Paragraph(value) => ("paragraph", content(value)),
        BlockData::Heading {
            level,
            content: value,
        } => (
            "heading",
            json!({ "level": level, "content": content(value) }),
        ),
        BlockData::CodeBlock { info, body } => (
            "code_block",
            json!({ "info": info.map(span), "body": span(*body) }),
        ),
        BlockData::SegmentedCode { info, lines } => (
            "code_block",
            json!({ "info": info.map(span), "lines": lines.iter().copied().map(raw_line).collect::<Vec<_>>() }),
        ),
        BlockData::IndentedCode { lines } => (
            "indented_code",
            json!({ "lines": lines.iter().copied().map(raw_line).collect::<Vec<_>>() }),
        ),
        BlockData::Details(opening) => ("details", html_opening_value(*opening, source)),
        BlockData::Summary {
            opening,
            content: value,
        } => {
            let mut opening = html_opening_value(*opening, source);
            opening["content"] = content(value);
            ("summary", opening)
        }
        BlockData::ThematicBreak => ("thematic_break", Value::Null),
        BlockData::Math { body } => ("math_block", json!({ "body": text(body) })),
        BlockData::Footnote {
            label,
            content: value,
        } => (
            "footnote",
            json!({ "label": span(*label), "content": content(value) }),
        ),
        BlockData::Vmr(vmr) => {
            let parameters = vmr.parameters(source);
            (
                "vmr_container",
                json!({
                    "header": span(vmr.header),
                    "name": span(parameters.name),
                    "args": parameters.args.map(span),
                    "attributes": parameters.attributes.map(span),
                }),
            )
        }
        BlockData::Quote => ("blockquote", Value::Null),
        BlockData::List { ordered, start, .. } => {
            ("list", json!({ "ordered": ordered, "start": start }))
        }
        BlockData::ListItem { task } => ("list_item", json!({ "task": task })),
        BlockData::Html => ("html_block", json!({})),
        BlockData::SegmentedHtml(lines) => (
            "html_block",
            json!({ "lines": lines.iter().copied().map(raw_line).collect::<Vec<_>>() }),
        ),
        BlockData::Table(table) => ("table", table_value(table)),
        BlockData::CustomTag { tag, body } => (
            "custom",
            json!({
                "name": tag.tag_name(source),
                "attrs": tag.attrs(source).map(|attr| (attr.name, attr.value.unwrap_or_default())).collect::<Vec<_>>(),
                "body": span(*body),
            }),
        ),
        BlockData::Extension { kind } => ("extension", json!({ "kind": kind })),
        BlockData::ExtensionWithFields { kind, fields } => (
            "extension",
            json!({
                "kind": kind,
                "fields": fields_value(fields, value.span),
            }),
        ),
    };
    json!({
        "id": value.id.0,
        "span": span(value.span),
        "status": status_name(value.status),
        "kind": kind,
        "data": data,
    })
}

fn html_opening_value(opening: HtmlOpening, source: &str) -> Value {
    json!({
        "opening": span(opening.span),
        "attrs": opening.tag(source).attrs(source)
            .map(|attr| (attr.name, attr.value.unwrap_or_default()))
            .collect::<Vec<_>>(),
    })
}

fn status_name(status: BlockStatus) -> &'static str {
    match status {
        BlockStatus::Open => "open",
        BlockStatus::Closed => "closed",
        BlockStatus::Incomplete => "incomplete",
    }
}

fn table_value(table: &TableData) -> Value {
    json!({
        "alignments": table.alignments().iter().map(|align| match align {
            Alignment::None | Alignment::Left => "left",
            Alignment::Center => "center",
            Alignment::Right => "right",
        }).collect::<Vec<_>>(),
        "header": table.header().iter().map(content).collect::<Vec<_>>(),
        "rows": table.rows().map(|row| row.iter().map(content).collect::<Vec<_>>()).collect::<Vec<_>>(),
    })
}

fn fields_value(fields: &[PluginField], matched: Span) -> Vec<Value> {
    fields
        .iter()
        .map(|field| {
            let value = match field.value {
                PluginFieldValue::Text { start, end } => {
                    json!([matched.start + start as usize, matched.start + end as usize,])
                }
                PluginFieldValue::Integer(value) => json!(value),
                PluginFieldValue::Bool(value) => json!(value),
            };
            json!([field.id, value])
        })
        .collect()
}

fn change_value(change: &Change, source: &str) -> Value {
    match change {
        Change::Insert {
            parent,
            block: value,
        } => {
            json!({ "op": "insert", "parent": parent.map(|id| id.0), "block": block(value, source) })
        }
        Change::Replace(value) => json!({ "op": "replace", "block": block(value, source) }),
        Change::AppendText { id, span: range } => {
            json!({ "op": "append_text", "id": id.0, "span": span(*range) })
        }
        Change::AddTextFragment { id, span: range } => {
            json!({ "op": "add_text_fragment", "id": id.0, "span": span(*range) })
        }
        Change::AddRawLine { id, line, end } => {
            json!({ "op": "add_raw_line", "id": id.0, "line": raw_line(*line), "end": end })
        }
        Change::AddRow { id, end, cells } => {
            json!({ "op": "add_row", "id": id.0, "end": end, "cells": cells.iter().map(content).collect::<Vec<_>>() })
        }
        Change::ReplaceTableCell {
            id,
            index,
            content: value,
        } => {
            json!({ "op": "replace_cell", "id": id.0, "index": index, "cell": content(value) })
        }
        Change::SetStatus { id, status, end } => {
            json!({ "op": "status", "id": id.0, "status": status_name(*status), "end": end })
        }
        Change::Remove { id } => json!({ "op": "remove", "id": id.0 }),
        Change::PreviewInlines { id, nodes } => {
            json!({ "op": "preview_inlines", "id": id.0, "nodes": inlines(nodes) })
        }
        Change::Preview { id, hint } => {
            json!({ "op": "preview", "id": id.0, "hint": hint.as_ref().map(|hint| match hint {
                PreviewHint::Table { table, loading } => json!({ "kind": "table", "data": table_value(table), "loading": loading }),
                PreviewHint::TableRow(cells) => json!({ "kind": "table_row", "cells": cells.iter().map(content).collect::<Vec<_>>() }),
            }) })
        }
    }
}

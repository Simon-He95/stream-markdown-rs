use std::collections::BTreeMap;

use stream_markdown_rs::{
    Block, BlockData, BlockId, BlockStatus, Change, Inline, InlineContent, InlineKind, Options,
    Parser, Preview, PreviewHint, Text, parse,
};

#[derive(Default)]
struct View {
    blocks: BTreeMap<BlockId, Block>,
    previews: BTreeMap<BlockId, Vec<Inline>>,
    hints: BTreeMap<BlockId, PreviewHint>,
}

impl View {
    fn apply(&mut self, changes: &mut Vec<Change>) {
        for change in changes.drain(..) {
            match change {
                Change::Insert { block, .. } => {
                    assert!(self.blocks.insert(block.id, block).is_none());
                }
                Change::Replace(block) => {
                    self.clear_preview(block.id);
                    assert!(self.blocks.insert(block.id, block).is_some());
                }
                Change::Remove { id } => {
                    self.clear_preview(id);
                    assert!(self.blocks.remove(&id).is_some());
                }
                Change::AppendText { id, span } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    let body = match &mut block.data {
                        BlockData::Paragraph(InlineContent::Plain(body))
                        | BlockData::Footnote {
                            content: InlineContent::Plain(body),
                            ..
                        }
                        | BlockData::Heading {
                            content: InlineContent::Plain(body),
                            ..
                        }
                        | BlockData::Summary {
                            content: InlineContent::Plain(body),
                            ..
                        }
                        | BlockData::Math {
                            body: Text::Source(body),
                        }
                        | BlockData::CodeBlock { body, .. } => body,
                        BlockData::CustomTag(element) => &mut element.body,
                        _ => panic!("unexpected text update: {block:?}"),
                    };
                    assert_eq!(span.start, body.end);
                    body.end = span.end;
                    block.span.end = span.end;
                }
                Change::ReplaceTableCell { id, index, content } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    let BlockData::Table(table) = &mut block.data else {
                        panic!("replaced a cell in non-table block {id:?}");
                    };
                    table.replace_cell(index, content);
                }
                Change::AddRow { id, end, cells } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    let BlockData::Table(table) = &mut block.data else {
                        panic!("unexpected row update: {block:?}");
                    };
                    table.add_row(cells);
                    block.span.end = end;
                }
                Change::SetStatus { id, status, end } => {
                    if matches!(status, BlockStatus::Closed | BlockStatus::Incomplete) {
                        self.clear_preview(id);
                    }
                    let block = self.blocks.get_mut(&id).unwrap();
                    block.status = status;
                    block.span.end = end;
                }
                Change::CloseInlineElement { id, start, end } => {
                    let block = self.blocks.get_mut(&id).unwrap();
                    let content = match &mut block.data {
                        BlockData::Paragraph(content) | BlockData::Heading { content, .. } => {
                            content
                        }
                        _ => panic!("invalid inline closer target: {id:?}"),
                    };
                    assert!(content.close_inline_element(start, end));
                }
                Change::Preview { id, hint } => {
                    if let Some(hint) = hint {
                        self.hints.insert(id, hint);
                    } else {
                        self.hints.remove(&id);
                    }
                }
                Change::PreviewInlines { id, nodes } => {
                    if nodes.is_empty() {
                        self.previews.remove(&id);
                    } else {
                        self.previews.insert(id, nodes);
                    }
                }
                change => panic!("unexpected nested change: {change:?}"),
            }
        }
    }

    fn clear_preview(&mut self, id: BlockId) {
        self.previews.remove(&id);
        self.hints.remove(&id);
    }

    fn assert_no_previews(&self) {
        assert!(
            self.previews.is_empty(),
            "inline previews survived: {:?}",
            self.previews
        );
        assert!(
            self.hints.is_empty(),
            "block previews survived: {:?}",
            self.hints
        );
    }
}

fn preview_kinds(nodes: &[Inline]) -> Vec<&'static str> {
    let mut found = Vec::new();
    for node in nodes {
        match &node.kind {
            InlineKind::LinkPreview(_) => found.push("link preview"),
            InlineKind::ImagePreview(_) => found.push("image preview"),
            InlineKind::CodePreview(_) => found.push("code preview"),
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children)
            | InlineKind::Highlight(children)
            | InlineKind::Insert(children) => found.extend(preview_kinds(children)),
            InlineKind::Link(link) | InlineKind::Image(link) => {
                found.extend(preview_kinds(&link.children));
            }
            _ => {}
        }
    }
    found
}

fn nodes_of(block: &Block) -> &[Inline] {
    match &block.data {
        BlockData::Paragraph(InlineContent::Parsed(nodes))
        | BlockData::Heading {
            content: InlineContent::Parsed(nodes),
            ..
        } => nodes,
        _ => &[],
    }
}

fn walk(nodes: &[Inline], out: &mut Vec<Inline>) {
    for node in nodes {
        out.push(node.clone());
        match &node.kind {
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children)
            | InlineKind::Highlight(children)
            | InlineKind::Insert(children) => walk(children, out),
            InlineKind::Link(link)
            | InlineKind::LinkPreview(link)
            | InlineKind::Image(link)
            | InlineKind::ImagePreview(link) => walk(&link.children, out),
            InlineKind::CustomTag(element) => walk(&element.children, out),
            _ => {}
        }
    }
}

fn walk_content(content: &InlineContent, out: &mut Vec<Inline>) {
    if let InlineContent::Parsed(nodes) = content {
        walk(nodes, out);
    }
}

fn all_nodes(blocks: &[Block]) -> Vec<Inline> {
    let mut out = Vec::new();
    for block in blocks {
        match &block.data {
            BlockData::Paragraph(content)
            | BlockData::Heading { content, .. }
            | BlockData::Footnote { content, .. }
            | BlockData::Summary { content, .. } => walk_content(content, &mut out),
            BlockData::Table(table) => {
                for cell in table.header().iter().chain(table.rows().flatten()) {
                    walk_content(cell, &mut out);
                }
            }
            BlockData::CustomTag(element) => walk(&element.children, &mut out),
            _ => {}
        }
    }
    out
}

fn block_visible_text(source: &str, block: &Block) -> String {
    match &block.data {
        BlockData::Paragraph(InlineContent::Plain(span)) => span_text(source, *span).to_owned(),
        BlockData::Paragraph(InlineContent::Parsed(nodes))
        | BlockData::Heading {
            content: InlineContent::Parsed(nodes),
            ..
        } => visible_text(source, nodes),
        _ => String::new(),
    }
}

fn has_target(nodes: &[Inline]) -> bool {
    nodes.iter().any(|node| match &node.kind {
        InlineKind::Link(_) | InlineKind::Image(_) => true,
        InlineKind::Emphasis(children)
        | InlineKind::Strong(children)
        | InlineKind::Strikethrough(children)
        | InlineKind::Highlight(children)
        | InlineKind::Insert(children) => has_target(children),
        InlineKind::LinkPreview(link) | InlineKind::ImagePreview(link) => {
            has_target(&link.children)
        }
        _ => false,
    })
}

fn has_image(nodes: &[Inline]) -> bool {
    nodes.iter().any(|node| match &node.kind {
        InlineKind::Image(_) | InlineKind::ImagePreview(_) => true,
        InlineKind::Emphasis(children)
        | InlineKind::Strong(children)
        | InlineKind::Strikethrough(children)
        | InlineKind::Highlight(children)
        | InlineKind::Insert(children) => has_image(children),
        InlineKind::Link(link) | InlineKind::LinkPreview(link) => has_image(&link.children),
        _ => false,
    })
}

fn span_text(source: &str, span: stream_markdown_rs::Span) -> &str {
    &source[span.start..span.end]
}

fn visible_text(source: &str, nodes: &[Inline]) -> String {
    let mut out = String::new();
    for node in nodes {
        match &node.kind {
            InlineKind::Text(text)
            | InlineKind::Code(text)
            | InlineKind::CodePreview(text)
            | InlineKind::Subscript(text)
            | InlineKind::Superscript(text) => out.push_str(text.as_str(source)),
            InlineKind::Math { body, .. } => {
                out.push('$');
                out.push_str(body.as_str(source));
                out.push('$');
            }
            InlineKind::Emphasis(children)
            | InlineKind::Strong(children)
            | InlineKind::Strikethrough(children)
            | InlineKind::Highlight(children)
            | InlineKind::Insert(children) => out.push_str(&visible_text(source, children)),
            InlineKind::Link(link) | InlineKind::LinkPreview(link) => {
                out.push_str(&visible_text(source, &link.children));
            }
            InlineKind::Image(_) | InlineKind::ImagePreview(_) => {}
            InlineKind::NumericReference(label) => out.push_str(span_text(source, *label)),
            InlineKind::SoftBreak | InlineKind::HardBreak => out.push('\n'),
            InlineKind::Html => out.push_str(span_text(source, node.span)),
            InlineKind::FootnoteReference { label, .. } => {
                out.push_str(span_text(source, *label));
            }
            InlineKind::CustomTag(_)
            | InlineKind::Extension(_)
            | InlineKind::ExtensionWithFields { .. } => {
                out.push_str(span_text(source, node.span));
            }
        }
    }
    out
}

fn final_view(source: &str, options: &Options) -> (Vec<Block>, View) {
    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    for ch in source.chars() {
        let mut buffer = [0; 4];
        parser
            .append(ch.encode_utf8(&mut buffer), &mut changes)
            .unwrap();
        view.apply(&mut changes);
    }
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    assert_eq!(parser.metrics().scanned_bytes, source.len());
    (view.blocks.values().cloned().collect(), view)
}

fn options() -> Options {
    Options {
        preview: Preview::Ai,
        math: true,
        markstream: true,
        ..Options::default()
    }
}

#[test]
fn malformed_inline_targets_finalize_like_a_complete_parse_without_previews() {
    let sources = [
        "![a](https://e.com/a.png",
        "[a](https://e.com/a.png",
        "![a](https://e.com/a b.png)",
        "[a](https://e.com/a b.png)",
        "![a](https://e.com/a b.png) tail",
        "see ![a](https://e.com/a b.png) end",
        "[a] (https://e.com/l)",
        "![a] (https://e.com/l)",
        "[a]  (https://e.com/l)",
        "![a](ftp://e.com/a.png",
        "[a](ftp://e.com/a.png",
        "[a](ftp://e.com/a.png)",
        "[a](javascript:alert(1))",
        "![a](javascript:alert(1))",
        "![a](javascript:alert(1)) tail",
        "vec![1, 2, 3] to build it.",
        "Hello![world] there",
        "![alt]",
        "![alt] ",
        "`code",
        "`code with **strong**",
        "好的![微笑] 我们继续。",
        "**vec![1, 2]** ok",
        "[![img](a.png)]( http://s",
        "See [![logo](a.png)]( http://s",
        "See the [docs for *streaming*](https://example.com/guide",
        "[www.trusted.com/login] (<https://www.trusted.com>@evil.com",
        "![a](https://e.com/a.png \"t",
        "![a](https://e.com/a.png \"t\")",
        "![a](www.e.com/a.png",
        "![a](例子.com",
        "![a](user@example.com",
        "[a](www.e.com/a.png",
        "![a](<https://e.com/a.png",
        "![](https://e.com/a.png",
        "![$x$](https://e.com/a.png",
        "![a](mailto:x@example.com",
        "![a $x$](https://e.com/a.png",
        "![a](https://e.com/a.png\nnext line",
        "![a](https://e.com/a.png\n\nnext paragraph",
        "![a](https://e.com/a.png\n# heading",
        "> ![a](https://e.com/a b.png)",
        "- [x](http://s",
        "**![a](https://e.com/a b.png)** ok",
        "text [x](http://s z",
        "::: tip\n<https://example.com>\n:::",
        "::: tip\n[a *em*](http://s\n:::",
        "::: tip\n[x](https://ok.com \"title\"\n:::",
        "::: tip\n[x](http://s \"t\n:::",
        "**[x](https://ok.com \"title\"**",
        "[x](http://s \"t",
        "[see] (<https://a.com>)",
        "[x] (http://s)",
        "| h |\n|---|\n| [x](http://s |",
        "| A | B |\n| - | - |\n| ![a](https://e.com/a.png | x |",
        "- [a](https://e.com/a.png",
        "好的 [说明](https://e.com/中文",
    ];
    let options = options();
    for source in sources {
        let expected = parse(source, &options).unwrap().nodes;
        let (actual, view) = final_view(source, &options);
        assert_eq!(actual, expected, "{source:?}");
        view.assert_no_previews();
        assert_eq!(
            preview_kinds(&all_nodes(&actual)),
            Vec::<&str>::new(),
            "{source:?}"
        );
        assert!(
            actual
                .iter()
                .all(|block| matches!(block.status, BlockStatus::Closed | BlockStatus::Incomplete)),
            "{source:?}"
        );
    }
}

#[test]
fn unclosed_and_dead_targets_never_remain_images_or_links() {
    let options = options();
    for source in [
        "![a](https://e.com/a.png",
        "![a](https://e.com/a b.png)",
        "![a] (https://e.com/l)",
        "![a](ftp://e.com/a.png",
        "![a](<https://e.com/a.png",
        "![](https://e.com/a.png",
        "![$x$](https://e.com/a.png",
        "![a $x$](https://e.com/a.png",
        "![a](mailto:x@example.com",
    ] {
        let blocks = parse(source, &options).unwrap().nodes;
        let images = all_nodes(&blocks)
            .into_iter()
            .filter(|node| matches!(node.kind, InlineKind::Image(_)))
            .count();
        assert_eq!(images, 0, "{source:?}");
        assert_eq!(
            visible_text(source, nodes_of(&blocks[0])),
            source,
            "{source:?}"
        );
    }
}

#[test]
fn malformed_targets_keep_their_source_text_and_linkified_tail() {
    let options = options();
    let cases = [
        ("![a](https://e.com/a.png", "![a](", "https://e.com/a.png"),
        ("[a](https://e.com/a.png", "[a](", "https://e.com/a.png"),
        ("![a](www.e.com/a.png", "![a](", "www.e.com/a.png"),
        ("![a](user@example.com", "![a](", "user@example.com"),
        ("[a](www.e.com/a.png", "[a](", "www.e.com/a.png"),
    ];
    for (source, prefix, label) in cases {
        let blocks = parse(source, &options).unwrap().nodes;
        let nodes = nodes_of(&blocks[0]);
        assert_eq!(visible_text(source, nodes), source, "{source:?}");
        let Some(Inline {
            kind: InlineKind::Link(link),
            ..
        }) = nodes
            .iter()
            .find(|node| matches!(node.kind, InlineKind::Link(_)))
        else {
            panic!("expected a linkified tail for {source:?}: {nodes:?}");
        };
        assert_eq!(visible_text(source, &link.children), label, "{source:?}");
        assert!(
            visible_text(source, &nodes[..nodes.len() - 1]).starts_with(prefix),
            "{source:?}"
        );
    }
}

#[test]
fn spaced_brackets_are_not_folded_into_links() {
    let options = options();
    for (source, expected_visible) in [
        ("[a] (https://e.com/l)", "[a] (https://e.com/l)"),
        ("![a] (https://e.com/l)", "![a] (https://e.com/l)"),
        ("[a]  (https://e.com/l)", "[a]  (https://e.com/l)"),
        ("[x] (http://s)", "[x] (http://s)"),
        ("[see] (<https://a.com>)", "[see] (https://a.com)"),
    ] {
        let blocks = parse(source, &options).unwrap().nodes;
        let nodes = nodes_of(&blocks[0]);
        assert_eq!(visible_text(source, nodes), expected_visible, "{source:?}");
        assert!(
            nodes
                .iter()
                .all(|node| !matches!(node.kind, InlineKind::Image(_))),
            "{source:?}"
        );
        for node in nodes {
            if let InlineKind::Link(link) = &node.kind {
                assert_eq!(
                    link.children.len(),
                    1,
                    "the link must be the URL itself: {source:?}"
                );
            }
        }
    }
}

#[test]
fn a_closed_image_bracket_that_cannot_be_an_image_stays_text_while_streaming() {
    let options = options();
    for (source, expected_visible) in [
        (
            "Use vec![1, 2, 3] to build it.",
            "Use vec![1, 2, 3] to build it.",
        ),
        ("Hello![world] there", "Hello![world] there"),
        ("好的![微笑] 我们继续。", "好的![微笑] 我们继续。"),
        ("![alt] ", "![alt] "),
        ("![alt]", "![alt]"),
        ("**vec![1, 2]** ok", "**vec![1, 2]** ok"),
    ] {
        let mut parser = Parser::new(options.clone());
        let mut changes = Vec::new();
        let mut view = View::default();
        parser.append(source, &mut changes).unwrap();
        view.apply(&mut changes);
        assert!(
            preview_kinds(
                &view
                    .previews
                    .values()
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>()
            )
            .is_empty(),
            "a confirmed inline overlay can remain while the block is open: {:?}",
            view.previews
        );
        let block = view.blocks.values().last().unwrap();
        match &block.data {
            BlockData::Paragraph(InlineContent::Plain(span)) => {
                assert_eq!(span_text(source, *span), expected_visible);
            }
            BlockData::Paragraph(InlineContent::Parsed(nodes)) => {
                assert_eq!(visible_text(source, nodes), expected_visible, "{source:?}");
                assert!(
                    !has_target(nodes),
                    "a closed bracket became a target before it could be one: {source:?}: {block:?}"
                );
            }
            _ => panic!("unexpected block for {source:?}: {block:?}"),
        }
    }
}

#[test]
fn previews_are_replaced_when_the_target_closes_or_the_session_finishes() {
    let options = options();

    let mut parser = Parser::new(options.clone());
    let mut changes = Vec::new();
    let mut view = View::default();
    parser.append("![alt](", &mut changes).unwrap();
    view.apply(&mut changes);
    assert!(
        view.previews
            .values()
            .flatten()
            .any(|node| matches!(node.kind, InlineKind::ImagePreview(_))),
        "expected an image preview after the destination opens: {:?}",
        view.previews
    );
    parser.append("https://e.com/a.png", &mut changes).unwrap();
    view.apply(&mut changes);
    assert!(
        view.previews
            .values()
            .flatten()
            .any(|node| matches!(node.kind, InlineKind::ImagePreview(_))),
        "expected an image preview while the destination is open"
    );
    parser.append(")", &mut changes).unwrap();
    view.apply(&mut changes);
    assert!(
        view.previews
            .values()
            .flatten()
            .all(|node| matches!(node.kind, InlineKind::Image(_)))
    );
    assert_eq!(
        preview_kinds(
            &view
                .previews
                .values()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
        ),
        Vec::<&str>::new()
    );
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    view.assert_no_previews();
    let block = view.blocks.values().next().unwrap();
    let [
        Inline {
            kind: InlineKind::Image(link),
            ..
        },
    ] = nodes_of(block)
    else {
        panic!("expected a confirmed image: {block:?}");
    };
    assert_eq!(
        link.destination.as_str(parser.source_text()),
        "https://e.com/a.png"
    );

    let mut parser = Parser::new(options.clone());
    parser
        .append("![a](https://e.com/a.png", &mut changes)
        .unwrap();
    view = View::default();
    view.apply(&mut changes);
    assert!(
        view.previews
            .values()
            .flatten()
            .any(|node| matches!(node.kind, InlineKind::ImagePreview(_))),
        "expected an image preview before finalization"
    );
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    view.assert_no_previews();
    let block = view.blocks.values().next().unwrap();
    assert!(
        nodes_of(block).iter().all(|node| !matches!(
            node.kind,
            InlineKind::Image(_) | InlineKind::ImagePreview(_)
        )),
        "finalization kept an image for an unclosed destination: {block:?}"
    );

    let mut parser = Parser::new(options.clone());
    parser
        .append("[a](https://e.com/a.png", &mut changes)
        .unwrap();
    view = View::default();
    view.apply(&mut changes);
    assert!(
        view.previews
            .values()
            .flatten()
            .any(|node| matches!(node.kind, InlineKind::LinkPreview(_))),
        "expected a link preview before finalization"
    );
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    view.assert_no_previews();
    let block = view.blocks.values().next().unwrap();
    assert!(
        nodes_of(block).iter().all(|node| !matches!(
            node.kind,
            InlineKind::LinkPreview(_) | InlineKind::ImagePreview(_)
        )),
        "finalization kept a provisional link: {block:?}"
    );

    let mut parser = Parser::new(options);
    parser.append("`code", &mut changes).unwrap();
    view = View::default();
    view.apply(&mut changes);
    assert!(
        view.previews
            .values()
            .flatten()
            .any(|node| matches!(node.kind, InlineKind::CodePreview(_))),
        "expected a code preview before finalization"
    );
    parser.finish(&mut changes).unwrap();
    view.apply(&mut changes);
    view.assert_no_previews();
    let block = view.blocks.values().next().unwrap();
    assert_eq!(block_visible_text(parser.source_text(), block), "`code");
    assert!(
        all_nodes(std::slice::from_ref(block))
            .iter()
            .all(|node| !matches!(node.kind, InlineKind::Code(_) | InlineKind::CodePreview(_))),
        "finalization kept code for an unclosed backtick: {block:?}"
    );
}

#[test]
fn an_image_inside_an_unfinished_link_stays_visible_at_final() {
    let options = options();
    let source = "See [![logo](a.png)]( http://s";
    let (blocks, view) = final_view(source, &options);
    view.assert_no_previews();
    let nodes = nodes_of(&blocks[0]);
    assert_eq!(visible_text(source, nodes), "See []( http://s");
    let images: Vec<_> = nodes
        .iter()
        .filter(|node| matches!(node.kind, InlineKind::Image(_)))
        .collect();
    assert_eq!(images.len(), 1, "{nodes:?}");
    let InlineKind::Image(image) = &images[0].kind else {
        unreachable!()
    };
    assert_eq!(image.destination.as_str(source), "a.png");
    assert_eq!(visible_text(source, &image.children), "logo");
    assert!(
        nodes
            .iter()
            .any(|node| matches!(&node.kind, InlineKind::Text(text) if text.as_str(source) == "]( http://s")),
        "{nodes:?}"
    );
}

#[test]
fn unsafe_or_unknown_schemes_are_delivered_verbatim_for_the_renderer() {
    let options = options();
    for (source, destination) in [
        ("[a](javascript:alert(1))", "javascript:alert(1)"),
        ("![a](javascript:alert(1))", "javascript:alert(1)"),
        ("[a](ftp://e.com/a.png)", "ftp://e.com/a.png"),
        ("[a](data:text/html;base64,AA)", "data:text/html;base64,AA"),
        ("![a](data:text/html;base64,AA)", "data:text/html;base64,AA"),
        ("[a](file:///C:/x.png)", "file:///C:/x.png"),
        ("![a](file:///C:/x.png)", "file:///C:/x.png"),
    ] {
        let parsed = parse(source, &options).unwrap().nodes;
        let (streamed, view) = final_view(source, &options);
        view.assert_no_previews();
        for blocks in [&parsed, &streamed] {
            let node = all_nodes(blocks)
                .into_iter()
                .find(|node| matches!(node.kind, InlineKind::Link(_) | InlineKind::Image(_)))
                .unwrap_or_else(|| panic!("expected a target node for {source:?}"));
            let delivered = match &node.kind {
                InlineKind::Link(link) | InlineKind::Image(link) => link.destination.as_str(source),
                _ => unreachable!(),
            };
            assert_eq!(delivered, destination, "{source:?}");
        }
    }
}

#[test]
fn table_cells_admonitions_and_custom_tags_finalize_without_provisional_targets() {
    let options = options();

    let table_source = "| h |\n|---|\n| [x](https://e.com/s |";
    let (blocks, view) = final_view(table_source, &options);
    view.assert_no_previews();
    let table = blocks
        .iter()
        .find_map(|block| match &block.data {
            BlockData::Table(table) => Some(table),
            _ => None,
        })
        .expect("expected a table");
    let cells: Vec<_> = table
        .header()
        .iter()
        .chain(table.rows().flatten())
        .collect();
    assert_eq!(cells.len(), 2);
    let InlineContent::Parsed(nodes) = cells[1] else {
        panic!("expected a parsed table cell: {:?}", cells[1]);
    };
    assert_eq!(visible_text(table_source, nodes), "[x](https://e.com/s");
    assert!(!has_image(nodes), "a table cell kept an image");

    let admonition_source = "::: tip\n[a *em*](https://e.com/s\n:::";
    let (blocks, view) = final_view(admonition_source, &options);
    view.assert_no_previews();
    let paragraph = blocks
        .iter()
        .find(|block| matches!(block.data, BlockData::Paragraph(_)))
        .expect("expected an admonition body paragraph");
    let nodes = nodes_of(paragraph);
    assert_eq!(
        visible_text(admonition_source, nodes),
        "[a em](https://e.com/s"
    );
    assert!(
        nodes
            .iter()
            .any(|node| matches!(node.kind, InlineKind::Link(_))),
        "{nodes:?}"
    );

    let custom_options = Options {
        custom_tags: vec!["tool-call".into()],
        ..options
    };
    let tag_source = "<tool-call>[x](https://e.com/s</tool-call>";
    let (blocks, view) = final_view(tag_source, &custom_options);
    view.assert_no_previews();
    let nodes = all_nodes(&blocks);
    assert!(
        nodes
            .iter()
            .any(|node| matches!(node.kind, InlineKind::Link(_))),
        "expected the linkified tail inside the custom tag: {nodes:?}"
    );
    assert!(
        nodes.iter().all(|node| !matches!(
            node.kind,
            InlineKind::LinkPreview(_) | InlineKind::ImagePreview(_) | InlineKind::CodePreview(_)
        )),
        "{nodes:?}"
    );
}

#[test]
fn finalization_is_idempotent_and_rejects_later_appends() {
    let options = options();
    let source = "![a](https://e.com/a b.png) tail";
    let mut parser = Parser::new(options);
    let mut changes = Vec::new();
    parser.append(source, &mut changes).unwrap();
    changes.clear();
    parser.finish(&mut changes).unwrap();
    assert!(!changes.is_empty());
    changes.clear();
    parser.finish(&mut changes).unwrap();
    assert!(changes.is_empty());
    assert_eq!(
        parser.append("more", &mut changes),
        Err(stream_markdown_rs::ParseError::Finished)
    );
}

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Tag, TagEnd};

#[derive(Debug, Eq, PartialEq)]
enum Atom {
    Start(&'static str),
    End(&'static str),
    Marker(&'static str),
    Field(String),
    Text(String),
    Html(String),
}

#[derive(Default)]
pub struct Trace(Vec<Atom>);

impl Trace {
    pub fn start(&mut self, name: &'static str) {
        self.0.push(Atom::Start(name));
    }

    pub fn end(&mut self, name: &'static str) {
        self.0.push(Atom::End(name));
    }

    pub fn marker(&mut self, name: &'static str) {
        self.0.push(Atom::Marker(name));
    }

    pub fn field(&mut self, value: impl Into<String>) {
        self.0.push(Atom::Field(value.into()));
    }

    pub fn text(&mut self, next: &str) {
        if next.is_empty() {
            return;
        }
        if let Some(Atom::Text(previous)) = self.0.last_mut() {
            previous.push_str(next);
        } else {
            self.0.push(Atom::Text(next.into()));
        }
    }

    pub fn html(&mut self, next: &str) {
        if let Some(Atom::Html(previous)) = self.0.last_mut() {
            previous.push_str(next);
        } else {
            self.0.push(Atom::Html(next.into()));
        }
    }

    pub fn difference(&self, actual: &Self) -> Option<String> {
        let index = self
            .0
            .iter()
            .zip(&actual.0)
            .position(|(expected, actual)| expected != actual);
        if index.is_none() && self.0.len() == actual.0.len() {
            return None;
        }
        let index = index.unwrap_or_else(|| self.0.len().min(actual.0.len()));
        Some(format!(
            "event {index}: pulldown {}, candidate {}",
            summarize(self.0.get(index)),
            summarize(actual.0.get(index)),
        ))
    }
}

fn summarize(atom: Option<&Atom>) -> String {
    match atom {
        Some(Atom::Text(text) | Atom::Field(text) | Atom::Html(text)) if text.len() > 120 => {
            format!(
                "{:?}… ({} bytes)",
                &text[..crate::corpus::boundary_at_or_before(text, 120)],
                text.len()
            )
        }
        Some(atom) => format!("{atom:?}"),
        None => "<end>".into(),
    }
}

pub fn pulldown<'a>(events: impl IntoIterator<Item = Event<'a>>) -> Result<Trace, String> {
    let mut trace = Trace::default();
    for event in events {
        match event {
            Event::Start(tag) => start(&mut trace, tag)?,
            Event::End(tag) => trace.end(end_name(tag)?),
            Event::Text(text) => trace.text(&text),
            Event::Code(text) => {
                trace.start("code");
                trace.text(&text);
                trace.end("code");
            }
            Event::Html(text) | Event::InlineHtml(text) => trace.html(&text),
            Event::SoftBreak => trace.marker("softbreak"),
            Event::HardBreak => trace.marker("hardbreak"),
            Event::Rule => trace.marker("thematic-break"),
            Event::TaskListMarker(checked) => {
                trace.marker(if checked { "checked" } else { "unchecked" })
            }
            event => return Err(format!("unsupported pulldown projection: {event:?}")),
        }
    }
    Ok(trace)
}

fn start(trace: &mut Trace, tag: Tag<'_>) -> Result<(), String> {
    trace.start(end_name(tag.to_end())?);
    match tag {
        Tag::Heading { level, .. } => trace.field((level as u8).to_string()),
        Tag::CodeBlock(kind) => trace.field(match kind {
            CodeBlockKind::Fenced(info) => info.as_ref().to_owned(),
            CodeBlockKind::Indented => String::new(),
        }),
        Tag::Link {
            dest_url, title, ..
        }
        | Tag::Image {
            dest_url, title, ..
        } => {
            trace.field(dest_url.as_ref());
            trace.field(title.as_ref());
        }
        Tag::Table(columns) => {
            for column in columns {
                trace.field(match column {
                    Alignment::None => "none",
                    Alignment::Left => "left",
                    Alignment::Center => "center",
                    Alignment::Right => "right",
                });
            }
        }
        Tag::List(start) => trace.field(start.map(|value| value.to_string()).unwrap_or_default()),
        _ => {}
    }
    Ok(())
}

fn end_name(tag: TagEnd) -> Result<&'static str, String> {
    Ok(match tag {
        TagEnd::Paragraph => "paragraph",
        TagEnd::Heading(_) => "heading",
        TagEnd::CodeBlock => "code-block",
        TagEnd::Emphasis => "emphasis",
        TagEnd::Strong => "strong",
        TagEnd::Strikethrough => "strikethrough",
        TagEnd::Link => "link",
        TagEnd::Image => "image",
        TagEnd::HtmlBlock => "html-block",
        TagEnd::Table => "table",
        TagEnd::TableHead => "table-head",
        TagEnd::TableRow => "table-row",
        TagEnd::TableCell => "table-cell",
        TagEnd::List(_) => "list",
        TagEnd::Item => "item",
        TagEnd::BlockQuote(None) => "quote",
        tag => return Err(format!("unsupported pulldown tag projection: {tag:?}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_does_not_depend_on_adjacent_text_segmentation() {
        let mut expected = Trace::default();
        expected.text("a ");
        expected.text("b");
        let mut actual = Trace::default();
        actual.text("a b");
        assert!(expected.difference(&actual).is_none());
        actual.marker("softbreak");
        assert!(expected.difference(&actual).is_some());
    }
}

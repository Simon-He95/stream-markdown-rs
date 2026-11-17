use crate::{pulldown_options, Engine, Input, Pulldown};
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Parser, Tag};

pub struct OwnedPulldown;

pub struct Replay<'a> {
    pub source: String,
    pub events: Vec<Event<'a>>,
}

impl<'a> Engine<'a> for OwnedPulldown {
    type Document = Vec<Event<'a>>;
    type Session = Replay<'a>;

    fn parse(source: &'a str) -> Result<Self::Document, String> {
        Pulldown::parse(source)
    }

    fn start(_: &'a Input) -> Self::Session {
        Replay {
            source: String::new(),
            events: Vec::new(),
        }
    }

    fn append(session: &mut Self::Session, prefix: &'a str, start: usize) -> Result<(), String> {
        session.source.push_str(&prefix[start..]);
        session.events.clear();
        let source = Source {
            input_start: session.source.as_ptr() as usize,
            fixture: prefix,
        };
        session.events.extend(
            Parser::new_ext(&session.source, pulldown_options()).map(|event| source.event(event)),
        );
        Ok(())
    }

    fn finish(_: &mut Self::Session) -> Result<(), String> {
        Ok(())
    }

    #[cfg(not(feature = "count-allocations"))]
    fn has_output(session: &Self::Session) -> bool {
        !session.events.is_empty()
    }
}

struct Source<'a> {
    input_start: usize,
    fixture: &'a str,
}

impl<'a> Source<'a> {
    fn text(&self, value: CowStr<'_>) -> CowStr<'a> {
        // Reuse source-relative text without keeping references into the growing String.
        if let CowStr::Borrowed(text) = value {
            if let Some(start) = (text.as_ptr() as usize).checked_sub(self.input_start) {
                if let Some(matching) = self.fixture.get(start..start.saturating_add(text.len())) {
                    return CowStr::Borrowed(matching);
                }
            }
        }
        value.into_static()
    }

    fn tag(&self, tag: Tag<'_>) -> Tag<'a> {
        match tag {
            Tag::CodeBlock(CodeBlockKind::Fenced(info)) => {
                Tag::CodeBlock(CodeBlockKind::Fenced(self.text(info)))
            }
            Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            } => Tag::Link {
                link_type,
                dest_url: self.text(dest_url),
                title: self.text(title),
                id: self.text(id),
            },
            Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            } => Tag::Image {
                link_type,
                dest_url: self.text(dest_url),
                title: self.text(title),
                id: self.text(id),
            },
            tag => tag.into_static(),
        }
    }

    fn event(&self, event: Event<'_>) -> Event<'a> {
        match event {
            Event::Start(tag) => Event::Start(self.tag(tag)),
            Event::Text(value) => Event::Text(self.text(value)),
            Event::Code(value) => Event::Code(self.text(value)),
            Event::Html(value) => Event::Html(self.text(value)),
            Event::InlineHtml(value) => Event::InlineHtml(self.text(value)),
            event => event.into_static(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owning_chunks_preserves_native_events_across_buffer_growth() {
        let source = "中文 [link][id] and ![image](/pic) `code` &amp; <i>word</i>.\n\n[id]: /x \"title\"\n\n```rust\ncode\n```\n";
        #[cfg(feature = "candidate")]
        let input = Input::new(source.to_owned());
        #[cfg(not(feature = "candidate"))]
        let input = source.to_owned();
        let mut replay = OwnedPulldown::start(&input);
        let mut start = 0;
        for end in crate::corpus::chunk_ends(source, 8) {
            let prefix = &source[..end];
            OwnedPulldown::append(&mut replay, prefix, start).unwrap();
            assert_eq!(replay.source, prefix);
            assert_eq!(replay.events, Pulldown::parse(prefix).unwrap());
            start = end;
        }
    }
}

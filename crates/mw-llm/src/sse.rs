//! Server-sent events, as all three backends stream them.
//!
//! [`SseParser`] turns bytes into events incrementally and does no I/O, so
//! the backends' decoders are tested by feeding it recorded or hand-written
//! streams. It follows the WHATWG event-stream format: `\n`, `\r\n` or `\r`
//! line ends, `data:` lines joined with `\n`, comments ignored, a blank line
//! dispatches.

/// One dispatched event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field; empty when the server sent none (OpenAI Chat
    /// Completions sends data-only events).
    pub event: String,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of the current, unfinished line.
    line: Vec<u8>,
    /// The previous chunk ended in `\r`, so a leading `\n` belongs to it.
    after_cr: bool,
    started: bool,
    event: String,
    data: String,
    has_data: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the next chunk and returns the events it completes.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for &byte in chunk {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\n' => self.end_line(&mut events),
                b'\r' => {
                    self.after_cr = true;
                    self.end_line(&mut events);
                }
                _ => self.line.push(byte),
            }
        }
        events
    }

    /// Ends the stream. An event without its closing blank line is still
    /// dispatched: some servers end the body right after the last `data:`.
    pub fn finish(&mut self) -> Option<SseEvent> {
        let mut events = Vec::new();
        if !self.line.is_empty() {
            self.end_line(&mut events);
        }
        self.dispatch(&mut events);
        events.pop()
    }

    fn end_line(&mut self, events: &mut Vec<SseEvent>) {
        let mut line = String::from_utf8_lossy(&self.line).into_owned();
        self.line.clear();
        if !self.started {
            self.started = true;
            if let Some(rest) = line.strip_prefix('\u{feff}') {
                line = rest.to_owned();
            }
        }
        if line.is_empty() {
            self.dispatch(events);
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line.as_str(), ""),
        };
        match field {
            "event" => self.event = value.to_owned(),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            // `id` and `retry` only matter for reconnecting, which these APIs
            // don't support.
            _ => {}
        }
    }

    fn dispatch(&mut self, events: &mut Vec<SseEvent>) {
        let event = std::mem::take(&mut self.event);
        if !self.has_data {
            return;
        }
        self.has_data = false;
        events.push(SseEvent {
            event,
            data: std::mem::take(&mut self.data),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut parser = SseParser::new();
        let mut events: Vec<SseEvent> =
            chunks.iter().flat_map(|chunk| parser.push(chunk)).collect();
        events.extend(parser.finish());
        events
    }

    fn event(event: &str, data: &str) -> SseEvent {
        SseEvent {
            event: event.into(),
            data: data.into(),
        }
    }

    #[test]
    fn named_and_data_only_events() {
        let events = parse_all(&[
            b"event: ping\ndata: {\"type\": \"ping\"}\n\ndata: {\"a\":1}\n\ndata: [DONE]\n\n",
        ]);
        assert_eq!(
            events,
            [
                event("ping", r#"{"type": "ping"}"#),
                event("", r#"{"a":1}"#),
                event("", "[DONE]"),
            ]
        );
    }

    #[test]
    fn events_split_across_chunks_anywhere() {
        let text = "event: message_start\r\ndata: {\"x\":\"é\"}\r\n\r\nevent: message_stop\r\ndata: {}\r\n\r\n";
        let bytes = text.as_bytes();
        let expected = vec![
            event("message_start", r#"{"x":"é"}"#),
            event("message_stop", "{}"),
        ];
        for split in 0..bytes.len() {
            let (a, b) = bytes.split_at(split);
            assert_eq!(parse_all(&[a, b]), expected, "split at {split}");
        }
    }

    #[test]
    fn comments_multiline_data_and_bare_cr() {
        let events = parse_all(&[b": keep-alive\rdata: one\rdata:two\r\revent: x\rid: 7\r\r"]);
        // The second block has an event name but no data, so nothing fires.
        assert_eq!(events, [event("", "one\ntwo")]);
    }

    #[test]
    fn unterminated_last_event_and_bom() {
        let events = parse_all(&["\u{feff}data: first\n\ndata: last".as_bytes()]);
        assert_eq!(events, [event("", "first"), event("", "last")]);
    }
}

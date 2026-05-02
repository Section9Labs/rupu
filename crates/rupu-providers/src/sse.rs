use crate::error::ProviderError;

/// A parsed SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event_type: String,
    pub data: String,
}

/// Line-by-line SSE parser. Feed it byte chunks, get complete events.
/// Handles the W3C Server-Sent Events format: `event:` and `data:` fields
/// separated by blank lines. Supports optional space after colon per spec.
/// Maximum SSE buffer size (1 MB). Protects against malicious servers
/// sending continuous data without newlines to exhaust memory.
const MAX_BUFFER_SIZE: usize = 1024 * 1024;

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
    current_event: Option<String>,
    current_data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of bytes. Returns any complete events parsed.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, ProviderError> {
        let text =
            std::str::from_utf8(chunk).map_err(|e| ProviderError::SseParse(e.to_string()))?;
        self.buffer.push_str(text);

        if self.buffer.len() > MAX_BUFFER_SIZE {
            self.buffer.clear();
            self.current_event.take();
            self.current_data.clear();
            return Err(ProviderError::SseParse(format!(
                "buffer overflow: no newline in {} bytes",
                MAX_BUFFER_SIZE
            )));
        }

        let mut events = Vec::new();

        while let Some(newline_pos) = self.buffer.find('\n') {
            let line = self.buffer[..newline_pos]
                .trim_end_matches('\r')
                .to_string();
            self.buffer = self.buffer[newline_pos + 1..].to_string();

            if line.is_empty() {
                // Blank line = end of event
                if !self.current_data.is_empty() {
                    // If no explicit event type, default to "message" (OpenAI compat)
                    let event_type = self
                        .current_event
                        .take()
                        .unwrap_or_else(|| "message".to_string());
                    let data = self.current_data.join("\n");
                    self.current_data.clear();
                    events.push(SseEvent { event_type, data });
                } else {
                    self.current_event.take();
                    self.current_data.clear();
                }
            } else if let Some(value) = line.strip_prefix("event:") {
                // W3C SSE spec: optional space after colon
                self.current_event = Some(value.trim_start().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                self.current_data.push(value.trim_start().to_string());
            }
            // Ignore other lines (comments starting with :, unknown fields)
        }

        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_event() {
        let mut parser = SseParser::new();
        let chunk = b"event: message_start\ndata: {\"type\":\"message_start\"}\n\n";
        let events = parser.feed(chunk).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message_start");
        assert!(events[0].data.contains("message_start"));
    }

    #[test]
    fn test_parse_multiple_events() {
        let mut parser = SseParser::new();
        let chunk = b"event: content_block_start\ndata: {\"index\":0}\n\nevent: content_block_delta\ndata: {\"text\":\"Hi\"}\n\n";
        let events = parser.feed(chunk).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "content_block_start");
        assert_eq!(events[1].event_type, "content_block_delta");
    }

    #[test]
    fn test_parse_chunked_input() {
        let mut parser = SseParser::new();
        let events1 = parser.feed(b"event: mess").unwrap();
        assert!(events1.is_empty());
        let events2 = parser.feed(b"age_start\ndata: {\"ok\":true}\n\n").unwrap();
        assert_eq!(events2.len(), 1);
        assert_eq!(events2[0].event_type, "message_start");
    }

    #[test]
    fn test_parse_event_with_cr_lf() {
        let mut parser = SseParser::new();
        let chunk = b"event: test\r\ndata: hello\r\n\r\n";
        let events = parser.feed(chunk).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "hello");
    }

    #[test]
    fn test_empty_chunk() {
        let mut parser = SseParser::new();
        let events = parser.feed(b"").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_ignore_comments() {
        let mut parser = SseParser::new();
        let chunk = b": this is a comment\nevent: ping\ndata: {}\n\n";
        let events = parser.feed(chunk).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "ping");
    }

    #[test]
    fn test_multiline_data() {
        let mut parser = SseParser::new();
        let chunk = b"event: test\ndata: line1\ndata: line2\n\n";
        let events = parser.feed(chunk).unwrap();
        assert_eq!(events[0].data, "line1\nline2");
    }

    #[test]
    fn test_feed_invalid_utf8_returns_error() {
        let mut parser = SseParser::new();
        let result = parser.feed(&[0xFF, 0xFE]);
        assert!(result.is_err());
    }

    #[test]
    fn test_bare_data_emits_default_event_type() {
        // OpenAI/Copilot send bare `data:` lines without `event:` prefix
        let mut parser = SseParser::new();
        let events = parser.feed(b"data: orphan\n\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message");
        assert_eq!(events[0].data, "orphan");
    }

    #[test]
    fn test_bare_data_does_not_contaminate_next_event() {
        let mut parser = SseParser::new();
        let events1 = parser.feed(b"data: first\n\n").unwrap();
        assert_eq!(events1.len(), 1);
        assert_eq!(events1[0].data, "first");
        let events2 = parser.feed(b"event: ping\ndata: clean\n\n").unwrap();
        assert_eq!(events2.len(), 1);
        assert_eq!(events2[0].event_type, "ping");
        assert_eq!(events2[0].data, "clean");
    }

    #[test]
    fn test_bare_data_openai_json() {
        let mut parser = SseParser::new();
        let events = parser
            .feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n")
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message");
        assert!(events[0].data.contains("choices"));
    }

    #[test]
    fn test_multiple_bare_data_events() {
        let mut parser = SseParser::new();
        let events = parser
            .feed(b"data: {\"id\":1}\n\ndata: {\"id\":2}\n\n")
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "message");
        assert_eq!(events[1].event_type, "message");
    }

    #[test]
    fn test_done_signal_bare_data() {
        // OpenAI sends `data: [DONE]` as final event
        let mut parser = SseParser::new();
        let events = parser.feed(b"data: [DONE]\n\n").unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message");
        assert_eq!(events[0].data, "[DONE]");
    }

    #[test]
    fn test_no_space_after_colon() {
        // W3C SSE spec: space after colon is optional
        let mut parser = SseParser::new();
        let chunk = b"event:message_start\ndata:{\"ok\":true}\n\n";
        let events = parser.feed(chunk).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message_start");
        assert_eq!(events[0].data, "{\"ok\":true}");
    }

    #[test]
    fn test_buffer_overflow_protection() {
        let mut parser = SseParser::new();
        // Feed >1MB of data without any newline
        let chunk = vec![b'a'; MAX_BUFFER_SIZE + 1];
        let result = parser.feed(&chunk);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("buffer overflow"));
    }

    #[test]
    fn test_buffer_under_limit_ok() {
        let mut parser = SseParser::new();
        // Feed data just under the limit with a newline at the end
        let mut chunk = vec![b'a'; 1000];
        chunk.push(b'\n');
        let result = parser.feed(&chunk);
        assert!(result.is_ok());
    }
}

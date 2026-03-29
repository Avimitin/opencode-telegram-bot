use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event_type: String,
    pub data: Value,
}

/// Parse an SSE stream from an HTTP response body.
pub struct SseStream {
    stream: Box<dyn futures_util::Stream<Item = Result<Bytes, reqwest::Error>> + Unpin + Send>,
    buffer: String,
}

impl SseStream {
    pub fn new(response: reqwest::Response) -> Self {
        SseStream {
            stream: Box::new(response.bytes_stream()),
            buffer: String::new(),
        }
    }

    /// Read the next SSE event. Returns None when the stream ends.
    pub async fn next_event(&mut self) -> Option<SseEvent> {
        loop {
            // Try to parse a complete event from the buffer
            if let Some(event) = self.try_parse_event() {
                return Some(event);
            }

            // Read more data
            match self.stream.next().await {
                Some(Ok(chunk)) => {
                    if let Ok(text) = std::str::from_utf8(&chunk) {
                        self.buffer.push_str(text);
                    }
                }
                _ => return None,
            }
        }
    }

    fn try_parse_event(&mut self) -> Option<SseEvent> {
        // SSE events are separated by blank lines (\n\n)
        let separator = "\n\n";
        let pos = self.buffer.find(separator)?;

        let event_text = self.buffer[..pos].to_string();
        self.buffer = self.buffer[pos + separator.len()..].to_string();

        let mut event_type = String::new();
        let mut data_lines = Vec::new();

        for line in event_text.lines() {
            if let Some(value) = line.strip_prefix("event:") {
                event_type = value.trim().to_string();
            } else if let Some(value) = line.strip_prefix("data:") {
                data_lines.push(value.trim().to_string());
            } else if line.starts_with(':') {
                // Comment, ignore
            }
        }

        if data_lines.is_empty() && event_type.is_empty() {
            return None;
        }

        let data_str = data_lines.join("\n");
        let data = serde_json::from_str(&data_str).unwrap_or(Value::String(data_str));

        Some(SseEvent { event_type, data })
    }
}

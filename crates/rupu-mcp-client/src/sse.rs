//! Incremental SSE framing. The caller enforces an aggregate byte budget.
use crate::Error;
use serde_json::Value;
#[derive(Default)]
pub(crate) struct Decoder {
    pending: Vec<u8>,
    data: Vec<u8>,
    started: bool,
}
impl Decoder {
    pub fn finish(&mut self) -> Result<Vec<Value>, Error> {
        if self.pending.last() == Some(&b'\r') {
            self.push(b"\n")
        } else {
            Ok(Vec::new())
        }
    }
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, Error> {
        self.pending.extend_from_slice(chunk);
        let mut messages = Vec::new();
        if !self.started {
            if self.pending.len() < 3 {
                return Ok(messages);
            }
            if self.pending.starts_with(&[0xef, 0xbb, 0xbf]) {
                self.pending.drain(..3);
            }
            self.started = true;
        }
        while let Some(end) = self.pending.iter().position(|b| *b == b'\r' || *b == b'\n') {
            if self.pending[end] == b'\r' && end + 1 == self.pending.len() {
                break;
            }
            let consumed = end
                + if self.pending[end] == b'\r' && self.pending.get(end + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
            let line = self.pending[..end].to_vec();
            self.pending.drain(..consumed);
            if line.is_empty() {
                if !self.data.is_empty() {
                    self.data.pop(); // trailing separator from the last data line
                    if !self.data.is_empty() {
                        messages.push(
                            serde_json::from_slice(&self.data)
                                .map_err(|_| Error::Protocol("invalid SSE JSON"))?,
                        );
                    }
                    self.data.clear();
                }
            } else if let Some(data) = line.strip_prefix(b"data:") {
                let data = data.strip_prefix(b" ").unwrap_or(data);
                self.data.extend_from_slice(data);
                self.data.push(b'\n');
            }
        }
        Ok(messages)
    }
}

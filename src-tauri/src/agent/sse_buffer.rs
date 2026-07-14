// SPDX-License-Identifier: Apache-2.0
//! SSE line buffering across TCP chunks.
//!
//! Server-Sent Events frame their payload as `data: <json>\n` lines, but the
//! underlying byte stream comes in arbitrary TCP chunks. A single SSE event
//! commonly straddles two chunks. The naive `String::from_utf8_lossy(&bytes)
//! .lines()`-per-chunk approach drops every cross-chunk event, which in
//! production showed up as bash commands missing characters, write_file
//! losing trailing content, and tool-call arguments arriving as truncated
//! JSON ("Invalid arguments" 400s).
//!
//! This helper is the small, testable kernel of the fix used in both the
//! OpenAI and Anthropic streaming parsers. It accepts bytes piecewise and
//! yields complete lines on demand.

// Scaffolding: the streaming chat path (openrouter::client::stream_chat) is not
// yet wired into the live loop and still splits chunks inline, so this tested
// fix-kernel currently has no production caller. Retained — see the module docs
// above and the v0.3.5 regression tests below — for when streaming adopts it.
#![allow(dead_code)]

/// Streaming SSE line splitter. Bytes go in via [`feed`]; complete lines
/// (with the trailing `\n` stripped, and any `\r` from CRLF dropped) come
/// out via [`take_line`].
#[derive(Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(4096),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Pull one complete line if available. Returns the line as an owned
    /// String, lossy-decoded only at line boundaries so partial UTF-8
    /// codepoints inside a chunk can never be corrupted.
    pub fn take_line(&mut self) -> Option<String> {
        let nl_pos = self.buf.iter().position(|&b| b == b'\n')?;
        let line_bytes: Vec<u8> = self.buf.drain(..=nl_pos).collect();
        let mut s = String::from_utf8_lossy(&line_bytes[..line_bytes.len() - 1]).into_owned();
        if s.ends_with('\r') {
            s.pop();
        }
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact bug we shipped in v0.3.5: an SSE data line split across
    /// two TCP chunks must reassemble, not get dropped.
    #[test]
    fn splits_sse_event_across_two_chunks() {
        let mut b = LineBuffer::new();
        b.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"Sele");
        // No newline yet — nothing to yield.
        assert!(b.take_line().is_none());

        b.feed(b"ct-Object\"}}]}\n");
        let line = b.take_line().expect("line should be complete now");
        assert_eq!(
            line, r#"data: {"choices":[{"delta":{"content":"Select-Object"}}]}"#,
            "the bash command 'Select-Object' must survive the split — \
             before the fix it would arrive truncated as 'Sele' or 'Select-Obj'"
        );
    }

    /// Multiple complete events arriving in one big chunk must all come out.
    #[test]
    fn emits_multiple_lines_from_single_chunk() {
        let mut b = LineBuffer::new();
        b.feed(b"data: a\ndata: b\ndata: c\n");
        assert_eq!(b.take_line().as_deref(), Some("data: a"));
        assert_eq!(b.take_line().as_deref(), Some("data: b"));
        assert_eq!(b.take_line().as_deref(), Some("data: c"));
        assert!(b.take_line().is_none());
    }

    /// CRLF (`\r\n`) line endings (some providers send these) must be
    /// normalised — without the strip the JSON parser sees a stray \r
    /// after the closing brace.
    #[test]
    fn strips_trailing_carriage_return() {
        let mut b = LineBuffer::new();
        b.feed(b"data: hello\r\n");
        assert_eq!(b.take_line().as_deref(), Some("data: hello"));
    }

    /// Critically: a chunk boundary inside a UTF-8 multi-byte sequence
    /// (e.g., a Chinese character) must not be lossily decoded. We delay
    /// decoding until a full line is available, which guarantees the
    /// boundary is on a '\n' byte (always ASCII).
    #[test]
    fn preserves_utf8_across_chunk_boundary() {
        let mut b = LineBuffer::new();
        // "你好" = E4 BD A0 E5 A5 BD in UTF-8. Split mid-codepoint.
        b.feed(b"data: \xe4\xbd");
        assert!(b.take_line().is_none()); // no newline yet
        b.feed(b"\xa0\xe5\xa5\xbd\n");
        assert_eq!(b.take_line().as_deref(), Some("data: 你好"));
    }

    /// Three chunks: first holds the "data:" prefix and start of the
    /// payload, second holds the middle, third holds the tail + newline.
    /// All three must accumulate into one line.
    #[test]
    fn three_chunks_assemble() {
        let mut b = LineBuffer::new();
        b.feed(b"data: {\"foo\":");
        b.feed(b"\"bar\",\"x\":");
        b.feed(b"42}\n");
        assert_eq!(
            b.take_line().as_deref(),
            Some(r#"data: {"foo":"bar","x":42}"#)
        );
    }

    /// An incomplete tail after the last newline must remain buffered for
    /// the next feed.
    #[test]
    fn leftover_after_newline_is_buffered() {
        let mut b = LineBuffer::new();
        b.feed(b"data: complete\ndata: partial");
        assert_eq!(b.take_line().as_deref(), Some("data: complete"));
        assert!(b.take_line().is_none());
        b.feed(b" rest\n");
        assert_eq!(b.take_line().as_deref(), Some("data: partial rest"));
    }
}

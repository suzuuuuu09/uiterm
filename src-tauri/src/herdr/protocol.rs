//! herdr-core Framed Mode binary protocol codec.
//!
//! Frame layout: `[channel: u32 LE][kind: u8][len: u32 LE][payload: len bytes]`
//! Maximum payload size: 16 MiB (16 * 1024 * 1024 bytes).

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const HEADER_LEN: usize = 9;
pub const MAX_PAYLOAD_LEN: usize = 16 * 1024 * 1024;
pub const INITIAL_CREDIT_BYTES: u32 = 256 * 1024;

pub const FRAME_DATA: u8 = 0;
pub const FRAME_CREDIT: u8 = 1;
pub const FRAME_CLOSE: u8 = 2;
pub const FRAME_RESIZE: u8 = 3;
pub const FRAME_FOCUS: u8 = 4;
pub const FRAME_FACTS: u8 = 5;
pub const FRAME_AUTOMATIC_INPUT: u8 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub channel: u32,
    pub kind: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn data(channel: u32, payload: Vec<u8>) -> Self {
        Self {
            channel,
            kind: FRAME_DATA,
            payload,
        }
    }

    pub fn automatic_input(channel: u32, payload: Vec<u8>) -> Self {
        Self {
            channel,
            kind: FRAME_AUTOMATIC_INPUT,
            payload,
        }
    }

    pub fn credit(channel: u32, bytes: u32) -> Self {
        Self {
            channel,
            kind: FRAME_CREDIT,
            payload: bytes.to_le_bytes().to_vec(),
        }
    }

    pub fn close(channel: u32, payload: Vec<u8>) -> Self {
        Self {
            channel,
            kind: FRAME_CLOSE,
            payload,
        }
    }

    pub fn resize(channel: u32, cols: u16, rows: u16) -> Self {
        let mut payload = Vec::with_capacity(4);
        payload.extend_from_slice(&cols.to_le_bytes());
        payload.extend_from_slice(&rows.to_le_bytes());
        Self {
            channel,
            kind: FRAME_RESIZE,
            payload,
        }
    }

    pub fn focus(channel: u32, focused: bool) -> Self {
        Self {
            channel,
            kind: FRAME_FOCUS,
            payload: vec![u8::from(focused)],
        }
    }

    pub fn facts(channel: u32, json_bytes: Vec<u8>) -> Self {
        Self {
            channel,
            kind: FRAME_FACTS,
            payload: json_bytes,
        }
    }

    pub fn parse_resize(&self) -> Option<(u16, u16)> {
        if self.kind != FRAME_RESIZE || self.payload.len() != 4 {
            return None;
        }
        let cols = u16::from_le_bytes([self.payload[0], self.payload[1]]);
        let rows = u16::from_le_bytes([self.payload[2], self.payload[3]]);
        (cols > 0 && rows > 0).then_some((cols, rows))
    }

    pub fn encode_header(&self) -> io::Result<[u8; HEADER_LEN]> {
        if self.payload.len() > MAX_PAYLOAD_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "frame payload {} exceeds max {}",
                    self.payload.len(),
                    MAX_PAYLOAD_LEN
                ),
            ));
        }
        let mut header = [0u8; HEADER_LEN];
        header[0..4].copy_from_slice(&self.channel.to_le_bytes());
        header[4] = self.kind;
        header[5..9].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        Ok(header)
    }
}

pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    let header = frame.encode_header()?;
    writer.write_all(&header).await?;
    if !frame.payload.is_empty() {
        writer.write_all(&frame.payload).await?;
    }
    writer.flush().await
}

pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Frame> {
    let mut header = [0u8; HEADER_LEN];
    reader.read_exact(&mut header).await?;
    let channel = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let kind = header[4];
    let len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
    if len > MAX_PAYLOAD_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "incoming frame length {} exceeds max {}",
                len, MAX_PAYLOAD_LEN
            ),
        ));
    }
    let mut payload = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut payload).await?;
    }
    Ok(Frame {
        channel,
        kind,
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_all_frame_kinds() {
        let frames = vec![
            Frame::data(0, br#"{"jsonrpc":"2.0"}"#.to_vec()),
            Frame::credit(7, 65536),
            Frame::close(7, br#"{"reason":"exited","exit_code":0}"#.to_vec()),
            Frame::resize(7, 120, 40),
            Frame::focus(7, true),
            Frame::facts(7, br##"{"fg":"#ffffff"}"##.to_vec()),
            Frame::automatic_input(7, b"\x1b[?62;22c".to_vec()),
        ];

        let mut buf = Vec::new();
        for f in &frames {
            write_frame(&mut buf, f).await.unwrap();
        }

        let mut cursor = io::Cursor::new(buf);
        for expected in &frames {
            let decoded = read_frame(&mut cursor).await.unwrap();
            assert_eq!(&decoded, expected);
        }
    }

    #[test]
    fn parse_resize_payload() {
        let f = Frame::resize(3, 132, 43);
        assert_eq!(f.parse_resize(), Some((132, 43)));
    }
}

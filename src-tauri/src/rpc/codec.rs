//! Wire framing for the core IPC link.
//!
//! The `service NunyaCoreService` block in the core's `proto/nunya.proto` is there to generate
//! message types and to keep the Go handler table honest; nothing on the wire is gRPC.
//! `rpc.Serve` in the core's `internal/rpc/dispatch.go` reads and writes these two little-endian
//! frames directly:
//!
//! ```text
//! request   [u32 id][u16 method_len][method][u32 payload_len][payload]
//! response  [u32 id][u8  status    ][u32 payload_len][payload]
//! ```
//!
//! `status` is 0 for a normal reply and 1 for a failure, in which case the payload is a UTF-8
//! error string rather than an encoded message.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Handler returned a normally encoded response message.
pub const STATUS_OK: u8 = 0;

/// Payload is an error string. The Go side writes this for an unknown method, a decode failure,
/// a handler error, or a recovered panic.
pub const STATUS_ERR: u8 = 1;

/// Refuse absurd frames rather than trying to allocate them. The largest legitimate response is a
/// connection dump or a diagnostics bundle; both are far below this.
const MAX_FRAME: u32 = 64 * 1024 * 1024;

/// One decoded response frame.
#[derive(Debug)]
pub struct Response {
    pub id: u32,
    pub status: u8,
    pub payload: Vec<u8>,
}

impl Response {
    /// Interpret the frame as the Go side intends: payload-as-message, or payload-as-error-text.
    pub fn into_result(self) -> Result<Vec<u8>, String> {
        if self.status == STATUS_OK {
            Ok(self.payload)
        } else {
            Err(String::from_utf8_lossy(&self.payload).into_owned())
        }
    }
}

/// Encode a request frame. Kept separate from the write so the caller can hold its lock for the
/// shortest possible time, and so this stays unit-testable without a socket.
pub fn encode_request(id: u32, method: &str, payload: &[u8]) -> Vec<u8> {
    let method = method.as_bytes();
    let mut buf = Vec::with_capacity(4 + 2 + method.len() + 4 + payload.len());
    buf.extend_from_slice(&id.to_le_bytes());
    buf.extend_from_slice(&(method.len() as u16).to_le_bytes());
    buf.extend_from_slice(method);
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

/// Read exactly one response frame.
///
/// Returns `UnexpectedEof` when the core closes the link, which the read loop treats as "the core
/// is gone" rather than as a protocol error.
pub async fn read_response<R>(reader: &mut R) -> io::Result<Response>
where
    R: AsyncReadExt + Unpin,
{
    let mut head = [0u8; 9];
    reader.read_exact(&mut head).await?;

    let id = u32::from_le_bytes(head[0..4].try_into().unwrap());
    let status = head[4];
    let len = u32::from_le_bytes(head[5..9].try_into().unwrap());

    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("core sent a {len} byte frame, refusing to allocate it"),
        ));
    }

    let mut payload = vec![0u8; len as usize];
    if len > 0 {
        reader.read_exact(&mut payload).await?;
    }

    Ok(Response {
        id,
        status,
        payload,
    })
}

/// Write a pre-encoded request frame and flush it.
pub async fn write_all<W>(writer: &mut W, frame: &[u8]) -> io::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    writer.write_all(frame).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests;

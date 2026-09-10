use crate::protocol::MAX_FRAME_BYTES;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Reject a frame before allocating unbounded data, including unterminated input.
pub async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<Vec<u8>>, String> {
    read_frame_limit(reader, MAX_FRAME_BYTES).await
}

pub async fn read_frame_limit<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Option<Vec<u8>>, String> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await.map_err(|e| e.to_string())?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err("truncated-frame".into())
            };
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(available.len());
        if frame.len() + count > limit {
            return Err("frame-too-large".into());
        }
        let completed = available[count - 1] == b'\n';
        frame.extend_from_slice(&available[..count]);
        reader.consume(count);
        if completed {
            return Ok(Some(frame));
        }
    }
}

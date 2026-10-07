use crate::protocol::MAX_FRAME_BYTES;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

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
    (&mut *reader)
        .take(limit as u64)
        .read_until(b'\n', &mut frame)
        .await
        .map_err(|e| e.to_string())?;
    if frame.last() == Some(&b'\n') {
        return Ok(Some(frame));
    }
    // Peek after reaching the cap instead of allocating cap+1 bytes, which
    // could double the Vec's capacity for a hostile maximum-size frame.
    if frame.len() == limit
        && !reader
            .fill_buf()
            .await
            .map_err(|e| e.to_string())?
            .is_empty()
    {
        return Err("frame-too-large".into());
    }
    if frame.is_empty() {
        Ok(None)
    } else {
        Err("truncated-frame".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn exact_limits_preserve_bytes_and_the_next_frame_across_chunks() {
        for size in [1, 2, 31, 32, 127, 128, 8192, 8193] {
            let mut first = vec![0xff; size];
            first[size - 1] = b'\n';
            let mut input = first.clone();
            input.extend_from_slice("資料😀\r\n".as_bytes());
            for chunk in [1, 2, 31, 8192] {
                let mut reader = BufReader::with_capacity(chunk, input.as_slice());
                assert_eq!(
                    read_frame_limit(&mut reader, size).await.unwrap(),
                    Some(first.clone())
                );
                assert_eq!(
                    read_frame(&mut reader).await.unwrap().unwrap(),
                    "資料😀\r\n".as_bytes()
                );
                assert_eq!(read_frame(&mut reader).await.unwrap(), None);
            }
        }
    }

    #[tokio::test]
    async fn distinguishes_empty_partial_and_oversized_frames_at_every_limit() {
        for limit in [0, 1, 2, 31, 32, 127, 128, 8192, 8193] {
            for chunk in [1, 31, 8192] {
                let mut empty = BufReader::with_capacity(chunk, &b""[..]);
                assert_eq!(read_frame_limit(&mut empty, limit).await.unwrap(), None);
                let partial = vec![b'x'; limit];
                let mut reader = BufReader::with_capacity(chunk, partial.as_slice());
                let result = read_frame_limit(&mut reader, limit).await;
                if limit == 0 {
                    assert_eq!(result.unwrap(), None);
                } else {
                    assert_eq!(result.unwrap_err(), "truncated-frame");
                }
                for terminated in [false, true] {
                    let mut oversized = vec![b'x'; limit + 1];
                    if terminated {
                        oversized[limit] = b'\n';
                    }
                    let mut reader = BufReader::with_capacity(chunk, oversized.as_slice());
                    assert_eq!(
                        read_frame_limit(&mut reader, limit).await.unwrap_err(),
                        "frame-too-large"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn a_pending_frame_keeps_partial_bytes_across_other_select_branches() {
        let (mut writer, input) = tokio::io::duplex(64);
        let mut reader = BufReader::with_capacity(3, input);
        writer.write_all(b"{\"data\":").await.unwrap();
        {
            let next = read_frame(&mut reader);
            tokio::pin!(next);
            assert!(tokio::time::timeout(Duration::from_millis(1), &mut next)
                .await
                .is_err());
            writer.write_all(b"42}\nnext\n").await.unwrap();
            assert_eq!(next.await.unwrap().unwrap(), b"{\"data\":42}\n");
        }
        assert_eq!(read_frame(&mut reader).await.unwrap().unwrap(), b"next\n");
    }

    #[tokio::test]
    async fn reaching_the_cap_waits_for_eof_or_one_more_byte() {
        for extra_byte in [false, true] {
            let (mut writer, input) = tokio::io::duplex(16);
            let mut reader = BufReader::new(input);
            writer.write_all(b"xxxx").await.unwrap();
            let next = read_frame_limit(&mut reader, 4);
            tokio::pin!(next);
            assert!(tokio::time::timeout(Duration::from_millis(1), &mut next)
                .await
                .is_err());
            if extra_byte {
                writer.write_all(b"\n").await.unwrap();
            }
            writer.shutdown().await.unwrap();
            assert_eq!(
                next.await.unwrap_err(),
                if extra_byte {
                    "frame-too-large"
                } else {
                    "truncated-frame"
                }
            );
        }
    }
}

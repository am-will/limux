use std::io::{self, BufRead, ErrorKind};
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::time::timeout;

pub const MAX_REQUEST_LEN: usize = 1024 * 1024;
pub const MAX_CONNECTIONS: usize = 64;
pub const CLIENT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn read_request_frame_async<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    line_buf: &mut Vec<u8>,
) -> io::Result<bool> {
    line_buf.clear();
    let eof = loop {
        let available = match timeout(CLIENT_IDLE_TIMEOUT, reader.fill_buf()).await {
            Ok(result) => result?,
            Err(_) => return Ok(false),
        };

        if available.is_empty() {
            break true;
        }

        match available.iter().position(|byte| *byte == b'\n') {
            Some(position) => {
                if line_buf.len() + position > MAX_REQUEST_LEN {
                    return Ok(false);
                }
                line_buf.extend_from_slice(&available[..position]);
                reader.consume(position + 1);
                break false;
            }
            None => {
                let len = available.len();
                line_buf.extend_from_slice(available);
                reader.consume(len);
                if line_buf.len() > MAX_REQUEST_LEN {
                    return Ok(false);
                }
            }
        }
    };

    Ok(!(eof && line_buf.is_empty()))
}

pub fn read_request_frame<R: BufRead>(reader: &mut R, line_buf: &mut Vec<u8>) -> io::Result<bool> {
    line_buf.clear();
    let eof = loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if is_timeout(&error) => return Ok(false),
            Err(error) => return Err(error),
        };

        if available.is_empty() {
            break true;
        }

        match available.iter().position(|byte| *byte == b'\n') {
            Some(position) => {
                if line_buf.len() + position > MAX_REQUEST_LEN {
                    return Ok(false);
                }
                line_buf.extend_from_slice(&available[..position]);
                reader.consume(position + 1);
                break false;
            }
            None => {
                let len = available.len();
                line_buf.extend_from_slice(available);
                reader.consume(len);
                if line_buf.len() > MAX_REQUEST_LEN {
                    return Ok(false);
                }
            }
        }
    };

    Ok(!(eof && line_buf.is_empty()))
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::BufReader;
    use std::io::Cursor;

    #[test]
    fn sync_reader_accepts_partial_line_at_eof() {
        let mut reader = BufReader::new(Cursor::new(b"ping".to_vec()));
        let mut line_buf = Vec::new();

        assert!(read_request_frame(&mut reader, &mut line_buf).expect("read frame"));
        assert_eq!(line_buf, b"ping");
        assert!(!read_request_frame(&mut reader, &mut line_buf).expect("read eof"));
    }

    #[tokio::test]
    async fn async_reader_rejects_oversized_request() {
        let payload = vec![b'a'; MAX_REQUEST_LEN + 1];
        let mut reader = tokio::io::BufReader::new(Cursor::new(payload));
        let mut line_buf = Vec::new();

        assert!(!read_request_frame_async(&mut reader, &mut line_buf)
            .await
            .expect("oversized frame closes connection"));
    }
}

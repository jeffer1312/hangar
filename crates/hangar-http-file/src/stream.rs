use std::collections::VecDeque;

use axum::body::{Body, Bytes};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub(crate) enum Piece {
    Bytes(Bytes),
    Data { offset: u64, left: u64 },
}
pub(crate) fn file_stream(file: tokio::fs::File, pieces: VecDeque<Piece>) -> Body {
    Body::from_stream(futures_util::stream::try_unfold(
        (file, pieces),
        |(mut file, mut pieces)| async move {
            match pieces.pop_front() {
                None => Ok::<_, std::io::Error>(None),
                Some(Piece::Bytes(bytes)) => Ok(Some((bytes, (file, pieces)))),
                Some(Piece::Data { offset, left }) => {
                    file.seek(std::io::SeekFrom::Start(offset)).await?;
                    let mut bytes = vec![0; (left.min(64 * 1024)) as usize];
                    let n = file.read(&mut bytes).await?;
                    if n == 0 {
                        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
                    }
                    bytes.truncate(n);
                    if left > n as u64 {
                        pieces.push_front(Piece::Data {
                            offset: offset + n as u64,
                            left: left - n as u64,
                        });
                    }
                    Ok(Some((Bytes::from(bytes), (file, pieces))))
                }
            }
        },
    ))
}

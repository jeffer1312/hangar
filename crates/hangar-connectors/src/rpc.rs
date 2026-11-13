//! Enquadramento por linha, usado pelo host e por quem escreve conector em Rust.
use crate::protocol::{ConnectorError, ErrorCode, MAX_MESSAGE_BYTES};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// Lê uma mensagem (sem o `\n`). `None` é o fim limpo da entrada; linha pela metade no fim é
/// mensagem truncada, e linha acima do teto é recusada sem ser cortada.
pub async fn read_message<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Vec<u8>>, ConnectorError> {
    read_message_limited(reader, MAX_MESSAGE_BYTES).await
}

pub async fn read_message_limited<R: AsyncBufRead + Unpin>(reader: &mut R, limit: usize) -> Result<Option<Vec<u8>>, ConnectorError> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf().await
            .map_err(|error| ConnectorError::new(ErrorCode::Unavailable, format!("leitura do conector falhou: {:?}", error.kind())))?;
        if buffer.is_empty() {
            return if line.is_empty() { Ok(None) } else { Err(ConnectorError::protocol("mensagem truncada no fim da saída")) };
        }
        let (take, done) = match buffer.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index + 1, true),
            None => (buffer.len(), false),
        };
        // `limit` conta a mensagem sem o `\n`.
        if line.len() + take - usize::from(done) > limit {
            reader.consume(take);
            return Err(ConnectorError::new(ErrorCode::PayloadTooLarge, format!("mensagem acima de {limit} bytes")));
        }
        line.extend_from_slice(&buffer[..take]);
        reader.consume(take);
        if done {
            line.pop();
            if line.last() == Some(&b'\r') { line.pop(); }
            return Ok(Some(line));
        }
    }
}

/// Escreve uma mensagem numa linha, recusando a que passa do teto.
pub async fn write_message<W: AsyncWrite + Unpin>(writer: &mut W, message: &Value) -> Result<(), ConnectorError> {
    let mut bytes = serde_json::to_vec(message).map_err(|_| ConnectorError::protocol("mensagem não serializou"))?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(ConnectorError::new(ErrorCode::PayloadTooLarge, format!("mensagem acima de {MAX_MESSAGE_BYTES} bytes")));
    }
    bytes.push(b'\n');
    let failed = |error: std::io::Error| ConnectorError::new(ErrorCode::Unavailable, format!("escrita ao conector falhou: {:?}", error.kind()));
    writer.write_all(&bytes).await.map_err(failed)?;
    writer.flush().await.map_err(failed)
}

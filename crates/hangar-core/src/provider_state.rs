//! Sinais de estado próprios dos agentes com terminal, sem dependência do servidor.
use std::path::Path;
use std::io::{Read, Seek, SeekFrom};

const WIRE_CHUNK: u64 = 64 << 10;
const WIRE_LIMIT: u64 = 4 << 20;

/// A última fronteira do main vence escrita de configuração e atividade dos subagentes.
pub fn kimi_turn_open(path: &Path) -> std::io::Result<Option<bool>> {
    let mut file = std::fs::File::open(path)?;
    let end = file.seek(SeekFrom::End(0))?;
    let mut position = end;
    let mut pending = Vec::new();
    while position > 0 && end - position < WIRE_LIMIT {
        let size = position.min(WIRE_CHUNK);
        position -= size;
        file.seek(SeekFrom::Start(position))?;
        let mut bytes = vec![0; size as usize];
        file.read_exact(&mut bytes)?;
        bytes.extend_from_slice(&pending);
        let mut lines = bytes.split(|byte| *byte == b'\n');
        pending = if position > 0 {
            lines.next().unwrap_or_default().to_vec()
        } else {
            Vec::new()
        };
        for line in lines.rev() {
            if !line.windows(5).any(|part| part == b"turn.") { continue; }
            let Ok(text) = std::str::from_utf8(line) else { continue; };
            let Some(value) = crate::pycompat::pyjson::loads_lossless(text) else { continue; };
            match value.get("type").and_then(serde_json::Value::as_str) {
                Some("turn.prompt" | "turn.steer") => return Ok(Some(true)),
                Some("turn.ended" | "turn.cancel") => return Ok(Some(false)),
                _ => {}
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn kimi_turn_boundary_survives_a_large_configuration_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wire.jsonl");
        let config = serde_json::json!({"type":"config.update","systemPrompt":"x".repeat(200_000)});
        std::fs::write(&path, format!("{{\"type\":\"turn.prompt\"}}\n{config}\n")).unwrap();
        assert_eq!(kimi_turn_open(&path).unwrap(), Some(true));
        std::fs::write(&path, format!("{{\"type\":\"turn.ended\"}}\n{config}\n")).unwrap();
        assert_eq!(kimi_turn_open(&path).unwrap(), Some(false));
    }
}

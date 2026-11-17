//! `hangar-server claude-md-block`: o bloco Sessões-irmãs no `~/.claude/CLAUDE.md` (as contas o
//! linkam). O texto vem pela entrada padrão: ele mora no heredoc do `install-hangar-send.sh`. Com o
//! marcador de início, cada trecho entre os marcadores vira o bloco; sem, o bloco vai para o fim.
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use crate::{Invocation, files};

/// Nome antigo de propósito: trocar o marcador duplicaria o bloco nas máquinas que já o têm.
pub const START: &str = "<!-- claude-pocket:sessoes-irmas:start -->";
pub const END: &str = "<!-- claude-pocket:sessoes-irmas:end -->";

pub fn cli(args: &[String]) -> ExitCode {
    crate::cli(args, "claude-md-block", true, run)
}

pub fn run(invocation: &Invocation, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let block = String::from_utf8_lossy(&invocation.stdin);
    let block = block.trim_end_matches(['\n', '\r']);
    if block.is_empty() {
        let _ = writeln!(err, "erro: o texto do bloco Sessões-irmãs não chegou pela entrada padrão");
        return 2;
    }
    let file = invocation.target_home().join(".claude").join("CLAUDE.md");
    match update(&file, block) {
        Ok(Outcome::Replaced) => {
            let _ = writeln!(out, "ok: bloco Sessões-irmãs ATUALIZADO em {}", file.display());
            0
        }
        Ok(Outcome::Appended) => {
            let _ = writeln!(out, "ok: bloco Sessões-irmãs ADICIONADO em {}", file.display());
            0
        }
        Ok(Outcome::Unclosed) => {
            let _ = writeln!(err, "aviso: {} tem o início do bloco Sessões-irmãs sem o fim; arquivo deixado como está", file.display());
            1
        }
        Err(detail) => {
            let _ = writeln!(err, "erro: {detail}");
            1
        }
    }
}

enum Outcome { Replaced, Appended, Unclosed }

fn update(file: &Path, block: &str) -> Result<Outcome, String> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("não consegui criar {}: {}", dir.display(), files::os_error(&error, dir)))?;
    }
    // Link segue link (como o `touch` e o `open` de antes): grava-se no alvo e o link fica.
    if !file.exists() {
        std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(file)
            .map_err(|error| format!("não consegui criar {}: {}", file.display(), files::os_error(&error, file)))?;
    }
    let raw = std::fs::read(file).map_err(|error| format!("não consegui ler {}: {}", file.display(), files::os_error(&error, file)))?;
    // O bloco entra no fim de linha que o arquivo já usa.
    let crlf = raw.windows(2).any(|pair| pair == b"\r\n");
    let block = if crlf { block.replace('\n', "\r\n") } else { block.to_owned() };
    // O arquivo inteiro vai por temporário + rename: disco cheio no meio não deixa o CLAUDE.md global
    // cortado. Arquivo sem escrita continua recusado, como antes, e o link segue link.
    let write = |bytes: &[u8]| {
        let refused = |error: std::io::Error| format!("não consegui gravar {}: {}", file.display(), files::os_error(&error, file));
        std::fs::OpenOptions::new().write(true).open(file).map_err(refused)?;
        let target = files::through_links(file);
        let mode = files::mode_of(&target).unwrap_or(0o644);
        files::replace(&target, bytes, mode).map_err(|detail| format!("não consegui gravar {}: {detail}", file.display()))
    };
    if !raw.windows(START.len()).any(|window| window == START.as_bytes()) {
        let eol = if crlf { "\r\n" } else { "\n" };
        let mut whole = raw.clone();
        whole.extend_from_slice(format!("{eol}{block}{eol}").as_bytes());
        write(&whole)?;
        return Ok(Outcome::Appended);
    }
    let text = std::str::from_utf8(&raw).map_err(|_| format!("{} não é texto UTF-8 — bloco Sessões-irmãs não gravado", file.display()))?;
    let Some(updated) = replace_blocks(text, &block) else { return Ok(Outcome::Unclosed) };
    if updated != text {
        write(updated.as_bytes())?;
    }
    Ok(Outcome::Replaced)
}

/// Cada trecho do início ao fim mais próximo vira o bloco; `None` quando nenhum início tem fim.
pub fn replace_blocks(text: &str, block: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len() + block.len());
    let mut rest = text;
    let mut replaced = false;
    while let Some(start) = rest.find(START) {
        let after = start + START.len();
        let Some(end) = rest[after..].find(END) else { break };
        out.push_str(&rest[..start]);
        out.push_str(block);
        rest = &rest[after + end + END.len()..];
        replaced = true;
    }
    out.push_str(rest);
    replaced.then_some(out)
}

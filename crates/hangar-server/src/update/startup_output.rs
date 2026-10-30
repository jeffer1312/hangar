//! A saída da versão nova que não respondeu depois do reinício: só traceback e linhas de nível
//! ERROR/CRITICAL, com `scrub`, para o relatório dizer por que ela não subiu. Fora desse caso o log
//! do backend não sai da máquina.
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use regex::Regex;

use crate::diag_report::Scrubber;

const MAX_LINES: usize = 40;
const LINE_MAX: usize = 300;
/// Uma subida que morre escreve pouco; o teto só protege de um log que cresceu sem parar.
const READ_MAX: u64 = 256 * 1024;

/// Onde a saída do backend estava antes do reinício.
pub enum Mark {
    /// Windows: a tarefa grava em `hangar-backend*.log` (`app.main --log`); o mais novo e o tamanho dele.
    File { dir: PathBuf, newest: Option<(PathBuf, u64)>, at: SystemTime },
    /// systemd do usuário: a unit roda sem `--log`, a saída fica no journal.
    Journal { since: SystemTime },
    /// Reinício na mão: ninguém sobe a versão nova, não há o que ler.
    Nothing,
}

fn backend_logs(dir: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut logs: Vec<_> = entries.flatten().filter(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        name.starts_with("hangar-backend") && name.ends_with(".log")
    }).filter_map(|e| {
        let meta = e.metadata().ok()?;
        Some((e.path(), meta.modified().ok()?, meta.len()))
    }).collect();
    logs.sort_by_key(|(_, modified, _)| *modified);
    logs
}

impl Mark {
    pub fn take(manual: bool) -> Mark {
        if manual { return Mark::Nothing; }
        if cfg!(windows) {
            let Some(base) = crate::runtime::local_policy::log_base() else { return Mark::Nothing };
            let dir = base.join("privado");
            let newest = backend_logs(&dir).pop().map(|(path, _, len)| (path, len));
            return Mark::File { dir, newest, at: SystemTime::now() };
        }
        Mark::Journal { since: SystemTime::now() }
    }

    /// O que entrou depois da marca. Bloqueante. `Err`: por que não deu para ler, que também vai ao
    /// relatório; sem isso ele sairia sem a causa e sem dizer por quê.
    pub fn read(&self) -> Result<String, String> {
        match self {
            Mark::Nothing => Ok(String::new()),
            Mark::File { dir, newest, at } => {
                let logs = backend_logs(dir);
                if logs.is_empty() { return Err("nenhum hangar-backend*.log na pasta de logs".to_owned()); }
                let mut out = String::new();
                for (path, modified, _) in logs {
                    let offset = match newest { Some((marked, len)) if *marked == path => *len, _ if modified > *at => 0, _ => continue };
                    out += &tail_from(&path, offset);
                    out.push('\n');
                }
                Ok(out)
            }
            Mark::Journal { since } => {
                let epoch = since.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let output = std::process::Command::new("journalctl")
                    .args(["--user", "-u", "hangar-backend.service", "--since", &format!("@{epoch}"), "-n", "5000", "-o", "cat", "--no-pager"])
                    .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output()
                    .map_err(|e| format!("journalctl não rodou: {e}"))?;
                if !output.status.success() { return Err(format!("journalctl saiu com {}", output.status)); }
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            }
        }
    }
}

/// Do `offset` ao fim, no máximo `READ_MAX` do fim. O backend segue escrevendo: o Windows abre com
/// compartilhamento de leitura e escrita por padrão.
fn tail_from(path: &Path, offset: u64) -> String {
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    let len = file.metadata().map_or(0, |m| m.len());
    let start = if len < offset { 0 } else { offset.max(len.saturating_sub(READ_MAX)) };
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.take(READ_MAX).read_to_end(&mut bytes).is_err() { return String::new(); }
    String::from_utf8_lossy(&bytes).into_owned()
}

static RUST_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\S+Z\s+(TRACE|DEBUG|INFO|WARN|ERROR)\b").unwrap());
static LEVEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:\S+\s+){0,3}(ERROR|CRITICAL)(?::|\s|$)").unwrap());
static EXCEPTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][\w.]*(Error|Exception|Exit|Interrupt)\b").unwrap());

/// As linhas que podem sair: blocos de traceback, nível ERROR/CRITICAL e a falha do lançador. As do
/// `hangar-server` ficam de fora (o relatório já as leva pela lista fixa de campos). As últimas
/// `MAX_LINES`, cortadas em `LINE_MAX`, depois do `scrub`.
pub fn errors(text: &str, scrubber: &Scrubber) -> Vec<String> {
    let mut kept = Vec::new();
    let mut in_traceback = false;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || RUST_LINE.is_match(line) { continue; }
        let keep = if line.starts_with("Traceback (most recent call last)") {
            in_traceback = true;
            true
        } else if in_traceback {
            if !line.starts_with(char::is_whitespace) { in_traceback = false; }
            line.starts_with(char::is_whitespace) || EXCEPTION.is_match(line)
        } else {
            LEVEL.is_match(line) || line.contains("hangar-server launch:")
        };
        if keep { kept.push(line); }
    }
    let skip = kept.len().saturating_sub(MAX_LINES);
    kept[skip..].iter().map(|line| scrubber.scrub(line).chars().take(LINE_MAX).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrubber() -> Scrubber { Scrubber::new(Some("/home/fulano".into()), Some(r"C:\Users\Fulano\AppData\Local".into())) }

    #[test]
    fn keeps_traceback_and_error_lines_only() {
        let text = concat!(
            "2026-10-30 08:31:00,100 INFO [hangar.sse] sessão projeto-exemplo abriu\n",
            "2026-10-30 08:31:00,200 WARNING [hangar.loop_monitor] laço lento\n",
            "2026-10-30T11:31:00.300Z  WARN estado: falhou code=x detail=\"conversa secreta\"\n",
            "2026-10-30T11:31:00.400Z ERROR lista: falhou detail=\"outra conversa\"\n",
            "Traceback (most recent call last):\n",
            "  File \"C:\\Users\\Fulano\\.hangar\\current\\backend\\app\\main.py\", line 12, in <module>\n",
            "    from app import api\n",
            "ModuleNotFoundError: No module named 'app.novo'\n",
            "[hangar] AVISO: log preso\n",
            "2026-10-30 08:31:01,000 ERROR [hangar] subida falhou Authorization: Bearer s3cr3t em /home/fulano/x\n",
            "[hangar] hangar-server launch: python_missing: C:\\Users\\Fulano\\.hangar\\current\\python\\python.exe\n",
        );
        assert_eq!(errors(text, &scrubber()), vec![
            "Traceback (most recent call last):".to_owned(),
            "  File \"~\\.hangar\\current\\backend\\app\\main.py\", line 12, in <module>".to_owned(),
            "    from app import api".to_owned(),
            "ModuleNotFoundError: No module named 'app.novo'".to_owned(),
            "2026-10-30 08:31:01,000 ERROR [hangar] subida falhou Authorization: Bearer *** em ~/x".to_owned(),
            "[hangar] hangar-server launch: python_missing: ~\\.hangar\\current\\python\\python.exe".to_owned(),
        ]);
    }

    #[test]
    fn caps_the_count_and_the_width() {
        let text: String = (0..60).map(|i| format!("ERROR linha {i} {}\n", "x".repeat(400))).collect();
        let out = errors(&text, &scrubber());
        assert_eq!(out.len(), MAX_LINES);
        assert!(out[0].starts_with("ERROR linha 20 "));
        assert!(out.iter().all(|l| l.chars().count() == LINE_MAX));
    }

    #[test]
    fn file_mark_reads_what_came_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("hangar-backend.log");
        std::fs::write(&log, "ERROR velho\n").unwrap();
        let mark = Mark::File { dir: dir.path().into(), newest: Some((log.clone(), 12)), at: SystemTime::now() };
        std::fs::write(&log, "ERROR velho\nERROR novo\n").unwrap();
        assert_eq!(errors(&mark.read().unwrap(), &scrubber()), vec!["ERROR novo".to_owned()]);
    }
}

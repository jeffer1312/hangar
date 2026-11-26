//! Linha de status da sessão Kimi montada pelos arquivos: o Kimi Code não oferece statusline ao Hangar, e a barra
//! da sessão (modelo, contexto, cota) não deve depender do rodapé desenhado na tela.
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// O que a conversa diz do agente principal: modelo e esforço vigentes e os tokens no contexto.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct WireFacts {
    pub alias: Option<String>,
    pub effort: Option<String>,
    pub tokens: Option<f64>,
}

impl WireFacts {
    fn absorb(&mut self, line: &str) {
        let Ok(event) = serde_json::from_str::<Value>(line) else { return };
        // Subagente tem o próprio modelo e contexto; a barra é da conversa principal.
        if event["agentId"].as_str().is_some_and(|agent| agent != "main") {
            return;
        }
        match event["type"].as_str() {
            Some("profile.bind" | "config.update") => {
                if let Some(alias) = event["modelAlias"].as_str().filter(|s| !s.is_empty()) {
                    self.alias = Some(alias.to_owned());
                }
                if let Some(effort) = event["thinkingEffort"].as_str().filter(|s| !s.is_empty()) {
                    self.effort = Some(effort.to_owned());
                }
            }
            Some("token_counting.measured" | "token_counting.turn_recorded") => {
                if let Some(tokens) = event["tokens"].as_f64().filter(|n| n.is_finite() && *n >= 0.0) {
                    self.tokens = Some(tokens);
                }
            }
            _ => {}
        }
    }
}

/// Leitura incremental do `wire.jsonl`: só o que cresceu desde a última rodada, e a linha ainda incompleta espera.
#[derive(Default)]
pub(crate) struct WireTail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    facts: WireFacts,
}

impl WireTail {
    pub(crate) fn read(&mut self, path: &Path) -> std::io::Result<&WireFacts> {
        let mut file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        // Outro arquivo ou arquivo que encolheu: a conversa mudou, recomeça do zero.
        if self.path != path || len < self.offset {
            *self = Self { path: path.to_owned(), ..Self::default() };
        }
        if len > self.offset {
            file.seek(SeekFrom::Start(self.offset))?;
            let mut chunk = Vec::new();
            file.take(len - self.offset).read_to_end(&mut chunk)?;
            self.offset += chunk.len() as u64;
            self.partial.extend_from_slice(&chunk);
            if let Some(end) = self.partial.iter().rposition(|b| *b == b'\n') {
                let complete: Vec<u8> = self.partial.drain(..=end).collect();
                for line in String::from_utf8_lossy(&complete).lines() {
                    self.facts.absorb(line);
                }
            }
        }
        Ok(&self.facts)
    }
}

fn amount(n: f64) -> String {
    if n >= 1e6 {
        let m = n / 1e6;
        if (m - m.round()).abs() < 0.05 { format!("{}M", m.round()) } else { format!("{m:.1}M") }
    } else if n >= 1e3 {
        format!("{}k", (n / 1e3).round())
    } else {
        format!("{}", n.round())
    }
}

fn until(seconds: f64) -> Option<String> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    let minutes = (seconds / 60.0).ceil() as u64;
    let (days, hours, mins) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
    Some(match (days, hours, mins) {
        (0, 0, m) => format!("{m}m"),
        (0, h, 0) => format!("{h}h"),
        (0, h, m) => format!("{h}h{m}m"),
        (d, 0, _) => format!("{d}d"),
        (d, h, _) => format!("{d}d{h}h"),
    })
}

/// A linha no formato que os clientes já leem (`🤖`, `💬 ctx`, `⚡5h`, `📅7d`, `🗓30d`). Sem modelo conhecido, nada.
pub(crate) fn line(facts: &WireFacts, config: &str, quotas: Option<&Value>, now: f64) -> Option<String> {
    let alias = facts.alias.as_deref()?;
    let table = config.parse::<toml::Table>().ok();
    let model = table.as_ref().and_then(|t| t.get("models")?.get(alias)?.as_table().cloned());
    let text = |key: &str| model.as_ref().and_then(|m| m.get(key)?.as_str().map(str::to_owned));
    let name = text("display_name").unwrap_or_else(|| alias.rsplit('/').next().unwrap_or(alias).to_owned());
    let mut head = format!("🤖 {name}");
    if let Some(effort) = &facts.effort {
        head.push_str(&format!(" ({effort})"));
    }
    let mut parts = vec![head];
    let window = model.as_ref().and_then(|m| m.get("max_context_size")?.as_integer()).map(|n| n as f64);
    if let (Some(used), Some(window)) = (facts.tokens, window.filter(|w| *w > 0.0)) {
        parts.push(format!("💬 ctx {}/{}", amount(used), amount(window)));
    }
    let provider = text("provider").or_else(|| alias.split_once('/').map(|(p, _)| p.to_owned()));
    let windows = provider
        .and_then(|provider| quotas?.get(format!("kimi:{provider}"))?.get("cota")?.get("janelas")?.as_array().cloned())
        .unwrap_or_default();
    for window in windows {
        let icon = match window["rotulo"].as_str() {
            Some("5h") => "⚡5h",
            Some("7d") => "📅7d",
            Some("30d") => "🗓30d",
            _ => continue,
        };
        let Some(pct) = window["pct"].as_f64().filter(|p| p.is_finite()) else { continue };
        let mut segment = format!("{icon}:{}%", pct.round());
        if let Some(reset) = window["reset_ts"].as_f64().and_then(|ts| until(ts - now)) {
            segment.push_str(&format!(" ↺{reset}"));
        }
        parts.push(segment);
    }
    Some(parts.join(" │ "))
}

/// O cache de cota que a tela de contas grava (`AccountService::quota_path`).
pub(crate) fn quota_cache_path(home: &Path) -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Local"))
            .join("hangar/cotas-cache.json")
    } else {
        home.join(".hangar/cotas-cache.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CONFIG: &str = "[models.\"kimi-code/k3-256k\"]\nprovider = \"managed:kimi-code\"\nmodel = \"k3-256k\"\nmax_context_size = 262144\ndisplay_name = \"K3-256k\"\n";

    #[test]
    fn wire_follows_the_main_agent_model_effort_and_context() {
        let mut facts = WireFacts::default();
        for line in [
            r#"{"type":"profile.bind","agentId":"main","modelAlias":"kimi-code/k3.1-pro","thinkingEffort":"high"}"#,
            r#"{"type":"token_counting.measured","agentId":"main","tokens":18164}"#,
            r#"{"type":"token_counting.measured","agentId":"sub-1","tokens":999999}"#,
            r#"{"type":"config.update","agentId":"main","modelAlias":"kimi-code/k3-256k","thinkingEffort":"max"}"#,
            r#"{"type":"token_counting.turn_recorded","agentId":"main","turnId":5,"tokens":21058}"#,
        ] {
            facts.absorb(line);
        }
        assert_eq!(facts, WireFacts { alias: Some("kimi-code/k3-256k".into()), effort: Some("max".into()), tokens: Some(21058.0) });
    }

    #[test]
    fn tail_reads_only_what_grew_and_waits_for_the_line_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wire.jsonl");
        std::fs::write(&path, "{\"type\":\"profile.bind\",\"agentId\":\"main\",\"modelAlias\":\"a/x\"}\n{\"type\":\"token_counting.measured\",\"agentId\":\"main\",\"tok").unwrap();
        let mut tail = WireTail::default();
        assert_eq!(tail.read(&path).unwrap().tokens, None);
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(&mut file, b"ens\":7}\n").unwrap();
        let facts = tail.read(&path).unwrap();
        assert_eq!((facts.alias.as_deref(), facts.tokens), (Some("a/x"), Some(7.0)));
    }

    #[test]
    fn line_joins_model_context_and_plan_windows() {
        let facts = WireFacts { alias: Some("kimi-code/k3-256k".into()), effort: Some("high".into()), tokens: Some(21058.0) };
        let quotas = json!({"kimi:managed:kimi-code":{"cota":{"janelas":[
            {"rotulo":"5h","pct":70.6,"reset_ts":1000.0 + 5880.0},
            {"rotulo":"30d","pct":4.7,"reset_ts":1000.0 + 24.0 * 86400.0}]}}});
        assert_eq!(
            line(&facts, CONFIG, Some(&quotas), 1000.0).as_deref(),
            Some("🤖 K3-256k (high) │ 💬 ctx 21k/262k │ ⚡5h:71% ↺1h38m │ 🗓30d:5% ↺24d")
        );
        assert_eq!(line(&WireFacts::default(), CONFIG, Some(&quotas), 1000.0), None);
        let unknown = WireFacts { alias: Some("p/m".into()), ..WireFacts::default() };
        assert_eq!(line(&unknown, CONFIG, None, 1000.0).as_deref(), Some("🤖 m"));
    }
}

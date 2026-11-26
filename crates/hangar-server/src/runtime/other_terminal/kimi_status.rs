//! Linha de status da sessão Kimi montada pelos arquivos: o Kimi Code não oferece statusline ao Hangar, e a barra
//! da sessão (modelo, contexto, cota) não deve depender do rodapé desenhado na tela.
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
                let alias = event["modelAlias"].as_str().filter(|s| !s.is_empty());
                let switched = alias.is_some_and(|alias| self.alias.as_deref() != Some(alias));
                // Campo presente manda (vazio ou nulo desliga); modelo trocado sem ele não herda o esforço do anterior.
                if event.get("thinkingEffort").is_some() || switched {
                    self.effort = event["thinkingEffort"].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
                }
                if let Some(alias) = alias {
                    self.alias = Some(alias.to_owned());
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

const BLOCK: usize = 256 * 1024;
const MAX_LINE: usize = 1 << 20;

/// Leitura incremental do `wire.jsonl`: só o que cresceu desde a última rodada, e a linha ainda incompleta espera.
#[derive(Default)]
pub(crate) struct WireTail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    /// A linha em curso passou de `MAX_LINE`: o resto dela é descartado até a quebra.
    skipping: bool,
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
            // Em blocos: a primeira leitura de uma conversa longa não carrega o arquivo inteiro na memória.
            file.seek(SeekFrom::Start(self.offset))?;
            let mut block = vec![0; BLOCK];
            let mut remaining = len - self.offset;
            while remaining > 0 {
                let want = remaining.min(BLOCK as u64) as usize;
                let read = file.read(&mut block[..want])?;
                if read == 0 {
                    break;
                }
                self.offset += read as u64;
                remaining -= read as u64;
                self.consume(&block[..read]);
            }
        }
        Ok(&self.facts)
    }

    fn consume(&mut self, mut bytes: &[u8]) {
        while let Some(end) = bytes.iter().position(|b| *b == b'\n') {
            if !self.skipping && self.partial.len() + end <= MAX_LINE {
                self.partial.extend_from_slice(&bytes[..end]);
                self.facts.absorb(&String::from_utf8_lossy(&self.partial));
            }
            self.partial.clear();
            self.skipping = false;
            bytes = &bytes[end + 1..];
        }
        if self.skipping {
            return;
        }
        if self.partial.len() + bytes.len() > MAX_LINE {
            self.partial = Vec::new();
            self.skipping = true;
        } else {
            self.partial.extend_from_slice(bytes);
        }
    }
}

/// Arquivo relido só quando muda (data e tamanho); leitura que falha tenta de novo na rodada seguinte.
pub(crate) struct Stamped<T> {
    stamp: Option<(PathBuf, Option<SystemTime>, u64)>,
    value: Option<T>,
}

impl<T> Default for Stamped<T> {
    fn default() -> Self {
        Self { stamp: None, value: None }
    }
}

impl<T> Stamped<T> {
    pub(crate) fn get(&mut self, path: &Path, parse: impl FnOnce(&[u8]) -> Option<T>) -> Option<&T> {
        let stamp = std::fs::metadata(path).ok().map(|m| (path.to_owned(), m.modified().ok(), m.len()));
        if stamp.is_none() || self.stamp != stamp {
            self.stamp = None;
            self.value = None;
            if let Ok(bytes) = std::fs::read(path) {
                self.value = parse(&bytes);
                self.stamp = stamp;
            }
        }
        self.value.as_ref()
    }
}

fn amount(n: f64) -> String {
    // 999,5k já arredonda para 1000k: a faixa do M começa aí.
    if n >= 999_500.0 {
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
pub(crate) fn line(facts: &WireFacts, config: Option<&toml::Table>, quotas: Option<&Value>, now: f64) -> Option<String> {
    let alias = facts.alias.as_deref()?;
    let model = config.and_then(|t| t.get("models")?.get(alias)?.as_table().cloned());
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
    // A fonte de cota tem o nome da tabela do provedor (`kimi:<provider>`), que só o modelo diz.
    let windows = text("provider")
        .and_then(|provider| quotas?.get(format!("kimi:{provider}"))?.get("cota").cloned())
        .filter(|cota| cota["estado"] == "lida")
        .and_then(|cota| cota["janelas"].as_array().cloned())
        .unwrap_or_default();
    for window in windows {
        let icon = match window["rotulo"].as_str() {
            Some("5h") => "⚡5h",
            Some("7d") => "📅7d",
            Some("30d") => "🗓30d",
            _ => continue,
        };
        let Some(pct) = window["pct"].as_f64().filter(|p| p.is_finite()) else { continue };
        let reset_ts = window["reset_ts"].as_f64();
        // Janela que já virou: o percentual guardado é da anterior.
        if reset_ts.is_some_and(|ts| ts <= now) {
            continue;
        }
        let mut segment = format!("{icon}:{}%", pct.round());
        if let Some(reset) = reset_ts.and_then(|ts| until(ts - now)) {
            segment.push_str(&format!(" ↺{reset}"));
        }
        parts.push(segment);
    }
    Some(parts.join(" │ "))
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
    fn effort_follows_the_model_that_is_bound() {
        let mut facts = WireFacts::default();
        facts.absorb(r#"{"type":"profile.bind","agentId":"main","modelAlias":"a/x","thinkingEffort":"high"}"#);
        facts.absorb(r#"{"type":"config.update","agentId":"main","modelAlias":"a/x"}"#);
        assert_eq!(facts.effort.as_deref(), Some("high"), "o mesmo modelo sem o campo guarda o esforço");
        facts.absorb(r#"{"type":"config.update","agentId":"main","modelAlias":"a/y"}"#);
        assert_eq!(facts.effort, None, "outro modelo sem o campo não herda o esforço");
        facts.absorb(r#"{"type":"config.update","agentId":"main","thinkingEffort":"low"}"#);
        assert_eq!(facts.effort.as_deref(), Some("low"));
        facts.absorb(r#"{"type":"config.update","agentId":"main","thinkingEffort":null}"#);
        assert_eq!(facts.effort, None, "nulo desliga");
    }

    #[test]
    fn amount_moves_to_millions_before_rounding_to_1000k() {
        assert_eq!((amount(999_700.0), amount(999_400.0), amount(262_144.0)), ("1M".into(), "999k".into(), "262k".into()));
    }

    #[test]
    fn stamped_rereads_only_when_the_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let mut stamped = Stamped::<usize>::default();
        assert_eq!(stamped.get(&path, |b| Some(b.len())), None);
        std::fs::write(&path, "abc").unwrap();
        assert_eq!(stamped.get(&path, |b| Some(b.len())), Some(&3));
        assert_eq!(stamped.get(&path, |_| panic!("arquivo igual não é relido")), Some(&3));
        std::fs::write(&path, "abcde").unwrap();
        assert_eq!(stamped.get(&path, |b| Some(b.len())), Some(&5));
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
    fn tail_reads_big_files_in_blocks_and_skips_oversized_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wire.jsonl");
        let filler = format!("{{\"type\":\"context.append_message\",\"agentId\":\"main\",\"text\":\"{}\"}}\n", "x".repeat(300 * 1024));
        let huge = format!("{{\"type\":\"token_counting.measured\",\"agentId\":\"main\",\"tokens\":1,\"pad\":\"{}\"}}\n", "y".repeat(MAX_LINE + 10));
        let mut text = String::from("{\"type\":\"profile.bind\",\"agentId\":\"main\",\"modelAlias\":\"a/x\"}\n");
        text.push_str(&filler);
        text.push_str(&huge);
        text.push_str("{\"type\":\"token_counting.measured\",\"agentId\":\"main\",\"tokens\":42}\n");
        std::fs::write(&path, text).unwrap();
        let mut tail = WireTail::default();
        let facts = tail.read(&path).unwrap();
        assert_eq!((facts.alias.as_deref(), facts.tokens), (Some("a/x"), Some(42.0)));
        assert!(tail.partial.capacity() <= MAX_LINE + BLOCK, "a linha gigante não fica guardada");
    }

    #[test]
    fn line_joins_model_context_and_plan_windows() {
        let facts = WireFacts { alias: Some("kimi-code/k3-256k".into()), effort: Some("high".into()), tokens: Some(21058.0) };
        let config = CONFIG.parse::<toml::Table>().unwrap();
        let quotas = json!({"kimi:managed:kimi-code":{"cota":{"estado":"lida","janelas":[
            {"rotulo":"5h","pct":70.6,"reset_ts":1000.0 + 5880.0},
            {"rotulo":"7d","pct":100.0,"reset_ts":900.0},
            {"rotulo":"30d","pct":4.7,"reset_ts":1000.0 + 24.0 * 86400.0}]}}});
        assert_eq!(
            line(&facts, Some(&config), Some(&quotas), 1000.0).as_deref(),
            Some("🤖 K3-256k (high) │ 💬 ctx 21k/262k │ ⚡5h:71% ↺1h38m │ 🗓30d:5% ↺24d"),
            "a janela de 7d já virou e sai da barra"
        );
        assert_eq!(line(&WireFacts::default(), Some(&config), Some(&quotas), 1000.0), None);
        let unknown = WireFacts { alias: Some("p/m".into()), ..WireFacts::default() };
        assert_eq!(line(&unknown, Some(&config), Some(&quotas), 1000.0).as_deref(), Some("🤖 m"));
        let expired = json!({"kimi:managed:kimi-code":{"cota":{"estado":"expirada","janelas":[{"rotulo":"5h","pct":90.0}]}}});
        let no_quota = WireFacts { tokens: None, ..facts };
        assert_eq!(line(&no_quota, Some(&config), Some(&expired), 1000.0).as_deref(), Some("🤖 K3-256k (high)"));
    }
}

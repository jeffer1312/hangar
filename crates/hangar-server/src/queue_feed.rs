// crates/hangar-server/src/queue_feed.rs
//! A fila durável ao vivo de um hub: lê a projeção `<conta>/.hangar-queue/<nome>.jsonl` e produz os
//! `message`/`queue_confirmed` das bolhas `queued-`/`local-`, como o `PromptQueue.follow` do Python.
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::Notify;

use crate::runtime::gateway::RuntimeRegistry;
use crate::side::{Binding, Hub, Out};
use crate::tail::Watchers;
use crate::transcript::history::{entry_event, from_current_session, is_local_output, transcript_start_ts};
use crate::transcript::{py, pyjson, Provider};

/// Nota local gravada até isto antes de o seguidor nascer ainda sai na primeira rodada
/// (`_FOLGA_SAIDA_LOCAL_S`); a mais velha já está no `/history`, no lugar do relógio dela.
pub(crate) const LOCAL_NOTE_GRACE_S: f64 = 30.0;

/// Desistência, entrega, confirmação e consumo: o que, mudando, faz a entrada sair de novo.
type Signature = (Option<bool>, Option<bool>, bool, Option<bool>);

/// Identidade do arquivo entre rodadas: a projeção é trocada inteira, então a marca muda a cada gravação.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Marker(u64, u64, u64, i128);

fn marker(path: &Path) -> io::Result<Option<Marker>> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Some(Marker(meta.dev(), meta.ino(), meta.size(), i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()))))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(Some(Marker(0, meta.creation_time(), meta.file_size(), i128::from(meta.last_write_time()))))
    }
}

/// `float(v or 0.0)` do Python. `Err`: valor que o Python recusaria (a linha é pulada).
pub(crate) fn py_float(v: Option<&Value>) -> Result<f64, ()> {
    if !py::truthy(v) {
        return Ok(0.0);
    }
    match v {
        Some(Value::Bool(_)) => Ok(1.0),
        Some(Value::Number(n)) => n.as_f64().ok_or(()),
        Some(Value::String(s)) => parse_py_float(s),
        _ => Err(()),
    }
}

/// `float(str)`: espaços nas pontas, `_` só entre dígitos, `nan`/`inf`/`infinity` com sinal.
// ponytail: dígito fora do ASCII (que o Python também aceita) é recusado; nenhum gravador o produz.
fn parse_py_float(s: &str) -> Result<f64, ()> {
    let s = py::strip(s);
    let bytes = s.as_bytes();
    let mut clean = String::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        if c == '_' {
            let between = i > 0 && bytes[i - 1].is_ascii_digit() && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
            if !between {
                return Err(());
            }
            continue;
        }
        clean.push(c);
    }
    clean.parse().map_err(|_| ())
}

/// Um evento da fila para o hub. `local_ts`: relógio da nota local (`local-`), que o retrato usa para
/// não entregar nota velha a quem abre o chat depois.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueueFrame {
    pub event: &'static str,
    pub data: String,
    /// Id da bolha (`queued-<id>`, `local-<id>`).
    pub id: String,
    pub local_ts: Option<f64>,
}

/// Relógio de parede em segundos, o `time.time()` do Python.
pub(crate) fn wall_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// Quadro do leitor do transcript que pede a confirmação da fila do Codex: `user_msg` desta geração,
/// ou atraso no canal (pode ter perdido um).
pub(crate) fn confirm_signal(out: &Result<Out, RecvError>, generation: u64) -> bool {
    match out {
        Ok(Out::Tail(g, frame)) => *g == generation && is_user_msg(frame),
        Err(RecvError::Lagged(_)) => true,
        _ => false,
    }
}

fn is_user_msg(frame: &[u8]) -> bool {
    // Barato antes do serde: o leitor manda todo quadro, inclusive resultado de ferramenta grande.
    if memchr::memmem::find(frame, b"\"user_msg\"").is_none() {
        return false;
    }
    let Ok(frame) = std::str::from_utf8(frame) else { return false };
    let data: String = frame.split("\r\n").filter_map(|line| line.strip_prefix("data: ")).collect::<Vec<_>>().join("\n");
    serde_json::from_str::<Value>(&data).is_ok_and(|v| v.get("kind").and_then(Value::as_str) == Some("user_msg"))
}

/// Batimento: relê mesmo sem aviso do observador da pasta (aviso perdido, observador que não armou).
const HEARTBEAT: Duration = Duration::from_secs(5);

/// Inscrição no observador da pasta, solta quando o produtor acaba (inclusive abortado).
struct Watch {
    watchers: Watchers,
    path: PathBuf,
    wake: Arc<Notify>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.watchers.unsubscribe(&self.path, &self.wake);
    }
}

/// O produtor da fila de uma geração do hub: corta pelo início da conversa ligada, publica cada rodada
/// pelo hub e acaba com a geração (troca de conversa, hub fechado). Codex sem terminal pede ao ator a
/// confirmação da fila ao nascer e a cada `user_msg` do transcript.
pub(crate) async fn run(hub: Weak<Hub>, generation: u64, binding: Binding, path: PathBuf, watchers: Watchers) {
    let Some(h) = hub.upgrade() else { return };
    let name = h.name.clone();
    let codex = (binding.provider == Provider::Codex && binding.headless).then(|| h.tx.subscribe());
    let runtime = h.runtime_registry();
    drop(h);
    let mut tail = codex;
    if tail.is_some() {
        confirm_codex(runtime.as_deref(), &name).await;
    }
    let mut follow = Some(QueueFollow::new(0.0, wall_now()));
    let watch = Watch { watchers, path, wake: Arc::new(Notify::new()) };
    let mut armed = false;
    loop {
        // Hub fechado ou geração trocada sem o aborto chegar: o produtor não sobra lendo a pasta.
        if hub.upgrade().and_then(|h| h.generation()) != Some(generation) {
            return;
        }
        if !armed {
            armed = watch.watchers.subscribe(&watch.path, &watch.wake);
        }
        let (mut current, file, jsonl) = (follow.take().expect("devolvido a cada rodada"), watch.path.clone(), binding.jsonl.clone());
        let polled = match tokio::task::spawn_blocking(move || {
            let unreadable = current.find_start(&jsonl);
            let polled = current.poll(&file);
            (current, unreadable, polled)
        })
        .await
        {
            Ok((current, unreadable, polled)) => {
                follow = Some(current);
                if let Some(e) = unreadable
                    && crate::warn_limit::allow(Some(&name), "queue_start_unreadable")
                {
                    tracing::warn!(session = %name, code = "queue_start_unreadable", kind = ?e.kind(), os_error = ?e.raw_os_error(),
                        "fila: início da conversa ilegível; segue sem corte");
                }
                polled
            }
            Err(e) => {
                tracing::error!(session = %name, panic = e.is_panic(), code = "queue_feed_crashed", "fila: leitura caiu; os aparelhos religam com um hub novo");
                if let Some(h) = hub.upgrade() {
                    h.close_generation(generation);
                }
                return;
            }
        };
        match polled {
            Ok(polled) => {
                if !polled.skipped.is_empty() && crate::warn_limit::allow(Some(&name), "queue_line_skipped") {
                    tracing::warn!(session = %name, code = "queue_line_skipped", lines = ?polled.skipped, "fila: linha ilegível pulada; o resto da fila segue");
                }
                let Some(h) = hub.upgrade() else { return };
                if let Some(present) = &polled.present {
                    h.retain_queue(generation, present);
                }
                for frame in &polled.frames {
                    if !h.publish_queue(generation, frame) {
                        return;
                    }
                }
            }
            Err(e) => {
                if crate::warn_limit::allow(Some(&name), "queue_read_failed") {
                    tracing::warn!(session = %name, code = "queue_read_failed", kind = ?e.kind(), os_error = ?e.raw_os_error(),
                        "fila: projeção ilegível; tenta de novo no batimento");
                }
            }
        }
        tokio::select! {
            _ = watch.wake.notified() => {}
            _ = tokio::time::sleep(HEARTBEAT) => {}
            closed = tail_confirm(&mut tail, generation) => {
                if closed { return; }
                confirm_codex(runtime.as_deref(), &name).await;
            }
        }
    }
}

/// Espera o próximo pedido de confirmação do leitor do transcript. `true`: o hub fechou o canal.
async fn tail_confirm(tail: &mut Option<broadcast::Receiver<Out>>, generation: u64) -> bool {
    let Some(rx) = tail else { return std::future::pending().await };
    loop {
        let out = rx.recv().await;
        if matches!(out, Err(RecvError::Closed) | Ok(Out::Close)) {
            return true;
        }
        if confirm_signal(&out, generation) {
            return false;
        }
    }
}

/// Teto da confirmação: o produtor espera por ela antes de reler a fila.
const CONFIRM_WAIT: Duration = Duration::from_secs(10);

/// A confirmação da fila pelo ator do Codex sem terminal desta sessão; sem ator aberto aqui, nada (o
/// Python também não confirmava).
async fn confirm_codex(runtime: Option<&RuntimeRegistry>, name: &str) {
    let Some(runtime) = runtime else { return };
    let Some((_, _, handle)) = runtime.codex_headless(name).await else { return };
    let code = match tokio::time::timeout(CONFIRM_WAIT, handle.confirm()).await {
        Ok(Ok(_)) => return,
        Ok(Err(e)) => e.code,
        Err(_) => "timeout".to_owned(),
    };
    if crate::warn_limit::allow(Some(name), "queue_confirm_failed") {
        tracing::warn!(session = %name, code = "queue_confirm_failed", error = %code, "fila: confirmação do Codex falhou; tenta no próximo envio");
    }
}

/// O que uma rodada produziu, mais as linhas puladas (número da linha, motivo) para o diário.
/// `present`: ids das bolhas no arquivo relido; `None` quando a marca não mudou e nada foi lido.
#[derive(Default, Debug)]
pub(crate) struct Polled {
    pub frames: Vec<QueueFrame>,
    pub skipped: Vec<(usize, &'static str)>,
    pub present: Option<HashSet<String>>,
}

/// Estado do seguidor de uma geração do hub. `min_ts`: início da conversa (0.0 = sem corte);
/// `start`: relógio de quando ele nasceu.
pub(crate) struct QueueFollow {
    min_ts: f64,
    start: f64,
    first: bool,
    seen: HashMap<String, Signature>,
    /// `None`: nunca leu.
    marker: Option<Option<Marker>>,
}

impl QueueFollow {
    pub(crate) fn new(min_ts: f64, start: f64) -> Self {
        QueueFollow { min_ts, start, first: true, seen: HashMap::new(), marker: None }
    }

    /// Sem corte, o início da conversa é procurado de novo a cada rodada, como no painel: o transcript que
    /// ainda não tinha linha passa a cortar a conversa anterior, e a fila é relida com o corte. Devolve o
    /// erro de abrir o transcript, menos o do que ainda não nasceu; pasta ausente (conta trocada) é erro.
    pub(crate) fn find_start(&mut self, jsonl: &Path) -> Option<io::Error> {
        if self.min_ts != 0.0 {
            return None;
        }
        if let Err(e) = std::fs::File::open(jsonl)
            && (e.kind() != io::ErrorKind::NotFound || !jsonl.parent().is_some_and(Path::is_dir))
        {
            return Some(e);
        }
        self.min_ts = transcript_start_ts(jsonl);
        if self.min_ts > 0.0 {
            self.marker = None;
        }
        None
    }

    /// Uma rodada: relê o arquivo se a marca dele mudou desde a última leitura. Erro de leitura não
    /// guarda a marca, e a próxima rodada tenta de novo.
    pub(crate) fn poll(&mut self, path: &Path) -> io::Result<Polled> {
        // A marca de antes da leitura: uma gravação no meio dela faz a próxima rodada reler.
        let marker = marker(path)?;
        if self.marker == Some(marker) {
            return Ok(Polled::default());
        }
        let raw = match marker {
            None => None,
            Some(_) => match crate::runtime::queue::read_replaced(path) {
                Ok(raw) => Some(raw),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            },
        };
        let polled = self.round(raw.as_deref().unwrap_or_default());
        self.marker = Some(marker);
        Ok(polled)
    }

    fn round(&mut self, raw: &[u8]) -> Polled {
        let mut polled = Polled { present: Some(HashSet::new()), ..Polled::default() };
        // Só `\n` separa linhas: U+2028, U+2029 e U+0085 crus são texto da entrada.
        for (index, line) in raw.split(|b| *b == b'\n').enumerate() {
            let text = String::from_utf8_lossy(line);
            let line = py::strip(&text);
            if line.is_empty() {
                continue;
            }
            // Linha que não é JSON o Python pula calado, e aqui também; JSON que derrubava o Python vai ao diário.
            let Some(value) = pyjson::loads_lossless(line) else { continue };
            match self.entry(value, polled.present.get_or_insert_default()) {
                Ok(Some(frame)) => polled.frames.push(frame),
                Ok(None) => {}
                Err(reason) => polled.skipped.push((index + 1, reason)),
            }
        }
        self.first = false;
        polled
    }

    /// Uma linha da fila: o evento a emitir, nada (repetida, cortada) ou o motivo de pular a linha. A
    /// linha lida entra em `present` pelo id da bolha.
    fn entry(&mut self, value: Value, present: &mut HashSet<String>) -> Result<Option<QueueFrame>, &'static str> {
        let Value::Object(entry) = value else { return Err("not_object") };
        let id = match entry.get("id") {
            Some(Value::Object(_) | Value::Array(_)) => return Err("id"),
            id => id.map_or_else(|| "None".to_owned(), py::py_str),
        };
        if id.is_empty() {
            return Ok(None);
        }
        if !matches!(entry.get("text"), None | Some(Value::Null | Value::String(_))) {
            return Err("text");
        }
        let local = is_local_output(&entry);
        // O relógio só é lido onde o Python o leria; o valor que ele recusaria pula a linha antes de ela contar.
        let cut_ts = if (self.first && local) || self.min_ts != 0.0 { Some(py_float(entry.get("ts")).map_err(|()| "ts")?) } else { None };
        let confirmed = py::truthy(entry.get("confirmed")) && !local;
        let mut event = entry_event(&entry);
        let cut = cut_ts.is_some_and(|ts| {
            (self.first && local && ts < self.start - LOCAL_NOTE_GRACE_S)
                || (self.min_ts != 0.0 && !from_current_session(&entry, self.min_ts, ts))
        });
        // Cortada não é bolha desta conversa: fora de `present`, sai do retrato se já tinha entrado.
        if !cut {
            present.insert(event.id.clone());
        }
        if confirmed {
            event.queued_confirmed = Some(true);
        }
        let signature = (event.desistiu, event.queued_delivered, confirmed, event.queued_consumed);
        if self.seen.get(&id) == Some(&signature) {
            return Ok(None);
        }
        self.seen.insert(id, signature);
        if cut {
            return Ok(None);
        }
        let data = serde_json::to_string(&event).map_err(|_| "json")?;
        let local_ts = local.then(|| cut_ts.unwrap_or_else(|| py_float(entry.get("ts")).unwrap_or(0.0)));
        Ok(Some(QueueFrame { event: if confirmed { "queue_confirmed" } else { "message" }, data, id: event.id, local_ts }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::Value;

    fn golden() -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/golden/queue_follow.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// O passo como o gerador o aplica: troca atômica do arquivo, apagar, ou gravar o de outra sessão.
    fn apply(step: &Value, path: &Path) {
        let data = match (step.get("write"), step.get("write_b64")) {
            (Some(w), _) => Some(w.as_str().unwrap().as_bytes().to_vec()),
            (_, Some(b)) => Some(base64::engine::general_purpose::STANDARD.decode(b.as_str().unwrap()).unwrap()),
            _ => None,
        };
        if let Some(data) = data {
            let tmp = path.with_extension("jsonl.tmp");
            std::fs::write(&tmp, data).unwrap();
            std::fs::rename(&tmp, path).unwrap();
        } else if step.get("delete").is_some() {
            let _ = std::fs::remove_file(path);
        } else if step.get("other").is_some() {
            std::fs::write(path.with_file_name("outra-sessao.jsonl"), "{\"id\": \"o1\", \"text\": \"oi\"}\n").unwrap();
        }
    }

    /// Início da conversa (o corte da fila): o do Python com `or 0.0`. Onde o Python cai (`[1]`) ou conta
    /// bool como 1 ms, a busca segue para a próxima linha.
    #[test]
    fn transcript_start_matches_the_python_golden() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/golden/queue_start_ts.json");
        let golden: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let fixed = HashMap::from([("linhas_nao_json", 1_790_000_005.0), ("bool_nao_conta", 1_790_000_006.0)]);
        for case in golden["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("conversa.jsonl");
            match (case.get("content_b64"), &case["content"]) {
                (Some(b), _) => std::fs::write(&file, base64::engine::general_purpose::STANDARD.decode(b.as_str().unwrap()).unwrap()).unwrap(),
                (_, Value::String(s)) => std::fs::write(&file, s).unwrap(),
                _ => {}
            }
            let want = fixed.get(name).copied().unwrap_or_else(|| case["result"].as_f64().unwrap_or(0.0));
            assert_eq!(crate::transcript::history::transcript_start_ts(&file), want, "{name}");
        }
    }

    /// Codex sem terminal: cada `user_msg` que o leitor do transcript desta geração manda pede a
    /// confirmação da fila; resposta, geração anterior ou outro evento não.
    #[test]
    fn codex_confirms_on_a_user_message_of_its_own_generation() {
        use crate::side::Out;
        use crate::tail::sse_frame;
        let user = sse_frame("message", r#"{"kind":"user_msg","id":"u1","text":"oi"}"#, Some("k:10"));
        let answer = sse_frame("message", r#"{"kind":"assistant_msg","id":"a1","text":"user_msg"}"#, Some("k:20"));
        assert!(confirm_signal(&Ok(Out::Tail(3, user.clone())), 3));
        assert!(!confirm_signal(&Ok(Out::Tail(2, user.clone())), 3));
        assert!(!confirm_signal(&Ok(Out::Tail(3, answer)), 3));
        assert!(!confirm_signal(&Ok(Out::Side(user)), 3));
        assert!(confirm_signal(&Err(tokio::sync::broadcast::error::RecvError::Lagged(4)), 3));
    }

    /// Transcript que ainda não nasceu é espera calada; pasta da conversa sumida (conta trocada) é erro
    /// para o diário; a primeira linha com relógio vira o corte.
    #[test]
    fn find_start_reports_an_unreadable_transcript_and_cuts_from_the_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut follow = QueueFollow::new(0.0, 0.0);
        assert!(follow.find_start(&dir.path().join("sumiu").join("a.jsonl")).is_some(), "pasta da conversa sumida passou calada");
        let jsonl = dir.path().join("a.jsonl");
        assert!(follow.find_start(&jsonl).is_none(), "transcript ainda por nascer virou erro");
        std::fs::write(&jsonl, "{\"timestamp\":\"2026-10-15T12:00:00Z\"}\n").unwrap();
        assert!(follow.find_start(&jsonl).is_none());
        assert_eq!(follow.min_ts, 1_792_065_600.0);
    }

    fn events(frames: &[QueueFrame]) -> Vec<(String, String)> {
        frames.iter().map(|f| (f.event.to_owned(), f.data.clone())).collect()
    }

    /// Cada cenário do Python, passo a passo: mesmos eventos e `data` byte a byte. Onde o Python cai com
    /// linha ruim, vale o `.esperado_rust` (a mesma situação sem a linha).
    #[test]
    fn matches_the_python_follow_golden() {
        let golden = golden();
        let scenarios = golden["scenarios"].as_array().unwrap();
        let by_name: HashMap<&str, &Value> = scenarios.iter().map(|s| (s["name"].as_str().unwrap(), s)).collect();
        let mut failures = Vec::new();
        for scenario in scenarios {
            let name = scenario["name"].as_str().unwrap();
            if name.ends_with(".esperado_rust") {
                continue;
            }
            let expected = by_name.get(format!("{name}.esperado_rust").as_str()).copied().unwrap_or(scenario);
            let (steps, wanted) = (scenario["steps"].as_array().unwrap(), expected["steps"].as_array().unwrap());
            assert_eq!(steps.len(), wanted.len(), "{name}: passos");
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("fila.jsonl");
            let mut follow = QueueFollow::new(scenario["min_ts"].as_f64().unwrap(), scenario["clock"].as_f64().unwrap());
            for (i, (step, want)) in steps.iter().zip(wanted).enumerate() {
                apply(step, &path);
                let got = events(&follow.poll(&path).unwrap().frames);
                let want: Vec<(String, String)> = want["events"].as_array().unwrap().iter()
                    .map(|e| (e["event"].as_str().unwrap().to_owned(), e["data"].as_str().unwrap().to_owned())).collect();
                if got != want {
                    failures.push(format!("{name} passo {i}:\n  rust:   {got:?}\n  python: {want:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "{} passos diferem:\n{}", failures.len(), failures.join("\n"));
    }
}

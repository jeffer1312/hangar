//! Prévia do bloco em voo de uma sessão Claude com terminal, dentro do `Monitor`: porte do
//! `PreviewBroker` (`preview.py`) e da supressão do que já está no transcript (`sse.py`). A
//! primeira fonte é o arquivo do hook MessageDisplay; sem ele, o quadro da rodada de estado e,
//! enquanto o spinner corre, capturas de `FAST` só para a prévia.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use std::sync::LazyLock;

use hangar_api::chat::{ChatEvent, ChatKind};
use hangar_api::preview::PreviewEvent;
use regex::Regex;
use serde_json::Value;
use unicode_width::UnicodeWidthChar;

use super::monitor::{Frame, Sources};
use crate::list::facts_files::{FileKey, RACY_WINDOW, file_key};
use crate::terminal_state::{self, right};

pub const FAST: Duration = Duration::from_millis(150);
pub const SUBDIR: &str = ".hangar-preview";
/// Arquivo mais velho que isto só vale com o marcador `working` recente (`_PREVIEW_MAX_AGE`).
pub const MAX_AGE: f64 = 600.0;
/// Trecho mais curto que isto casa por acidente com qualquer resposta gravada.
const COMMITTED_MIN: usize = 16;

/// `.hangar-preview/<stem>.json` legível numa pasta de config.
#[derive(Clone, Debug, PartialEq)]
pub struct HookFile { pub text: String, pub ts: Option<f64> }

pub fn parse_hook(raw: &[u8]) -> Option<HookFile> {
    let Ok(Value::Object(o)) = serde_json::from_slice::<Value>(raw) else { return None };
    let text = o.get("text")?.as_str()?.to_owned();
    // `isinstance(ts, (int, float))` do Python, que também aceita booleano.
    let ts = match o.get("ts") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::Bool(b)) => Some(f64::from(u8::from(*b))),
        _ => None,
    };
    Some(HookFile { text, ts })
}

/// `read_sidecar`: o texto em voo, `""` quando o agente disse que não há nenhum, `None` para cair
/// no pane. Texto parado há mais de `MAX_AGE` só vale com o marcador `working` também recente
/// (turno longo só de ferramentas); vazio nunca envelhece.
pub fn hook_text(files: &[HookFile], marker: Option<(&str, f64)>, now: f64) -> Option<String> {
    for f in files {
        if f.ts.is_some_and(|ts| now - ts > MAX_AGE) && !f.text.is_empty() {
            if marker.is_some_and(|(state, ts)| state == "working" && now - ts <= MAX_AGE) {
                return Some(f.text.clone());
            }
            continue;
        }
        return Some(f.text.clone());
    }
    None
}

/// Arquivos do hook relidos só quando a versão (mtime, tamanho) muda.
#[derive(Default)]
pub struct HookFiles {
    cache: HashMap<PathBuf, (FileKey, Option<HookFile>)>,
    #[cfg(test)]
    reads: usize,
}

impl HookFiles {
    /// `<pasta>/.hangar-preview/<stem>.json` de cada pasta de config, na ordem; E/S bloqueante.
    pub fn read(&mut self, dirs: &[PathBuf], stem: &str) -> Vec<HookFile> {
        let mut previous = std::mem::take(&mut self.cache);
        let mut out = Vec::new();
        for dir in dirs {
            let path = dir.join(SUBDIR).join(format!("{stem}.json"));
            // Ausente é o caso normal: sessão sem o hook.
            let meta = match std::fs::metadata(&path) {
                Ok(meta) => meta,
                Err(e) => {
                    read_failed(stem, &e);
                    continue;
                }
            };
            let Some(key) = file_key(&meta) else { continue };
            let settled = key.0.elapsed().is_ok_and(|age| age > RACY_WINDOW);
            let parsed = match previous.remove(&path) {
                Some((k, parsed)) if k == key && settled => parsed,
                _ => {
                    #[cfg(test)]
                    { self.reads += 1; }
                    match std::fs::read(&path) {
                        Ok(raw) => parse_hook(&raw),
                        // Erro de E/S não entra no cache: a versão igual não pode prendê-lo.
                        Err(e) => {
                            read_failed(stem, &e);
                            continue;
                        }
                    }
                }
            };
            out.extend(parsed.clone());
            self.cache.insert(path, (key, parsed));
        }
        out
    }
}

/// `crop_cells` de cada linha: até `columns` células, sem o espaço que sobra no fim.
// ponytail: largura do `unicode-width`, igual ao `east_asian_width` do Python nos caracteres de
// terminal; diverge só em casos raros (jamo medial do hangul), onde o corte erra uma célula.
pub fn crop(pane: &str, columns: u32) -> String {
    let columns = columns as usize;
    let mut out = String::with_capacity(pane.len());
    for (i, line) in pane.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut used = 0;
        let cut = line.char_indices().find(|(_, c)| {
            used += c.width().unwrap_or(1);
            used > columns
        });
        out.push_str(match cut {
            Some((at, _)) => right(&line[..at]),
            None => line,
        });
    }
    out
}

/// Prévia do quadro e se o spinner corre. Com painel ancorado ou faixa dos mods à vista, o corte
/// vem antes da análise; sem eles, vale a análise que o pool já fez do quadro.
pub fn from_pane(frame: &Frame, columns: Option<u32>, anchor: Option<&str>) -> (String, bool) {
    match (columns, anchor) {
        (None, None) => (frame.analysis.preview.clone(), frame.analysis.spinner.is_some()),
        (Some(c), _) => terminal_state::pane_preview(&crop(&frame.text, c), anchor),
        (None, _) => terminal_state::pane_preview(&frame.text, anchor),
    }
}

static LIST_MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*[-•◦▪]\s+").unwrap());
static MARKDOWN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[`*_~#>]").unwrap());
static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// `_norm`: o pane já pintado (sem markdown, quebrado pela largura) contra o markdown cru do
/// transcript. Marcador de lista só no começo da linha, para hífen de prosa contar.
pub fn norm(s: &str) -> String {
    let s = LIST_MARK.replace_all(s, "");
    let s = MARKDOWN.replace_all(&s, "");
    SPACE.replace_all(&s, " ").trim().to_owned()
}

/// `preview_is_committed`: a prévia já é o bloco gravado, ou o gravado com chrome grudado no fim.
pub fn is_committed(preview: &str, committed: &str) -> bool {
    let n = norm(preview);
    n.chars().count() >= COMMITTED_MIN && !committed.is_empty() && (committed.contains(&n) || n.starts_with(committed))
}

fn read_failed(stem: &str, e: &std::io::Error) {
    if e.kind() != std::io::ErrorKind::NotFound && crate::warn_limit::allow(Some(stem), "preview_hook_read") {
        tracing::warn!(stem, code = "preview_hook_read", kind = ?e.kind(), "prévia: arquivo do hook ilegível");
    }
}

/// Gancho do leitor do transcript: a resposta que um quadro `message` acabou de gravar.
pub fn committed_from_frame(frame: &[u8]) -> Option<String> {
    // Barato antes do serde: o leitor manda todo quadro, inclusive resultado de ferramenta grande.
    if memchr::memmem::find(frame, b"\"assistant_msg\"").is_none() {
        return None;
    }
    let frame = std::str::from_utf8(frame).ok()?;
    let mut event = None;
    let mut data = Vec::new();
    for line in frame.split("\r\n") {
        if let Some(e) = line.strip_prefix("event: ") {
            event = Some(e);
        } else if let Some(d) = line.strip_prefix("data: ") {
            data.push(d);
        }
    }
    if event != Some("message") {
        return None;
    }
    let ev: ChatEvent = match serde_json::from_str(&data.join("\n")) {
        Ok(ev) => ev,
        Err(e) => {
            // Sem isto a supressão pararia calada e a resposta gravada voltaria como prévia.
            if crate::warn_limit::allow(None, "preview_committed_parse") {
                tracing::warn!(code = "preview_committed_parse", category = ?e.classify(), "prévia: quadro do transcript ilegível");
            }
            return None;
        }
    };
    committed_from_event(ev)
}

/// A resposta que o evento grava, normalizada como a prévia compara.
pub fn committed_from_event(ev: ChatEvent) -> Option<String> {
    (ev.kind == ChatKind::AssistantMsg).then_some(ev.text).flatten().filter(|t| !t.is_empty()).map(|t| norm(&t))
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Text { text: String, md: bool, full: bool }

/// Prévia de uma sessão entre rodadas.
#[derive(Default)]
pub struct Slot {
    /// Último lido da fonte e último publicado (vazio quando já estava no transcript).
    raw: Text,
    sent: Text,
    committed: Option<Arc<str>>,
    /// Spinner correndo no pane, ou texto no arquivo do hook: toques de `FAST` até a rodada seguinte.
    pub fast: bool,
    columns: Option<u32>,
    anchor: Option<String>,
    marker: Option<(String, f64)>,
    /// Último estado publicado: texto em voo com a sessão parada é a bolha fantasma, e vai ao log uma vez.
    pub state: Option<String>,
    ghost_logged: bool,
}

impl Slot {
    /// Largura, âncora e marcador da última rodada de estado, que os toques rápidos reusam.
    pub fn set_view(&mut self, columns: Option<u32>, anchor: Option<String>, marker: Option<(String, f64)>) {
        (self.columns, self.anchor, self.marker) = (columns, anchor, marker);
    }

    /// Leitura nova da fonte e o gravado de agora; devolve o que publicar. Como no `sse.py`, o
    /// gravado novo só apaga a prévia já publicada, e a fonte só é reconferida quando muda: um
    /// bloco gravado depois não traz de volta o texto que continua na tela.
    fn offer(&mut self, text: String, md: bool, full: bool, committed: Option<Arc<str>>) -> Option<Text> {
        let mut out = None;
        if committed != self.committed {
            self.committed = committed;
            if is_committed(&self.sent.text, self.committed.as_deref().unwrap_or("")) {
                self.sent = Text::default();
                out = Some(self.sent.clone());
            }
        }
        let raw = Text { text, md, full };
        if raw != self.raw {
            let gone = is_committed(&raw.text, self.committed.as_deref().unwrap_or(""));
            let candidate = Text { text: if gone { String::new() } else { raw.text.clone() }, ..raw.clone() };
            self.raw = raw;
            if candidate != self.sent {
                self.sent = candidate;
                out = Some(self.sent.clone());
            }
        }
        out
    }
}

/// O hub avisou que uma resposta entrou no transcript: a prévia que a repete sai já, sem esperar o
/// toque seguinte nem reler a fonte. `false`: ninguém mais ouve.
pub async fn committed_changed<S: Sources>(src: &S, slot: &mut Slot, epoch: u64) -> bool {
    let committed = src.committed();
    if src.epoch() != epoch || committed == slot.committed {
        return true;
    }
    slot.committed = committed;
    if slot.sent.text.is_empty() || !is_committed(&slot.sent.text, slot.committed.as_deref().unwrap_or("")) {
        return true;
    }
    slot.sent = Text { md: slot.sent.md, full: slot.sent.full, text: String::new() };
    src.publish_preview(PreviewEvent { session: src.name().to_owned(), text: String::new(), md: slot.sent.md, full: slot.sent.full, vivo: false }).await
}

/// Uma leitura da prévia. `frame`: o quadro da rodada de estado; `None` no toque rápido, que
/// captura só para a prévia quando o arquivo do hook não decide. `false`: ninguém mais ouve.
pub async fn tick<S: Sources>(src: &S, slot: &mut Slot, frame: Option<&Frame>, epoch: u64) -> bool {
    if src.epoch() != epoch {
        return true;
    }
    let files = match src.sid() {
        Some(sid) => src.preview_files(&sid).await,
        None => Vec::new(),
    };
    let marker = slot.marker.as_ref().map(|(s, t)| (s.as_str(), *t));
    let (text, md, full) = match hook_text(&files, marker, src.wall()) {
        Some(text) => {
            slot.fast = !text.is_empty();
            (text, true, true)
        }
        None => {
            let captured;
            let frame = match frame {
                Some(f) => f,
                None => match src.preview_capture().await {
                    Some(Ok(f)) => { captured = f; &captured }
                    // Falha ou fonte sem captura rápida: fica o texto que tinha, até a próxima rodada.
                    Some(Err(e)) => {
                        if crate::warn_limit::allow(Some(src.name()), "preview_capture_failed") {
                            tracing::warn!(session = src.name(), code = "preview_capture_failed", detail = e.code.as_str(), "prévia: captura rápida falhou");
                        }
                        slot.fast = false;
                        return true;
                    }
                    None => { slot.fast = false; return true; }
                },
            };
            let (text, working) = from_pane(frame, slot.columns, slot.anchor.as_deref());
            slot.fast = working;
            (text, false, false)
        }
    };
    // `/clear` no meio da leitura: o texto é da conversa apagada.
    if src.epoch() != epoch {
        return true;
    }
    // Uma linha por resposta: a próxima volta a `working` rearma o registro.
    if slot.state.as_deref() == Some("working") { slot.ghost_logged = false; }
    match slot.offer(text, md, full, src.committed()) {
        Some(t) if !t.text.is_empty() && slot.state.as_deref().is_some_and(|s| s != "working") && !std::mem::replace(&mut slot.ghost_logged, true) => {
            tracing::info!(session = src.name(), state = slot.state.as_deref().unwrap_or(""), md = t.md, full = t.full,
                chars = t.text.chars().count(), "prévia com texto em sessão parada");
            src.publish_preview(PreviewEvent { session: src.name().to_owned(), text: t.text, md: t.md, full: t.full, vivo: false }).await
        }
        Some(t) => src.publish_preview(PreviewEvent { session: src.name().to_owned(), text: t.text, md: t.md, full: t.full, vivo: false }).await,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use crate::state::facts::Dead;
    use crate::state::monitor::{CaptureFailed, FileFacts, Monitor, RoundFacts, POLL};
    use crate::terminal_state::analyze;
    use hangar_api::state::StateEvent;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::sync::Notify;

    fn golden() -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/golden/preview.json");
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    fn frame(text: &str) -> Frame { Frame { text: text.into(), analysis: analyze(text) } }

    const RULE: &str = "────────────────────────────────────────────────────────────";

    fn pane(body: &str, spinner: bool) -> String {
        let spin = if spinner { "✻ Thinking…\n" } else { "" };
        format!("❯ pergunta\n● {body}\n{spin}{RULE}\n❯ \n{RULE}")
    }

    /// Fonte de mentira: o pane e o arquivo do hook valem até o teste trocar.
    struct Fake {
        pane: Mutex<String>,
        hook: Mutex<Vec<HookFile>>,
        committed: Mutex<Option<Arc<str>>>,
        captures: AtomicU32,
        fast_captures: AtomicU32,
        hook_reads: AtomicU32,
        previews: Mutex<Vec<PreviewEvent>>,
        wake: Arc<Notify>,
        hub: Arc<Notify>,
    }

    impl Fake {
        fn new(pane: String) -> Arc<Self> {
            Arc::new(Self { pane: Mutex::new(pane), hook: Mutex::default(), committed: Mutex::default(),
                captures: AtomicU32::new(0), fast_captures: AtomicU32::new(0), hook_reads: AtomicU32::new(0),
                previews: Mutex::default(), wake: Arc::default(), hub: Arc::default() })
        }
        fn texts(&self) -> Vec<(String, bool, bool)> {
            self.previews.lock().unwrap().iter().map(|p| (p.text.clone(), p.md, p.full)).collect()
        }
    }

    impl Sources for Arc<Fake> {
        fn name(&self) -> &str { "s" }
        fn sid(&self) -> Option<String> { Some("sid".into()) }
        fn epoch(&self) -> u64 { 0 }
        fn wake(&self) -> Arc<Notify> { self.wake.clone() }
        async fn facts(&self) -> RoundFacts { RoundFacts::default() }
        async fn capture(&self) -> Result<Frame, CaptureFailed> {
            self.captures.fetch_add(1, Ordering::SeqCst);
            Ok(frame(&self.pane.lock().unwrap()))
        }
        async fn preview_capture(&self) -> Option<Result<Frame, CaptureFailed>> {
            self.fast_captures.fetch_add(1, Ordering::SeqCst);
            Some(Ok(frame(&self.pane.lock().unwrap())))
        }
        async fn has_session(&self) -> Option<bool> { Some(true) }
        async fn dead(&self) -> Result<Dead, String> { Ok(Dead::Ok) }
        async fn observe_permission(&self, _: &str, mode: &str) -> Result<(String, String), String> { Ok((mode.into(), "manual".into())) }
        async fn files(&self, _: Option<&str>) -> FileFacts { FileFacts::default() }
        async fn publish(&self, _: StateEvent) -> bool { true }
        async fn preview_files(&self, _: &str) -> Vec<HookFile> {
            self.hook_reads.fetch_add(1, Ordering::SeqCst);
            self.hook.lock().unwrap().clone()
        }
        fn committed(&self) -> Option<Arc<str>> { self.committed.lock().unwrap().clone() }
        async fn publish_preview(&self, event: PreviewEvent) -> bool {
            self.previews.lock().unwrap().push(event);
            true
        }
        fn wall(&self) -> f64 { 1_000_000.0 }
        fn hub_wake(&self) -> Arc<Notify> { self.hub.clone() }
        async fn emit(&self, _: &'static str, _: Value) -> bool { true }
        fn runtime_wake(&self) -> Arc<Notify> { Arc::default() }
        fn runtime_problem(&self) -> Option<(String, String)> { None }
        async fn ask_payload(&self) -> Result<Option<hangar_api::ask::AskQuestion>, String> { Ok(None) }
        fn deliverable(&self) {}
        fn trust_dialog(&self) {}
    }

    #[tokio::test(start_paused = true)]
    async fn commit_clears_without_waiting_the_tick() {
        // Parado: o próximo toque seria só daqui a 0,75 s. O hub avisa e a prévia sai na hora.
        let voo = "Resposta inteira que ficou na tela";
        let fake = Fake::new(pane(voo, false));
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fake.texts().last().map(|t| t.0.clone()).as_deref(), Some(voo));
        let captures = fake.captures.load(Ordering::SeqCst);
        *fake.committed.lock().unwrap() = Some(norm(voo).into());
        fake.hub.notify_one();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fake.texts().last().map(|t| t.0.clone()).as_deref(), Some(""));
        assert_eq!(fake.captures.load(Ordering::SeqCst), captures, "sem captura nem rodada a mais");
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn hook_file_first_then_pane() {
        let fake = Fake::new(pane("texto do pane em voo", false));
        *fake.hook.lock().unwrap() = vec![HookFile { text: "do hook".into(), ts: Some(1_000_000.0) }];
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Arquivo vazio é resposta ("nada em voo"), não ausência: o pane não entra.
        *fake.hook.lock().unwrap() = vec![HookFile { text: String::new(), ts: Some(1.0) }];
        tokio::time::sleep(POLL).await;
        fake.hook.lock().unwrap().clear();
        tokio::time::sleep(POLL).await;
        assert_eq!(fake.texts(), [("do hook".into(), true, true), (String::new(), true, true),
            ("texto do pane em voo".into(), false, false)]);
        assert!(fake.previews.lock().unwrap().iter().all(|p| p.session == "s" && !p.vivo));
        task.abort();
    }

    #[test]
    fn old_file_valid_while_marker_working() {
        for row in golden()["sidecars"].as_array().unwrap() {
            let files: Vec<_> = row["files"].as_array().unwrap().iter()
                .filter_map(|f| f.as_str().and_then(|raw| parse_hook(raw.as_bytes()))).collect();
            let marker = row["marker"].as_array().map(|m| (m[0].as_str().unwrap(), m[1].as_f64().unwrap()));
            let got = hook_text(&files, marker, row["now"].as_f64().unwrap());
            assert_eq!(got.as_deref(), row["expected"].as_str(), "{}", row["name"]);
        }
    }

    #[test]
    fn hook_files_reread_only_on_new_version() {
        let dir = std::env::temp_dir().join(format!("hangar-preview-{}", std::process::id()));
        let file = dir.join(SUBDIR).join("sid.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"text": "um", "ts": 1}"#).unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(60);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
        let mut files = HookFiles::default();
        let dirs = [dir.clone(), dir.join("sem-arquivo")];
        assert_eq!(files.read(&dirs, "sid"), [HookFile { text: "um".into(), ts: Some(1.0) }]);
        assert_eq!(files.reads, 1);
        assert_eq!(files.read(&dirs, "sid").len(), 1);
        assert_eq!(files.reads, 1, "versão igual e velha: sem reler");
        std::fs::write(&file, r#"{"text": "dois", "ts": 2}"#).unwrap();
        assert_eq!(files.read(&dirs, "sid")[0].text, "dois");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn crop_before_analysis() {
        for row in golden()["panes"].as_array().unwrap() {
            let f = frame(row["pane"].as_str().unwrap());
            let columns = row["columns"].as_u64().map(|c| c as u32);
            let (text, _) = from_pane(&f, columns, row["anchor"].as_str());
            assert_eq!(text, row["expected"].as_str().unwrap(), "{}", row["name"]);
        }
    }

    #[test]
    fn suppressed_when_in_transcript() {
        for row in golden()["committed"].as_array().unwrap() {
            let committed = norm(row["committed"].as_str().unwrap());
            assert_eq!(committed, row["norm"].as_str().unwrap(), "{}", row["name"]);
            assert_eq!(is_committed(row["preview"].as_str().unwrap(), &committed), row["expected"].as_bool().unwrap(), "{}", row["name"]);
        }
        let mut slot = Slot::default();
        let gravado: Arc<str> = norm("Texto já gravado aqui completo").into();
        assert_eq!(slot.offer("Texto já gravado aqui completo".into(), false, false, Some(gravado)), None,
            "já no transcript: sai vazio, igual ao que estava");
        let frame = tail_frame("assistant_msg", "**Pronto**, terminei.");
        assert_eq!(committed_from_frame(&frame).as_deref(), Some("Pronto, terminei."));
        assert_eq!(committed_from_frame(&tail_frame("user_msg", "oi")), None);
    }

    fn tail_frame(kind: &str, text: &str) -> bytes::Bytes {
        let data = serde_json::json!({"kind": kind, "id": "x", "text": text}).to_string();
        crate::tail::sse_frame("message", &data, Some("k:1"))
    }

    #[test]
    fn cleared_on_commit() {
        let mut slot = Slot::default();
        let voo = "Resposta ainda em voo no pane";
        let sent = slot.offer(voo.into(), false, false, None).unwrap();
        assert_eq!(sent.text, voo);
        let gravado: Arc<str> = norm(voo).into();
        assert_eq!(slot.offer(voo.into(), false, false, Some(gravado)).unwrap().text, "");
        // Outro bloco gravado com o mesmo texto ainda na tela: nada volta como bolha fantasma.
        let outro: Arc<str> = norm("Outra resposta que nada tem a ver").into();
        assert_eq!(slot.offer(voo.into(), false, false, Some(outro.clone())), None);
        assert_eq!(slot.offer("Bloco novo que começou agora".into(), false, false, Some(outro)).unwrap().text,
            "Bloco novo que começou agora");
    }

    #[tokio::test(start_paused = true)]
    async fn fast_captures_only_for_pane_preview() {
        // Pane trabalhando sem arquivo: capturas de 0,15 s entre as rodadas, só para a prévia.
        let fake = Fake::new(pane("em voo", true));
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 2 + Duration::from_millis(10)).await;
        assert_eq!(fake.captures.load(Ordering::SeqCst), 3, "rodadas de estado contam só as de 0,75 s");
        assert_eq!(fake.fast_captures.load(Ordering::SeqCst), 8, "quatro toques por intervalo");
        task.abort();

        // Arquivo do hook com texto: toques rápidos só conferem o arquivo, sem captura.
        let fake = Fake::new(pane("em voo", true));
        *fake.hook.lock().unwrap() = vec![HookFile { text: "do hook".into(), ts: Some(1_000_000.0) }];
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 2 + Duration::from_millis(10)).await;
        assert_eq!(fake.fast_captures.load(Ordering::SeqCst), 0);
        assert_eq!(fake.hook_reads.load(Ordering::SeqCst), 11, "uma por rodada e uma por toque");
        task.abort();

        // Parado e sem arquivo: só as rodadas de estado.
        let fake = Fake::new(pane("resposta pronta", false));
        let task = tokio::spawn(Monitor::new(fake.clone()).run());
        tokio::time::sleep(POLL * 2 + Duration::from_millis(10)).await;
        assert_eq!(fake.fast_captures.load(Ordering::SeqCst), 0);
        assert_eq!(fake.captures.load(Ordering::SeqCst), 3);
        task.abort();
    }
}

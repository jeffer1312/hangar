//! Ditado em pedaços durante a fala: a regra de corte de `packages/core/src/dictationPieces.ts`, provada pelos mesmos
//! casos (`dictationPieces.fixtures.json`), porque o nativo não importa o core.
use std::{collections::VecDeque, sync::Mutex, time::{Duration, Instant}};
use serde_json::{Value, json};
use tokio::{runtime::Handle, task::JoinHandle};
use crate::api::{Api, Failure};
use super::dictation::{Vad, wav};

pub(super) const FRAME_MS: u64 = 30;
pub(super) const MIN_PIECE_MS: u64 = 8000;
pub(super) const PAUSE_MS: u64 = 400;
pub(super) const MAX_PIECE_MS: u64 = 20000;
pub(super) const FORCED_WINDOW_MS: u64 = 3000;
pub(super) const TAIL_MERGE_MS: u64 = 4000;
pub(super) const MIN_VOICE_MS: u64 = 500;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Piece { pub start_ms: u64, pub end_ms: u64, pub voiced_ms: u64, pub skip: bool }

/// `Whole` = nenhum corte: o caminho de sempre, arquivo inteiro. `Merge` substitui o último pedaço já cortado.
#[derive(Debug, PartialEq)]
pub(super) enum Tail { Whole(Piece), Send(Piece), Merge(Piece) }

struct Frame { ts: u64, level: f32, dur: u64, voiced: bool }

pub(super) struct Cutter {
    vad: Vad,
    /// O detector conta tempo em `Instant`: o quadro vira `origin + ts_ms`.
    origin: Instant,
    last_ts: u64,
    piece_start: u64,
    /// Quadros do pedaço em curso (no máximo ~20 s).
    frames: VecDeque<Frame>,
    /// Fim do último quadro de voz antes do silêncio em curso; `None` se o último quadro foi voz.
    pause_start: Option<u64>,
    last_piece: Option<Piece>,
}

impl Cutter {
    pub(super) fn new() -> Self {
        Self { vad: Vad::default(), origin: Instant::now(), last_ts: 0, piece_start: 0, frames: VecDeque::new(), pause_start: None, last_piece: None }
    }

    fn cut_at(&mut self, at: u64) -> Piece {
        let mut voiced_ms = 0;
        while self.frames.front().is_some_and(|f| f.ts <= at) {
            let frame = self.frames.pop_front().unwrap();
            if frame.voiced { voiced_ms += frame.dur; }
        }
        let piece = Piece { start_ms: self.piece_start, end_ms: at, voiced_ms, skip: voiced_ms < MIN_VOICE_MS };
        self.piece_start = at;
        self.last_piece = Some(piece);
        piece
    }

    /// `ts_ms` = fim do quadro desde o início da gravação; `level` = RMS 0..1. Devolve o pedaço quando corta.
    pub(super) fn feed(&mut self, ts_ms: u64, level: f32) -> Option<Piece> {
        let ts = ts_ms;
        let dur = ts.saturating_sub(self.last_ts);
        self.vad.step(level, self.origin + Duration::from_millis(ts));
        // O piso vale para o quadro, não para o pico: em quem fala baixo o pico mal passa dele.
        let voiced = level > 0.01 && self.vad.quiet_since.is_none();
        self.frames.push_back(Frame { ts, level, dur, voiced });
        if voiced { self.pause_start = None; } else if self.pause_start.is_none() { self.pause_start = Some(self.last_ts); }
        self.last_ts = ts;
        if let Some(pause) = self.pause_start {
            // Pausa que atravessa um corte conta a partir do corte: ela pertence ao pedaço novo.
            let start = pause.max(self.piece_start);
            let middle = start + (ts - start).div_ceil(2);
            if ts - start >= PAUSE_MS && middle - self.piece_start >= MIN_PIECE_MS { return Some(self.cut_at(middle)); }
        }
        if ts - self.piece_start >= MAX_PIECE_MS {
            // ponytail: varredura linear da janela (~100 quadros); empate fica com o mais antigo.
            let low = self.frames.iter().filter(|f| f.ts + FORCED_WINDOW_MS >= ts)
                .fold(None::<&Frame>, |low, f| if low.is_none_or(|low| f.level < low.level) { Some(f) } else { low })?.ts;
            return Some(self.cut_at(low));
        }
        None
    }

    pub(super) fn stop(&mut self) -> Tail {
        let last = self.last_piece;
        let rest = self.cut_at(self.last_ts);
        let Some(last) = last else { return Tail::Whole(rest) };
        if rest.end_ms - rest.start_ms >= TAIL_MERGE_MS { return Tail::Send(rest); }
        let voiced_ms = last.voiced_ms + rest.voiced_ms;
        Tail::Merge(Piece { start_ms: last.start_ms, end_ms: rest.end_ms, voiced_ms, skip: voiced_ms < MIN_VOICE_MS })
    }
}

/// Cru dos pedaços na ordem. Sem organização a emenda sairia sem pontuação ("ela trava Não sei"): frase sem pontuação
/// no fim seguida de maiúscula ganha um ponto.
pub(super) fn join<'a>(texts: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for text in texts.into_iter().map(str::trim).filter(|t| !t.is_empty()) {
        if !out.is_empty() {
            let open = !out.ends_with(['.', '!', '?', '…', ':', ';', ',']);
            if open && text.chars().next().is_some_and(char::is_uppercase) { out.push('.'); }
            out.push(' ');
        }
        out.push_str(text);
    }
    out
}

/// Pedaço em voo. Soltar aborta: cancelar, nova gravação e a volta ao arquivo inteiro não deixam pedido sobrando.
pub(super) struct Job<T>(JoinHandle<T>);
impl<T> Drop for Job<T> { fn drop(&mut self) { self.0.abort(); } }
impl<T> Job<T> {
    pub(super) fn spawn(runtime: &Handle, task: impl Future<Output = T> + Send + 'static) -> Self where T: Send + 'static { Self(runtime.spawn(task)) }
    pub(super) async fn join(mut self) -> Option<T> { (&mut self.0).await.ok() }
}

pub(super) type PieceJob = Job<Result<Value, Failure>>;

/// Pedaços da gravação em curso, na ordem; `None` = descartado por pouca voz.
pub(super) struct Pieces { api: Api, cutter: Cutter, jobs: Vec<Option<PieceJob>> }

/// 16 kHz mono PCM16: 32 bytes por ms.
const BYTES_PER_MS: u64 = 32;

impl Pieces {
    pub(super) fn new(api: Api) -> Self { Self { api, cutter: Cutter::new(), jobs: Vec::new() } }

    /// `sampled` = bytes de PCM já medidos pelo nível `level`.
    pub(super) fn feed(&mut self, sampled: usize, level: f32, pcm: &Mutex<Vec<u8>>, runtime: &Handle) {
        let ts = sampled as u64 / BYTES_PER_MS;
        // Passo sem áudio novo repete o nível anterior: não é quadro.
        if ts <= self.cutter.last_ts { return; }
        if let Some(piece) = self.cutter.feed(ts, level) { self.send(piece, &pcm.lock().unwrap(), runtime); }
    }

    fn send(&mut self, piece: Piece, pcm: &[u8], runtime: &Handle) {
        let len = (pcm.len() & !1) as u64;
        let at = |ms: u64| ms.saturating_mul(BYTES_PER_MS).min(len) as usize;
        let (api, bytes) = (self.api.clone(), wav(&pcm[at(piece.start_ms)..at(piece.end_ms)]));
        self.jobs.push((!piece.skip).then(|| Job::spawn(runtime, async move { api.transcribe_piece(bytes).await })));
    }

    /// Fim da gravação: manda o resto, ou o último pedaço junto com ele. `None` = nenhum corte: o caminho de sempre.
    pub(super) fn finish(mut self, pcm: &[u8], runtime: &Handle) -> Option<Vec<Option<PieceJob>>> {
        let piece = match self.cutter.stop() {
            Tail::Whole(_) => return None,
            Tail::Send(piece) => piece,
            Tail::Merge(piece) => { self.jobs.pop(); piece }
        };
        // O resto vai até o fim do áudio: o que chegou depois do último passo medido também é dele.
        self.send(Piece { end_ms: u64::MAX, ..piece }, pcm, runtime);
        Some(self.jobs)
    }
}

/// Cru juntado, provedor do último pedaço e avisos da transcrição (troca de provedor).
#[derive(Debug, PartialEq)]
pub(super) struct Transcribed { pub raw: String, pub provider: Option<String>, pub warning: Option<String> }

/// Espera os pedaços na ordem. Um que falhou devolve a falha e solta (aborta) os outros.
pub(super) async fn transcribed(jobs: Vec<Option<PieceJob>>) -> Result<Transcribed, Failure> {
    let (mut texts, mut provider, mut warnings) = (Vec::new(), None, Vec::<String>::new());
    for job in jobs.into_iter().flatten() {
        let value = job.join().await.unwrap_or_else(|| Err(Failure::local("invalid_response")))?;
        texts.push(value["text"].as_str().unwrap_or("").to_owned());
        if let Some(name) = value["provider"].as_str() { provider = Some(name.to_owned()); }
        if let Some(warning) = value["aviso"].as_str().filter(|w| !w.is_empty() && !warnings.iter().any(|seen| seen == w)) {
            warnings.push(warning.to_owned());
        }
    }
    Ok(Transcribed { raw: join(texts.iter().map(String::as_str)), provider, warning: (!warnings.is_empty()).then(|| warnings.join(" · ")) })
}

/// A resposta no formato do `/transcribe` de hoje (`with_provider` do servidor): a organização com o provedor e o aviso
/// da transcrição antes do dela.
pub(super) fn with_transcription(mut value: Value, transcribed: &Transcribed) -> Value {
    let cleaning = value["aviso"].as_str().filter(|a| !a.is_empty()).map(str::to_owned);
    if let Some(provider) = &transcribed.provider { value["provider"] = json!(provider); }
    if let Some(warning) = &transcribed.warning { value["aviso_transcricao"] = json!(warning); }
    let warnings: Vec<&str> = transcribed.warning.as_deref().into_iter().chain(cleaning.as_deref()).collect();
    if !warnings.is_empty() { value["aviso"] = json!(warnings.join(" · ")); }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixtures() -> Value { serde_json::from_str(include_str!("../../../packages/core/src/dictationPieces.fixtures.json")).unwrap() }

    fn check(name: &str, got: &Piece, want: &Value) {
        assert_eq!((got.start_ms, got.end_ms, got.skip), (want["startMs"].as_u64().unwrap(), want["endMs"].as_u64().unwrap(),
            want["skip"].as_bool().unwrap()), "{name}");
        let (low, high) = (want["voicedMs"][0].as_u64().unwrap(), want["voicedMs"][1].as_u64().unwrap());
        assert!((low..=high).contains(&got.voiced_ms), "{name}: voiced {} fora de {low}..={high}", got.voiced_ms);
    }

    #[test]
    fn dictation_pieces_constants_are_the_shared_ones() {
        let c = &fixtures()["constants"];
        assert_eq!([FRAME_MS, MIN_PIECE_MS, PAUSE_MS, MAX_PIECE_MS, FORCED_WINDOW_MS, TAIL_MERGE_MS, MIN_VOICE_MS],
            ["frameMs", "minPieceMs", "pauseMs", "maxPieceMs", "forcedWindowMs", "tailMergeMs", "minVoiceMs"].map(|k| c[k].as_u64().unwrap()));
    }

    /// Quadros `(ts_ms, nível)` de um caso, pela regra descrita no `about` das fixtures.
    fn frames(case: &Value) -> Vec<(u64, f32)> {
        let frame = case["frameMs"].as_u64().unwrap();
        let (mut out, mut start, mut ts) = (Vec::new(), 0, frame);
        for segment in case["segments"].as_array().unwrap() {
            let end = start + segment["ms"].as_u64().unwrap();
            let levels = segment["levels"].as_array().unwrap();
            let mut index = 0;
            while ts <= end {
                out.push((ts, levels[index % levels.len()].as_f64().unwrap() as f32));
                index += 1;
                ts += frame;
            }
            start = end;
        }
        out
    }

    fn case(prefix: &str) -> Value {
        fixtures()["cases"].as_array().unwrap().iter().find(|c| c["name"].as_str().unwrap().starts_with(prefix)).unwrap().clone()
    }

    #[test]
    fn dictation_pieces_cut_every_shared_case_like_the_core() {
        for case in fixtures()["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let mut cutter = Cutter::new();
            let pieces: Vec<Piece> = frames(case).into_iter().filter_map(|(ts, level)| cutter.feed(ts, level)).collect();
            let want = case["pieces"].as_array().unwrap();
            assert_eq!(pieces.len(), want.len(), "{name}");
            for (got, want) in pieces.iter().zip(want) { check(name, got, want); }
            let (tail, piece) = match cutter.stop() { Tail::Whole(p) => ("whole", p), Tail::Send(p) => ("send", p), Tail::Merge(p) => ("merge", p) };
            assert_eq!(tail, case["stop"]["tail"].as_str().unwrap(), "{name}");
            check(name, &piece, &case["stop"]["piece"]);
        }
    }

    #[test]
    fn dictation_pieces_join_every_shared_case() {
        for case in fixtures()["join"].as_array().unwrap() {
            let texts: Vec<&str> = case["pieces"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
            assert_eq!(join(texts), case["expected"].as_str().unwrap());
        }
    }

    /// Servidor falso do `/api/dictation/transcribe`: responde pelo tamanho do PCM recebido e conta cada pedido.
    async fn fake(reply: fn(usize) -> (u16, Value)) -> (Api, tokio::sync::mpsc::UnboundedReceiver<(String, usize)>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = Api::new(&format!("http://{}", listener.local_addr().unwrap()), "test").unwrap();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut buffer = vec![0; 65536];
                    let (head, length) = loop {
                        let count = stream.read(&mut buffer).await.unwrap();
                        if count == 0 { return; }
                        bytes.extend_from_slice(&buffer[..count]);
                        let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                        let head = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                        let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).unwrap().trim().parse().unwrap();
                        break (end + 4, length);
                    };
                    while bytes.len() < head + length {
                        let count = stream.read(&mut buffer).await.unwrap();
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                    let line = String::from_utf8_lossy(&bytes).lines().next().unwrap().to_owned();
                    let pcm = length - 44;
                    let _ = tx.send((line, pcm));
                    let (status, body) = reply(pcm);
                    let body = body.to_string();
                    let response = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        (api, rx)
    }

    /// Próximo pedido do servidor falso; o teto só estoura se nenhum pedido sair.
    async fn next(seen: &mut tokio::sync::mpsc::UnboundedReceiver<(String, usize)>) -> (String, usize) {
        tokio::time::timeout(Duration::from_secs(10), seen.recv()).await.expect("nenhum pedaço enviado").unwrap()
    }

    fn record(case: &Value, api: Api) -> (Pieces, Mutex<Vec<u8>>) {
        let frames = frames(case);
        let pcm = Mutex::new(vec![1; frames.last().unwrap().0 as usize * 32]);
        let mut pieces = Pieces::new(api);
        for (ts, level) in frames { pieces.feed(ts as usize * 32, level, &pcm, &Handle::current()); }
        (pieces, pcm)
    }

    #[tokio::test]
    async fn dictation_pieces_send_each_cut_skip_noise_and_join_in_order() {
        let (api, mut seen) = fake(|pcm| match pcm / 32 {
            9210 => (200, json!({"text": "um", "provider": "groq", "aviso": "Groq caiu", "aviso_transcricao": "Groq caiu"})),
            _ => (200, json!({"text": "Dois", "provider": "openai"})),
        }).await;
        // Fala, 18 s de ruído (pedaço descartado) e fala.
        let (pieces, pcm) = record(&case("18 s só de ruído"), api);
        let jobs = pieces.finish(&pcm.lock().unwrap(), &Handle::current()).unwrap();
        assert_eq!(transcribed(jobs).await.unwrap(), Transcribed { raw: "um. Dois".into(), provider: Some("openai".into()), warning: Some("Groq caiu".into()) });
        let mut sizes = vec![next(&mut seen).await, next(&mut seen).await];
        sizes.sort_by_key(|(_, pcm)| *pcm);
        assert!(sizes.iter().all(|(line, _)| line.starts_with("POST /api/dictation/transcribe?limpar=0 ")), "{sizes:?}");
        // O descartado não sai; o resto vai do corte até o fim do áudio.
        assert_eq!(sizes.iter().map(|(_, pcm)| pcm / 32).collect::<Vec<_>>(), [9210, 30000 - 17220]);
        assert!(seen.try_recv().is_err());
    }

    #[tokio::test]
    async fn dictation_pieces_short_tail_resends_the_last_piece_with_it() {
        let (api, mut seen) = fake(|pcm| (200, json!({"text": format!("de {} ms", pcm / 32)}))).await;
        let (pieces, pcm) = record(&case("final de 2 s"), api);
        assert_eq!(next(&mut seen).await.1 / 32, 9210);
        let jobs = pieces.finish(&pcm.lock().unwrap(), &Handle::current()).unwrap();
        // O último pedaço sai substituído pelo pedaço que vai do começo dele até o fim.
        assert_eq!(transcribed(jobs).await.unwrap().raw, "de 11190 ms");
    }

    #[tokio::test]
    async fn dictation_pieces_short_dictation_has_no_cut_and_a_failed_piece_fails_all() {
        let (api, mut seen) = fake(|_| (503, json!({"detail": "ocupado"}))).await;
        let (pieces, pcm) = record(&case("ditado de 6 s"), api.clone());
        assert!(pieces.finish(&pcm.lock().unwrap(), &Handle::current()).is_none());
        assert!(seen.try_recv().is_err());
        let (pieces, pcm) = record(&case("16 s com pausa"), api);
        assert!(transcribed(pieces.finish(&pcm.lock().unwrap(), &Handle::current()).unwrap()).await.is_err());
    }

    #[test]
    fn dictation_pieces_organized_text_carries_the_transcription_warning_first() {
        let transcribed = Transcribed { raw: "a".into(), provider: Some("openai".into()), warning: Some("Groq caiu".into()) };
        let value = with_transcription(json!({"text": "A.", "raw": "a", "aviso": "organização falhou"}), &transcribed);
        assert_eq!((value["aviso"].as_str(), value["aviso_transcricao"].as_str(), value["provider"].as_str()),
            (Some("Groq caiu · organização falhou"), Some("Groq caiu"), Some("openai")));
        let plain = with_transcription(json!({"text": "A."}), &Transcribed { raw: "a".into(), provider: None, warning: None });
        assert!(plain.get("aviso").is_none() && plain.get("aviso_transcricao").is_none());
    }
}

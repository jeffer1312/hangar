use super::*;
use std::{io::Read, sync::Mutex};

const STYLES: [&str; 3] = ["limpar", "prosa", "briefing"];
const PCM_LIMIT: usize = 16_000 * 2 * 180;
const SILENCE: Duration = Duration::from_secs(2);
const COUNTDOWN: Duration = Duration::from_secs(3);

fn organization_warning(value:&Value)->Option<String> {
    let warning=value["aviso"].as_str().filter(|s|!s.is_empty())?;
    let Some(code)=value["organization_code"].as_str().filter(|code|!code.is_empty()) else{return Some(warning.into())};
    let message=tr(code);
    let message=if message==code {warning.into()} else {message};
    Some(if let Some(transcription)=value["aviso_transcricao"].as_str().filter(|s|!s.is_empty()) {format!("{transcription} · {message}")}else{message})
}

#[derive(Clone, Default)]
struct OrganizationSelection {
    mode: String,
    claude_model: String,
    codex_model: String,
    supported: bool,
    include_recent_messages: bool,
}
impl OrganizationSelection {
    fn read(value:&Value)->Self {
        let field=|key:&str|value["campos"][key]["valor"].as_str().unwrap_or("").to_owned();
        let mode=field("dictation_organization_mode");
        Self{mode:if matches!(mode.as_str(),"harness"|"external_api"){mode}else{"none".into()},
            claude_model:field("dictation_claude_model"),codex_model:field("dictation_codex_model"),
            include_recent_messages:value["campos"]["dictation_include_recent_messages"]["valor"].as_bool().unwrap_or(false),
            supported:value["campos"].get("dictation_organization_mode").is_some()}
    }
    fn options(&self,session:Option<&SessionInfo>)->api::DictationOptions {
        api::DictationOptions{mode:if self.mode.is_empty(){"none".into()}else{self.mode.clone()},
            model:session.filter(|_|self.mode=="harness").map(|session|if session.provider=="codex"{self.codex_model.clone()}else{self.claude_model.clone()}),
            generation:session.and_then(|session|session.lifecycle_id.clone().or_else(||session.jsonl.clone())),
            account:session.and_then(|session|session.conta.clone()),rust_capable:self.supported,
            include_recent_messages:self.mode!="none"&&self.include_recent_messages}
    }
}

#[derive(Default)]
struct Vad {
    peak: f32,
    last: Option<Instant>,
    quiet_since: Option<Instant>,
}

impl Vad {
    fn step(&mut self, rms: f32, now: Instant) -> bool {
        let elapsed = self.last.map(|last| now.duration_since(last).as_secs_f32() * 1000.).unwrap_or(0.);
        self.last = Some(now);
        let decayed = self.peak * 0.98_f32.powf(elapsed / 55.);
        self.peak = if rms > decayed { decayed + (rms - decayed) * 0.08 } else { decayed };
        if self.peak <= 0.01 || rms >= self.peak * 0.25 {
            self.quiet_since = None;
            return false;
        }
        let since = *self.quiet_since.get_or_insert(now);
        now.duration_since(since) >= SILENCE
    }
}

/// Junta os canais num só e reduz para 16 kHz pela média de cada janela: o formato que o backend transcreve.
struct Downmix {
    channels: usize,
    step: f64,
    phase: f64,
    sum: f32,
    count: u32,
    last: f32,
}

impl Downmix {
    fn new(channels: u16, rate: u32) -> Self {
        Self { channels: channels.max(1) as usize, step: rate as f64 / 16_000., phase: 0., sum: 0., count: 0, last: 0. }
    }

    fn push<T: Copy>(&mut self, data: &[T], sample: impl Fn(T) -> f32, out: &mut Vec<u8>) {
        for frame in data.chunks(self.channels) {
            let mono = frame.iter().map(|value| sample(*value)).sum::<f32>() / frame.len() as f32;
            self.sum += mono;
            self.count += 1;
            self.phase += 1.;
            while self.phase >= self.step {
                self.phase -= self.step;
                if self.count > 0 { self.last = self.sum / self.count as f32; self.sum = 0.; self.count = 0; }
                if out.len() + 2 > PCM_LIMIT { return; }
                out.extend_from_slice(&((self.last.clamp(-1., 1.) * 32767.) as i16).to_le_bytes());
            }
        }
    }
}

struct Recorder {
    stream: Option<cpal::Stream>,
    failed: Arc<std::sync::atomic::AtomicBool>,
    pcm: Arc<Mutex<Vec<u8>>>,
    playback: Option<Instant>,
    sampled: usize,
    last_signal: (f32, f32),
    last_pcm_at: Option<Instant>,
}

impl Recorder {
    fn start() -> Result<Self, Failure> {
        let mut recorder = Self { stream: None, failed: Default::default(), pcm: Default::default(), playback: None,
            sampled: 0, last_signal: (0., 0.), last_pcm_at: None };
        if let Some(path) = std::env::var_os("HANGAR_NATIVE_DICTATION_WAV") {
            let mut bytes = Vec::new();
            std::fs::File::open(path).and_then(|file| file.take((PCM_LIMIT + 4097) as u64).read_to_end(&mut bytes))
                .map_err(|_| Failure::local("dictation_wav_error"))?;
            *recorder.pcm.lock().unwrap() = wav_pcm(&bytes).ok_or_else(|| Failure::local("dictation_wav_error"))?.to_vec();
            recorder.playback = Some(Instant::now());
            return Ok(recorder);
        }
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = cpal::default_host().default_input_device().ok_or_else(|| Failure::local("dictation_no_microphone"))?;
        let config = device.default_input_config().map_err(|error| {
            eprintln!("dictation config: {error}");
            Failure::local("dictation_recorder_error")
        })?;
        // Reserva os 180 s de uma vez: crescer o Vec dentro da função de áudio copiaria megabytes em tempo real.
        *recorder.pcm.lock().unwrap() = Vec::with_capacity(PCM_LIMIT);
        let (pcm, failed) = (recorder.pcm.clone(), recorder.failed.clone());
        let mut mix = Downmix::new(config.channels(), config.sample_rate());
        let on_error = move |error: cpal::Error| {
            eprintln!("dictation stream: {error}");
            // Estouro de buffer perde um pedaço e segue; o resto é microfone sumido ou captura parada.
            if !matches!(error.kind(), cpal::ErrorKind::Xrun | cpal::ErrorKind::RealtimeDenied) {
                failed.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        };
        macro_rules! input {
            ($t:ty, $to_f32:expr) => {
                device.build_input_stream::<$t, _, _>(config.clone().into(),
                    move |data, _| mix.push(data, $to_f32, &mut pcm.lock().unwrap()), on_error, None)
            };
        }
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => input!(f32, |v: f32| v),
            cpal::SampleFormat::I16 => input!(i16, |v: i16| v as f32 / 32768.),
            cpal::SampleFormat::I32 => input!(i32, |v: i32| v as f32 / 2_147_483_648.),
            cpal::SampleFormat::U16 => input!(u16, |v: u16| (v as f32 - 32768.) / 32768.),
            cpal::SampleFormat::U8 => input!(u8, |v: u8| (v as f32 - 128.) / 128.),
            cpal::SampleFormat::I8 => input!(i8, |v: i8| v as f32 / 128.),
            cpal::SampleFormat::F64 => input!(f64, |v: f64| v as f32),
            other => {
                eprintln!("dictation format: {other:?}");
                return Err(Failure::local("dictation_recorder_error"));
            }
        }.map_err(|error| {
            eprintln!("dictation open: {error}");
            Failure::local("dictation_recorder_error")
        })?;
        stream.play().map_err(|error| {
            eprintln!("dictation play: {error}");
            Failure::local("dictation_recorder_error")
        })?;
        recorder.stream = Some(stream);
        Ok(recorder)
    }

    fn failed(&self) -> bool { self.failed.load(std::sync::atomic::Ordering::Relaxed) }

    fn signal(&mut self) -> (f32, f32) {
        let bytes = self.pcm.lock().unwrap();
        let end = self.playback.map(|start| (start.elapsed().as_millis() as usize * 32).min(bytes.len())).unwrap_or(bytes.len()) & !1;
        let start = self.sampled.min(end);
        self.sampled = end;
        // O sistema entrega blocos; um intervalo sem bloco ainda é áudio recente, mas uma captura travada não é fala eterna.
        if start == end {
            return if self.last_pcm_at.is_some_and(|at| at.elapsed() < Duration::from_millis(4096 / 32 + 55)) {
                self.last_signal
            } else { (0., 0.) };
        }
        let mut peak: f32 = 0.;
        let mut sum = 0.;
        let mut count = 0;
        for sample in bytes[start..end].chunks_exact(2) {
            let value = i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.;
            peak = peak.max(value.abs());
            sum += value * value;
            count += 1;
        }
        self.last_signal = (peak, if count == 0 { 0. } else { (sum / count as f32).sqrt() });
        self.last_pcm_at = Some(Instant::now());
        self.last_signal
    }

    fn finish(mut self) -> Result<Vec<u8>, Failure> {
        drop(self.stream.take());
        let pcm = self.pcm.lock().unwrap();
        // Só zeros é captura muda (permissão negada no macOS entrega silêncio): o Whisper inventaria texto.
        if pcm.len() < 2 || pcm.iter().all(|byte| *byte == 0) { return Err(Failure::local("dictation_empty_audio")); }
        Ok(wav(&pcm[..pcm.len() & !1]))
    }
}

fn wav(pcm: &[u8]) -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data");
    bytes.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    bytes.extend_from_slice(pcm);
    bytes
}

fn wav_pcm(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() > PCM_LIMIT + 4096 || bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" { return None; }
    let (mut offset, mut format, mut data) = (12usize, false, None);
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?) as usize;
        let chunk = bytes.get(offset + 8..offset.checked_add(8)?.checked_add(size)?)?;
        match &bytes[offset..offset + 4] {
            b"fmt " => format = chunk.get(..16) == Some(&b"\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0"[..]),
            b"data" => data = Some(chunk), _ => {}
        }
        offset += 8 + size + (size % 2);
    }
    data.filter(|pcm| format && !pcm.is_empty() && pcm.len() <= PCM_LIMIT && pcm.len() % 2 == 0)
}

/// Sessão que recebe o texto do ditado, capturada ao começar a gravar ou a transcrever um arquivo, com a conexão da
/// máquina dela: o pedido segue para ela mesmo com outra sessão aberta.
#[derive(Clone)]
pub(super) struct DictationTarget { key: SessionKey, life: Option<String>, api: Api }

/// Onde o resultado de um ditado vai parar.
#[derive(Debug, PartialEq)]
enum Place { Open, Away(SessionKey), Gone }

/// Mesma sessão = mesmo nome e mesmo ciclo de vida, o critério do `follow_transcript`: `/clear` troca só o transcript,
/// e a chave devolvida já é a do transcript de agora. Sem lista confiável da máquina (`None`: não lida, com erro, peer
/// desligado, convite encerrado) não dá para dizer que a sessão acabou, e o texto vai para o rascunho da chave guardada.
fn place(target: &DictationTarget, open: Option<&SessionInfo>, list: Option<&[SessionInfo]>) -> Place {
    let same = |s: &&SessionInfo| s.name == target.key.name && s.lifecycle_id == target.life;
    if open.filter(same).is_some() { return Place::Open; }
    let Some(list) = list else { return Place::Away(target.key.clone()) };
    match list.iter().find(same) {
        Some(session) => Place::Away(SessionKey::new(&target.key.server, session).unwrap_or_else(|| target.key.clone())),
        None => Place::Gone,
    }
}

/// O que sair da tela do ditado faz com ele.
#[derive(Debug, PartialEq)]
enum OnLeave { Keep, Stop, Cancel }

/// Texto que chega com a sessão fora da tela: no fim do rascunho, separado por espaço. Devolve o rascunho e o trecho do
/// texto, para as versões o trocarem depois.
fn dictation_append(draft: &str, text: &str) -> (String, std::ops::Range<usize>) {
    let space = if draft.is_empty() || draft.ends_with(char::is_whitespace) { "" } else { " " };
    let start = draft.len() + space.len();
    (format!("{draft}{space}{text}"), start..start + text.len())
}

/// Grava o áudio nos anexos da sessão (disco desta máquina ou `POST /upload`) e transcreve o arquivo salvo
/// (`?arquivo=`). O caminho volta mesmo quando a transcrição falha; falha ao gravar não tem caminho, e o "de novo"
/// tenta de novo com a cópia em memória.
async fn upload_and_transcribe(api: Api, uploads: disk::Uploads, name: String, bytes: Vec<u8>, style: Option<&'static str>, options: api::DictationOptions)
    -> (Option<String>, Result<Value, Failure>) {
    let saved = match uploads {
        disk::Uploads::Remote => api.upload_audio(&name, "ditado.wav", bytes).await,
        local => local.upload(&api, &name, "ditado.wav", bytes, None).await,
    };
    let path = match saved {
        Ok(saved) => saved.path,
        Err(error) => return (None, Err(error)),
    };
    let file = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
    let result = api.dictate_saved(&name, &file, style, &options).await;
    (Some(path), result)
}

#[derive(Default)]
pub(super) struct Dictation {
    organization: Option<(String,OrganizationSelection)>,
    snapshot: Option<api::DictationOptions>,
    snapshot_style: Option<&'static str>,
    pending_send: Option<SendIntent>,
    seq: u64,
    owner: Option<SessionOwner>,
    /// Sessão que recebe o texto; `None` na tela sem sessão, onde sair dela cancela como antes.
    target: Option<DictationTarget>,
    recorder: Option<Recorder>,
    request: Option<JoinHandle<()>>,
    started: Option<Instant>,
    level: f32,
    /// Nível de cada passo da gravação, o mais novo no fim: vira a onda que desliza, como no web.
    bars: std::collections::VecDeque<f32>,
    result: Option<Value>,
    style: Option<(u64, &'static str)>,
    style_writes: u64,
    style_task: Option<Task<()>>,
    audio: Arc<Mutex<Vec<u8>>>,
    file_name: Option<String>,
    /// Onde o áudio ficou nos anexos da sessão (o `path` do upload ou da resposta). Da lista de recentes, só o nome.
    server_path: Option<String>,
    file_owner: Option<SessionOwner>,
    file_generation: u64,
    versions: HashMap<String, Value>,
    inserted: Option<(String, std::ops::Range<usize>)>,
    cleaning: bool,
    error: Option<String>,
    /// Erro que veio com o resultado (falha, texto vazio, rascunho mudado): é da sessão de destino e só aparece com ela
    /// aberta. `error` fica para os desta tela (estilo, envio automático).
    result_error: Option<String>,
    hands_free: bool,
    auto_send: bool,
    timed_out: bool,
    vad: Vad,
    countdown: Option<Instant>,
}

struct SendIntent { owner: Option<SessionOwner>, draft: String, attachments: Vec<String>, steer: bool, recipients: Option<(bool, Vec<String>)> }

#[derive(Debug, PartialEq, Eq)]
enum SendPreparation { Ready, Stop, Wait }

impl Dictation {
    fn prepare_send(&mut self, owner: Option<SessionOwner>, draft: String, attachments: Vec<String>,
        steer: bool, recording: bool, transcribing: bool) -> SendPreparation {
        if !recording && !transcribing { return SendPreparation::Ready; }
        if self.pending_send.is_none() { self.pending_send = Some(SendIntent { owner, draft, attachments, steer, recipients: None }); }
        self.auto_send = false;
        self.countdown = None;
        if recording { SendPreparation::Stop } else { SendPreparation::Wait }
    }

    fn complete_send(&mut self, owner: &Option<SessionOwner>, draft: &str, attachments: &[String]) -> Option<SendIntent> {
        self.pending_send.take().filter(|intent| &intent.owner == owner && intent.draft == draft && intent.attachments == attachments)
    }
    pub(super) fn recording(&self) -> bool { self.recorder.is_some() }
    pub(super) fn processing(&self) -> bool { self.request.is_some() }
    pub(super) fn send_pending(&self) -> bool { self.pending_send.is_some() }
    pub(super) fn cancel_pending_send(&mut self) { self.pending_send = None; }

    fn observe_send_recipients(&mut self, recipients: &Option<(bool, Vec<String>)>) {
        if self.pending_send.as_ref().is_some_and(|intent| &intent.recipients != recipients) {
            self.pending_send = None;
        }
    }

    fn observe_file_owner(&mut self, owner: Option<SessionOwner>) -> u64 {
        if self.file_owner != owner {
            self.pending_send = None;
            // `error` é da tela que ficou para trás; o ditado em curso segue, então só a troca o limpa.
            if self.owner.is_some() { self.error = None; }
            self.file_owner = owner;
            self.file_generation += 1;
        }
        self.file_generation
    }

    fn style(&self, connection: u64) -> Option<&'static str> {
        self.style.filter(|(owner, _)| *owner == connection).map(|(_, style)| style)
    }

    fn text_in_field(&self, value: &str) -> bool {
        self.inserted.as_ref().is_some_and(|(draft, range)| value.get(range.clone()) == draft.get(range.clone()))
    }

    fn draft_matches(&self, value: &str) -> bool {
        self.inserted.as_ref().is_none_or(|(draft, _)| draft == value)
    }

    /// Saiu da tela do ditado (`owner` é o dono de agora). Com sessão de destino na mesma conexão nada se perde: a
    /// gravação para e vai para ela, e o pedido em voo segue. Sem destino (tela sem sessão) ou com outra conexão, cuja
    /// resposta o filtro de conexão descartaria, cancela.
    fn on_leave(&self, owner: &Option<SessionOwner>, connection: u64) -> OnLeave {
        if self.owner.is_none() || self.owner == *owner { return OnLeave::Keep; }
        if self.target.is_none() || self.owner.as_ref().is_some_and(|(own, ..)| *own != connection) { return OnLeave::Cancel; }
        if self.recorder.is_some() || self.countdown.is_some() { OnLeave::Stop } else { OnLeave::Keep }
    }

    /// Versões da gravação (cru e a que veio) para trocar sem novo pedido; arquivo anexado não tem versões.
    fn remember_versions(&mut self, value: &Value) {
        if self.file_name.is_some() { return; }
        let text = value.get("text").and_then(Value::as_str).unwrap_or("").trim();
        let raw = value.get("raw").and_then(Value::as_str).unwrap_or(text);
        let applied = value.get("estilo_aplicado").and_then(Value::as_str).unwrap_or("cru");
        if self.result.as_ref().and_then(|v| v.get("raw")) != value.get("raw") { self.versions.clear(); }
        let mut raw_version = value.clone();
        raw_version["text"] = json!(raw);
        raw_version["raw"] = json!(raw);
        raw_version["estilo_aplicado"] = json!("cru");
        if let Some(fields) = raw_version.as_object_mut() {
            fields.remove("aviso");
            fields.remove("organization_code");
            if let Some(warning) = fields.get("aviso_transcricao").cloned() {
                fields.insert("aviso".into(), warning);
            }
        }
        self.versions.insert("cru".into(), raw_version);
        if applied != "cru" {
            self.versions.insert(applied.to_owned(), value.clone());
        }
    }

    /// Arquivo de áudio vazio tem a frase do web; gravação vazia, a de gravar de novo.
    fn empty_text_error(&self) -> String {
        if self.file_name.is_some() { tr_shared("composer_transcricao_vazia", &[]) } else { tr("dictation_empty_text") }
    }

    /// Nome do áudio nos anexos da sessão: o último pedaço do caminho guardado.
    fn server_file(&self) -> Option<&str> {
        self.server_path.as_deref().and_then(|path| path.rsplit(['/', '\\']).next()).filter(|name| !name.is_empty())
    }

    /// O que vai no `?arquivo=` do "de novo": o nome com o transcript de sempre; depois de `/clear` (outra pasta de
    /// anexos), o caminho inteiro, que o backend valida dentro da pasta de anexos do projeto.
    fn saved_for_retry(&self, same_transcript: bool) -> Option<String> {
        if self.target.is_none() { return None; }
        if same_transcript { self.server_file().map(str::to_owned) } else { self.server_path.clone() }
    }

    /// Destino de um áudio dos anexos que volta ao ditado, ou a frase da recusa: conversa ainda carregando, outro
    /// ditado em curso, nenhuma sessão aberta.
    fn saved_audio_target(&self, ready: bool, target: Option<DictationTarget>, filename: &str) -> Result<DictationTarget, String> {
        if !ready { return Err(tr("attach_audio_not_ready").replace("{name}", filename)); }
        if self.recorder.is_some() || self.request.is_some() { return Err(tr_shared("composer_aguarde_transcricao", &[])); }
        target.ok_or_else(|| tr("attach_audio_session_changed"))
    }

    fn cancel(&mut self) {
        self.snapshot = None;
        self.snapshot_style = None;
        self.pending_send = None;
        self.seq += 1;
        self.owner = None;
        self.target = None;
        self.recorder = None;
        if let Some(task) = self.request.take() { task.abort(); }
        self.started = None;
        self.level = 0.;
        self.bars.clear();
        self.result = None;
        self.audio = Default::default();
        self.file_name = None;
        self.server_path = None;
        self.versions.clear();
        self.inserted = None;
        self.cleaning = false;
        self.error = None;
        self.result_error = None;
        self.hands_free = false;
        self.auto_send = false;
        self.timed_out = false;
        self.vad = Vad::default();
        self.countdown = None;
    }
}

impl Drop for Dictation { fn drop(&mut self) { self.cancel(); } }

impl Hangar {
    fn dictation_recipients(&self) -> Option<(bool, Vec<String>)> {
        let key = self.selected_key()?;
        let mut peers = self.live_peers(&key.name);
        peers.sort();
        Some((self.group_targets(&key, "").is_some(), peers))
    }
    fn dictation_attachments(&self) -> Vec<String> {
        self.composer_key().and_then(|key| self.attachments.get(&key))
            .map(|items| items.iter().map(|item| item.id.to_string()).collect()).unwrap_or_default()
    }

    pub(super) fn defer_dictation_send(&mut self, steer: bool, cx: &mut Context<Self>) -> bool {
        let recording = self.dictation.recording();
        if !self.dictation_here(cx) {
            if recording { self.stop_dictation(false, false, cx); }
            return false;
        }
        if recording && self.dictation.recorder.as_ref().is_some_and(|r| r.pcm.lock().unwrap().len() < 2) {
            self.cancel_dictation();
            return false;
        }
        let owner = self.dictation_owner(cx);
        let draft = self.composer.read(cx).value().to_string();
        let attachments = self.dictation_attachments();
        let first_send = !self.dictation.send_pending();
        let preparation = self.dictation.prepare_send(owner, draft, attachments, steer, recording, self.dictation.processing());
        if first_send {
            let recipients = self.dictation_recipients();
            if let Some(intent) = self.dictation.pending_send.as_mut() { intent.recipients = recipients; }
        }
        match preparation {
            SendPreparation::Ready => false,
            SendPreparation::Stop => { self.stop_dictation(false, false, cx); cx.notify(); true }
            SendPreparation::Wait => true,
        }
    }
    /// Dono do ditado: a sessão aberta ou, sem ela, a tela sem sessão (nome vazio) antes do Enviar.
    pub(super) fn dictation_owner(&self, cx: &App) -> Option<SessionOwner> {
        self.session_owner().or_else(|| {
            if !self.new_chat_screen() || self.opening.is_some() { return None; }
            Some((self.connection, self.new_chat_api(cx)?.identity(), String::new()))
        })
    }

    pub(super) fn check_dictation_owner(&mut self, cx: &mut Context<Self>) -> u64 {
        let owner = self.dictation_owner(cx);
        let generation = self.dictation.observe_file_owner(owner.clone());
        self.dictation.observe_send_recipients(&self.dictation_recipients());
        // O player do ditado só tem controles na barra da sessão dele: fora dela, tocaria sem como parar.
        if self.dictation.owner.is_some() && self.dictation.owner != owner { self.stop_audio("dictation"); }
        match self.dictation.on_leave(&owner, self.connection) {
            OnLeave::Keep => return generation,
            OnLeave::Cancel => self.cancel_dictation(),
            OnLeave::Stop => {
                // A contagem do envio automático é da tela que ficou para trás; a gravação vai para a origem.
                self.dictation.countdown = None;
                self.stop_dictation(false, false, cx);
            }
        }
        self.redraw(panes::Area::Bottom, cx);
        generation
    }

    /// Destino do ditado que começa agora: a sessão aberta, com a máquina dela. Tela sem sessão: nenhum.
    fn open_dictation_target(&self) -> Option<DictationTarget> {
        Some(DictationTarget { key: self.selected_key()?, life: self.selected.as_ref()?.lifecycle_id.clone(), api: self.session_api()? })
    }

    /// Lista da máquina `server` que pode dizer que uma sessão acabou: lida e sem erro. A ativa conta com o SSE no ar.
    fn trusted_sessions(&self, server: &str) -> Option<&[SessionInfo]> {
        let key = servers::norm(server);
        if self.is_active_key(&key) { return (self.list_online && self.list_error.is_none()).then_some(self.sessions.as_slice()); }
        self.remote.get(&key).filter(|list| list.loaded && list.error.is_none()).map(|list| list.sessions.as_slice())
    }

    fn dictation_place(&self, target: &DictationTarget) -> Place {
        let open = self.selected.as_ref().filter(|_| self.session_server().as_deref() == Some(target.key.server.as_str()));
        place(target, open, self.trusted_sessions(&target.key.server))
    }

    /// O ditado é da tela aberta: só então barra, estado e Cancelar aparecem e o texto entra no campo.
    pub(super) fn dictation_here(&self, cx: &App) -> bool {
        match &self.dictation.target {
            Some(target) => self.dictation_place(target) == Place::Open,
            None => self.dictation.owner.is_none() || self.dictation.owner == self.dictation_owner(cx),
        }
    }

    /// Para onde vai o pedido: a sessão de destino, mesmo fora da tela; sem ela, a tela sem sessão de agora.
    fn dictation_request(&self, cx: &App) -> Option<(Api, Option<String>)> {
        match &self.dictation.target {
            Some(target) => Some((target.api.clone(), Some(target.key.name.clone()))),
            None => self.dictation_target(cx),
        }
    }

    /// Nome do áudio para TOCAR pelo servidor, só enquanto o transcript é o do destino: o `GET /uploads/{nome}` lê a
    /// pasta do transcript de agora, que o `/clear` troca.
    fn dictation_saved_file(&self) -> Option<String> {
        let target = self.dictation.target.as_ref()?;
        if self.selected_key().as_ref() != Some(&target.key) { return None; }
        self.dictation.server_file().map(str::to_owned)
    }

    /// Áudio dos anexos que não se pôde ler: sumido (retenção) tem frase própria, o resto é a falha da leitura.
    /// `pub(super)`: a lista de recentes (`app.rs`) usa a mesma frase.
    pub(super) fn saved_audio_failure(error: &Failure) -> String {
        // Do disco desta máquina, arquivo ausente chega sem status (`invalid_response`); do backend, 404.
        if error.status == Some(404) || (error.status.is_none() && error.detail == "invalid_response") { tr("dictation_audio_gone") }
        else { Self::fetch_failure(error) }
    }

    /// Cancelar solta a gravação guardada: o player dela para junto.
    fn cancel_dictation(&mut self) {
        self.dictation.cancel();
        self.stop_audio("dictation");
    }

    fn dictation_ready(&self, cx: &App) -> bool {
        let Some((api,_))=self.dictation_target(cx) else{return false};
        if !self.dictation.organization.as_ref().is_some_and(|(identity,_)|*identity==api.identity()){return false;}
        if self.selected.is_none() { return self.new_chat_screen() && self.opening.is_none(); }
        self.selected_key().is_some() && self.chat_online && self.history_installed
    }

    /// Para onde vai o áudio: a sessão aberta, ou a máquina escolhida nos chips da tela sem sessão, sem sessão ainda.
    fn dictation_target(&self, cx: &App) -> Option<(Api, Option<String>)> {
        match self.selected_key() {
            Some(key) => Some((self.session_api()?, Some(key.name))),
            None => Some((self.new_chat_api(cx)?, None)),
        }
    }

    pub(super) fn watch_dictation(_window: &Window, cx: &mut Context<Self>) {
        let mut style_connection = None;
        cx.observe_self(move |this, cx| {
            this.check_dictation_owner(cx);
            let identity=this.dictation_target(cx).map(|(api,_)|api.identity());
            let current=Some((this.connection,identity.clone()));
            if style_connection != current {
                style_connection = current;
                this.dictation.style_task = None;
            }
            if identity.is_some() && !this.dictation.organization.as_ref().is_some_and(|(owner,_)|Some(owner)==identity.as_ref()) && this.dictation.style_task.is_none() {
                this.load_dictation_style(cx);
            }
        }).detach();
        let owner = cx.entity().downgrade();
        cx.intercept_keystrokes(move |_, _, cx| {
            let _ = owner.update(cx, |this, cx| this.cancel_dictation_countdown(cx));
        }).detach();
    }

    fn cancel_dictation_countdown(&mut self, cx: &mut Context<Self>) {
        if self.dictation.countdown.take().is_some() {
            self.redraw(panes::Area::Bottom, cx);
            cx.notify();
        }
    }

    pub(super) fn load_dictation_style(&mut self, cx: &mut Context<Self>) {
        let Some((api,_)) = self.dictation_target(cx) else { return; };
        let identity=api.identity();
        let (connection, writes) = (self.connection, self.dictation.style_writes);
        let job = self.runtime.spawn(async move { api.config().await });
        self.dictation.style_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.connection != connection || this.dictation.style_writes != writes { return; }
                if this.dictation_target(cx).is_none_or(|(api,_)|api.identity()!=identity){return;}
                if let Ok(Ok(value)) = result {
                    this.dictation.organization=Some((identity,OrganizationSelection::read(&value)));
                    if this.dictation.error.as_deref() == Some(tr("dictation_config_failed").as_str()) {
                        this.dictation.error = None;
                    }
                    if let Some(style) = STYLES.into_iter().find(|style| value.pointer("/campos/ditado_estilo/valor").and_then(Value::as_str) == Some(*style)) {
                        this.dictation.style = Some((connection, style));
                        cx.notify();
                    }
                    cx.notify();
                } else {
                    this.dictation.error=Some(tr("dictation_config_failed"));cx.notify();
                }
            });
        }));
    }

    fn set_dictation_style(&mut self, style: &'static str, cx: &mut Context<Self>) {
        if self.dictation.recorder.is_some() || self.dictation.request.is_some() { return; }
        let Some((api,_)) = self.dictation_target(cx) else { return; };
        let identity = api.identity();
        let (connection, before) = (self.connection, self.dictation.style);
        self.dictation.style = Some((connection, style));
        self.dictation.style_writes += 1;
        self.dictation.error = None;
        let mine = self.dictation.style_writes;
        let job = self.runtime.spawn(async move {
            api.server_send(reqwest::Method::POST, &["config"], Some(json!({"ditado_estilo": style})), 8).await
        });
        cx.spawn(async move |this, cx| {
            let result = job.await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
            let _ = this.update(cx, |this, cx| {
                if this.connection != connection || this.dictation.style_writes != mine { return; }
                if this.dictation_target(cx).is_none_or(|(api,_)|api.identity()!=identity) { return; }
                if let Err(error) = result {
                    this.dictation.style = before;
                    this.dictation.error = Some(Self::failure(&error));
                    cx.notify();
                }
            });
        }).detach();
        cx.notify();
    }

    fn dictation_options(&self, cx: &App)->api::DictationOptions {
        let identity=self.dictation_target(cx).map(|(api,_)|api.identity());
        self.dictation.organization.as_ref().filter(|(owner,_)|Some(owner)==identity.as_ref())
            .map(|(_,selection)|selection.options(self.selected.as_ref())).unwrap_or_default()
    }

    pub(super) fn toggle_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Um microfone só: com a voz ligada, o ditado espera.
        if self.voice.call.is_some() { return; }
        self.cancel_dictation_countdown(cx);
        if self.dictation.request.is_some() { return; }
        if self.dictation.recorder.is_some() {
            self.stop_dictation(false, false, cx);
            self.composer.update(cx, |input, cx| input.focus(window, cx));
            return;
        }
        if self.connection_dialog || self.settings.is_some() || window.has_active_dialog(cx) { return; }
        if !self.dictation_ready(cx) { return; }
        self.cancel_dictation();
        self.dictation.snapshot=Some(self.dictation_options(cx));
        self.dictation.snapshot_style=self.dictation.style(self.connection);
        match Recorder::start() {
            Ok(recorder) => {
                self.dictation.owner = self.dictation_owner(cx);
                self.dictation.target = self.open_dictation_target();
                self.dictation.hands_free = appearance::get().hands_free;
                self.dictation.recorder = Some(recorder);
                self.dictation.started = Some(Instant::now());
                let seq = self.dictation.seq;
                cx.spawn_in(window, async move |this, cx| {
                    loop {
                        cx.background_executor().timer(Duration::from_millis(55)).await;
                        let keep = this.update_in(cx, |this, window, cx| {
                            if this.dictation.seq != seq { return false; }
                            let Some(recorder) = &mut this.dictation.recorder else { return false; };
                            if recorder.failed() {
                                // Microfone caiu no meio (headset trocado): transcreve o que já foi gravado.
                                if recorder.pcm.lock().unwrap().len() >= 2 {
                                    this.stop_dictation(false, false, cx);
                                } else {
                                    this.cancel_dictation();
                                    window.push_notification(Notification::error(tr("dictation_recorder_error")), cx);
                                }
                                this.redraw(panes::Area::Bottom, cx);
                                return false;
                            }
                            let (level, rms) = recorder.signal();
                            this.dictation.level = level;
                            // Teto de 320, o mesmo do web: cobre a faixa inteira numa janela larga.
                            if this.dictation.bars.len() >= 320 { this.dictation.bars.pop_front(); }
                            // RMS ×5 como no web: voz normal fica em 0,05–0,2 e sem ganho a onda mal sai do chão.
                            this.dictation.bars.push_back((rms * 5.).min(1.));
                            if this.dictation.hands_free && this.dictation.vad.step(rms, Instant::now()) {
                                this.stop_dictation(true, false, cx);
                                this.redraw(panes::Area::Bottom, cx);
                                return false;
                            }
                            this.redraw(panes::Area::Bottom, cx);
                            if this.dictation.started.is_some_and(|start| start.elapsed() >= Duration::from_secs(180)) {
                                this.stop_dictation(false, this.dictation.hands_free, cx);
                            }
                            true
                        });
                        if !matches!(keep, Ok(true)) { break; }
                    }
                }).detach();
            }
            Err(error) => window.push_notification(Notification::error(Self::dictation_failure(&error)), cx),
        }
        cx.notify();
    }

    pub(super) fn transcribe_file(&mut self, key: &SessionKey, filename: String, bytes: Vec<u8>, cx: &mut Context<Self>) -> Result<(), String> {
        if bytes.len() as u64 > api::MAX_BYTES { return Err(tr("attach_too_big_named").replace("{name}", &filename)); }
        if !self.dictation_ready(cx) { return Err(tr("attach_audio_not_ready").replace("{name}", &filename)); }
        if self.composer_key().as_ref() != Some(key) { return Err(tr("attach_audio_session_changed")); }
        let Some(owner) = self.dictation_owner(cx) else { return Err(tr("attach_audio_session_changed")); };
        if self.dictation.recorder.is_some() || self.dictation.request.is_some() {
            return Err(tr_shared("composer_aguarde_transcricao", &[]));
        }
        let Some((api, session)) = self.dictation_target(cx) else { return Err(tr("connection_failed")); };
        self.cancel_dictation();
        self.dictation.owner = Some(owner);
        self.dictation.target = self.open_dictation_target();
        self.dictation.file_name = Some(filename.clone());
        *self.dictation.audio.lock().unwrap() = bytes.clone();
        let (tx, connection, seq) = (self.tx.clone(), self.connection, self.dictation.seq);
        self.dictation.request = Some(self.runtime.spawn(async move {
            let result = api.transcribe(session.as_deref(), &filename, bytes, false, None).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Dictation(seq, None, result) }).await;
        }));
        cx.notify();
        Ok(())
    }

    /// Áudio dos anexos da sessão aberta de volta ao ditado: o servidor transcreve o arquivo que já tem (`?arquivo=`),
    /// nada desce nem sobe de novo.
    pub(super) fn dictate_upload(&mut self, filename: String, cx: &mut Context<Self>) -> Result<(), String> {
        let target = self.dictation.saved_audio_target(self.dictation_ready(cx), self.open_dictation_target(), &filename)?;
        self.cancel_dictation();
        self.dictation.snapshot=Some(self.dictation_options(cx));
        self.dictation.snapshot_style=self.dictation.style(self.connection);
        self.dictation.owner = self.dictation_owner(cx);
        self.dictation.file_name = Some(filename.clone());
        self.dictation.server_path = Some(filename.clone());
        let (api, name) = (target.api.clone(), target.key.name.clone());
        self.dictation.target = Some(target);
        let (tx, connection, seq) = (self.tx.clone(), self.connection, self.dictation.seq);
        self.dictation.request = Some(self.runtime.spawn(async move {
            let result = api.transcribe_saved(&name, &filename, false, None).await;
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Dictation(seq, None, result) }).await;
        }));
        cx.notify();
        Ok(())
    }

    fn stop_dictation(&mut self, silence: bool, timed_out: bool, cx: &mut Context<Self>) {
        let Some(recorder) = self.dictation.recorder.take() else { return; };
        let Some((api, session)) = self.dictation_request(cx) else { self.cancel_dictation(); return; };
        self.dictation.auto_send = silence && self.dictation.hands_free;
        self.dictation.timed_out = timed_out;
        let audio_cache = self.dictation.audio.clone();
        let style = self.dictation.snapshot_style;
        let options=self.dictation.snapshot.clone().unwrap_or_default();
        // Com sessão de destino, o áudio entra nos anexos dela antes de transcrever: a falha não o perde e o "de novo"
        // não reenvia. A tela sem sessão não tem pasta e segue no `/transcribe` com corpo.
        let uploads = self.dictation.target.as_ref().map(|target| self.uploads_for(&target.key));
        let (tx, connection, seq) = (self.tx.clone(), self.connection, self.dictation.seq);
        // O fluxo de áudio não troca de thread; soltá-lo e montar o WAV é rápido o bastante para a tela.
        let audio = recorder.finish();
        self.dictation.request = Some(self.runtime.spawn(async move {
            let (path, result) = match audio {
                Ok(bytes) => {
                    *audio_cache.lock().unwrap() = bytes.clone();
                    match (session, uploads) {
                        (Some(name), Some(uploads)) => upload_and_transcribe(api, uploads, name, bytes, style,options).await,
                        (session, _) => (None, api.dictate(session.as_deref(), "ditado.wav", bytes, style,&options).await),
                    }
                }
                Err(error) => (None, Err(error)),
            };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Dictation(seq, path, result) }).await;
        }));
        cx.notify();
    }

    fn start_dictation_countdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let deadline = Instant::now() + COUNTDOWN;
        let seq = self.dictation.seq;
        self.dictation.countdown = Some(deadline);
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let keep = this.update_in(cx, |this, window, cx| {
                    if this.dictation.seq != seq || this.dictation.countdown != Some(deadline)
                        || this.dictation.owner.is_none() || this.dictation.owner != this.dictation_owner(cx) { return false; }
                    if Instant::now() < deadline {
                        this.redraw(panes::Area::Bottom, cx);
                        cx.notify();
                        return true;
                    }
                    this.dictation.countdown = None;
                    let key = this.selected_key();
                    // Sem sessão, enviar é criar: deu certo quando a abertura começou.
                    if this.selected.is_none() {
                        this.submit(false, false, window, cx);
                        if this.opening.is_none() { this.dictation.error = Some(tr("dictation_auto_send_failed")); }
                    } else if !this.can_send() || key.as_ref().is_none_or(|key| this.delivery.pending(key)
                        || this.uploading.contains_key(key) || this.attachments.get(key).is_some_and(|files| !files.is_empty())) {
                        this.dictation.error = Some(tr("dictation_auto_send_failed"));
                    } else {
                        this.submit(false, false, window, cx);
                        if key.as_ref().is_some_and(|key| !this.delivery.pending(key)) {
                            this.dictation.error = Some(tr("dictation_auto_send_failed"));
                        }
                    }
                    this.redraw(panes::Area::Bottom, cx);
                    cx.notify();
                    false
                });
                if !matches!(keep, Ok(true)) { break; }
            }
        }).detach();
        self.redraw(panes::Area::Bottom, cx);
        cx.notify();
    }

    fn revise_dictation(&mut self, style: Option<&'static str>, window: &mut Window, cx: &mut Context<Self>) {
        if self.dictation.request.is_some() || self.dictation.recorder.is_some()
            || self.dictation.owner.is_none() || !self.dictation_here(cx)
            || (self.dictation.file_name.is_some() && style.is_some()) { return; }
        if !self.dictation.draft_matches(&self.composer.read(cx).value()) {
            self.dictation.result_error = Some(tr("dictation_draft_changed"));
            cx.notify();
            return;
        }
        (self.dictation.error, self.dictation.result_error) = (None, None);
        if let Some(value) = style.and_then(|style| self.dictation.versions.get(style)).cloned() {
            self.receive_dictation(self.dictation.seq, None, Ok(value), window, cx);
            return;
        }
        let Some((api, session)) = self.dictation_request(cx) else { return; };
        let raw =self.dictation.result.as_ref().and_then(|v| v.get("raw")).and_then(Value::as_str).unwrap_or("").to_owned();
        let references=self.dictation.result.as_ref().and_then(|value|value.get("recent_messages")).cloned();
        let audio = self.dictation.audio.lock().unwrap().clone();
        // Nome guardado com o mesmo transcript; depois de `/clear`, o caminho inteiro (outra pasta de anexos).
        let same_transcript = self.dictation.target.as_ref().is_some_and(|target| self.selected_key().as_ref() == Some(&target.key));
        let saved = self.dictation.saved_for_retry(same_transcript);
        // Upload que falhou na primeira vez: a gravação sobe de novo antes de transcrever.
        let uploads = self.dictation.target.as_ref().map(|target| self.uploads_for(&target.key));
        if (style.is_some() && raw.is_empty()) || (style.is_none() && audio.is_empty() && saved.is_none()) { return; }
        self.dictation.seq += 1;
        self.dictation.cleaning = style.is_some();
        let clean = self.dictation.file_name.is_none();
        let filename = self.dictation.file_name.clone().unwrap_or_else(|| "ditado.wav".into());
        let recording_style = if clean { self.dictation.snapshot_style } else { None };
        let options=self.dictation.snapshot.clone().unwrap_or_default();
        let (tx, connection, seq) = (self.tx.clone(), self.connection, self.dictation.seq);
        self.dictation.request = Some(self.runtime.spawn(async move {
            let (path, result) = if let Some(style) = style {
                (None, api.server_send(reqwest::Method::POST, &["ditado", "relimpar"], Some(json!({"texto": raw, "estilo": style,
                    "session":session,"generation":options.generation,"organization_mode":options.mode,"organization_model":options.model,"organization_account":options.account,
                    "include_recent_messages":options.include_recent_messages,"recent_messages":references})), 210).await
                    .and_then(|mut value| {
                        let fields = value.as_object_mut().ok_or_else(|| Failure::local("invalid_response"))?;
                        fields.insert("raw".into(), json!(raw));
                        Ok(value)
                    }))
            } else if let (Some(file), Some(name)) = (saved, session.as_deref()) {
                (None, if clean{api.dictate_saved(name,&file,recording_style,&options).await}else{api.transcribe_saved(name,&file,false,None).await})
            } else if let (true, Some(name), Some(uploads)) = (clean, session.clone(), uploads) {
                upload_and_transcribe(api, uploads, name, audio, recording_style,options).await
            } else { (None, if clean{api.dictate(session.as_deref(),&filename,audio,recording_style,&options).await}else{api.transcribe(session.as_deref(),&filename,audio,false,None).await}) };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Dictation(seq, path, result) }).await;
        }));
        cx.notify();
    }

    pub(super) fn receive_dictation(&mut self, seq: u64, path: Option<String>, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        if self.dictation.seq != seq || self.dictation.owner.is_none() { return; }
        let place = match &self.dictation.target {
            Some(target) => self.dictation_place(target),
            // Tela sem sessão: o resultado só vale nela, como antes.
            None if self.dictation.owner == self.dictation_owner(cx) => Place::Open,
            None => return,
        };
        let auto_send = std::mem::take(&mut self.dictation.auto_send);
        let requested_send = self.dictation.send_pending();
        let owner = self.dictation_owner(cx);
        let draft = self.composer.read(cx).value().to_string();
        let attachments = self.dictation_attachments();
        self.dictation.observe_send_recipients(&self.dictation_recipients());
        let pending_send = self.dictation.complete_send(&owner, &draft, &attachments);
        let result_ready = result.as_ref().ok().and_then(|v| v.get("text")).and_then(Value::as_str).is_some_and(|text| !text.trim().is_empty());
        let here = matches!(&place, Place::Open);
        let timed_out = std::mem::take(&mut self.dictation.timed_out);
        self.dictation.request = None;
        self.dictation.started = None;
        self.dictation.level = 0.;
        self.dictation.bars.clear();
        self.dictation.cleaning = false;
        (self.dictation.error, self.dictation.result_error) = (None, None);
        // O áudio está nos anexos da sessão (upload deste pedido ou `path` da resposta): ouvir e transcrever de novo
        // sem reenviar.
        let saved = path.or_else(|| result.as_ref().ok().and_then(|value| value.get("path")).and_then(Value::as_str).map(str::to_owned));
        if saved.is_some() { self.dictation.server_path = saved; }
        match place {
            Place::Open => self.receive_dictation_here(result, auto_send && !requested_send, timed_out, window, cx),
            Place::Away(key) => self.receive_dictation_away(key, result, window, cx),
            Place::Gone => {
                let name = self.dictation.target.as_ref().map(|target| target.key.name.clone()).unwrap_or_default();
                self.cancel_dictation();
                window.push_notification(Notification::warning(tr("dictation_target_gone").replace("{session}", &name)), cx);
            }
        }
        if here && result_ready && self.dictation.result_error.is_none() {
            if let Some(intent) = pending_send {
                // O texto completo passa novamente pela confirmação de comandos destrutivos.
                self.submit(intent.steer, false, window, cx);
            } else if requested_send {
                self.dictation.result_error = Some(tr("dictation_send_changed"));
            }
        }
        cx.notify();
    }

    /// A sessão do ditado está aberta: o texto entra no cursor (ou no lugar da versão anterior), como sempre foi.
    fn receive_dictation_here(&mut self, result: Result<Value, Failure>, auto_send: bool, timed_out: bool, window: &mut Window, cx: &mut Context<Self>) {
        match result {
            Ok(value) => {
                let text = value.get("text").and_then(Value::as_str).unwrap_or("").trim();
                if text.is_empty() {
                    self.dictation.result_error = Some(self.dictation.empty_text_error());
                } else if !self.dictation.draft_matches(&self.composer.read(cx).value()) {
                    self.dictation.result_error = Some(tr("dictation_draft_changed"));
                } else {
                    let draft_still_empty = self.composer.read(cx).value().trim().is_empty()
                        && self.composer_key().is_some_and(|key| self.attachments.get(&key).is_none_or(Vec::is_empty));
                    let previous = self.dictation.inserted.as_ref().map(|(_, range)| range.clone());
                    let inserted = self.composer.update(cx, |input, cx| {
                        let draft = input.value().to_string();
                        let range = previous.unwrap_or_else(|| input.selected_range());
                        let replacement = dictation_insert(&draft, range.clone(), text);
                        input.set_selected_range(range.clone(), cx);
                        input.replace(replacement.clone(), window, cx);
                        (input.value().to_string(), range.start..range.start + replacement.len())
                    });
                    self.dictation.inserted = Some(inserted);
                    self.dictation.remember_versions(&value);
                    // Consome a mudança programática antes do observador de @menção.
                    self.refresh_mention(cx);
                    self.mention.close();
                    if let Some(warning) = organization_warning(&value) {
                        window.push_notification(Notification::warning(warning), cx);
                    }
                    let warning = value.get("aviso").and_then(Value::as_str).is_some_and(|s| !s.is_empty());
                    self.dictation.result = Some(value);
                    if timed_out && !warning { self.dictation.result_error = Some(tr("dictation_silence_timeout")); }
                    if auto_send && draft_still_empty && !warning { self.start_dictation_countdown(window, cx); }
                }
            }
            Err(error) => self.dictation.result_error = Some(Self::dictation_failure(&error)),
        }
    }

    /// A sessão do ditado está fora da tela: o texto vai para o fim do rascunho dela, sem envio automático, e a falha
    /// fica guardada para aparecer na volta.
    fn receive_dictation_away(&mut self, key: SessionKey, result: Result<Value, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        match result {
            Ok(value) => {
                let text = value.get("text").and_then(Value::as_str).unwrap_or("").trim();
                if text.is_empty() {
                    let message = self.dictation.empty_text_error();
                    window.push_notification(Notification::warning(tr("dictation_empty_away")
                        .replace("{session}", &key.name).replace("{error}", &message)), cx);
                    self.dictation.result_error = Some(message);
                    return;
                }
                let (draft, range) = dictation_append(self.drafts.get(&key).map_or("", String::as_str), text);
                self.drafts.insert(key, draft.clone());
                // Voltando à sessão o campo mostra este rascunho: as versões trocam o trecho que entrou aqui.
                self.dictation.inserted = Some((draft, range));
                self.dictation.remember_versions(&value);
                if let Some(warning) = organization_warning(&value) {
                    window.push_notification(Notification::warning(warning), cx);
                }
                self.dictation.result = Some(value);
            }
            Err(error) => {
                let message = Self::dictation_failure(&error);
                // A frase já traz o ponto final dela: "{error}." sairia com dois.
                window.push_notification(Notification::error(tr("dictation_failed_away")
                    .replace("{session}", &key.name).replace("{error}", message.trim_end_matches('.'))), cx);
                self.dictation.result_error = Some(message);
            }
        }
    }

    /// O microfone, a pílula do estilo (ao lado dele, como no web) e a faixa de estado do ditado, só quando há o que mostrar.
    pub(super) fn render_dictation(&self, readable: bool, cx: &mut Context<Self>) -> (Button, Option<AnyElement>, Option<AnyElement>) {
        let here = self.dictation_here(cx);
        let recording = self.dictation.recorder.is_some();
        let in_flight = self.dictation.request.is_some();
        let transcribing = here && in_flight;
        // Um microfone só: com a transcrição de outra sessão em voo, este espera também.
        let elsewhere = in_flight && !here;
        let label = tr(if recording { "dictation_stop" } else if transcribing { "dictation_working" }
            else if elsewhere { "dictation_busy_elsewhere" } else { "dictation_start" });
        let mic = if recording {
            Button::new("dictation-toggle").ghost().size_7().rounded_md()
                .child(div().size_3().rounded_sm().bg(theme::danger()))
        } else { chrome::icon_button("dictation-toggle", IconName::Mic, label.clone(), cx) };
        let voice = self.voice.call.is_some();
        let mic = mic.accessibility_label(label.clone())
            .disabled(voice || in_flight || (!recording && (!readable || !self.dictation_ready(cx))))
            .loading(transcribing)
            .tooltip(if voice { tr("voice_dictation_blocked") } else { format!("{label} · {}", tr("dictation_shortcut")) })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_dictation(window, cx)));
        let owner = here && self.dictation.owner.is_some();
        let style = self.dictation.style(self.connection).unwrap_or("prosa");
        let options=if recording||in_flight {self.dictation.snapshot.clone().unwrap_or_else(||self.dictation_options(cx))}else{self.dictation_options(cx)};
        let organized=options.mode!="none";
        let entity = cx.entity().downgrade();
        // Gravando, some: trocar no meio não muda nada (o backend lê o estilo no fim) e o espaço é do botão de parar.
        let pill = (!recording&&organized).then(|| chrome::pill_button("dictation-style", cx).pl(px(10.)).gap(px(6.))
            .tooltip(tr("dictation_style")).accessibility_label(format!("{}: {}", tr("dictation_style"), style_label(style)))
            .disabled(in_flight || !readable)
            .child(div().text_xs().text_color(theme::muted()).child(style_label(style)))
            .child(chrome::small_icon(IconName::ChevronDown, 12., theme::faint()))
            .dropdown_menu(move |menu, _, cx| {
                let _ = entity.update(cx, |this, cx| this.load_dictation_style(cx));
                STYLES.into_iter().fold(menu, |menu, next| {
                    let entity = entity.clone();
                    menu.item(PopupMenuItem::element(move |_, _| div().flex().flex_col().gap_1().max_w(px(320.))
                        .child(style_label(next)).child(div().text_xs().text_color(theme::muted()).whitespace_normal()
                            .child(tr(&format!("voice_style_{next}_hint")))))
                    .checked(next == style).on_click(move |_, _, cx| {
                        let _ = entity.update(cx, |this, cx| this.set_dictation_style(next, cx));
                    }))
                })
            }).into_any_element()).or_else(||(!recording&&!organized).then(||chrome::pill_button("dictation-no-organization",cx)
                .child(tr("voice_organization_none")).tooltip(tr("voice_organization_none_hint")).disabled(in_flight)
                .on_click(cx.listener(|this,_,window,cx|this.open_settings(settings::Page::Voice,window,cx))).into_any_element()));
        let versions = owner && self.dictation.snapshot.as_ref().is_some_and(|options|options.mode!="none") && self.dictation.file_name.is_none() && self.dictation.result.is_some()
            && (transcribing || self.dictation.text_in_field(&self.composer.read(cx).value()));
        let has_audio = !self.dictation.audio.lock().unwrap().is_empty();
        let playable = has_audio || self.dictation_saved_file().is_some();
        let again = owner && self.dictation.result.is_none() && (has_audio || (self.dictation.target.is_some() && self.dictation.server_path.is_some()));
        let file_audio = owner && self.dictation.file_name.is_some() && playable;
        let controls = (versions || again || file_audio).then(|| div().flex().flex_wrap().items_center().gap_2()
            .when(versions, |el| {
                let applied = self.dictation.result.as_ref().and_then(|v| v.get("estilo_aplicado")).and_then(Value::as_str).unwrap_or("cru");
                el.child(div().text_xs().text_color(theme::muted()).child(tr("dictation_versions")))
                    .children(["cru", "limpar", "prosa", "briefing"].into_iter().map(|version| {
                        Button::new(SharedString::from(format!("dictation-version-{version}"))).ghost().small()
                            .label(style_label(version)).selected(version == applied).disabled(recording || transcribing)
                            .on_click(cx.listener(move |this, _, window, cx| this.revise_dictation(Some(version), window, cx)))
                    }))
            })
            .when(again, |el| el.child(
                Button::new("dictation-retranscribe").ghost().small().label(tr("dictation_again"))
                    .disabled(recording || transcribing)
                    .on_click(cx.listener(|this, _, window, cx| this.revise_dictation(None, window, cx)))))
            // A gravação que virou o texto, para ouvir de novo antes de enviar.
            .when(!recording && playable, |el| el.child(self.audio_controls("dictation", |this, cx| {
                let audio = this.dictation.audio.lock().unwrap().clone();
                if audio.is_empty() {
                    // Sem a cópia em memória, a que o servidor guardou nos anexos da sessão.
                    let (Some(api), Some(key), Some(file)) = (this.session_api(), this.selected_key(), this.dictation_saved_file()) else { return; };
                    let (uploads, source) = (this.uploads_for(&key), Source::Upload(file.clone()));
                    this.toggle_audio("dictation".into(), &file, async move {
                        uploads.fetch(&api, &key.name, &source).await.map_err(|error| Self::saved_audio_failure(&error))
                    }, cx);
                    return;
                }
                let filename = this.dictation.file_name.clone().unwrap_or_else(|| "ditado.wav".into());
                this.toggle_audio("dictation".into(), &filename, async move { Ok(audio) }, cx);
            }, cx))));
        let status = (here && (recording || transcribing)).then(|| {
            let label = tr(if recording { "dictation_active" } else if self.dictation.cleaning { "dictation_cleaning" } else { "dictation_working" });
            let seconds = self.dictation.started.map(|start| start.elapsed().as_secs()).unwrap_or(0);
            div().flex().items_center().gap_2().text_sm().text_color(theme::muted())
                .child(div().id("dictation-status").role(Role::Status).aria_label(label.clone()).child(label))
                .when(recording, |el| el
                    .child(div().font_family(theme::MONO).child(format!("{}:{:02}", seconds / 60, seconds % 60)))
                    // Onda: cresce da esquerda até encher a faixa; cheia, a mais nova fica na ponta direita e as velhas
                    // saem pela esquerda (a de dentro encolhe até a largura da de fora e alinha as barras à direita).
                    .child(div().id("dictation-level").role(Role::Meter).aria_label(tr("dictation_level"))
                        .aria_min_numeric_value(0.).aria_max_numeric_value(100.).aria_numeric_value((self.dictation.level * 100.) as f64)
                        .flex_1().min_w_0().h(px(64.)).flex().items_center().overflow_hidden()
                        .child(div().min_w_0().h_full().flex().items_center().justify_end().gap(px(2.)).overflow_hidden()
                            .children(self.dictation.bars.iter().map(|level| div().flex_shrink_0().w(px(3.)).rounded(px(3.))
                                .h(px(8. + level.clamp(0., 1.) * 56.)).bg(theme::accent()))))))
                .when(!recording, |el| el.child(div().flex_1()))
                .child(Button::new("dictation-cancel").ghost().small().label(tr("dictation_cancel"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cancel_dictation();
                        this.composer.update(cx, |input, cx| input.focus(window, cx));
                        cx.notify();
                    })))
                .into_any_element()
        });
        let countdown = self.dictation.countdown.map(|deadline| {
            let seconds = ((deadline.saturating_duration_since(Instant::now()).as_millis() + 999) / 1000).clamp(1, 3);
            let label = tr("dictation_countdown").replace("{seconds}", &seconds.to_string());
            let owner = cx.entity().downgrade();
            div().flex().items_center().gap_2()
                .child(div().id("dictation-countdown").role(Role::Status).aria_label(label.clone())
                    .text_sm().text_color(theme::accent_text()).child(label))
                .child(Button::new("dictation-countdown-cancel").ghost().small().label(tr("dictation_countdown_cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_dictation_countdown(cx))))
                .child(canvas(|_, _, _| (), move |_, _, window, _| {
                    window.on_mouse_event::<MouseDownEvent>(move |_, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = owner.update(cx, |this, cx| this.cancel_dictation_countdown(cx));
                        }
                    });
                }).w_0().h_0())
                .into_any_element()
        });
        // O erro do resultado é da sessão de destino; o do estilo e o do envio automático são desta tela.
        let message = self.dictation.error.clone().or_else(|| self.dictation.result_error.clone().filter(|_| here));
        let error = message.map(|error| div().id("dictation-error").role(Role::Alert)
            .text_sm().text_color(theme::danger()).child(error));
        let busy = controls.is_some() || status.is_some() || countdown.is_some() || error.is_some();
        let strip = (readable && busy).then(|| div().flex().flex_col().gap_2().children(controls).children(status).children(countdown)
            .children(error).into_any_element());
        (mic, pill.filter(|_| readable), strip)
    }

    fn dictation_failure(error: &Failure) -> String {
        // "De novo" de um áudio que a retenção já apagou: a frase da tela, não a do backend.
        if error.code.as_deref() == Some("erro_upload_inexistente") { return tr("dictation_audio_gone"); }
        match error.status {
            Some(503) => tr("dictation_unconfigured"),
            Some(401 | 403 | 429) => Self::failure(error),
            Some(_) => error.detail.clone(),
            None if error.uncertain => tr("connection_failed"),
            None => tr(&error.detail),
        }
    }
}

fn style_label(style: &str) -> String {
    tr(match style { "limpar" => "voice_style_limpar", "briefing" => "voice_style_briefing", "cru" => "dictation_style_raw", _ => "voice_style_prosa" })
}

fn dictation_insert(value: &str, range: std::ops::Range<usize>, text: &str) -> String {
    let leading = value[..range.start].chars().next_back().is_some_and(|c| !c.is_whitespace());
    let trailing = value[range.end..].chars().next().is_some_and(|c| !c.is_whitespace());
    format!("{}{text}{}", if leading { " " } else { "" }, if trailing { " " } else { "" })
}

#[cfg(test)]
mod tests {
    #[test]
    fn leaving_and_returning_cannot_restore_pending_send() {
        let mut dictation = super::Dictation::default();
        let owner = Some((1, "servidor".into(), "sessao".into()));
        dictation.observe_file_owner(owner.clone());
        dictation.prepare_send(owner.clone(), "texto".into(), vec![], false, false, true);
        dictation.observe_file_owner(Some((1, "servidor".into(), "outra".into())));
        dictation.observe_file_owner(owner.clone());
        assert!(dictation.complete_send(&owner, "texto", &[]).is_none());
    }

    #[test]
    fn group_change_cancels_send_even_when_restored_before_result() {
        let mut dictation = super::Dictation::default();
        let owner = Some((1, "servidor".into(), "sessao".into()));
        dictation.prepare_send(owner.clone(), "texto".into(), vec![], false, false, true);
        dictation.pending_send.as_mut().unwrap().recipients = Some((false, vec!["par".into()]));
        dictation.observe_send_recipients(&Some((true, vec!["par".into()])));
        dictation.observe_send_recipients(&Some((false, vec!["par".into()])));
        assert!(dictation.complete_send(&owner, "texto", &[]).is_none());
    }
    #[test]
    fn every_content_stops_recording_before_send() {
        for (draft, attachments) in [("", vec![]), ("texto", vec![]), ("", vec!["imagem".to_owned()]), ("", vec!["arquivo".to_owned()])] {
            let mut dictation = super::Dictation::default();
            let owner = Some((1, "servidor".into(), "sessao".into()));
            assert_eq!(dictation.prepare_send(owner, draft.into(), attachments, false, true, false), super::SendPreparation::Stop);
        }
    }

    #[test]
    fn send_intent_is_consumed_once_and_preserves_the_first_click() {
        let mut dictation = super::Dictation::default();
        let owner = Some((1, "servidor".into(), "sessao".into()));
        dictation.prepare_send(owner.clone(), "texto".into(), vec!["imagem".into()], false, true, false);
        assert_eq!(dictation.prepare_send(owner.clone(), "outro".into(), vec![], true, false, true), super::SendPreparation::Wait);
        let intent = dictation.complete_send(&owner, "texto", &["imagem".into()]).unwrap();
        assert!(!intent.steer);
        assert!(dictation.complete_send(&owner, "texto", &["imagem".into()]).is_none());
    }

    #[test]
    fn changed_destination_or_content_cancels_pending_send() {
        for change in 0..3 {
            let mut dictation = super::Dictation::default();
            let owner = Some((1, "servidor".into(), "sessao".into()));
            dictation.prepare_send(owner.clone(), "texto".into(), vec!["imagem".into()], false, true, false);
            let current = if change == 0 { Some((1, "servidor".into(), "outra".into())) } else { owner };
            let draft = if change == 1 { "editado" } else { "texto" };
            let attachments = if change == 2 { vec![] } else { vec!["imagem".into()] };
            assert!(dictation.complete_send(&current, draft, &attachments).is_none());
            assert!(dictation.pending_send.is_none());
        }
    }
    use super::{dictation_append, dictation_insert, place, wav, wav_pcm, Dictation, DictationTarget, Downmix, OnLeave, Place, Recorder, Vad};
    use crate::{api::{Api, dto::SessionInfo}, delivery::SessionKey};
    use std::time::{Duration, Instant};
    #[test]
    fn downmix_turns_48k_stereo_into_16k_mono() {
        let mut mix = Downmix::new(2, 48_000);
        let mut out = Vec::new();
        let frames: Vec<f32> = (0..480).flat_map(|_| [0.5, -0.5]).chain((0..480).flat_map(|_| [1., 1.])).collect();
        mix.push(&frames, |v: f32| v, &mut out);
        let samples: Vec<i16> = out.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(samples.len(), 320, "10 ms a 48 kHz viram 160 amostras a 16 kHz");
        assert_eq!(samples[0], 0, "canais opostos se anulam");
        assert_eq!(samples[319], 32767);
    }
    #[test]
    fn hands_free_waits_for_speech_and_two_seconds_of_silence() {
        let base = Instant::now();
        let mut vad = Vad::default();
        for tick in 0..40 { assert!(!vad.step(0., base + Duration::from_millis(tick * 55))); }
        for tick in 40..70 { assert!(!vad.step(0.3, base + Duration::from_millis(tick * 55))); }
        for tick in 70..107 { assert!(!vad.step(0., base + Duration::from_millis(tick * 55))); }
        assert!(vad.step(0., base + Duration::from_millis(107 * 55)));
    }
    #[test]
    fn versions_preserve_surroundings_and_cancel_releases_audio_without_changing_style() {
        let mut state = Dictation::default();
        assert_eq!(state.style(1), None);
        state.style = Some((1, "limpar"));
        assert_eq!(state.style(2), None);
        state.owner = Some((1, "m".into(), "s".into()));
        let mut draft = "antes ação depois".to_owned();
        state.inserted = Some((draft.clone(), 6..12));
        assert!(!state.text_in_field(""));
        assert!(state.text_in_field(&draft));
        assert!(state.text_in_field("antes ação depois com acréscimo"));
        assert!(!state.text_in_field("antes edição depois"));
        assert!(state.draft_matches(&draft));
        assert!(!state.draft_matches("antes edição depois"));
        let range = state.inserted.as_ref().unwrap().1.clone();
        let replacement = dictation_insert(&draft, range.clone(), "reorganização");
        draft.replace_range(range, &replacement);
        assert_eq!(draft, "antes reorganização depois");
        state.versions.insert("cru".into(), serde_json::json!({"text": "ação"}));
        *state.audio.lock().unwrap() = vec![1, 2];
        let in_flight_audio = state.audio.clone();
        let seq = state.seq;
        state.cancel();
        in_flight_audio.lock().unwrap().push(3);
        assert!(state.audio.lock().unwrap().is_empty());
        assert!(state.versions.is_empty() && state.inserted.is_none() && state.owner.is_none());
        assert_ne!(state.seq, seq);
        assert_eq!(state.style(1), Some("limpar"));
    }
    #[test]
    fn attached_audio_keeps_original_filename_and_bytes_until_cancel() {
        let mut state = Dictation::default();
        state.owner = Some((1, "machine".into(), "session".into()));
        state.file_name = Some("ação gravada.M4A".into());
        let bytes = b"\0\0\0\x18ftypM4A ".to_vec();
        *state.audio.lock().unwrap() = bytes.clone();
        state.error = Some("failed".into());
        assert_eq!(state.file_name.as_deref(), Some("ação gravada.M4A"));
        assert_eq!(*state.audio.lock().unwrap(), bytes);
        assert!(!state.auto_send && state.result.is_none() && state.versions.is_empty());
        let old_seq = state.seq;
        let old_audio = state.audio.clone();
        state.cancel();
        assert!(state.file_name.is_none() && state.owner.is_none() && state.error.is_none());
        assert!(state.audio.lock().unwrap().is_empty());
        assert_eq!(*old_audio.lock().unwrap(), bytes);
        assert_ne!(state.seq, old_seq);
    }
    #[test]
    fn audio_file_generation_changes_even_when_returning_without_active_dictation() {
        let mut state = Dictation::default();
        let original = Some((1, "machine".into(), "session-a".into()));
        let generation = state.observe_file_owner(original.clone());
        assert_eq!(state.observe_file_owner(original.clone()), generation);
        state.observe_file_owner(Some((1, "machine".into(), "session-b".into())));
        assert_ne!(state.observe_file_owner(original.clone()), generation);
        assert!(state.owner.is_none() && state.recorder.is_none() && state.request.is_none());
        let generation = state.observe_file_owner(Some((1, "machine-a".into(), String::new())));
        state.observe_file_owner(Some((1, "machine-b".into(), String::new())));
        assert_ne!(state.observe_file_owner(Some((1, "machine-a".into(), String::new()))), generation);
    }
    #[test]
    fn dictation_preserves_draft_around_cursor_or_selection_and_cancels_old_result() {
        for (value, range, expected) in [
            ("depois", 0..0, "fala depois"), ("antes", 5..5, "antes fala"),
            ("antes depois", 6..6, "antes fala depois"), ("trocar isto", 0..6, "fala isto"),
            ("ação fim", 0..6, "fala fim"), ("tudo", 0..4, "fala"),
            ("", 0..0, "fala"), ("antes\n", 6..6, "antes\nfala"),
        ] {
            let mut result = value.to_owned();
            result.replace_range(range.clone(), &dictation_insert(value, range, "fala"));
            assert_eq!(result, expected);
        }
        let mut state = Dictation::default();
        state.owner = Some((1, "m".into(), "s".into()));
        let old = state.seq;
        state.cancel();
        assert_ne!(state.seq, old);
        assert!(state.owner.is_none());
        let mut pcm = vec![0; 3203];
        pcm[3201] = 128;
        let mut recorder = Recorder { stream: None, failed: Default::default(), pcm: std::sync::Arc::new(std::sync::Mutex::new(pcm)),
            playback: None, sampled: 0, last_signal: (0., 0.), last_pcm_at: None };
        assert_eq!(recorder.signal().0, 1.);
        recorder.last_pcm_at = Some(Instant::now() - Duration::from_millis(200));
        assert_eq!(recorder.signal(), (0., 0.));
    }
    #[test]
    fn wav_roundtrip_rejects_truncation_and_wrong_format() {
        let pcm = [0, 0, 0xff, 0x7f, 0, 0x80];
        let mut bytes = wav(&pcm);
        assert_eq!(wav_pcm(&bytes), Some(pcm.as_slice()));
        assert!(wav_pcm(&bytes[..bytes.len() - 1]).is_none());
        bytes[22] = 2;
        assert!(wav_pcm(&bytes).is_none());
    }

    fn target(name: &str, life: &str) -> DictationTarget {
        DictationTarget { key: SessionKey { server: "http://m/".into(), name: name.into(), jsonl: "/a.jsonl".into() },
            life: Some(life.into()), api: Api::new("http://m/", "t").unwrap() }
    }

    fn session(name: &str, life: &str, jsonl: &str) -> SessionInfo {
        SessionInfo { name: name.into(), lifecycle_id: Some(life.into()), jsonl: Some(jsonl.into()), ..Default::default() }
    }

    #[test]
    fn leaving_stops_the_recording_for_the_origin_and_never_cancels_its_request() {
        let mut state = Dictation::default();
        let origin = Some((1, "http://m/".to_owned(), "x".to_owned()));
        let other = Some((1, "http://m/".to_owned(), "y".to_owned()));
        state.owner = origin.clone();
        state.target = Some(target("x", "k:1"));
        state.recorder = Some(Recorder { stream: None, failed: Default::default(), pcm: Default::default(), playback: None,
            sampled: 0, last_signal: (0., 0.), last_pcm_at: None });
        assert_eq!(state.on_leave(&origin, 1), OnLeave::Keep, "na própria sessão nada muda");
        assert_eq!(state.on_leave(&other, 1), OnLeave::Stop, "gravando: para e transcreve para a origem");
        state.recorder = None;
        assert_eq!(state.on_leave(&other, 1), OnLeave::Keep, "transcrevendo: o pedido segue");
        assert_eq!(state.on_leave(&other, 2), OnLeave::Cancel, "outra conexão descartaria a resposta");
        state.target = None;
        assert_eq!(state.on_leave(&other, 1), OnLeave::Cancel, "tela sem sessão cancela como antes");
        state.target = Some(target("x", "k:1"));
        state.result_error = Some("falhou".into());
        state.cancel();
        assert!(state.target.is_none() && state.owner.is_none() && state.result_error.is_none());
    }

    #[test]
    fn off_screen_result_goes_to_the_end_of_that_sessions_draft() {
        assert_eq!(dictation_append("", "fala"), ("fala".to_owned(), 0..4));
        assert_eq!(dictation_append("antes", "fala"), ("antes fala".to_owned(), 6..10));
        assert_eq!(dictation_append("antes\n", "fala"), ("antes\nfala".to_owned(), 6..10));
        let (draft, range) = dictation_append("ação", "fala");
        assert_eq!(&draft[range.clone()], "fala", "o trecho é o do texto, em bytes");
        // A versão escolhida depois troca só o trecho, sem espaço a mais.
        let mut revised = draft.clone();
        revised.replace_range(range.clone(), &dictation_insert(&draft, range, "fala limpa"));
        assert_eq!(revised, "ação fala limpa");
        let x = target("x", "k:1");
        let list = [session("x", "k:1", "/a.jsonl"), session("y", "k:2", "/y.jsonl")];
        assert_eq!(place(&x, Some(&list[1]), Some(&list)), Place::Away(x.key.clone()));
        assert_eq!(place(&x, Some(&list[0]), Some(&list)), Place::Open);
    }

    #[test]
    fn clear_keeps_the_target_but_recreated_or_closed_is_gone() {
        let x = target("x", "k:1");
        let cleared = [session("x", "k:1", "/b.jsonl")];
        let key = SessionKey { jsonl: "/b.jsonl".into(), ..x.key.clone() };
        assert_eq!(place(&x, None, Some(&cleared)), Place::Away(key), "/clear: mesma sessão, chave do transcript novo");
        assert_eq!(place(&x, Some(&cleared[0]), Some(&cleared)), Place::Open);
        let recreated = [session("x", "k:2", "/c.jsonl")];
        assert_eq!(place(&x, Some(&recreated[0]), Some(&recreated)), Place::Gone, "mesmo nome, outra sessão");
        assert_eq!(place(&x, None, Some(&[])), Place::Gone, "fechada");
    }

    #[test]
    fn unknown_or_failed_list_never_drops_the_text() {
        let x = target("x", "k:1");
        // Lista não lida, com erro, peer desligado ou convite encerrado: `trusted_sessions` devolve `None`.
        assert_eq!(place(&x, None, None), Place::Away(x.key.clone()), "o texto vai para o rascunho da chave guardada");
        let other = session("y", "k:2", "/y.jsonl");
        assert_eq!(place(&x, Some(&other), None), Place::Away(x.key.clone()));
        assert_eq!(place(&x, Some(&session("x", "k:1", "/a.jsonl")), None), Place::Open, "aberta é aberta mesmo sem lista");
    }

    #[test]
    fn switching_session_clears_the_screen_error_only_on_the_switch() {
        let mut state = Dictation::default();
        let origin = Some((1, "http://m/".to_owned(), "x".to_owned()));
        let other = Some((1, "http://m/".to_owned(), "y".to_owned()));
        state.owner = origin.clone();
        state.target = Some(target("x", "k:1"));
        state.observe_file_owner(origin.clone());
        state.error = Some("envio automático falhou".into());
        state.result_error = Some("rascunho mudou".into());
        state.observe_file_owner(other.clone());
        assert!(state.error.is_none(), "o erro de X não aparece em Y");
        assert!(state.result_error.is_some(), "o do resultado fica para a volta a X");
        state.error = Some("estilo".into());
        state.observe_file_owner(other.clone());
        assert_eq!(state.error.as_deref(), Some("estilo"), "erro do estilo em Y continua visível");
    }

    #[test]
    fn saved_audio_name_is_the_last_part_of_the_server_path_until_cancel() {
        let mut state = Dictation::default();
        assert_eq!(state.server_file(), None);
        state.server_path = Some("/home/u/.hangar/uploads/p-1a/s1/ditado-3.wav".into());
        assert_eq!(state.server_file(), Some("ditado-3.wav"));
        state.server_path = Some(r"C:\Users\u\.hangar\uploads\p\s\ditado.wav".into());
        assert_eq!(state.server_file(), Some("ditado.wav"));
        state.server_path = Some("ditado-4.wav".into());
        assert_eq!(state.server_file(), Some("ditado-4.wav"), "a lista de recentes guarda só o nome");
        state.cancel();
        assert_eq!(state.server_path, None);
    }

    #[test]
    fn retry_sends_the_name_or_after_clear_the_whole_path() {
        let mut state = Dictation::default();
        state.server_path = Some("/home/u/.hangar/uploads/p-1a/s1/ditado-3.wav".into());
        assert_eq!(state.saved_for_retry(true), None, "sem sessão de destino não há anexos");
        state.target = Some(target("x", "k:1"));
        assert_eq!(state.saved_for_retry(true).as_deref(), Some("ditado-3.wav"));
        assert_eq!(state.saved_for_retry(false).as_deref(), Some("/home/u/.hangar/uploads/p-1a/s1/ditado-3.wav"),
            "depois de /clear a pasta é outra: vai o caminho inteiro");
        state.server_path = None;
        assert_eq!(state.saved_for_retry(true), None, "upload que falhou: o de novo sobe a cópia em memória");
    }

    #[test]
    fn dictation_snapshot_keeps_an_unselected_model_and_the_original_destination() {
        let first=super::OrganizationSelection::read(&serde_json::json!({"campos":{
            "dictation_organization_mode":{"valor":"harness"},
            "dictation_claude_model":{"valor":""},
            "dictation_include_recent_messages":{"valor":true}}}));
        let mut destination=session("destination","k:first","first.jsonl");
        destination.provider="claude".into();destination.conta=Some("first-account".into());
        let mut dictation=Dictation::default();dictation.snapshot=Some(first.options(Some(&destination)));
        let later=super::OrganizationSelection::read(&serde_json::json!({"campos":{"dictation_organization_mode":{"valor":"none"}}}));
        destination.lifecycle_id=Some("k:second".into());destination.conta=Some("second-account".into());
        let snapshot=dictation.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.model.as_deref(),Some(""),"nenhum modelo é uma escolha congelada, não herança futura");
        assert_eq!(snapshot.generation.as_deref(),Some("k:first"));assert_eq!(snapshot.account.as_deref(),Some("first-account"));
        assert!(snapshot.include_recent_messages);assert_eq!(snapshot.mode,"harness");
        assert_eq!(later.options(Some(&destination)).mode,"none");
        dictation.cancel();assert!(dictation.snapshot.is_none());
    }

    #[test]
    fn raw_version_preserves_the_spelling_references_for_the_next_revision() {
        let references=serde_json::json!([{"role":"user","text":"O projeto usa PostgreSQL"}]);
        let value=serde_json::json!({"text":"Texto organizado.","raw":"texto original","estilo_aplicado":"prosa",
            "organization_mode":"harness","recent_messages":references});
        let mut dictation=Dictation::default();dictation.remember_versions(&value);
        let raw=dictation.versions["cru"].clone();dictation.result=Some(raw.clone());
        assert_eq!(raw["text"],"texto original");
        assert_eq!(raw["recent_messages"],references,"Cru não pode obrigar a revisão seguinte a reler a conversa");
        assert_eq!(dictation.result.as_ref().unwrap()["recent_messages"],references);
    }

    #[test]
    fn retry_of_an_audio_retention_removed_says_it_in_the_apps_words() {
        let gone = crate::api::Failure { status: Some(404), detail: "o áudio não está mais na pasta da sessão".into(),
            retry_after: None, uncertain: false, code: Some("erro_upload_inexistente".into()) };
        assert_eq!(super::Hangar::dictation_failure(&gone), crate::i18n::tr("dictation_audio_gone"));
        let other = crate::api::Failure { code: None, detail: "502: fora".into(), status: Some(502), ..gone };
        assert_eq!(super::Hangar::dictation_failure(&other), "502: fora");
    }

    #[test]
    fn saved_audio_back_to_dictation_refuses_loading_busy_or_without_session() {
        use crate::i18n::{tr, tr_shared};
        let mut state = Dictation::default();
        let ok = state.saved_audio_target(true, Some(target("x", "k:1")), "ditado-3.wav");
        assert_eq!(ok.map(|t| t.key.name).as_deref(), Ok("x"));
        assert_eq!(state.saved_audio_target(true, None, "ditado-3.wav").err(), Some(tr("attach_audio_session_changed")));
        state.recorder = Some(Recorder { stream: None, failed: Default::default(), pcm: Default::default(), playback: None,
            sampled: 0, last_signal: (0., 0.), last_pcm_at: None });
        assert_eq!(state.saved_audio_target(true, Some(target("x", "k:1")), "ditado-3.wav").err(),
            Some(tr_shared("composer_aguarde_transcricao", &[])), "gravando: o ditado em curso não é trocado");
        assert_eq!(state.saved_audio_target(false, Some(target("x", "k:1")), "ditado-3.wav").err(),
            Some(tr("attach_audio_not_ready").replace("{name}", "ditado-3.wav")), "conversa carregando vem primeiro");
    }
}

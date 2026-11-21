use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use crate::error::VideoError;
use crate::ffi::{self, RenderParam};
use crate::frame::{Buffer, FrameRef, render_size};
use crate::library::Library;
use crate::text::header_fields;

/// Ordem dos bytes de cada pixel no buffer. O quarto byte sai sempre 255.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat { Rgba, Bgra }

impl PixelFormat {
    fn mpv(self) -> &'static CStr { match self { PixelFormat::Rgba => c"rgb0", PixelFormat::Bgra => c"bgr0" } }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub format: PixelFormat,
    /// Vão em todo pedido HTTP do vídeo, inclusive os de faixa (Range) que a busca faz.
    pub headers: Vec<(String, String)>,
}

/// O que mudou desde o último aviso: a tela redesenha nos dois casos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change { State, Frame }

/// Estado lido das propriedades observadas da libmpv.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// O arquivo abriu (`FILE_LOADED`).
    pub loaded: bool,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub paused: bool,
    /// 0 a 100.
    pub volume: f64,
    pub muted: bool,
    /// Chegou ao fim e parou no último quadro (`keep-open`).
    pub ended: bool,
    /// Parado esperando a rede (`paused-for-cache`).
    pub buffering: bool,
    /// Até onde o vídeo já chegou, em segundos do vídeo (`demuxer-cache-time`).
    pub cached_until: Option<f64>,
    pub video_size: Option<(u32, u32)>,
    /// `dwidth` e `dheight` como chegaram, um evento cada.
    dims: (u32, u32),
    /// O arquivo não abriu ou a reprodução caiu; o texto é o da libmpv.
    pub error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot { loaded: false, position: None, duration: None, paused: false, volume: 100., muted: false, ended: false,
            buffering: false, cached_until: None, video_size: None, dims: (0, 0), error: None }
    }
}

/// Opções de antes do `mpv_initialize`. As `true` são obrigatórias; as outras só existem em parte das versões e
/// builds (sem Lua não há `osc` nem `ytdl`), e a falta delas não impede tocar.
const OPTIONS: [(&str, &str, bool); 18] = [
    ("vo", "libmpv", true),
    ("keep-open", "yes", true),
    // O cabeçalho do dono vai por https a outra máquina: sem verificar o certificado, quem estivesse no caminho o leria.
    ("tls-verify", "yes", true),
    ("idle", "yes", false),
    ("hwdec", "auto-safe", false),
    ("config", "no", false),
    ("load-scripts", "no", false),
    // Sem isso uma URL http passaria pelo yt-dlp do sistema antes de abrir.
    ("ytdl", "no", false),
    ("osc", "no", false),
    ("input-default-bindings", "no", false),
    ("input-vo-keyboard", "no", false),
    ("terminal", "no", false),
    ("sub-auto", "no", false),
    ("audio-file-auto", "no", false),
    ("cover-art-auto", "no", false),
    ("network-timeout", "30", false),
    ("user-agent", "Hangar", false),
    ("audio-client-name", "Hangar", false),
];

/// Propriedades observadas; o índice + 1 é o `reply_userdata` do evento.
const OBSERVED: [(&CStr, c_int); 10] = [
    (c"time-pos", ffi::FORMAT_DOUBLE),
    (c"duration", ffi::FORMAT_DOUBLE),
    (c"pause", ffi::FORMAT_FLAG),
    (c"volume", ffi::FORMAT_DOUBLE),
    (c"mute", ffi::FORMAT_FLAG),
    (c"eof-reached", ffi::FORMAT_FLAG),
    (c"paused-for-cache", ffi::FORMAT_FLAG),
    (c"demuxer-cache-time", ffi::FORMAT_DOUBLE),
    (c"dwidth", ffi::FORMAT_INT64),
    (c"dheight", ffi::FORMAT_INT64),
];

/// Antes de a tela dizer o tamanho dela, o quadro sai no máximo neste.
const FIRST_TARGET: (u32, u32) = (1920, 1080);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(PoisonError::into_inner) }

/// As funções do `mpv_handle` podem ser chamadas de qualquer thread.
struct Handle(*mut ffi::Handle);
// SAFETY: o cliente da libmpv é seguro entre threads (client.h); só `mpv_terminate_destroy` exige exclusividade, e ele
// roda depois do `join` das threads que usam o handle.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

/// O contexto de render só é usado pela thread de render e liberado depois do `join` dela.
struct Render(*mut ffi::RenderContext);
// SAFETY: ver acima; nenhuma chamada de render acontece em duas threads ao mesmo tempo.
unsafe impl Send for Render {}
unsafe impl Sync for Render {}

struct Signal { pending: bool, force: bool, closed: bool, target: (u32, u32) }

/// O aviso da libmpv de que há quadro novo: só marca e acorda, porque dentro dele nenhuma função da libmpv pode ser
/// chamada.
struct Wake { signal: Mutex<Signal>, cond: Condvar }

impl Wake {
    fn poke(&self, force: bool) {
        let mut signal = lock(&self.signal);
        signal.pending = true;
        signal.force |= force;
        drop(signal);
        self.cond.notify_one();
    }

    fn close(&self) {
        lock(&self.signal).closed = true;
        self.cond.notify_one();
    }
}

unsafe extern "C" fn on_update(ctx: *mut c_void) {
    // SAFETY: `ctx` é o `Arc<Wake>` do `Inner`, vivo até depois de o callback ser tirado e o contexto liberado.
    let wake = unsafe { &*ctx.cast::<Wake>() };
    wake.poke(false);
}

struct Slot { buffer: Buffer, seq: u64 }

struct Inner {
    lib: Arc<Library>,
    handle: Handle,
    render: Render,
    wake: Arc<Wake>,
    state: Mutex<Snapshot>,
    slot: Mutex<Slot>,
    closing: AtomicBool,
    format: PixelFormat,
    notify: Box<dyn Fn(Change) + Send + Sync>,
}

/// Um vídeo aberto. Soltar fecha a libmpv numa thread própria: o fim espera o núcleo dela parar.
pub struct Player {
    inner: Arc<Inner>,
    render_thread: Option<JoinHandle<()>>,
    event_thread: Option<JoinHandle<()>>,
}

impl Player {
    /// Bloqueante (carrega a biblioteca e inicia o núcleo): fora da thread da interface. `source` é um caminho local
    /// ou uma URL http(s); `notify` é chamado de outras threads a cada mudança.
    pub fn open(source: &str, options: &Options, notify: impl Fn(Change) + Send + Sync + 'static) -> Result<Player, VideoError> {
        let headers = header_fields(&options.headers)?;
        let source = CString::new(source).map_err(|_| VideoError::Init("source with NUL".into()))?;
        let lib = Library::shared()?;
        let api = &lib.api;
        c_numeric_locale();
        // SAFETY: sem argumentos; nulo é a falha documentada.
        let raw = unsafe { (api.create)() };
        if raw.is_null() { return Err(VideoError::Init("mpv_create failed (LC_NUMERIC must be \"C\")".into())); }
        let render = match configure(api, raw, &headers) {
            Ok(render) => render,
            Err(why) => {
                // SAFETY: handle criado acima e ainda sem nenhuma outra thread.
                unsafe { (api.terminate_destroy)(raw) };
                return Err(VideoError::Init(why));
            }
        };
        let wake = Arc::new(Wake { signal: Mutex::new(Signal { pending: true, force: false, closed: false, target: FIRST_TARGET }), cond: Condvar::new() });
        let inner = Arc::new(Inner {
            lib: lib.clone(), handle: Handle(raw), render: Render(render), wake: wake.clone(), state: Mutex::new(Snapshot::default()),
            slot: Mutex::new(Slot { buffer: Buffer::new(), seq: 0 }), closing: AtomicBool::new(false), format: options.format,
            notify: Box::new(notify),
        });
        // SAFETY: contexto vivo; `wake` vive no `Inner`, que só sai depois de o callback ser retirado no `shutdown`.
        unsafe { (api.render_context_set_update_callback)(render, Some(on_update), Arc::as_ptr(&wake).cast_mut().cast()) };
        let mut player = Player { inner: inner.clone(), render_thread: None, event_thread: None };
        let spawn = |name: &str, run: fn(Arc<Inner>)| {
            let inner = inner.clone();
            std::thread::Builder::new().name(name.into()).spawn(move || run(inner))
                .map_err(|e| VideoError::Init(format!("no thread for {name}: {e}")))
        };
        player.render_thread = Some(spawn("mpv-render", run_render)?);
        player.event_thread = Some(spawn("mpv-events", run_events)?);
        player.command_strings(&[c"loadfile", source.as_c_str()]);
        Ok(player)
    }

    pub fn snapshot(&self) -> Snapshot { lock(&self.inner.state).clone() }

    /// Número do último quadro pronto; 0 antes do primeiro.
    pub fn frame_seq(&self) -> u64 { lock(&self.inner.slot).seq }

    /// O último quadro, se for mais novo que `newer_than`. O render seguinte espera `f` terminar: só copiar aqui.
    pub fn with_frame<R>(&self, newer_than: u64, f: impl FnOnce(FrameRef<'_>) -> R) -> Option<R> {
        let slot = lock(&self.inner.slot);
        if slot.seq <= newer_than || slot.buffer.width == 0 { return None; }
        Some(f(FrameRef { width: slot.buffer.width, height: slot.buffer.height, stride: slot.buffer.stride(), pixels: slot.buffer.bytes(), seq: slot.seq }))
    }

    /// Área do vídeo na tela, em pixels físicos: o quadro sai nesse tamanho (sem passar do vídeo). Mudou com o vídeo
    /// parado, o quadro atual é redesenhado.
    pub fn set_target(&self, width: u32, height: u32) {
        let target = (width.max(1), height.max(1));
        let mut signal = lock(&self.inner.wake.signal);
        if signal.target == target { return; }
        signal.target = target;
        drop(signal);
        self.inner.wake.poke(true);
    }

    /// Tocar; no fim, volta ao começo primeiro (com `keep-open`, despausar no último quadro não faz nada).
    pub fn play(&self) {
        if lock(&self.inner.state).ended { self.seek_to(0.); }
        self.set_paused(false);
    }

    pub fn toggle(&self) {
        let state = self.snapshot();
        if state.paused || state.ended { self.play() } else { self.set_paused(true) }
    }

    pub fn set_paused(&self, paused: bool) { self.command(&["set", "pause", if paused { "yes" } else { "no" }]); }

    pub fn seek_to(&self, seconds: f64) {
        if !seconds.is_finite() { return; }
        self.command(&["seek", &format!("{:.3}", seconds.max(0.)), "absolute"]);
    }

    pub fn seek_by(&self, seconds: f64) {
        if !seconds.is_finite() { return; }
        self.command(&["seek", &format!("{seconds:.3}"), "relative"]);
    }

    pub fn set_volume(&self, volume: f64) {
        if !volume.is_finite() { return; }
        self.command(&["set", "volume", &format!("{:.0}", volume.clamp(0., 100.))]);
    }

    pub fn set_muted(&self, muted: bool) { self.command(&["set", "mute", if muted { "yes" } else { "no" }]); }

    pub fn stop(&self) { self.command(&["stop"]); }

    fn command(&self, args: &[&str]) {
        let Ok(args) = args.iter().map(|arg| CString::new(*arg)).collect::<Result<Vec<_>, _>>() else { return };
        let refs: Vec<&CStr> = args.iter().map(CString::as_c_str).collect();
        self.command_strings(&refs);
    }

    /// Assíncrono: a libmpv copia os argumentos e responde depois, sem travar quem chamou. A recusa imediata (comando
    /// mal formado) vai ao estado como erro.
    fn command_strings(&self, args: &[&CStr]) {
        let mut pointers: Vec<*const c_char> = args.iter().map(|arg| arg.as_ptr()).collect();
        pointers.push(std::ptr::null());
        let api = &self.inner.lib.api;
        // SAFETY: lista terminada em nulo de strings vivas durante a chamada; o handle vive até o `shutdown`.
        let code = unsafe { (api.command_async)(self.inner.handle.0, 0, pointers.as_mut_ptr()) };
        if code < 0 {
            lock(&self.inner.state).error = Some(error_text(api, code));
            (self.inner.notify)(Change::State);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let parts = Arc::new(Mutex::new(Some((self.inner.clone(), self.render_thread.take(), self.event_thread.take()))));
        let job = parts.clone();
        let spawned = std::thread::Builder::new().name("mpv-shutdown".into())
            .spawn(move || if let Some((inner, render, events)) = lock(&job).take() { shutdown(inner, render, events) });
        // Sem thread nova, fecha aqui mesmo: deixar a libmpv viva tocaria o áudio sem tela.
        if spawned.is_err() && let Some((inner, render, events)) = lock(&parts).take() { shutdown(inner, render, events); }
    }
}

/// Ordem exigida pela libmpv: as threads param, o contexto de render sai e só então o núcleo é destruído.
fn shutdown(inner: Arc<Inner>, render: Option<JoinHandle<()>>, events: Option<JoinHandle<()>>) {
    let api = &inner.lib.api;
    inner.wake.close();
    if let Some(thread) = render { let _ = thread.join(); }
    inner.closing.store(true, Ordering::Release);
    // SAFETY: handle vivo; acorda o `mpv_wait_event` (ou faz o próximo voltar na hora).
    unsafe { (api.wakeup)(inner.handle.0) };
    if let Some(thread) = events { let _ = thread.join(); }
    // SAFETY: nenhuma outra thread usa mais o handle nem o contexto.
    unsafe {
        (api.render_context_set_update_callback)(inner.render.0, None, std::ptr::null_mut());
        (api.render_context_free)(inner.render.0);
        (api.terminate_destroy)(inner.handle.0);
    }
}

fn configure(api: &ffi::Api, raw: *mut ffi::Handle, headers: &[String]) -> Result<*mut ffi::RenderContext, String> {
    let set = |name: &str, value: &str| -> Result<(), String> {
        let (cname, cvalue) = (CString::new(name).map_err(|_| name.to_owned())?, CString::new(value).map_err(|_| name.to_owned())?);
        // SAFETY: handle criado e ainda não iniciado; strings vivas durante a chamada.
        let code = unsafe { (api.set_option_string)(raw, cname.as_ptr(), cvalue.as_ptr()) };
        if code < 0 { Err(format!("option {name}: {}", error_text(api, code))) } else { Ok(()) }
    };
    for (name, value, required) in OPTIONS {
        if let Err(why) = set(name, value) && required { return Err(why); }
    }
    for header in headers { set("http-header-fields-append", header)?; }
    // SAFETY: handle configurado.
    let code = unsafe { (api.initialize)(raw) };
    if code < 0 { return Err(format!("mpv_initialize: {}", error_text(api, code))); }
    for (index, (name, format)) in OBSERVED.iter().enumerate() {
        // SAFETY: nome vivo, formato válido; a falha só deixaria a propriedade sem valor.
        let _ = unsafe { (api.observe_property)(raw, index as u64 + 1, name.as_ptr(), *format) };
    }
    let mut params = [
        RenderParam { kind: ffi::RENDER_PARAM_API_TYPE, data: ffi::RENDER_API_TYPE_SW.as_ptr().cast_mut().cast() },
        RenderParam { kind: ffi::RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
    ];
    let mut render = std::ptr::null_mut();
    // SAFETY: lista terminada em `INVALID`; o tipo "sw" não pede outros parâmetros na criação.
    let code = unsafe { (api.render_context_create)(&mut render, raw, params.as_mut_ptr()) };
    if code < 0 || render.is_null() { return Err(format!("mpv_render_context_create: {}", error_text(api, code))); }
    Ok(render)
}

fn run_render(inner: Arc<Inner>) {
    let api = &inner.lib.api;
    let mut back = Buffer::new();
    let mut failed = false;
    loop {
        let (force, target) = {
            let mut signal = lock(&inner.wake.signal);
            while !signal.pending && !signal.closed { signal = inner.wake.cond.wait(signal).unwrap_or_else(PoisonError::into_inner); }
            if signal.closed { return; }
            signal.pending = false;
            (std::mem::take(&mut signal.force), signal.target)
        };
        // SAFETY: contexto vivo até depois do `join` desta thread; só ela o usa.
        let flags = unsafe { (api.render_context_update)(inner.render.0) };
        if flags & ffi::RENDER_UPDATE_FRAME == 0 && !force { continue; }
        let Some((width, height)) = lock(&inner.state).video_size.and_then(|video| render_size(video, target)) else { continue };
        back.resize(width, height);
        let mut size = [width as c_int, height as c_int];
        let mut stride = back.stride();
        let mut params = [
            RenderParam { kind: ffi::RENDER_PARAM_SW_SIZE, data: size.as_mut_ptr().cast() },
            RenderParam { kind: ffi::RENDER_PARAM_SW_FORMAT, data: inner.format.mpv().as_ptr().cast_mut().cast() },
            RenderParam { kind: ffi::RENDER_PARAM_SW_STRIDE, data: (&raw mut stride).cast() },
            RenderParam { kind: ffi::RENDER_PARAM_SW_POINTER, data: back.as_mut_ptr() },
            RenderParam { kind: ffi::RENDER_PARAM_INVALID, data: std::ptr::null_mut() },
        ];
        // SAFETY: `back` tem `stride * height` bytes alinhados a 64; os parâmetros vivem durante a chamada.
        let code = unsafe { (api.render_context_render)(inner.render.0, params.as_mut_ptr()) };
        if code < 0 {
            // Uma vez só: o próximo aviso tentaria de novo e encheria o estado do mesmo erro.
            if !failed {
                failed = true;
                lock(&inner.state).error = Some(format!("render: {}", error_text(api, code)));
                (inner.notify)(Change::State);
            }
            continue;
        }
        failed = false;
        back.make_opaque();
        {
            let mut slot = lock(&inner.slot);
            std::mem::swap(&mut slot.buffer, &mut back);
            slot.seq += 1;
        }
        (inner.notify)(Change::Frame);
    }
}

fn run_events(inner: Arc<Inner>) {
    let api = &inner.lib.api;
    loop {
        // SAFETY: handle vivo até depois do `join` desta thread; só ela espera eventos.
        let event = unsafe { (api.wait_event)(inner.handle.0, -1.) };
        if inner.closing.load(Ordering::Acquire) || event.is_null() { return; }
        // SAFETY: o evento vale até a próxima espera.
        let event = unsafe { &*event };
        match event.event_id {
            ffi::EVENT_NONE => continue,
            ffi::EVENT_SHUTDOWN => return,
            _ => {}
        }
        let (changed, resized) = {
            let mut state = lock(&inner.state);
            let before = state.clone();
            match event.event_id {
                ffi::EVENT_FILE_LOADED => { state.loaded = true; state.error = None; }
                ffi::EVENT_END_FILE if !event.data.is_null() => {
                    // SAFETY: `data` de `END_FILE` é um `mpv_event_end_file`.
                    let end = unsafe { &*event.data.cast::<ffi::EventEndFile>() };
                    if end.reason == ffi::END_FILE_REASON_ERROR { state.error = Some(error_text(api, end.error)); }
                }
                ffi::EVENT_PROPERTY_CHANGE if !event.data.is_null() => {
                    // SAFETY: `data` de `PROPERTY_CHANGE` é um `mpv_event_property`.
                    apply(&mut state, event.reply_userdata, unsafe { &*event.data.cast::<ffi::EventProperty>() });
                }
                _ => {}
            }
            (*state != before, state.video_size != before.video_size)
        };
        // Outro tamanho de vídeo: o próximo quadro já sai na proporção nova.
        if resized { inner.wake.poke(true); }
        if changed { (inner.notify)(Change::State); }
    }
}

fn apply(state: &mut Snapshot, id: u64, prop: &ffi::EventProperty) {
    // SAFETY (das três): o formato confere com o tipo apontado por `data`, como `client.h` define.
    let double = || (prop.format == ffi::FORMAT_DOUBLE && !prop.data.is_null()).then(|| unsafe { *prop.data.cast::<f64>() });
    let flag = || prop.format == ffi::FORMAT_FLAG && !prop.data.is_null() && unsafe { *prop.data.cast::<c_int>() } != 0;
    let int = || (prop.format == ffi::FORMAT_INT64 && !prop.data.is_null()).then(|| unsafe { *prop.data.cast::<i64>() });
    let size = |value: Option<i64>| value.and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0);
    match id {
        1 => state.position = double(),
        2 => state.duration = double().filter(|d| *d > 0.),
        3 => state.paused = flag(),
        4 => state.volume = double().unwrap_or(state.volume),
        5 => state.muted = flag(),
        6 => state.ended = flag(),
        7 => state.buffering = flag(),
        8 => state.cached_until = double(),
        9 => state.dims.0 = size(int()).unwrap_or(0),
        10 => state.dims.1 = size(int()).unwrap_or(0),
        _ => {}
    }
    // Só as duas medidas juntas valem: o vídeo sem uma delas ainda não tem tamanho.
    state.video_size = (state.dims.0 > 0 && state.dims.1 > 0).then_some(state.dims);
}

fn error_text(api: &ffi::Api, code: c_int) -> String {
    // SAFETY: a libmpv devolve uma string estática para qualquer código.
    let text = unsafe { (api.error_string)(code) };
    if text.is_null() { return format!("error {code}"); }
    // SAFETY: não nula e terminada em NUL.
    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
}

/// A libmpv exige o LC_NUMERIC em "C" (lê e escreve números com ponto). O app não troca o locale; uma biblioteca
/// que tenha trocado teria de voltar a ele aqui.
#[cfg(unix)]
fn c_numeric_locale() {
    // SAFETY: consulta e troca só o LC_NUMERIC; o app não formata números pelo locale do C em outra thread.
    unsafe {
        let current = libc::setlocale(libc::LC_NUMERIC, std::ptr::null());
        if current.is_null() || CStr::from_ptr(current).to_bytes() != b"C" { libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr()); }
    }
}

#[cfg(not(unix))]
fn c_numeric_locale() {}

#[cfg(test)]
mod tests {
    use super::{Snapshot, apply};
    use crate::ffi::{self, EventProperty};
    use std::ffi::{c_int, c_void};

    fn prop<T>(format: c_int, value: &mut T) -> EventProperty {
        EventProperty { name: std::ptr::null(), format, data: (value as *mut T).cast::<c_void>() }
    }

    #[test]
    fn video_size_needs_both_measures_and_unset_clears_it() {
        let mut state = Snapshot::default();
        apply(&mut state, 9, &prop(ffi::FORMAT_INT64, &mut 1280_i64));
        assert_eq!(state.video_size, None);
        apply(&mut state, 10, &prop(ffi::FORMAT_INT64, &mut 720_i64));
        assert_eq!(state.video_size, Some((1280, 720)));
        apply(&mut state, 10, &EventProperty { name: std::ptr::null(), format: ffi::FORMAT_NONE, data: std::ptr::null_mut() });
        assert_eq!(state.video_size, None);
    }

    #[test]
    fn flags_and_doubles_follow_their_format() {
        let mut state = Snapshot::default();
        apply(&mut state, 3, &prop(ffi::FORMAT_FLAG, &mut (1 as c_int)));
        apply(&mut state, 1, &prop(ffi::FORMAT_DOUBLE, &mut 12.5_f64));
        apply(&mut state, 2, &prop(ffi::FORMAT_DOUBLE, &mut 0_f64));
        assert!(state.paused);
        assert_eq!(state.position, Some(12.5));
        assert_eq!(state.duration, None, "duração zero é desconhecida");
        // Formato trocado não é lido como outro tipo.
        apply(&mut state, 1, &prop(ffi::FORMAT_FLAG, &mut (1 as c_int)));
        assert_eq!(state.position, None);
    }
}

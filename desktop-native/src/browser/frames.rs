//! Quadros do screencast da página da conversa no Windows. Uma thread decodifica o PNG; a textura do device da GPUI é
//! escrita na thread da interface (o contexto imediato do D3D11 é dela) e pintada com `paint_surface`.
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use base64::Engine as _;
use gpui_windows::SurfaceTexture;
use serde_json::Value;

use super::Event;

/// Confirma um quadro ao WebView2. Ele só manda o próximo com poucos sem confirmação, então confirmar depois de desenhar
/// faz a página produzir quadros no ritmo da tela, e nenhum quando a janela não desenha.
pub type Acker = Box<dyn Fn(Value) + Send + Sync>;

pub struct Frames {
    /// Textura pintada; trocada só quando o tamanho ou o device mudam.
    shown: Mutex<Option<Arc<SurfaceTexture>>>,
    slot: Mutex<Slot>,
    wake: Condvar,
    events: async_channel::Sender<Event>,
    ack: Acker,
}

#[derive(Default)]
struct Slot {
    /// Último quadro chegado (base64 e número para a confirmação): o que chega antes de ele ser decodificado o substitui.
    data: Option<(String, Value)>,
    /// Decodificado e ainda não escrito na textura.
    pixels: Option<(u32, u32, Vec<u8>)>,
    /// Há um quadro que a tela ainda não desenhou: o próximo espera, sem decodificar à toa.
    unseen: bool,
    /// Número do quadro escrito, confirmado quando a tela o desenhar.
    drawn: Option<Value>,
    /// Quadros trocados antes de decodificar, confirmados junto com o próximo desenho.
    owed: Vec<Value>,
    closed: bool,
}

impl Slot {
    fn next(&mut self) -> Option<(String, Value)> { if self.unseen { None } else { self.data.take() } }
}

/// Só para medir: com `HANGAR_NATIVE_NAV_TRACE` no ambiente, uma linha no stderr por etapa de cada quadro.
pub fn trace(what: std::fmt::Arguments) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("HANGAR_NATIVE_NAV_TRACE").is_some()) {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        eprintln!("[pagina {:>8.1}ms] {what}", START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(PoisonError::into_inner) }

impl Frames {
    pub fn new(events: async_channel::Sender<Event>, ack: Acker) -> Arc<Self> {
        let frames = Arc::new(Frames { shown: Mutex::new(None), slot: Mutex::default(), wake: Condvar::new(), events, ack });
        let worker = frames.clone();
        if let Err(e) = std::thread::Builder::new().name("webview2-frames".into()).spawn(move || worker.decode_loop()) {
            eprintln!("[pagina] sem thread para os quadros: {e}");
        }
        frames
    }

    /// `Page.screencastFrame`, na thread da interface: guarda e acorda quem decodifica.
    pub fn frame(&self, mut params: Value) {
        let id = params["sessionId"].take();
        let Some(Value::String(data)) = params.get_mut("data").map(Value::take) else { return (self.ack)(id) };
        trace(format_args!("quadro chegou ({} KB)", data.len() / 1024));
        let mut slot = lock(&self.slot);
        if let Some((_, old)) = slot.data.replace((data, id)) { slot.owed.push(old); }
        drop(slot);
        self.wake.notify_one();
    }

    fn decode_loop(&self) {
        let mut bytes = Vec::new();
        loop {
            let (data, id) = {
                let mut slot = lock(&self.slot);
                loop {
                    if slot.closed { return; }
                    if let Some(next) = slot.next() { break next; }
                    slot = self.wake.wait(slot).unwrap_or_else(PoisonError::into_inner);
                }
            };
            bytes.clear();
            let started = std::time::Instant::now();
            let decoded = base64::engine::general_purpose::STANDARD.decode_vec(&data, &mut bytes).map_err(|e| e.to_string())
                .and_then(|()| image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).map_err(|e| e.to_string()));
            match decoded {
                Ok(image) => {
                    let rgba = image.into_rgba8();
                    let (width, height) = rgba.dimensions();
                    trace(format_args!("decodificado {width}x{height} em {:.1}ms", started.elapsed().as_secs_f64() * 1000.));
                    let mut slot = lock(&self.slot);
                    slot.pixels = Some((width, height, rgba.into_raw()));
                    slot.unseen = true;
                    if let Some(old) = slot.drawn.replace(id) { slot.owed.push(old); }
                    drop(slot);
                    let _ = self.events.try_send(Event::Frame);
                }
                Err(e) => { eprintln!("[pagina] quadro do WebView2 descartado: {e}"); (self.ack)(id); }
            }
        }
    }

    /// Na thread da interface: escreve o quadro decodificado na textura. Devolve se a textura foi recriada (primeiro
    /// quadro, outro tamanho, device perdido): aí um desenho guardado ainda aponta para a antiga.
    pub fn commit(&self) -> bool {
        let Some(frame) = lock(&self.slot).pixels.take() else { return false };
        self.upload(&frame)
    }

    fn upload(&self, (width, height, pixels): &(u32, u32, Vec<u8>)) -> bool {
        let mut shown = lock(&self.shown);
        let reuse = shown.as_ref().filter(|t| t.is_current() && t.width() == *width && t.height() == *height).cloned();
        let (texture, replaced) = match reuse {
            Some(texture) => (texture, false),
            None => match SurfaceTexture::new(*width, *height) {
                Ok(texture) => (Arc::new(texture), true),
                Err(e) => { eprintln!("[pagina] textura da página não criada: {e}"); return false; }
            },
        };
        // Textura nova sem o quadro escrito pintaria vazia: fica a anterior.
        if let Err(e) = texture.write(pixels) { eprintln!("[pagina] quadro não escrito na textura: {e}"); return false; }
        trace(format_args!("escrito na textura (recriada: {replaced})"));
        *shown = Some(texture);
        replaced
    }

    /// A tela desenhou: confirma o quadro e libera o próximo para decodificar.
    pub fn seen(&self) {
        let (drawn, owed) = { let mut slot = lock(&self.slot); slot.unseen = false; (slot.drawn.take(), std::mem::take(&mut slot.owed)) };
        if drawn.is_some() { trace(format_args!("pintado")); }
        for id in drawn.into_iter().chain(owed) { (self.ack)(id); }
        self.wake.notify_one();
    }

    /// A textura ficou num device que se perdeu? Solta-a e avisa quem pinta, que reinicia o screencast: guardar os
    /// pixels do último quadro para refazê-la custava um quadro inteiro de memória por página viva.
    pub fn lost(&self) -> bool {
        let mut shown = lock(&self.shown);
        let lost = shown.as_ref().is_some_and(|t| !t.is_current());
        if lost { *shown = None; }
        lost
    }

    /// Textura atual, na thread da interface.
    pub fn shown(&self) -> Option<Arc<SurfaceTexture>> { lock(&self.shown).clone() }

    pub fn clear(&self) { *lock(&self.shown) = None; }

    pub fn close(&self) {
        lock(&self.slot).closed = true;
        self.wake.notify_one();
    }
}

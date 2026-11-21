//! Player de áudio do visor, dos anexos da conversa e do ditado: um áudio por vez, pela chave de quem pediu
//! ("file:<aba>", "ref:<linha>:<n>", "dictation", "recent:<nome>").
use super::*;
use crate::audio::{self, Playback};

#[derive(Default)]
pub(super) struct Player {
    current: Option<(String, Playback)>,
    loading: Option<String>,
    error: Option<(String, String)>,
    ticking: bool,
}

pub(super) struct AudioState { pub(super) playing: bool, pub(super) loading: bool, pub(super) duration: Option<f64>, pub(super) error: Option<String> }

/// Fatias da barra de posição: cada uma leva o áudio até o meio dela.
const SEGMENTS: usize = 48;

impl Hangar {
    /// Mesmo áudio: tocar/pausar. Outro: para o atual, busca os bytes, decodifica e toca.
    pub(super) fn toggle_audio(&mut self, key: String, name: &str,
        bytes: impl Future<Output = Result<Vec<u8>, String>> + Send + 'static, cx: &mut Context<Self>) {
        if let Some((_, playback)) = self.player.current.as_mut().filter(|(k, _)| *k == key) {
            playback.toggle();
            self.watch_audio(cx);
            cx.notify();
            return;
        }
        self.player.current = None;
        self.player.error = None;
        self.player.loading = Some(key.clone());
        let extension = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        let job = self.runtime.spawn(async move {
            let bytes = bytes.await?;
            tokio::task::spawn_blocking(move || audio::decode(bytes, extension.as_deref())).await.map_err(|e| e.to_string())?
        });
        cx.spawn(async move |this, cx| {
            let result = job.await.map_err(|e| e.to_string()).and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                // Outro áudio pedido no meio da carga: este chega tarde e não toca.
                if this.player.loading.as_deref() != Some(key.as_str()) { return; }
                this.player.loading = None;
                match result.and_then(|clip| Playback::start(Arc::new(clip))) {
                    Ok(playback) => { this.player.current = Some((key, playback)); this.watch_audio(cx); }
                    Err(error) => this.player.error = Some((key, error)),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// Fecha a lista de anexos recentes: o áudio tocado nela perde os controles e para junto.
    pub(super) fn close_recent(&mut self) {
        self.recent = None;
        if self.player.current.as_ref().is_some_and(|(k, _)| k.starts_with("recent:")) { self.player.current = None; }
        if self.player.loading.as_ref().is_some_and(|k| k.starts_with("recent:")) { self.player.loading = None; }
    }

    /// Fecha o áudio desta chave (a gravação do ditado foi trocada, a aba fechou).
    pub(super) fn stop_audio(&mut self, key: &str) {
        if self.player.current.as_ref().is_some_and(|(k, _)| k == key) { self.player.current = None; }
        if self.player.loading.as_deref() == Some(key) { self.player.loading = None; }
    }

    fn seek_audio(&mut self, key: &str, fraction: f64, cx: &mut Context<Self>) {
        let Some((_, playback)) = self.player.current.as_mut().filter(|(k, _)| k == key) else { return };
        playback.seek(fraction);
        if playback.paused() { playback.toggle(); }
        self.watch_audio(cx);
        cx.notify();
    }

    /// Redesenha a barra enquanto toca; no fim, pausa a saída. Erro do fluxo fecha o áudio e aparece no player dele.
    fn watch_audio(&mut self, cx: &mut Context<Self>) {
        if self.player.ticking { return; }
        self.player.ticking = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let playing = this.update(cx, |this, cx| {
                let settled = this.player.current.as_mut().map(|(_, playback)| playback.settle().map(|()| !playback.paused()));
                let playing = match settled {
                    Some(Ok(playing)) => playing,
                    Some(Err(error)) => {
                        if let Some((key, _)) = this.player.current.take() { this.player.error = Some((key, error)); }
                        false
                    }
                    None => false,
                };
                if !playing { this.player.ticking = false; }
                cx.notify();
                playing
            }).unwrap_or(false);
            if !playing { break; }
        }).detach();
    }

    /// O áudio da chave para quem desenha o próprio botão (a linha da aba Mídia): duração só depois de carregado.
    pub(super) fn audio_state(&self, key: &str) -> AudioState {
        let current = self.player.current.as_ref().filter(|(k, _)| k == key).map(|(_, playback)| playback);
        AudioState {
            playing: current.is_some_and(|playback| !playback.paused()),
            loading: self.player.loading.as_deref() == Some(key),
            duration: current.map(Playback::duration),
            error: self.player.error.as_ref().filter(|(k, _)| k == key).map(|(_, error)| error.clone()),
        }
    }

    /// Tocar/pausar, barra de posição (clique leva até ali) e tempo. `play` busca os bytes no primeiro clique.
    pub(super) fn audio_controls(&self, key: &str, play: impl Fn(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> Div {
        let current = self.player.current.as_ref().filter(|(k, _)| k == key).map(|(_, playback)| playback);
        let loading = self.player.loading.as_deref() == Some(key);
        let error = self.player.error.as_ref().filter(|(k, _)| k == key).map(|(_, error)| error.clone());
        let playing = current.is_some_and(|playback| !playback.paused());
        let (elapsed, duration) = current.map_or((0., 0.), |playback| (playback.elapsed().min(playback.duration()), playback.duration()));
        let filled = if duration > 0. { (elapsed / duration * SEGMENTS as f64).round() as usize } else { 0 };
        let label = tr(if playing { "audio_pause" } else { "audio_play" });
        let button = Button::new(SharedString::from(format!("audio-{key}"))).ghost().small()
            .icon(if playing { IconName::Pause } else { IconName::Play }).loading(loading)
            .tooltip(label.clone()).accessibility_label(label)
            .on_click(cx.listener(move |this, _, _, cx| play(this, cx)));
        let bar = div().w(px(180.)).h(px(16.)).flex().items_center().children((0..SEGMENTS).map(|n| {
            let (key, at) = (key.to_owned(), (n as f64 + 0.5) / SEGMENTS as f64);
            div().id(SharedString::from(format!("audio-{key}-{n}"))).flex_1().h_full().flex().items_center()
                .when(current.is_some(), |el| el.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.seek_audio(&key, at, cx))))
                .child(div().w_full().h(px(4.)).bg(if n < filled { theme::accent() } else { theme::border() }))
        }));
        div().flex().items_center().gap_2()
            .child(button)
            .child(bar)
            .child(div().font_family(theme::MONO).text_xs().text_color(theme::muted())
                .child(format!("{} / {}", audio::clock(elapsed), audio::clock(duration))))
            .when_some(error, |el, error| el.child(div().text_xs().text_color(theme::danger())
                .child(tr("audio_failed").replace("{error}", &error))))
    }
}

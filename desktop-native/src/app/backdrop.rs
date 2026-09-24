//! Fundo atrás das caixas: textura, luz, imagem escolhida e área de trabalho (janela ou vidro).
//! Imagem e papel de parede são decodificados fora da thread da janela; resposta de pedido velho é descartada.
use super::*;
use crate::{appearance::{self, Background, Wallpaper}, media};
use std::hash::{DefaultHasher, Hash, Hasher};

/// Operação sobre a imagem de fundo em andamento; enquanto houver uma, o grupo Fundo não aceita outra escolha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BackdropBusy { Choosing, Copying, Removing }

impl BackdropBusy {
    pub(super) fn note(self) -> String {
        tr(match self { Self::Choosing => "settings_image_choosing", Self::Copying => "settings_image_copying", Self::Removing => "settings_image_removing" })
    }
}

// Lado do ladrilho do grão (`media::grain`), em pixels lógicos.
const GRAIN_SIDE: f32 = 256.;

// Assinatura 0 é a imagem escolhida em arquivo; a do papel de parede nunca é 0. É assim que se sabe de qual
// fundo é a imagem que está na tela.
const FILE_IMAGE: u64 = 0;

fn signature(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish() | 1
}

impl Hangar {
    /// Carrega o que o fundo escolhido desenha. Sem imagem a desenhar, solta a da tela.
    pub(super) fn refresh_backdrop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backdrop_seq += 1;
        let seq = self.backdrop_seq;
        let a = appearance::get();
        let tx = self.tx.clone();
        let connection = self.connection;
        // A imagem do outro fundo não fica na tela enquanto a nova carrega.
        let file_image = a.background == Background::Image;
        if self.backdrop.as_ref().is_some_and(|(sig, _)| (*sig == FILE_IMAGE) != file_image) { self.set_backdrop(None, window, cx); }
        match (a.background, a.wallpaper) {
            (Background::Image, _) => {
                let Some(path) = appearance::image_path() else { return self.fail_backdrop(tr("backdrop_missing"), window, cx) };
                self.runtime.spawn(async move {
                    let result = tokio::task::spawn_blocking(move || media::load_backdrop(&path)).await
                        .unwrap_or(Err("backdrop_invalid")).map(|image| Some((FILE_IMAGE, image))).map_err(Failure::local);
                    let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Backdrop(seq, result) }).await;
                });
            }
            (Background::Desktop, Wallpaper::Glass) => {
                let Some(api) = self.api.clone() else { return self.fail_backdrop(tr("settings_desktop_offline"), window, cx) };
                // A mesma foto não é decodificada de novo: a releitura acontece a cada volta do foco.
                let current = self.backdrop.as_ref().map(|(sig, _)| *sig);
                self.runtime.spawn(async move {
                    let result = match api.desktop_wallpaper().await {
                        Ok(bytes) => {
                            let sig = signature(&bytes);
                            if current == Some(sig) { Ok(None) } else {
                                tokio::task::spawn_blocking(move || media::backdrop(&bytes)).await.ok().flatten()
                                    .map(|image| Some((sig, image))).ok_or_else(|| Failure::local("backdrop_invalid"))
                            }
                        }
                        Err(error) => Err(error),
                    };
                    let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Backdrop(seq, result) }).await;
                });
            }
            _ => {
                self.set_backdrop(None, window, cx);
                self.backdrop_note = None;
            }
        }
        cx.notify();
    }

    pub(super) fn receive_backdrop(&mut self, seq: u64, result: Result<Option<(u64, Arc<RenderImage>)>, Failure>,
        window: &mut Window, cx: &mut Context<Self>) {
        // Fundo trocado enquanto a imagem vinha: ela não vale mais.
        if seq != self.backdrop_seq {
            if let Ok(Some((_, image))) = result { cx.drop_image(image, Some(window)); }
            return;
        }
        match result {
            Ok(None) => {}
            Ok(Some(next)) => {
                self.set_backdrop(Some(next), window, cx);
                self.backdrop_note = None;
            }
            Err(error) => {
                let reason = match error.status {
                    Some(403) => tr("settings_wallpaper_remote"),
                    Some(404) => tr("settings_wallpaper_missing"),
                    _ => Self::failure(&error),
                };
                self.fail_backdrop(reason, window, cx);
            }
        }
        cx.notify();
    }

    /// Diz por que o fundo não desenha a imagem e cai no legível: Liso para a Imagem, Janela para o Vidro.
    fn fail_backdrop(&mut self, reason: String, window: &mut Window, cx: &mut Context<Self>) {
        self.set_backdrop(None, window, cx);
        let key = if appearance::get().background == Background::Image { "settings_image_fallback" } else { "settings_wallpaper_fallback" };
        let note = tr(key).replace("{reason}", &reason);
        // Aviso fora das configurações só quando o motivo muda: a releitura a cada foco não repete a notificação.
        if self.backdrop_note.as_ref() != Some(&note) { window.push_notification(Notification::warning(note.clone()), cx); }
        self.backdrop_note = Some(note);
        cx.notify();
    }

    fn set_backdrop(&mut self, next: Option<(u64, Arc<RenderImage>)>, window: &mut Window, cx: &mut Context<Self>) {
        theme::set_backdrop_ready(next.is_some());
        if let Some((_, old)) = std::mem::replace(&mut self.backdrop, next) { cx.drop_image(old, Some(window)); }
    }

    /// Escolha de arquivo para o fundo Imagem. A cópia só troca a anterior depois de validada.
    /// Enquanto a escolha, a cópia ou a remoção corre, o grupo Fundo fica travado na página: um clique no meio
    /// seria desfeito pelo resultado que chega depois, e duas operações disputariam o mesmo arquivo.
    pub(super) fn pick_backdrop(&mut self, cx: &mut Context<Self>) {
        if self.backdrop_busy.is_some() { return; }
        self.backdrop_busy = Some(BackdropBusy::Choosing);
        cx.notify();
        let prompt = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        let (tx, connection, runtime) = (self.tx.clone(), self.connection, self.runtime.clone());
        cx.spawn(async move |this, cx| {
            let chosen = prompt.await;
            let source = match chosen {
                Ok(Ok(Some(mut paths))) if !paths.is_empty() => paths.swap_remove(0),
                Ok(Ok(_)) => { let _ = this.update(cx, |this, cx| { this.backdrop_busy = None; cx.notify(); }); return; }
                _ => {
                    let _ = this.update(cx, |this, cx| { this.backdrop_busy = None; this.backdrop_note = Some(tr("picker_failed")); cx.notify(); });
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| { this.backdrop_busy = Some(BackdropBusy::Copying); cx.notify(); });
            runtime.spawn(async move {
                let result = match appearance::image_path() {
                    None => Err("backdrop_not_saved"),
                    Some(dest) => tokio::task::spawn_blocking(move || media::adopt_backdrop(&source, &dest)).await.unwrap_or(Err("backdrop_invalid")),
                };
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::BackdropPicked(result.map_err(Failure::local)) }).await;
            });
        }).detach();
    }

    pub(super) fn receive_picked_backdrop(&mut self, result: Result<Arc<RenderImage>, Failure>, window: &mut Window, cx: &mut Context<Self>) {
        self.backdrop_busy = None;
        match result {
            Ok(image) => {
                let mut next = appearance::get();
                next.background = Background::Image;
                self.apply_appearance(next, true, cx);
                self.backdrop_seq += 1;
                self.set_backdrop(Some((FILE_IMAGE, image)), window, cx);
                self.backdrop_note = None;
            }
            // O fundo de antes continua; o motivo aparece na página e numa notificação.
            Err(error) => {
                let note = tr("settings_image_rejected").replace("{reason}", &Self::failure(&error));
                window.push_notification(Notification::warning(note.clone()), cx);
                self.backdrop_note = Some(note);
            }
        }
        cx.notify();
    }

    /// Apaga a cópia guardada; a tela só vai para o Liso depois que o disco confirmar.
    pub(super) fn remove_backdrop(&mut self, cx: &mut Context<Self>) {
        if self.backdrop_busy.is_some() { return; }
        self.backdrop_busy = Some(BackdropBusy::Removing);
        let (tx, connection) = (self.tx.clone(), self.connection);
        self.runtime.spawn(async move {
            let result = match appearance::image_path() {
                None => Ok(()),
                Some(path) => tokio::task::spawn_blocking(move || match std::fs::remove_file(&path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                    _ => Ok(()),
                }).await.unwrap_or_else(|e| Err(e.to_string())),
            };
            let _ = tx.send(Envelope { connection, selection: None, payload: Payload::BackdropRemoved(result) }).await;
        });
        cx.notify();
    }

    pub(super) fn receive_removed_backdrop(&mut self, result: Result<(), String>, window: &mut Window, cx: &mut Context<Self>) {
        self.backdrop_busy = None;
        match result {
            Ok(()) => {
                let mut next = appearance::get();
                next.background = Background::Plain;
                self.apply_appearance(next, true, cx);
                self.refresh_backdrop(window, cx);
            }
            // A cópia ficou: a Imagem continua escolhida e desenhada, e o motivo aparece.
            Err(error) => {
                let note = tr("settings_image_not_removed").replace("{error}", &error);
                window.push_notification(Notification::warning(note.clone()), cx);
                self.backdrop_note = Some(note);
            }
        }
        cx.notify();
    }

    /// Camada atrás de tudo: imagem com véu, véu sobre a área de trabalho, luz e grão. O Liso não desenha nada.
    pub(super) fn render_backdrop(&self, window: &Window) -> Option<AnyElement> {
        let a = appearance::get();
        let image = self.backdrop.as_ref().map(|(_, image)| image.clone());
        let busy = match a.background {
            Background::Plain => return None,
            // Imagem que não abriu fica no Liso da raiz (`theme::window_fill`).
            Background::Image if image.is_none() => return None,
            Background::Image | Background::Desktop => true,
            Background::Texture | Background::Light => false,
        };
        let viewport = window.viewport_size();
        let (cols, rows) = ((f32::from(viewport.width) / GRAIN_SIDE).ceil() as usize, (f32::from(viewport.height) / GRAIN_SIDE).ceil() as usize);
        let grain = div().absolute().inset_0().flex().flex_wrap().opacity(theme::grain_opacity())
            .children((0..cols * rows).map(|_| img(self.grain.clone()).flex_shrink_0().size(px(GRAIN_SIDE))));
        let layer = div().absolute().inset_0().overflow_hidden()
            .when(busy, |el| el
                .when_some(image, |el, image| el.child(img(image).absolute().inset_0().size_full().object_fit(ObjectFit::Cover)))
                .child(div().absolute().inset_0().bg(theme::veil())))
            // A luz é uma sombra borrada sem corpo: o GPUI não tem gradiente radial.
            .when(a.background == Background::Light, |el| el.child(div().absolute().left(relative(0.1)).top(relative(-0.2))
                .w(relative(0.5)).h(relative(0.6)).rounded_full()
                .shadow(vec![BoxShadow { color: theme::glow(), offset: point(px(0.), px(0.)), blur_radius: px(180.), spread_radius: px(0.), inset: false }])))
            .child(grain);
        Some(layer.into_any_element())
    }
}

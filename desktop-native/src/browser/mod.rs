//! Navegador embutido do painel lateral. Motor por sistema, mesma superfície de chamada:
//! Windows/macOS usam o webview do sistema (wry) como janela filha; Linux, um Chromium sem janela pintado pela GPUI.
//! Windows e Linux falam CDP, e neles o app atende o hangar-preview e a tela remota.
pub mod model;
pub mod preview_fmt;
#[cfg(not(target_os = "macos"))]
pub mod chrome_import;
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub mod control;
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub mod pick;
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub mod server;
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub mod relay;
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub mod external;
#[cfg(target_os = "windows")]
pub mod cdp;
#[cfg(not(target_os = "macos"))]
pub mod keys;
#[cfg(target_os = "windows")]
pub(crate) mod frames;
/// Mesmo nome nos dois sistemas: uma sessão CDP no pipe do Chromium faz o papel do CDP do WebView2.
#[cfg(target_os = "linux")]
pub mod cdp {
    pub use super::chromium::pipe::Session as Cdp;
}

#[cfg(target_os = "linux")]
mod chromium;
#[cfg(target_os = "linux")]
pub use chromium::{Engine, POPUP_GONE, Warm, carry::take_notice as take_cookie_notice};

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod wry_engine;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use wry_engine::Engine;

pub enum Event {
    State(model::PageState),
    /// Quadro novo para desenhar; só o motor que pinta pela GPUI manda.
    // Fica em todo sistema para o `if let` de quem recebe não virar padrão irrefutável.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Frame,
    /// Mensagem da página da conversa (`window.hangarHost`), JSON cru no formato do MCP Apps.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Host(String),
    /// Clique ou tecla da pessoa dentro da página nativa (Windows): a GPUI não os vê.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    UserInput,
    /// Esc dentro da página nativa (Windows): cancela a escolha de elemento, que a GPUI não veria.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    Escape,
    /// Popup de verdade aberto pela página (janela com `window.opener`, o login OAuth): o painel o adota pelo alvo.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Popup(String),
    /// O alvo deste motor fechou sozinho (popup que chamou `window.close()`).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    Down,
    Up,
    Move,
}

/// Zoom que dá para aplicar: zero, negativo, infinito ou NaN viram 1 (dividiriam o viewport por nada).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn valid_zoom(zoom: f32) -> f32 { if zoom.is_finite() && zoom > 0. { zoom } else { 1. } }

/// Ponto da tela (px lógicos, relativo à página) em px CSS da página ampliada: com zoom, ela vê `1/zoom` do espaço.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn unzoom(p: gpui_kit::Point<gpui_kit::Pixels>, zoom: f32) -> gpui_kit::Point<gpui_kit::Pixels> {
    let z = valid_zoom(zoom);
    gpui_kit::point(gpui_kit::px(f32::from(p.x) / z), gpui_kit::px(f32::from(p.y) / z))
}

/// Retângulo em pixels inteiros da tela onde o quadro do screencast é pintado, em `size` px lógicos: posição ou
/// largura fracionada faz a GPU reamostrar a textura e o texto perde a nitidez. Sempre no tamanho do próprio quadro, a
/// partir do canto do cartão, que recorta o que sobra: o quadro de antes de um resize esticado até a moldura nova
/// deformava a página até o próximo chegar.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn frame_rect(bounds: gpui_kit::Bounds<gpui_kit::Pixels>, (w, h): (f32, f32), scale: f32) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    use gpui_kit::{Bounds, point, px, size};
    let snap = |v: f32| px((v * scale).round() / scale);
    Bounds { origin: point(snap(f32::from(bounds.origin.x)), snap(f32::from(bounds.origin.y))), size: size(snap(w), snap(h)) }
}

#[cfg(test)]
mod tests {
    use super::{frame_rect, unzoom, valid_zoom};
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn frame_of_another_size_keeps_its_own_size() {
        let card = Bounds { origin: point(px(10.), px(20.)), size: size(px(720.), px(487.)) };
        // Quadro de antes do resize: fica no tamanho dele, recortado pelo cartão.
        let old = frame_rect(card, (1238., 422.), 1.);
        assert_eq!(old.origin, point(px(10.), px(20.)));
        assert_eq!(old.size, size(px(1238.), px(422.)), "quadro antigo não estica até o cartão");
    }

    #[test]
    fn frame_rect_snaps_to_device_pixels() {
        let card = Bounds { origin: point(px(10.3), px(20.6)), size: size(px(100.), px(50.)) };
        let r = frame_rect(card, (100.3, 50.), 1.5);
        assert_eq!(r.origin, point(px(10.), px(31. / 1.5)));
        assert_eq!(r.size, size(px(100.), px(50.)));
    }

    #[test]
    fn unzoom_divides_screen_points() {
        assert_eq!(unzoom(point(px(200.), px(100.)), 2.), point(px(100.), px(50.)));
        assert_eq!(unzoom(point(px(200.), px(100.)), 0.5), point(px(400.), px(200.)));
        assert_eq!(unzoom(point(px(7.), px(9.)), 0.), point(px(7.), px(9.)), "zoom inválido não divide por zero");
    }

    #[test]
    fn invalid_zoom_is_one() {
        for bad in [0., -1., f32::NAN, f32::INFINITY] { assert_eq!(valid_zoom(bad), 1.); }
        assert_eq!(valid_zoom(1.5), 1.5);
    }
}

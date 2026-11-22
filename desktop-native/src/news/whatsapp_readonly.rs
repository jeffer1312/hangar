//! `whatsapp-somente-leitura`: aceitar o risco, ler o QR com o celular e ver o que ficou guardado, com as partes da página.

use std::{sync::Arc, time::Instant};

use gpui_kit::{component::{Disableable, Sizable, button::{Button, ButtonVariants}, checkbox::Checkbox}, *};

use crate::{app, i18n::tr, theme};

const CYCLE: f32 = 8.;
/// Marca o aceite, mostra o QR, conecta.
const TRANSITIONS: [f32; 3] = [1.4, 2.8, 4.8];
/// Número sempre mascarado, como o servidor manda.
const DEVICE: &str = "+55 64 •••••-1234";

thread_local! {
    static QR: Arc<Image> = Arc::new(Image::from_bytes(ImageFormat::Svg, sample_qr().into_bytes()));
}

/// Um QR de mentira (não abre nada): os três quadrados de canto e módulos espalhados.
fn sample_qr() -> String {
    const N: u32 = 21;
    let finder = |x: u32, y: u32| [(0, 0), (N - 7, 0), (0, N - 7)].into_iter()
        .find(|(fx, fy)| x >= *fx && x < fx + 7 && y >= *fy && y < fy + 7)
        .map(|(fx, fy)| { let (dx, dy) = (x - fx, y - fy); dx == 0 || dx == 6 || dy == 0 || dy == 6 || ((2..=4).contains(&dx) && (2..=4).contains(&dy)) });
    let near_finder = |x: u32, y: u32| (x < 8 && y < 8) || (x >= N - 8 && y < 8) || (x < 8 && y >= N - 8);
    let mut rects = String::new();
    for y in 0..N {
        for x in 0..N {
            let dark = match finder(x, y) {
                Some(dark) => dark,
                None => !near_finder(x, y) && (x * 31 + y * 17 + x * y * 7) % 5 < 2,
            };
            if dark { rects.push_str(&format!(r#"<rect x="{x}" y="{y}" width="1" height="1"/>"#)); }
        }
    }
    format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {N} {N}" shape-rendering="crispEdges"><g fill="#000">{rects}</g></svg>"##)
}

pub(super) fn render(started: Instant, _width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: parado no conectado, que é o que o recurso entrega.
    let step = if cx.reduce_motion() { 3 } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        let step = TRANSITIONS.partition_point(|at| *at <= t);
        super::repaint_in(TRANSITIONS.get(step).copied().unwrap_or(CYCLE) - t, window, cx);
        step
    };
    let stage = div().id("news-whatsapp").w_full().h(px(300.)).p(px(12.)).rounded(px(12.)).border_1().border_color(theme::glass_border())
        .bg(theme::popup_fill(theme::raised())).overflow_hidden().flex().flex_col().gap_3();
    let stage = match step {
        0 | 1 => {
            let accepted = step == 1;
            stage.child(app::whatsapp_risk_box(Checkbox::new("news-whatsapp-accept").checked(accepted).tab_stop(false).label(tr("whatsapp_accept"))))
                .child(div().flex().child(Button::new("news-whatsapp-install").primary().small().label(tr("whatsapp_install"))
                    .disabled(!accepted).tab_stop(false)))
        }
        2 => stage.child(app::whatsapp_pair_card().child(app::whatsapp_qr_row(Some(QR.with(Arc::clone)),
            Button::new("news-whatsapp-by-phone").ghost().small().label(tr("whatsapp_pair_by_phone")).tab_stop(false)))),
        _ => stage.child(app::whatsapp_connected_summary(app::WhatsAppConnected {
            device: Some(DEVICE), device_name: Some("Hangar"), last_seconds: Some(120.), since: Some("2025-06-01T12:00:00Z"),
            messages: 4195, chats: 128, groups: 23, media_saved: 566,
        })).child(app::whatsapp_usage_box()),
    };
    stage.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn sample_qr_has_the_three_corners_and_stays_small() {
        let svg = sample_qr();
        for corner in [r#"x="0" y="0""#, r#"x="20" y="0""#, r#"x="0" y="20""#] { assert!(svg.contains(corner), "{corner}"); }
        assert!(svg.len() < 12_000);
    }
}

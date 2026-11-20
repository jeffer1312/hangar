//! `tela-de-status`: o Status com tudo funcionando, uma queda que aparece na linha do tempo e vira ponto de atenção,
//! e o bloco dos últimos 7 dias destacado como quando a pessoa toca no ponto.

use std::time::Instant;
use chrono::{Duration as Span, Utc};
use gpui_kit::*;
use hangar_api::status::StatusSnapshot;
use serde_json::{Value, json};
use crate::{app::status_parts as parts, theme};

const CYCLE: f32 = 6.;
/// A queda chega: a marca vermelha na linha do tempo e o ponto de atenção no topo.
const CRASH: f32 = 2.2;
/// O ponto é tocado: o bloco dos 7 dias ganha o destaque.
const JUMP: f32 = 3.8;
const STAGE_HEIGHT: f32 = 430.;
const VERSION: &str = "0.1.0.9160";
const MIB: f64 = (1u64 << 20) as f64;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Calm,
    Crash,
    Jump,
}

fn frame(t: f32) -> Phase {
    if t >= JUMP { Phase::Jump } else if t >= CRASH { Phase::Crash } else { Phase::Calm }
}

fn until_next_step(t: f32) -> f32 {
    [CRASH, JUMP, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

/// Uma hora, um ponto por minuto, com uma onda leve: a curva parece de servidor de verdade.
fn series(now: chrono::DateTime<Utc>, base: f64, swing: f64) -> Value {
    Value::Array((0..60).rev().map(|ago| {
        let i = 60 - ago;
        let v = base + swing * ((i as f64 / 7.).sin() + 0.4 * (i as f64 / 2.3).cos());
        json!({ "ts": (now - Span::minutes(ago)).to_rfc3339(), "v": v })
    }).collect())
}

/// O retrato de amostra, lido pelo mesmo modelo da página.
fn snapshot(crashed: bool) -> Option<StatusSnapshot> {
    let now = Utc::now();
    let mut exits = vec![json!({ "ts": (now - Span::days(2) - Span::hours(5)).to_rfc3339(), "kind": "update",
        "from": "0.1.0.9150", "to": VERSION })];
    if crashed { exits.push(json!({ "ts": (now - Span::hours(3)).to_rfc3339(), "kind": "crash", "cause": "out_of_memory" })); }
    let attention = if crashed { json!([{ "severity": "warn", "code": "exits_crash", "target": "exits", "args": ["1"] }]) } else { json!([]) };
    // De pé desde a atualização, ou desde a volta da queda.
    let up_since = if crashed { now - Span::hours(3) } else { now - Span::days(2) - Span::hours(5) };
    serde_json::from_value(json!({
        "generated_at": now.to_rfc3339(),
        "server": {
            "version": VERSION, "pid": 4242, "started_at": up_since.to_rfc3339(),
            "rss_bytes": (182. * MIB) as u64, "cpu_pct": 3.1,
            "rss_series": series(now, 176. * MIB, 9. * MIB), "cpu_series": series(now, 3.2, 1.4),
            "rss_avg_bytes": (175. * MIB) as u64, "cpu_avg_pct": 3.0,
        },
        "exits": { "items": exits },
        "attention": attention,
    })).ok()
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado com o ponto de atenção e o bloco destacado.
    let phase = if cx.reduce_motion() { Phase::Jump } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let blocks = snapshot(phase != Phase::Calm).map(|s| parts::view_model(&s)).unwrap_or_default();
    let flash = phase == Phase::Jump;
    let cards = blocks.iter().filter_map(|block| match block {
        parts::Block::Health(h) => Some(parts::health_card(h, cx, |button, _| button.tab_stop(false)).into_any_element()),
        parts::Block::Server(s) => Some(parts::status_server(s, false).into_any_element()),
        parts::Block::Days(d) => Some(parts::status_days(d, flash).into_any_element()),
        _ => None,
    }).collect::<Vec<_>>();
    div().id("news-server-status").relative().w(width).min_h(px(STAGE_HEIGHT)).p(px(14.)).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(12.))
        .children(cards)
        // Miniatura só leitura: nada aqui dentro recebe o mouse.
        .child(div().absolute().top_0().left_0().size_full().block_mouse_except_scroll())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn the_crash_arrives_and_then_the_block_is_highlighted() {
        assert_eq!(frame(0.5), Phase::Calm);
        assert_eq!(frame(2.3), Phase::Crash);
        assert_eq!(frame(4.), Phase::Jump);
    }

    #[test]
    fn the_sample_reads_as_a_status_snapshot() {
        let calm = snapshot(false).expect("amostra sem queda");
        let crashed = snapshot(true).expect("amostra com queda");
        assert!(calm.attention.is_empty());
        assert_eq!(crashed.attention.len(), 1);
        assert_eq!(crashed.exits.map(|e| e.items.len()), Some(2));
    }
}

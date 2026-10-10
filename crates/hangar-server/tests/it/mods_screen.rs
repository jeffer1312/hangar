use std::path::PathBuf;

use hangar_server::mods::screen::{cell_width, cells, find_in, parse_ansi, read_screen, Region, Screen};
use serde_json::{Value, json};

fn folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mods_screen")
}

fn cases() -> Vec<Value> {
    serde_json::from_slice(&std::fs::read(folder().join("casos.json")).unwrap()).unwrap()
}

/// A captura `name` lida com os títulos e a âncora do `casos.json`, ou com a âncora pedida.
fn read(name: &str, anchor: Option<&str>) -> Screen {
    let case = cases().into_iter().find(|case| case["nome"] == name).unwrap();
    let ansi = std::fs::read_to_string(folder().join(format!("{name}.ansi"))).unwrap();
    let titles: Vec<String> = serde_json::from_value(case["titulos"].clone()).unwrap();
    let anchor = anchor.map(str::to_owned).or_else(|| case["ancora"].as_str().map(str::to_owned));
    read_screen(&ansi, case["colunas"].as_u64().unwrap() as usize, case["linhas"].as_u64().unwrap() as usize, &titles, anchor.as_deref())
}

fn tela(name: &str) -> Screen { read(name, None) }

fn tabs(screen: &Screen) -> Vec<(usize, usize, usize, bool)> {
    screen.tabs.iter().map(|tab| (tab.index, tab.start, tab.end, tab.active)).collect()
}

fn region(rows: (usize, usize), lo: usize, hi: usize) -> Option<Region> { Some(Region { rows, lo, hi }) }

#[test]
fn dock_with_three_tabs() {
    let t = tela("tmux-01-tres-paineis-150");
    assert_eq!((t.placement, t.border, t.tab_row, t.close, t.prompt), (Some("dock"), Some(89), Some(0), Some((0, 148)), Some(40)));
    assert_eq!(tabs(&t), vec![(0, 90, 99, false), (1, 101, 107, false), (2, 109, 117, true)]);
    assert_eq!((t.active, t.body.clone()), (Some(2), region((1, 40), 90, 150)));
    assert_eq!((t.dialog, t.survey, t.focus, t.draft.as_str()), (false, false, Some("prompt"), ""));
}

#[test]
fn clicking_a_title_moved_the_active_tab() {
    assert_eq!(tela("tmux-02-apos-clicar-mr-150").active, Some(1));
}

#[test]
fn psmux_absolute_sgr_reads_like_tmux() {
    let (tmux, psmux) = (tela("tmux-01-tres-paineis-150"), tela("psmux-01-tres-paineis-150"));
    assert_eq!((tabs(&psmux), psmux.close, psmux.border), (tabs(&tmux), tmux.close, tmux.border));
}

#[test]
fn single_pane_has_no_titles_and_is_the_shown_one() {
    let t = tela("tmux-132-um-painel-150");
    assert_eq!((t.tabs.len(), t.active, t.close), (0, Some(0), Some((0, 148))));
}

#[test]
fn boxed_pane() {
    let t = tela("tmux-100-caixa-100");
    assert_eq!((t.placement, t.boxed, t.tab_row, t.close), (Some("inline"), Some((23, 37)), Some(24), Some((23, 97))));
    assert_eq!(tabs(&t), vec![(0, 2, 11, true), (1, 13, 19, false), (2, 21, 29, false)]);
    assert_eq!(t.body, region((25, 37), 1, 99));
    let one = tela("tmux-133-um-painel-caixa-100");
    assert_eq!((one.tabs.len(), one.active, one.body), (0, Some(0), region((24, 37), 1, 99)));
}

#[test]
fn widths_where_dock_and_box_switch() {
    let dock = tela("tmux-240-ao-lado-110");
    assert_eq!((dock.placement, dock.close), (Some("dock"), Some((0, 108))));
    let boxed = tela("tmux-240-caixa-109");
    assert_eq!((boxed.placement, boxed.close), (Some("inline"), Some((23, 106))));
    let psmux = tela("psmux-240-vitrine-110");
    assert_eq!((psmux.border, psmux.close), (Some(70), Some((0, 108))));
}

#[test]
fn focus_in_the_docked_and_the_boxed_pane() {
    assert_eq!(tela("tmux-20-painel-focado-150").focus, Some("pane"));
    assert_eq!(tela("tmux-102-caixa-focada-100").focus, Some("pane"));
}

#[test]
fn ring_on_an_inactive_tab_is_not_the_active_tab() {
    // Inverso sem negrito no título de MR: a ativa continua a primeira ((a), captura 21-j-tab-07).
    let t = tela("tmux-21-anel-em-aba-inativa-150");
    assert_eq!((t.active, t.focus), (Some(0), Some("pane")));
}

#[test]
fn the_ctrl_x_tab_cycle() {
    let seen: Vec<_> = ["4-prompt", "5-faixa", "6-painel-1", "7-painel-2", "8-painel-3", "9-prompt"].iter()
        .map(|n| { let t = tela(&format!("tmux-14-ciclo-{n}")); (t.focus, t.active) }).collect();
    assert_eq!(seen, vec![(Some("prompt"), Some(2)), (Some("band"), Some(2)), (Some("pane"), Some(0)),
        (Some("pane"), Some(1)), (Some("pane"), Some(2)), (Some("prompt"), Some(2))]);
}

#[test]
fn band_full_collapsed_shrunk_and_absent() {
    let full = tela("tmux-452-faixa-expandida-150");
    assert_eq!((full.band_state, full.band), ("full", region((34, 40), 0, 89)));
    let collapsed = tela("tmux-140-faixa-recolhida-150");
    assert_eq!((collapsed.band_state, collapsed.collapsed_row), ("collapsed", Some(38)));
    assert_eq!(tela("tmux-240-caixa-104").band_state, "shrunk");
    assert_eq!(read("tmux-452-faixa-expandida-150", Some("texto que não está na tela")).band_state, "absent");
}

#[test]
fn tabs_that_do_not_fit_leave_the_title_out() {
    let t = tela("tmux-262-abas-transbordando-150");
    assert_eq!((t.tabs.iter().map(|tab| tab.index).collect::<Vec<_>>(), t.active), (vec![0, 1, 2, 3, 4, 5, 6], Some(2)));
}

#[test]
fn dialog_beside_and_in_a_box() {
    let beside = tela("tmux-510-dialogo-150");
    assert_eq!((beside.dialog, beside.prompt, beside.placement, beside.active, beside.close), (true, None, Some("dock"), Some(0), Some((0, 148))));
    assert_eq!((beside.focus, beside.band_state), (None, "absent"));
    let boxed = tela("psmux-700-dialogo-com-caixa-100");
    assert_eq!((boxed.dialog, boxed.placement, boxed.close), (true, None, None));
}

#[test]
fn session_name_in_the_rule_is_not_a_dialog() {
    // `claude --name` (ou `/rename`) escreve o nome na régua de cima da caixa de digitar.
    let band = tela("tmux-160-sessao-com-nome-150");
    assert_eq!((band.dialog, band.prompt, band.focus, band.band_state), (false, Some(40), Some("prompt"), "full"));
    let pane = tela("tmux-161-sessao-com-nome-painel-150");
    assert_eq!((pane.dialog, pane.placement, pane.active, pane.band_state), (false, Some("dock"), Some(0), "full"));
}

#[test]
fn the_survey_hides_the_band() {
    let t = tela("psmux-710-pesquisa-150");
    assert_eq!((t.survey, t.dialog, t.band_state), (true, false, "absent"));
}

#[test]
fn without_fullscreen_the_pane_is_boxed() {
    let t = tela("tmux-122-sem-tela-cheia-150");
    assert_eq!((t.placement, t.boxed, t.close), (Some("inline"), Some((4, 18)), Some((4, 147))));
}

#[test]
fn draft_ignores_the_dim_suggestion() {
    assert_eq!(tela("tmux-01-tres-paineis-150").draft, "");
    assert_eq!(tela("tmux-440-rascunho-150").draft, "rascunho de teste");
    // psmux: NBSP depois do `❯` e a sugestão com o SGR absoluto esmaecido.
    let psmux = format!("{rule}\n❯\u{a0}\u{1b}[0;2mTry \"x\"\u{1b}[0m\n{rule}", rule = "─".repeat(40));
    assert_eq!(read_screen(&psmux, 40, 3, &[], None).draft, "");
}

#[test]
fn label_only_inside_the_region() {
    let t = tela("tmux-232-longo-meio-150");
    assert_eq!(find_in(&t, "Botão do meio", t.body.as_ref().unwrap()), vec![(38, 92)]);
    assert!(find_in(&t, "Botão do meio", &Region { rows: (0, 40), lo: 0, hi: 79 }).is_empty());
}

#[test]
fn attributes_cross_the_line_break() {
    let grid = parse_ansi("\u{1b}[1;7mab\ncd\u{1b}[0me", 3, 2);
    assert_eq!(grid[1].iter().map(|cell| cell.inverse).collect::<Vec<_>>(), vec![true, true, false]);
}

#[test]
fn widths_of_the_glyphs_on_screen() {
    // O marcador da prosa, o `✕` e o `●` dos títulos ocupam uma célula; ideograma, duas ((b)).
    assert_eq!((cell_width('●'), cell_width('✕'), cell_width('▸'), cell_width('界')), (1, 1, 1, 2));
}

/// Retrato da leitura de cada captura, gerado por este código e revisado no diff: pega mudança sem querer
/// em qualquer campo, inclusive nos que os testes acima não olham. Gerar com `MODS_RETRATO=1`.
#[test]
fn screens_match_the_snapshot() {
    let actual: Vec<Value> = cases().iter().map(|case| {
        let name = case["nome"].as_str().unwrap();
        let screen = tela(name);
        json!({"name": name, "screen": screen, "cells": screen.text.iter().map(|line| cells(line)).collect::<Vec<_>>()})
    }).collect();
    let path = folder().join("retrato.json");
    if std::env::var_os("MODS_RETRATO").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
    }
    let saved: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).expect("gerar com MODS_RETRATO=1 e revisar o diff")).unwrap();
    assert_eq!(saved, actual, "a leitura de uma captura mudou: rever o diff do retrato e, se for de propósito, gerar de novo com MODS_RETRATO=1");
}

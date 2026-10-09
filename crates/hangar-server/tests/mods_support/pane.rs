//! Pane de mentira para o clique com terminal: devolve as capturas limpas da medição, troca de captura
//! conforme o clique, a tecla ou a roda, e faz o que o plugin do Hangar avisaria (press, fechar, foco,
//! rolagem). O tamanho vem da captura na frente da fila, salvo depois de um redimensionamento. Como o
//! executor, recusa a operação que chega depois do ponto de partida e guarda a reserva do pane.
#![allow(dead_code)]
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hangar_server::mods::click::{Pane, PaneFuture, PaneOp, PaneReply};
use hangar_server::mods::model::*;
use hangar_server::mods::state::*;
use hangar_server::terminal_input::PaneFormats;
use serde_json::{Value, json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mods_screen")
}

pub fn capture(name: &str) -> String {
    std::fs::read_to_string(root().join(format!("{name}.ansi"))).unwrap()
}

pub fn size_of(name: &str) -> (u16, u16) {
    let cases: Value = serde_json::from_slice(&std::fs::read(root().join("casos.json")).unwrap()).unwrap();
    let case = cases.as_array().unwrap().iter().find(|c| c["nome"] == name).unwrap();
    (case["colunas"].as_u64().unwrap() as u16, case["linhas"].as_u64().unwrap() as u16)
}

pub fn button(key: &str, label: &str, plugin: &str) -> Value {
    json!({"type": "Button", "props": {"key": key, "label": label}, "press": {"plugin": plugin, "handle": 1}})
}

/// O espelho: `(id, título, key, rótulo)` por painel, com a faixa do mod de exemplo (`pm-abrir`).
pub fn view(panes: &[(&str, &str, &str, &str)]) -> TerminalView {
    TerminalView {
        above: json!({"type": "Box", "children": [button("pm-abrir", "▸ xx-00000", "pm-mock")]}),
        columns: Some(85),
        panes: panes.iter().map(|(id, title, key, label)| TerminalPane { id: (*id).into(), title: (*title).into(),
            placement: "dock".into(), columns: Some(58),
            tree: json!({"type": "Box", "children": [button(key, label, if id.starts_with("pm-mock") { "pm-mock" } else { "vitrine" })]}), data: None }).collect(),
        shown: None,
        caps: Vec::new(),
    }
}

#[derive(Clone)]
pub enum Effect {
    Show(&'static str),
    Pressed(&'static str, &'static str),
    CloseAll,
    /// Foco sem o mod, como o plugin do Hangar de antes de o foco levá-lo.
    Focus(&'static str, &'static str, bool),
    /// Foco no elemento de um mod: lugar, mod e elemento.
    FocusOf(&'static str, &'static str, &'static str),
    Scroll(&'static str, i64),
    /// A sessão reabre com outro processo e a vida dada, no meio do clique. O executor da vida antiga morre
    /// com ela: daí em diante este pane recusa toda operação, como o executor encerrado.
    NewLife(u64),
    /// A sessão é renomeada no meio do clique: o nome antigo sai do registro, e o processo e o pane continuam.
    Rename,
}

#[derive(Default)]
struct State {
    queue: VecDeque<String>,
    forced: Option<(u16, u16)>,
    clients: usize,
    mouse: bool,
    /// Até quando o pane está reservado: a reserva vence sozinha, como no executor.
    held_until: Option<Instant>,
    /// Tudo o que mexe no pane, inclusive reservar (`hold <ms>`), soltar (`release`) e a reserva que venceu
    /// antes de ser solta (`hold vencida`).
    log: Vec<String>,
    /// O executor morreu com a vida da sessão (`NewLife`).
    dead: bool,
    /// A próxima ação que começar com isto fica sem resposta (o pane travado no meio do clique).
    stall: Option<String>,
    actions: Vec<String>,
    /// Quando cada ação saiu, para medir o intervalo entre elas.
    stamps: Vec<(Instant, String)>,
    /// Quanto cada operação demora a responder (no psmux, um processo do multiplexador por operação).
    cost: Duration,
    on_click: HashMap<(u16, u16), Vec<Effect>>,
    on_keys: VecDeque<(String, Vec<Effect>)>,
    on_wheel: VecDeque<Vec<Effect>>,
    on_resize: Vec<(u16, Vec<Effect>)>,
}

pub struct FakePane { pub name: String, pub mods: Mods, state: Mutex<State> }

impl FakePane {
    pub fn new(mods: &Mods, name: &str, screen: &str) -> Self {
        let state = State { queue: VecDeque::from([screen.to_owned()]), clients: 1, mouse: true, ..Default::default() };
        Self { name: name.into(), mods: mods.clone(), state: Mutex::new(state) }
    }
    pub fn clients(&self, n: usize) { self.state.lock().unwrap().clients = n; }
    pub fn mouse(&self, on: bool) { self.state.lock().unwrap().mouse = on; }
    pub fn queue(&self, screens: &[&str]) { self.state.lock().unwrap().queue = screens.iter().map(|s| (*s).to_owned()).collect(); }
    pub fn on_click(&self, cell: (u16, u16), effects: Vec<Effect>) { self.state.lock().unwrap().on_click.insert(cell, effects); }
    pub fn on_keys(&self, chord: &str, effects: Vec<Effect>) { self.state.lock().unwrap().on_keys.push_back((chord.into(), effects)); }
    pub fn on_wheel(&self, effects: Vec<Effect>) { self.state.lock().unwrap().on_wheel.push_back(effects); }
    pub fn on_resize(&self, rows: u16, effects: Vec<Effect>) { self.state.lock().unwrap().on_resize.push((rows, effects)); }
    pub fn cost(&self, each: Duration) { self.state.lock().unwrap().cost = each; }
    pub fn stall_on(&self, prefix: &str) { self.state.lock().unwrap().stall = Some(prefix.into()); }
    /// Ações que mexem no mod (clique, roda, tecla, tamanho), na ordem; leituras e reserva ficam de fora.
    pub fn actions(&self) -> Vec<String> { self.state.lock().unwrap().actions.clone() }
    pub fn held(&self) -> bool { self.state.lock().unwrap().held_until.is_some_and(|until| Instant::now() < until) }
    pub fn log(&self) -> Vec<String> { self.state.lock().unwrap().log.clone() }
    /// As ações com o instante em que cada uma chegou ao pane.
    pub fn stamps(&self) -> Vec<(Instant, String)> { self.state.lock().unwrap().stamps.clone() }

    fn apply(&self, state: &mut State, effects: &[Effect]) {
        for effect in effects {
            match effect {
                Effect::Show(screen) => state.queue = VecDeque::from([(*screen).to_owned()]),
                Effect::Pressed(site, key) => self.mods.pressed(&self.name, site, None, key),
                Effect::CloseAll => {
                    let life = self.mods.life(&self.name).unwrap();
                    let view = self.mods.terminal_view_in(&self.name, life).unwrap();
                    self.mods.terminal_ui(&self.name, TerminalView { panes: Vec::new(), shown: None, ..(*view).clone() });
                }
                Effect::Focus(site, element, denied) => {
                    let attempt = self.mods.armed_focus(&self.name).expect("alvo armado");
                    self.mods.focused(&self.name, &attempt, site, None, Some(element), *denied);
                }
                Effect::FocusOf(site, plugin, element) => {
                    let attempt = self.mods.armed_focus(&self.name).expect("alvo armado");
                    self.mods.focused(&self.name, &attempt, site, Some(plugin), Some(element), false);
                }
                Effect::Scroll(site, offset) => self.mods.scrolled(&self.name, site, *offset),
                Effect::NewLife(life) => {
                    self.mods.attach_terminal(&self.name, "proc-novo", *life, Arc::new(super::Probe::default()));
                    state.dead = true;
                }
                Effect::Rename => {
                    if let Some(life) = self.mods.life(&self.name) { self.mods.forget(&self.name, life); }
                }
            }
        }
    }

    /// Anota a ação e diz se ela fica sem resposta.
    fn act(state: &mut State, action: String) -> bool {
        let stalled = state.stall.as_deref().is_some_and(|prefix| action.starts_with(prefix));
        if stalled { state.stall = None; }
        state.log.push(action.clone());
        state.stamps.push((Instant::now(), action.clone()));
        state.actions.push(action);
        stalled
    }

    fn handle(&self, op: PaneOp) -> (Result<PaneReply, ModsError>, bool) {
        let mut state = self.state.lock().unwrap();
        let mut stalled = false;
        if state.held_until.is_some_and(|until| Instant::now() >= until) {
            state.held_until = None;
            state.log.push("hold vencida".into());
        }
        let reply = match op {
            PaneOp::Formats => {
                let (columns, rows) = state.forced.unwrap_or_else(|| size_of(&state.queue[0]));
                PaneReply::Formats(PaneFormats { mouse: state.mouse, in_mode: false, columns, rows })
            }
            PaneOp::Clients => PaneReply::Clients(state.clients),
            PaneOp::Screen => {
                let name = if state.queue.len() > 1 { state.queue.pop_front().unwrap() } else { state.queue[0].clone() };
                PaneReply::Screen(capture(&name))
            }
            PaneOp::Hold { millis } => {
                state.held_until = Some(Instant::now() + Duration::from_millis(millis));
                state.log.push(format!("hold {millis}"));
                PaneReply::Done
            }
            PaneOp::Release => { state.held_until = None; state.log.push("release".into()); PaneReply::Done }
            PaneOp::Mouse { row, col } => {
                stalled = Self::act(&mut state, format!("click {row} {col}"));
                let effects = state.on_click.get(&(row, col)).cloned().unwrap_or_default();
                self.apply(&mut state, &effects);
                PaneReply::Done
            }
            PaneOp::Wheel { row, col, down } => {
                stalled = Self::act(&mut state, format!("wheel {row} {col} {down}"));
                if let Some(effects) = state.on_wheel.pop_front() { self.apply(&mut state, &effects); }
                PaneReply::Done
            }
            PaneOp::Keys(keys) => {
                let chord = keys.join(" ");
                stalled = Self::act(&mut state, format!("keys {chord}"));
                if let Some((expected, effects)) = state.on_keys.pop_front() {
                    assert_eq!(chord, expected, "tecla fora da ordem esperada");
                    self.apply(&mut state, &effects);
                }
                PaneReply::Done
            }
            PaneOp::Resize { columns, rows } => {
                stalled = Self::act(&mut state, format!("resize {columns} {rows}"));
                state.forced = Some((columns, rows));
                let effects: Vec<Effect> = state.on_resize.iter().filter(|(r, _)| *r == rows).flat_map(|(_, e)| e.clone()).collect();
                self.apply(&mut state, &effects);
                PaneReply::Done
            }
        };
        (Ok(reply), stalled)
    }
}

impl Pane for FakePane {
    fn op(&self, op: PaneOp, start_by: Instant) -> PaneFuture {
        // Como o executor: a operação que chega à vez dela depois do ponto de partida não age.
        if Instant::now() >= start_by {
            return Box::pin(async { Err(pane_failed("mods_deadline")) });
        }
        if self.state.lock().unwrap().dead {
            return Box::pin(async { Err(pane_failed("terminal_gone")) });
        }
        // Reservar e soltar ficam no executor, sem processo do multiplexador.
        let cost = if matches!(op, PaneOp::Hold { .. } | PaneOp::Release) { Duration::ZERO } else { self.state.lock().unwrap().cost };
        let (result, stalled) = self.handle(op);
        Box::pin(async move {
            if stalled { std::future::pending::<()>().await; }
            tokio::time::sleep(cost).await;
            result
        })
    }
}

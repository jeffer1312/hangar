mod mods_support;

use std::sync::atomic::Ordering::SeqCst;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hangar_server::mods::state::*;
use mods_support::Probe;
use serde_json::{Value, json};

fn pane(id: &str, placement: &str) -> TerminalPane {
    TerminalPane { id: id.into(), title: id.into(), placement: placement.into(), columns: Some(58), tree: json!({"type": "Box"}), data: None }
}

fn view(ids: &[&str], shown: Option<&str>) -> TerminalView {
    TerminalView { above: json!({"type": "Box", "children": [{"type": "Button", "props": {"key": "abrir", "label": "▸ Abrir painéis"},
        "press": {"plugin": "m", "handle": 1}}]}), columns: Some(82), panes: ids.iter().map(|id| pane(id, "dock")).collect(),
        shown: shown.map(str::to_owned), caps: Vec::new() }
}

fn setup() -> (Mods, Arc<Probe>) {
    let mods = Mods::default();
    let probe = Arc::new(Probe::default());
    mods.attach_terminal("t", "proc-t", 1, probe.clone());
    (mods, probe)
}

fn last_ui(mods: &Mods) -> Value {
    serde_json::from_str(&mods.replay("t").into_iter().rev().find(|(event, _)| *event == "plugin_ui").unwrap().1).unwrap()
}

#[test]
fn terminal_view_is_published_with_source_and_shown() {
    let (mods, _) = setup();
    assert!(mods.owns("t") && mods.is_terminal("t") && mods.terminal_view_in("t", 1).is_none());
    assert_eq!(mods.life("t"), Some(1));
    assert_eq!(mods.bridge_session("t").as_deref(), Some("t"), "a ponte acha a sessão com terminal pelo nome de nascimento");
    assert!(mods.terminal_ui("t", view(&["a", "b"], Some("b"))));
    let ui = last_ui(&mods);
    assert_eq!((ui["shown_id"].as_str(), ui["columns"].as_u64(), ui["source"].as_str()), (Some("b"), Some(82), Some("terminal")));
    assert_eq!(ui["panes"].as_array().unwrap().len(), 2);
    assert!(!mods.terminal_ui("t", view(&["a", "b"], Some("b"))), "igual ao último não sai de novo");
}

/// O que o plugin atende vai ao app no `plugin_ui`; sem nada anunciado, a lista vem vazia.
#[test]
fn plugin_caps_reach_the_app() {
    let (mods, _) = setup();
    mods.terminal_ui("t", view(&[], None));
    assert_eq!(last_ui(&mods)["caps"], json!([]));
    assert!(mods.terminal_ui("t", TerminalView { caps: vec!["btw".into()], ..view(&[], None) }), "anúncio novo publica de novo");
    assert_eq!(last_ui(&mods)["caps"], json!(["btw"]));
}

/// O estado estruturado de um painel vai ao app como veio; painel sem ele não ganha o campo.
#[test]
fn pane_data_reaches_the_app_untouched() {
    let (mods, _) = setup();
    let mut with_data = pane("hangar-btw", "dock");
    with_data.data = Some(json!({"current": 0, "entries": [{"id": 1, "question": "q"}]}));
    mods.terminal_ui("t", TerminalView { panes: vec![with_data, pane("outro", "dock")], ..view(&[], None) });
    let ui = last_ui(&mods);
    assert_eq!(ui["panes"][0]["data"]["entries"][0]["question"], "q");
    assert!(ui["panes"][1].get("data").is_none());
}

#[test]
fn screen_shown_wins_and_falls_back_to_the_plugin() {
    let (mods, _) = setup();
    mods.terminal_ui("t", view(&["a", "b", "c"], Some("c")));
    mods.set_screen_shown("t", 1, Some("a".into()));
    assert_eq!(last_ui(&mods)["shown_id"], "a");
    // A tela mostrava um painel que fechou: vale o `shown` do plugin (o vizinho anterior).
    mods.terminal_ui("t", view(&["b", "c"], Some("b")));
    assert_eq!(last_ui(&mods)["shown_id"], "b");
    mods.set_screen_shown("t", 1, None);
    mods.terminal_ui("t", view(&[], None));
    assert!(last_ui(&mods)["shown_id"].is_null());
}

#[tokio::test]
async fn shown_reads_are_joined_in_one_window() {
    let (mods, probe) = setup();
    mods.terminal_ui("t", view(&["a", "b"], Some("a")));
    *probe.shown.lock().unwrap() = Some("b".into());
    mods.schedule_shown("t");
    mods.schedule_shown("t");
    tokio::time::sleep(SHOWN_READ_WINDOW + Duration::from_millis(150)).await;
    assert_eq!(probe.reads.load(SeqCst), 1);
    assert_eq!(last_ui(&mods)["shown_id"], "b");
}

#[tokio::test]
async fn a_scheduled_read_of_a_replaced_life_lands_nowhere() {
    let (mods, probe) = setup();
    *probe.shown.lock().unwrap() = Some("b".into());
    mods.schedule_shown("t");
    // Antes da janela fechar, a sessão reabre com outro processo, que mostra só o `a`.
    let fresh = Arc::new(Probe::default());
    mods.attach_terminal("t", "proc-novo", 2, fresh.clone());
    mods.terminal_ui("t", view(&["a", "b"], Some("a")));
    tokio::time::sleep(SHOWN_READ_WINDOW + Duration::from_millis(150)).await;
    assert_eq!(probe.reads.load(SeqCst), 1, "a leitura agendada roda no elo antigo");
    assert_eq!(last_ui(&mods)["shown_id"], "a", "e não cai na vida nova");
    // Na vida nova o agendamento lê o elo novo.
    *fresh.shown.lock().unwrap() = Some("b".into());
    mods.schedule_shown_in("t", 2);
    mods.schedule_shown_in("t", 1);
    tokio::time::sleep(SHOWN_READ_WINDOW + Duration::from_millis(150)).await;
    assert_eq!((probe.reads.load(SeqCst), fresh.reads.load(SeqCst)), (1, 1), "o pedido da vida 1 não agenda leitura na 2");
    assert_eq!(last_ui(&mods)["shown_id"], "b");
}

#[tokio::test]
async fn click_writes_and_waits_stay_in_their_life() {
    let (mods, _) = setup();
    mods.terminal_ui("t", view(&["a", "b"], Some("a")));
    // O `forget` encerra a espera na hora, sem gastar o prazo.
    let started = Instant::now();
    let waiting = { let mods = mods.clone(); tokio::spawn(async move { mods.wait_pressed("t", 1, "a", "m", "k", Instant::now(), Duration::from_secs(3)).await }) };
    tokio::time::sleep(Duration::from_millis(30)).await;
    mods.forget("t", 1);
    assert!(!waiting.await.unwrap());
    assert!(started.elapsed() < Duration::from_secs(1), "a espera acabou com a sessão");
    // Reaberta (vida 2): a espera da vida 1 acaba logo e não vê o press nem a rolagem da nova.
    mods.attach_terminal("t", "proc-novo", 2, Arc::new(Probe::default()));
    mods.terminal_ui("t", view(&["a", "b"], Some("a")));
    let started = Instant::now();
    mods.pressed("t", "a", None, "k");
    mods.scrolled("t", "a", 9);
    assert!(!mods.wait_pressed("t", 1, "a", "m", "k", started, Duration::from_secs(3)).await);
    assert_eq!(mods.wait_scroll("t", 1, "a", 0, Duration::from_secs(3)).await, None);
    assert!(!mods.wait_pane_gone("t", 1, "a", Duration::from_secs(3)).await);
    assert_eq!(mods.last_scroll("t", 1, "a"), (0, None));
    assert!(started.elapsed() < Duration::from_secs(1), "nenhuma espera da vida 1 gasta o prazo");
    assert!(mods.wait_pressed("t", 2, "a", "m", "k", started, Duration::from_millis(10)).await);
    // O que o clique da vida 1 escreve não chega à 2.
    mods.set_screen_shown("t", 1, Some("b".into()));
    assert_eq!(last_ui(&mods)["shown_id"], "a");
    mods.arm_focus("t", 1, "a", Some("m"), "k");
    assert!(mods.armed_focus("t").is_none());
    let attempt = mods.arm_focus("t", 2, "a", Some("m"), "k");
    mods.disarm_focus("t", 1, &attempt);
    assert_eq!(mods.armed_focus("t").as_deref(), Some(attempt.as_str()));
    assert!(mods.terminal_view_in("t", 1).is_none() && mods.terminal_view_in("t", 2).is_some());
}

#[tokio::test]
async fn presses_and_closed_panes_are_waited_for() {
    let (mods, _) = setup();
    mods.terminal_ui("t", view(&["a"], Some("a")));
    let since = Instant::now();
    let late = { let mods = mods.clone(); tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(50)).await; mods.pressed("t", "a", None, "k"); }) };
    assert!(mods.wait_pressed("t", 1, "a", "m", "k", since, Duration::from_secs(1)).await);
    late.await.unwrap();
    assert!(!mods.wait_pressed("t", 1, "a", "m", "k", Instant::now(), Duration::from_millis(50)).await, "press antes do clique não conta");
    let since = Instant::now();
    mods.pressed("t", "a", Some("outro"), "k");
    assert!(!mods.wait_pressed("t", 1, "a", "m", "k", since, Duration::from_millis(50)).await, "a mesma `key` de outro mod não conta");
    mods.pressed("t", "a", Some("m"), "k");
    assert!(mods.wait_pressed("t", 1, "a", "m", "k", since, Duration::from_millis(50)).await);
    assert!(!mods.wait_pane_gone("t", 1, "a", Duration::from_millis(30)).await);
    mods.terminal_ui("t", view(&[], None));
    assert!(mods.wait_pane_gone("t", 1, "a", Duration::from_millis(30)).await);
}

#[tokio::test]
async fn focus_target_rewrites_only_in_the_target_site_and_plugin() {
    let (mods, _) = setup();
    assert!(!mods.focus_target("t", "p", Some("m"), Some("x")).armed);
    let attempt = mods.arm_focus("t", 1, "p", Some("m"), "alvo");
    assert_eq!(mods.armed_focus("t").as_deref(), Some(attempt.as_str()));
    assert_eq!(mods.focus_target("t", "above-prompt", Some("m"), Some("x")).rewrite, None);
    assert_eq!(mods.focus_target("t", "p", None, None).rewrite, None, "parada do motor: sem plugin");
    assert_eq!(mods.focus_target("t", "p", Some("outro"), Some("x")).rewrite, None, "não atravessa de um mod para outro");
    assert_eq!(mods.focus_target("t", "p", Some("m"), Some("x")).rewrite.as_deref(), Some("alvo"));
    let seq = mods.focus_seq("t", 1);
    assert!(mods.focused("t", &attempt, "p", Some("m"), Some("alvo"), false));
    let seen = mods.wait_focus("t", 1, &attempt, seq, Duration::from_millis(50), |s| s.request_id == "p").await.unwrap();
    assert_eq!((seen.element.as_deref(), seen.plugin.as_deref(), seen.denied), (Some("alvo"), Some("m"), false));
    assert_eq!(mods.focus_target("t", "p", Some("m"), Some("x")).rewrite, None, "uma reescrita por alvo armado");
    mods.disarm_focus("t", 1, &attempt);
    assert!(!mods.focused("t", &attempt, "p", Some("m"), Some("x"), false));
}

#[tokio::test]
async fn scroll_offsets_are_followed() {
    let (mods, _) = setup();
    assert_eq!(mods.last_scroll("t", 1, "p"), (0, None));
    mods.scrolled("t", "p", 12);
    let (seq, offset) = mods.last_scroll("t", 1, "p");
    assert_eq!(offset, Some(12));
    assert_eq!(mods.wait_scroll("t", 1, "p", 0, Duration::from_millis(10)).await, Some((seq, 12)));
    assert_eq!(mods.wait_scroll("t", 1, "p", seq, Duration::from_millis(30)).await, None);
}

#[tokio::test]
async fn terminal_click_window_covers_a_slow_click_and_the_copy_wakes_the_end() {
    let (mods, _) = setup();
    let attempt = mods.begin_click("t", "a", "vitrine", "k");
    tokio::time::sleep(Duration::from_millis(1600)).await;
    assert_eq!(mods.match_click("t", "a", None, "k").as_deref(), Some(attempt.as_str()), "com terminal o press pode vir depois da rolagem");
    assert!(!mods.click_copied("t", "outra", "texto"));
    let late = { let (mods, attempt) = (mods.clone(), attempt.clone());
        tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(50)).await; assert!(mods.click_copied("t", &attempt, "texto")); }) };
    let started = Instant::now();
    assert_eq!(mods.finish_click("t", &attempt, Duration::from_secs(2)).await.0.as_deref(), Some("texto"));
    assert!(started.elapsed() < Duration::from_millis(500), "a cópia acorda o fim do clique em vez de esperar o prazo inteiro");
    late.await.unwrap();
}

#[test]
fn forget_stops_the_probe_and_keeps_the_birth_name_for_the_same_process() {
    let (mods, probe) = setup();
    mods.toast("t", 1, "m", "aviso", 4000);
    mods.terminal_ui("t", view(&["a"], Some("a")));
    mods.forget("t", 2);
    assert!(mods.owns("t") && !probe.stopped.load(SeqCst), "outra vida não esquece esta sessão");
    mods.forget("t", 1);
    assert!(probe.stopped.load(SeqCst) && !mods.owns("t"));
    // Reaberta com outro nome no mesmo processo (renomear), herda o nome de nascimento.
    mods.attach_terminal("t2", "proc-t", 2, Arc::new(Probe::default()));
    assert_eq!(mods.bridge_session("t").as_deref(), Some("t2"));
}

#[test]
fn a_new_process_inherits_nothing() {
    let (mods, probe) = setup();
    mods.toast("t", 1, "m", "aviso", 4000);
    mods.terminal_ui("t", view(&["a"], Some("a")));
    mods.begin_click("t", "a", "vitrine", "k");
    mods.attach_terminal("t", "proc-novo", 2, Arc::new(Probe::default()));
    assert!(probe.stopped.load(SeqCst), "o elo da sessão substituída para");
    assert_eq!(mods.match_click("t", "a", None, "k"), None, "o clique em aberto da sessão substituída não passa");
    assert!(!mods.replay("t").iter().any(|(event, _)| *event == "plugin_ui"), "a faixa da sessão substituída não passa");
    assert!(!mods.replay("t").iter().any(|(event, _)| *event == "plugin_toast"), "aviso de outro processo não passa (a8fd66ba)");
    assert!(mods.terminal_view_in("t", 2).is_none());
    assert_eq!(mods.life("t"), Some(2));
}

/// O `/ui` do plugin entrega ao elo a âncora da faixa, que o executor usa para reconhecer a faixa inteira.
#[test]
fn the_ui_hands_the_band_anchor_to_the_link() {
    let (mods, probe) = setup();
    mods.terminal_ui("t", TerminalView { above: json!({"type": "Box", "children": ["Revisão do MR"]}), ..view(&[], None) });
    assert_eq!(probe.anchor.lock().unwrap().as_deref(), Some("Revisão do MR"));
    mods.terminal_ui("t", TerminalView { above: Value::Null, ..view(&[], None) });
    assert_eq!(*probe.anchor.lock().unwrap(), None, "sem faixa, sem âncora");
}

mod mods_support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hangar_server::mods::click::{self, Ctx, Limits, Parts, Undo};
use hangar_server::mods::model::*;
use hangar_server::mods::screen::{find_in, read_screen};
use hangar_server::mods::state::*;
use mods_support::Probe;
use mods_support::pane::{Effect::*, FakePane, capture, view};
use serde_json::{Value, json};

const S: &str = "t";

fn vitrine() -> TerminalView {
    view(&[("vitrine-texto", "Texto", "V04-vitrine-texto", "Contar"), ("vitrine-botoes", "Botões", "V04-vitrine-botoes", "Contar"),
           ("vitrine-hover", "Hover", "V04-vitrine-hover", "Contar")])
}
fn pm() -> TerminalView {
    view(&[("pm-mock-pm", "xx-00000", "pm-a", "xxxxx"), ("pm-mock-mr", "MR ●2", "mr-a", "xxxxx"), ("pm-mock-jenkins", "Jenkins", "jenkins-a", "xxxxx")])
}
fn longo() -> TerminalView { view(&[("vitrine-longo", "Longo", "V37-meio", "Botão do meio")]) }
fn eight() -> TerminalView {
    view(&[("vitrine-texto", "Texto", "a", "Contar"), ("vitrine-botoes", "Botões", "b", "Contar"), ("vitrine-hover", "Hover", "c", "Contar"),
           ("vitrine-rico", "Mídia", "d", "x"), ("vitrine-quebrado", "Quebrado", "e", "x"), ("pm-mock-pm", "xx-00000", "pm-a", "xxxxx"),
           ("pm-mock-mr", "MR ●2", "mr-a", "xxxxx"), ("pm-mock-jenkins", "Jenkins", "jenkins-a", "xxxxx")])
}

fn setup(screen: &str, view: TerminalView) -> (Mods, FakePane) {
    let mods = Mods::default();
    mods.attach_terminal(S, "proc-t", 1, Arc::new(Probe::default()));
    mods.terminal_ui(S, view);
    let pane = FakePane::new(&mods, S, screen);
    (mods, pane)
}

/// Bem longe: o prazo não aperta, salvo nos testes dele.
fn far() -> Instant { Instant::now() + Duration::from_secs(30) }

/// Um pedido do app como o `spawn` o atende, sem a tarefa: o pedido e depois a limpeza.
async fn run(mods: &Mods, pane: &FakePane, until: Instant, call: ModsCall) -> Result<Value, ModsError> {
    let (limits, undo, clicked) = (Limits::quick(), Undo::default(), Mutex::default());
    let ctx = Ctx { name: S, pane, mods, limits: &limits, until, undo: &undo, life: 1, clicked: &clicked };
    let result = click::dispatch(&ctx, call).await;
    click::finish(&ctx).await;
    result
}
/// O mod do lugar, como o `view` desenha: a faixa e os painéis `pm-mock*` são do `pm-mock`.
fn plugin_of(site: &str) -> &'static str {
    if site == BAND_SITE || site.starts_with("pm-mock") { "pm-mock" } else { "vitrine" }
}
fn press_call(site: &str, key: &str) -> ModsCall {
    ModsCall::Press { site: site.into(), plugin: plugin_of(site).into(), key: key.into() }
}
async fn press(mods: &Mods, pane: &FakePane, site: &str, key: &str) -> Result<Value, ModsError> {
    run(mods, pane, far(), press_call(site, key)).await
}
async fn close(mods: &Mods, pane: &FakePane, site: &str) -> Result<Value, ModsError> {
    run(mods, pane, far(), ModsCall::Close { site: site.into() }).await
}
async fn show(mods: &Mods, pane: &FakePane, site: &str) -> Result<Value, ModsError> {
    run(mods, pane, far(), ModsCall::Show { site: site.into() }).await
}
fn code(result: Result<Value, ModsError>) -> String { result.unwrap_err().code }
/// Espera a condição com prazo de 5 s.
async fn until(check: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async { while !check() { tokio::time::sleep(Duration::from_millis(5)).await; } })
        .await.expect("condição esperada");
}
fn parts(mods: &Mods, pane: &Arc<FakePane>) -> Parts {
    Parts { name: S.into(), pane: pane.clone(), mods: mods.clone(), limits: Limits::quick(), busy: Arc::default(), life: 1,
        clicked: Arc::default() }
}

#[tokio::test]
async fn hidden_tab_is_activated_by_its_title_before_the_click() {
    let (mods, pane) = setup("tmux-400-vitrine-abas-150", vitrine());
    pane.on_click((0, 87), vec![Show("tmux-402-vitrine-texto-150")]);
    pane.on_click((1, 88), vec![Pressed("vitrine-texto", "V04-vitrine-texto")]);
    press(&mods, &pane, "vitrine-texto", "V04-vitrine-texto").await.unwrap();
    assert_eq!(pane.actions(), ["click 0 87", "click 1 88"]);
}

#[tokio::test]
async fn no_confirmation_refuses_without_repeating() {
    let (mods, pane) = setup("tmux-400-vitrine-abas-150", vitrine());
    assert_eq!(code(press(&mods, &pane, "vitrine-hover", "V04-vitrine-hover").await), "erro_mod_clique_sem_resposta");
    assert_eq!(pane.actions(), ["click 1 88"]);
}

#[tokio::test]
async fn click_follows_a_label_that_moved() {
    let (mods, pane) = setup("tmux-400-vitrine-abas-150", vitrine());
    pane.queue(&["tmux-400-vitrine-abas-150", "tmux-240-ao-lado-110"]);
    let titles: Vec<String> = ["Texto", "Botões", "Hover"].map(String::from).to_vec();
    let narrow = read_screen(&capture("tmux-240-ao-lado-110"), 110, 45, &titles, Some("▸ xx-00000"));
    let moved = find_in(&narrow, "Contar", narrow.body.as_ref().unwrap())[0];
    assert_ne!(moved, (1, 88));
    pane.on_click((moved.0 as u16, moved.1 as u16), vec![Pressed("vitrine-hover", "V04-vitrine-hover")]);
    press(&mods, &pane, "vitrine-hover", "V04-vitrine-hover").await.unwrap();
    assert_eq!(pane.actions(), [format!("click {} {}", moved.0, moved.1)]);
}

#[tokio::test]
async fn dialog_only_allows_the_shown_pane_and_its_close() {
    let (mods, pane) = setup("tmux-510-dialogo-150", pm());
    assert_eq!(code(press(&mods, &pane, "pm-mock-mr", "mr-a").await), "erro_mod_dialogo_aberto");
    assert_eq!(code(press(&mods, &pane, "above-prompt", "pm-abrir").await), "erro_mod_dialogo_aberto");
    assert_eq!(code(show(&mods, &pane, "pm-mock-mr").await), "erro_mod_dialogo_aberto");
    assert!(pane.actions().is_empty());
    pane.on_click((0, 148), vec![CloseAll]);
    close(&mods, &pane, "pm-mock-pm").await.unwrap();
    assert_eq!(pane.actions(), ["click 0 148"]);
}

#[tokio::test]
async fn dialog_with_the_boxed_pane_refuses_everything() {
    let (mods, pane) = setup("psmux-700-dialogo-com-caixa-100", pm());
    assert_eq!(code(press(&mods, &pane, "pm-mock-pm", "pm-a").await), "erro_mod_dialogo_aberto");
    assert_eq!(code(close(&mods, &pane, "pm-mock-pm").await), "erro_mod_dialogo_aberto");
    assert!(pane.actions().is_empty());
}

#[tokio::test]
async fn close_activates_the_tab_and_clicks_the_exact_cell() {
    let (mods, pane) = setup("tmux-05-antes-de-fechar-150", pm());
    pane.on_click((0, 104), vec![Show("tmux-02-apos-clicar-mr-150")]);
    pane.on_click((0, 148), vec![CloseAll]);
    close(&mods, &pane, "pm-mock-mr").await.unwrap();
    assert_eq!(pane.actions(), ["click 0 104", "click 0 148"]);
}

#[tokio::test]
async fn collapsed_band_is_expanded_before_the_click() {
    let (mods, pane) = setup("tmux-140-faixa-recolhida-150", pm());
    pane.on_click((38, 11), vec![Show("tmux-452-faixa-expandida-150")]);
    pane.on_click((34, 4), vec![Pressed("above-prompt", "pm-abrir")]);
    press(&mods, &pane, "above-prompt", "pm-abrir").await.unwrap();
    assert_eq!(pane.actions(), ["click 38 11", "click 34 4"]);
}

#[tokio::test]
async fn shrunk_band_with_a_terminal_attached_is_unreachable() {
    let (mods, pane) = setup("tmux-240-caixa-104", vitrine());
    assert_eq!(code(press(&mods, &pane, "above-prompt", "pm-abrir").await), "erro_mod_painel_nao_alcancavel");
    assert!(pane.actions().is_empty());
}

#[tokio::test]
async fn without_a_terminal_the_floor_is_restored() {
    let (mods, pane) = setup("tmux-240-caixa-104", vitrine());
    pane.clients(0);
    let (limits, undo) = (Limits::quick(), Undo::default());
    click::floor(&Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: far(), undo: &undo, life: 1, clicked: &Mutex::default() }).await.unwrap();
    assert_eq!(pane.actions()[0], "resize 144 45");
}

#[tokio::test]
async fn wheel_follows_the_offset_with_a_terminal_attached() {
    let (mods, pane) = setup("tmux-230-longo-topo-150", longo());
    pane.on_wheel(vec![Show("tmux-232-longo-meio-150"), Scroll("vitrine-longo", 67)]);
    pane.on_click((38, 92), vec![Pressed("vitrine-longo", "V37-meio")]);
    press(&mods, &pane, "vitrine-longo", "V37-meio").await.unwrap();
    assert_eq!(pane.actions(), ["wheel 20 115 true", "click 38 92"]);
}

#[tokio::test]
async fn without_a_terminal_the_window_is_stretched_and_given_back() {
    let (mods, pane) = setup("tmux-230-longo-topo-150", longo());
    pane.clients(0);
    pane.on_resize(click::TALL_ROWS, vec![Show("tmux-232-longo-meio-150")]);
    pane.on_click((38, 92), vec![Pressed("vitrine-longo", "V37-meio")]);
    press(&mods, &pane, "vitrine-longo", "V37-meio").await.unwrap();
    // A altura volta na limpeza, depois do clique.
    assert_eq!(pane.actions(), ["resize 150 250", "click 38 92", "resize 150 45"]);
}

#[tokio::test]
async fn wheel_without_the_label_goes_down_then_up_and_refuses() {
    // Sem tempo para o teclado (o anel caro demais), as duas pontas sem o rótulo são recusa.
    let (mods, pane) = setup("tmux-230-longo-topo-150", longo());
    let (limits, undo, clicked) = (Limits { ring_step: Duration::from_secs(60), ..Limits::quick() }, Undo::default(), Mutex::default());
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: far(), undo: &undo, life: 1, clicked: &clicked };
    let result = click::dispatch(&ctx, press_call("vitrine-longo", "V37-meio")).await;
    click::finish(&ctx).await;
    assert_eq!(code(result), "erro_mod_painel_nao_alcancavel");
    assert_eq!(pane.actions(), ["wheel 20 115 true", "wheel 20 115 false"]);
}

#[tokio::test]
async fn show_clicks_the_title_and_publishes_the_shown_pane() {
    let (mods, pane) = setup("tmux-01-tres-paineis-150", pm());
    pane.on_click((0, 104), vec![Show("tmux-02-apos-clicar-mr-150")]);
    assert_eq!(show(&mods, &pane, "pm-mock-mr").await.unwrap(), json!({"shown_id": "pm-mock-mr"}));
    let ui: Value = serde_json::from_str(&mods.replay(S).into_iter().rev().find(|(e, _)| *e == "plugin_ui").unwrap().1).unwrap();
    assert_eq!(ui["shown_id"], "pm-mock-mr");
}

#[tokio::test]
async fn a_band_click_before_the_band_arrives_is_a_missing_button() {
    // Sem o primeiro `/ui` não há espelho: a faixa responde como a sessão sem terminal, e o painel como painel.
    let mods = Mods::default();
    mods.attach_terminal(S, "proc-t", 1, Arc::new(Probe::default()));
    let pane = FakePane::new(&mods, S, "tmux-01-tres-paineis-150");
    assert_eq!(code(press(&mods, &pane, BAND_SITE, "pm-a").await), "erro_mod_botao_inexistente");
    assert_eq!(code(press(&mods, &pane, "pm-mock-mr", "mr-a").await), "erro_mod_painel_inexistente");
    assert!(pane.actions().is_empty());
}

#[tokio::test]
async fn a_click_of_a_replaced_life_does_nothing_in_the_new_one() {
    // A sessão reabriu com outro processo e o mesmo nome: o pedido da vida 1 não lê o espelho nem clica na nova.
    let (mods, pane) = setup("tmux-01-tres-paineis-150", pm());
    mods.attach_terminal(S, "proc-novo", 2, Arc::new(Probe::default()));
    mods.terminal_ui(S, pm());
    pane.on_click((0, 104), vec![Show("tmux-02-apos-clicar-mr-150")]);
    assert_eq!(code(show(&mods, &pane, "pm-mock-mr").await), "erro_mod_painel_inexistente");
    assert!(pane.actions().is_empty());
    let ui: Value = serde_json::from_str(&mods.replay(S).into_iter().rev().find(|(e, _)| *e == "plugin_ui").unwrap().1).unwrap();
    assert_eq!(ui["shown_id"], "pm-mock-jenkins", "o painel na frente da vida nova é o do espelho dela");
}

#[tokio::test]
async fn show_refuses_without_fullscreen_and_with_the_title_off_the_row() {
    let (mods, pane) = setup("tmux-01-tres-paineis-150", pm());
    pane.mouse(false);
    assert_eq!(code(show(&mods, &pane, "pm-mock-mr").await), "erro_mod_mouse_desligado");
    let (mods, pane) = setup("tmux-262-abas-transbordando-150", eight());
    assert_eq!(code(show(&mods, &pane, "pm-mock-jenkins").await), "erro_mod_painel_nao_alcancavel");
}

#[tokio::test]
async fn read_shown_reads_the_active_tab() {
    let (limits, undo) = (Limits::quick(), Undo::default());
    let (mods, pane) = setup("tmux-02-apos-clicar-mr-150", pm());
    assert_eq!(click::read_shown(&Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: far(), undo: &undo, life: 1, clicked: &Mutex::default() }).await.as_deref(), Some("pm-mock-mr"));
    let (mods, pane) = setup("psmux-700-dialogo-com-caixa-100", pm());
    assert_eq!(click::read_shown(&Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: far(), undo: &undo, life: 1, clicked: &Mutex::default() }).await, None);
}

#[tokio::test]
async fn a_short_deadline_sends_nothing_to_the_mod() {
    // Faixa inteira e o rótulo achado: só falta o clique, e não sobra tempo para a confirmação (300 ms de
    // `confirm` no `quick` mais 300 ms de folga). O clique não sai.
    let (mods, pane) = setup("tmux-452-faixa-expandida-150", pm());
    let result = run(&mods, &pane, Instant::now() + Duration::from_millis(400), press_call("above-prompt", "pm-abrir")).await;
    assert_eq!(code(result), "erro_mod_clique_sem_resposta");
    assert!(pane.actions().is_empty(), "nenhuma ação sem tempo para ela: {:?}", pane.actions());
}

#[tokio::test]
async fn the_task_answers_then_cleans_up_and_releases_the_pane() {
    let (mods, pane) = setup("tmux-230-longo-topo-150", longo());
    let pane = Arc::new(pane);
    pane.clients(0);
    pane.on_resize(click::TALL_ROWS, vec![Show("tmux-232-longo-meio-150")]);
    pane.on_click((38, 92), vec![Pressed("vitrine-longo", "V37-meio")]);
    let (task, answer) = click::spawn(parts(&mods, &pane), press_call("vitrine-longo", "V37-meio"), far());
    assert_eq!(answer.await.unwrap().unwrap(), json!({}));
    task.await.unwrap();
    assert_eq!(pane.actions(), ["resize 150 250", "click 38 92", "resize 150 45"]);
    assert!(!pane.held(), "a reserva do pane é solta no fim");
}

/// Com o pane lento, a reserva renovada para devolver a altura cobre também a operação mais lenta medida: a
/// última que começa no fim do prazo ainda roda no executor, e o `Release` chega atrás dela.
#[tokio::test]
async fn the_cleanup_hold_covers_the_measured_cost_of_an_operation() {
    let (mods, pane) = setup("tmux-230-longo-topo-150", longo());
    let pane = Arc::new(pane);
    pane.clients(0);
    pane.cost(Duration::from_millis(150));
    pane.on_resize(click::TALL_ROWS, vec![Show("tmux-232-longo-meio-150")]);
    pane.on_click((38, 92), vec![Pressed("vitrine-longo", "V37-meio")]);
    let (task, answer) = click::spawn(parts(&mods, &pane), press_call("vitrine-longo", "V37-meio"), far());
    assert_eq!(answer.await.unwrap().unwrap(), json!({}));
    task.await.unwrap();
    let log = pane.log();
    let give_back = log.iter().position(|a| a == "resize 150 45").unwrap();
    let renewal: u64 = log[..give_back].iter().rev().find_map(|a| a.strip_prefix("hold ")).unwrap().parse().unwrap();
    assert!(renewal >= (click::UNDO_MAX + click::HOLD_MARGIN + Duration::from_millis(150)).as_millis() as u64, "{log:?}");
    assert!(!log.contains(&"hold vencida".to_string()) && !pane.held(), "{log:?}");
}

#[tokio::test]
async fn a_cut_in_the_middle_of_the_stretch_gives_the_height_back() {
    let (mods, pane) = setup("tmux-230-longo-topo-150", longo());
    let pane = Arc::new(pane);
    pane.clients(0);
    pane.on_resize(click::TALL_ROWS, vec![Show("tmux-232-longo-meio-150")]);
    pane.stall_on("click 38 92");
    let (task, _answer) = click::spawn(parts(&mods, &pane), press_call("vitrine-longo", "V37-meio"), far());
    until(|| pane.actions().contains(&"click 38 92".to_string())).await;
    assert!(pane.held());
    // A tarefa some no meio: a guarda passa a limpeza a outra tarefa.
    task.abort();
    until(|| pane.actions().contains(&"resize 150 45".to_string()) && !pane.held()).await;
    assert_eq!(pane.actions(), ["resize 150 250", "click 38 92", "resize 150 45"]);
}

#[tokio::test]
async fn a_cut_in_the_middle_of_the_cleanup_redoes_what_was_left() {
    // A tarefa some com a volta da altura em curso: a guarda refaz a volta e solta o pane.
    let (mods, pane) = setup("tmux-230-longo-topo-150", longo());
    let pane = Arc::new(pane);
    pane.clients(0);
    pane.on_resize(click::TALL_ROWS, vec![Show("tmux-232-longo-meio-150")]);
    pane.on_click((38, 92), vec![Pressed("vitrine-longo", "V37-meio")]);
    pane.stall_on("resize 150 45");
    let (task, answer) = click::spawn(parts(&mods, &pane), press_call("vitrine-longo", "V37-meio"), far());
    assert_eq!(answer.await.unwrap().unwrap(), json!({}));
    until(|| pane.actions().contains(&"resize 150 45".to_string())).await;
    assert!(pane.held());
    task.abort();
    until(|| !pane.held()).await;
    assert_eq!(pane.actions(), ["resize 150 250", "click 38 92", "resize 150 45", "resize 150 45"]);
}

#[tokio::test]
async fn a_give_back_that_fails_in_the_request_is_retried_in_the_cleanup() {
    // A janela esticada não mostra o rótulo, e a volta imediata da altura fica sem resposta até o prazo:
    // a altura continua pendente e a limpeza a devolve.
    let (mods, pane) = setup("tmux-230-longo-topo-150", longo());
    pane.clients(0);
    pane.stall_on("resize 150 45");
    let until = Instant::now() + Duration::from_millis(1200);
    let result = run(&mods, &pane, until, press_call("vitrine-longo", "V37-meio")).await;
    assert_eq!(code(result), "erro_mod_clique_sem_resposta");
    assert_eq!(pane.actions(), ["resize 150 250", "resize 150 45", "resize 150 45"]);
}

#[tokio::test]
async fn one_request_at_a_time_in_the_pane_counting_the_cleanup() {
    let (mods, pane) = setup("tmux-230-longo-topo-150", longo());
    let pane = Arc::new(pane);
    let parts = parts(&mods, &pane);
    let held = parts.busy.clone().try_lock_owned().unwrap();
    let (_task, answer) = click::spawn(parts, ModsCall::Show { site: "vitrine-longo".into() }, Instant::now() + Duration::from_millis(100));
    assert_eq!(code(answer.await.unwrap()), "erro_mod_clique_sem_resposta", "a vez não veio no prazo");
    assert!(pane.actions().is_empty() && !pane.held());
    drop(held);
}

fn to_mr(pane: &FakePane) {
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-5-faixa")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-7-painel-2")]);
}
fn back_from_mr(pane: &FakePane) {
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-8-painel-3")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-9-prompt")]);
}
fn keys(pane: &FakePane) -> Vec<String> { pane.actions().into_iter().filter_map(|a| a.strip_prefix("keys ").map(String::from)).collect() }

#[tokio::test]
async fn keyboard_reaches_the_pane_presses_and_comes_back() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    to_mr(&pane);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "mr-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-mr", "mr-a")]);
    back_from_mr(&pane);
    press(&mods, &pane, "pm-mock-mr", "mr-a").await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "Tab", "Enter", "C-x Tab", "C-x Tab"]);
    assert_eq!(mods.armed_focus(S), None, "o alvo é desarmado no fim");
}

/// Com o plugin, a limpeza volta ao prompt pelo pedido a ele: nenhum `ctrl+x tab` depois do `Enter`, e a aba
/// na frente não muda.
#[tokio::test]
async fn keyboard_comes_back_by_the_plugin_without_the_ring() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    to_mr(&pane);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "mr-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-mr", "mr-a")]);
    pane.on_return(vec![Show("tmux-14-ciclo-9-prompt")]);
    press(&mods, &pane, "pm-mock-mr", "mr-a").await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "Tab", "Enter"]);
    assert_eq!(pane.actions().last().map(String::as_str), Some("plugin focus"));
    assert!(!pane.held());
}

/// O plugin avisou, mas a tela segue num mod: a volta continua pelo anel.
#[tokio::test]
async fn a_plugin_return_that_leaves_the_focus_on_a_mod_goes_on_by_the_ring() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    to_mr(&pane);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "mr-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-mr", "mr-a")]);
    pane.on_return(vec![]);
    back_from_mr(&pane);
    press(&mods, &pane, "pm-mock-mr", "mr-a").await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "Tab", "Enter", "C-x Tab", "C-x Tab"]);
    assert!(pane.actions().contains(&"plugin focus".to_string()));
    assert!(!pane.held());
}

#[tokio::test]
async fn keyboard_without_focus_on_the_target_never_sends_enter() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    to_mr(&pane);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "outro", true)]);
    back_from_mr(&pane);
    assert_eq!(code(press(&mods, &pane, "pm-mock-mr", "mr-a").await), "erro_mod_clique_sem_resposta");
    assert!(!keys(&pane).contains(&"Enter".to_string()));
}

#[tokio::test]
async fn keyboard_for_a_band_button() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-5-faixa"), Focus("above-prompt", "pm-abrir", false)]);
    pane.on_keys("Enter", vec![Pressed("above-prompt", "pm-abrir")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-7-painel-2")]);
    back_from_mr(&pane);
    press(&mods, &pane, "above-prompt", "pm-abrir").await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "Enter", "C-x Tab", "C-x Tab", "C-x Tab", "C-x Tab"]);
}

/// A faixa do `pm()` com outro mod desenhando um botão com a mesma `key` (`pm-abrir`) antes do do `pm-mock`.
fn band_with_a_shared_key() -> TerminalView {
    let mut v = pm();
    v.above = json!({"type": "Box", "children": [mods_support::pane::button("pm-abrir", "Outro", "outro-mod"),
        mods_support::pane::button("pm-abrir", "▸ xx-00000", "pm-mock")]});
    v
}

#[tokio::test]
async fn keyboard_skips_another_mod_with_the_same_key() {
    // O anel passa primeiro pelo botão do outro mod, com a mesma `key`: o `Enter` só sai no do mod pedido.
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", band_with_a_shared_key());
    pane.mouse(false);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-5-faixa"), FocusOf("above-prompt", "outro-mod", "pm-abrir")]);
    pane.on_keys("C-x Tab", vec![FocusOf("above-prompt", "pm-mock", "pm-abrir")]);
    pane.on_keys("Enter", vec![Pressed("above-prompt", "pm-abrir")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-7-painel-2")]);
    back_from_mr(&pane);
    press(&mods, &pane, "above-prompt", "pm-abrir").await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "Enter", "C-x Tab", "C-x Tab", "C-x Tab", "C-x Tab"]);
}

#[tokio::test]
async fn keyboard_refuses_a_shared_key_when_the_focus_comes_without_the_mod() {
    // O plugin do Hangar carregado antes de o foco levar o mod: com a `key` em dois mods, não há como saber
    // de qual é o foco, e a recusa é a de antes, sem `Enter`.
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", band_with_a_shared_key());
    pane.mouse(false);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-5-faixa"), Focus("above-prompt", "pm-abrir", false)]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-7-painel-2")]);
    back_from_mr(&pane);
    assert_eq!(code(press(&mods, &pane, "above-prompt", "pm-abrir").await), "erro_mod_botao_inexistente");
    assert!(!keys(&pane).contains(&"Enter".to_string()), "{:?}", keys(&pane));
}

#[tokio::test]
async fn keyboard_refuses_with_a_draft_or_a_dialog_before_any_key() {
    let (mods, pane) = setup("tmux-440-rascunho-150", pm());
    pane.mouse(false);
    assert_eq!(code(press(&mods, &pane, "pm-mock-mr", "mr-a").await), "erro_mod_rascunho_no_prompt");
    assert!(pane.actions().is_empty());
    let (mods, pane) = setup("tmux-510-dialogo-150", pm());
    pane.mouse(false);
    assert_eq!(code(press(&mods, &pane, "pm-mock-mr", "mr-a").await), "erro_mod_dialogo_aberto");
    assert!(pane.actions().is_empty());
}

#[tokio::test]
async fn a_dialog_in_the_middle_stops_the_keys() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    pane.on_keys("C-x Tab", vec![Show("tmux-510-dialogo-150")]);
    assert_eq!(code(press(&mods, &pane, "pm-mock-mr", "mr-a").await), "erro_mod_dialogo_aberto");
    assert_eq!(keys(&pane), ["C-x Tab"], "com o diálogo na tela nenhuma tecla a mais, nem na limpeza ((y))");
}

#[tokio::test]
async fn title_off_the_row_goes_to_the_keyboard() {
    // Oito abas, Jenkins fora da linha: o mouse não alcança o título e a reserva chega pelo ctrl+x tab.
    let (mods, pane) = setup("tmux-262-abas-transbordando-150", eight());
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-5-faixa")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-7-painel-2")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-8-painel-3")]);
    pane.on_keys("Tab", vec![Focus("pm-mock-jenkins", "jenkins-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-jenkins", "jenkins-a")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-9-prompt")]);
    press(&mods, &pane, "pm-mock-jenkins", "jenkins-a").await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "C-x Tab", "Tab", "Enter", "C-x Tab"]);
}

#[tokio::test]
async fn close_by_keyboard() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-5-faixa")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1")]);
    pane.on_keys("C-x x", vec![Show("tmux-14-ciclo-9-prompt"), CloseAll]);
    close(&mods, &pane, "pm-mock-pm").await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x x"]);
}

#[tokio::test]
async fn the_keyboard_never_starts_without_time_for_the_enter() {
    // 500 ms de prazo: a primeira tecla pediria a espera do foco (100 ms no `quick`), a confirmação do
    // `Enter` (300 ms) e a folga (300 ms). Não sai tecla nenhuma, e o alvo armado é desarmado na limpeza.
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    let result = run(&mods, &pane, Instant::now() + Duration::from_millis(500), press_call("pm-mock-mr", "mr-a")).await;
    assert_eq!(code(result), "erro_mod_clique_sem_resposta");
    assert!(keys(&pane).is_empty(), "{:?}", pane.actions());
    assert_eq!(mods.armed_focus(S), None);
}

#[tokio::test]
async fn a_cut_in_the_middle_of_the_keyboard_goes_back_and_disarms() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    let pane = Arc::new(pane);
    pane.mouse(false);
    to_mr(&pane);
    pane.on_keys("Tab", vec![]);
    back_from_mr(&pane);
    pane.stall_on("keys Tab");
    let (task, _answer) = click::spawn(parts(&mods, &pane), press_call("pm-mock-mr", "mr-a"), far());
    until(|| keys(&pane).contains(&"Tab".to_string())).await;
    assert!(mods.armed_focus(S).is_some() && pane.held());
    // A tarefa some com o teclado no painel e o alvo armado: a limpeza volta ao prompt e desarma.
    task.abort();
    until(|| !pane.held()).await;
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "Tab", "C-x Tab", "C-x Tab"]);
    assert_eq!(mods.armed_focus(S), None, "o próximo ctrl+x tab da pessoa não é reescrito");
}

/// O espelho `pm()` com a árvore do painel do MR trocada.
fn pm_with_mr(tree: Value) -> TerminalView {
    let mut view = pm();
    view.panes[1].tree = tree;
    view
}
fn mr_button(key: &str, label: &str) -> Value { mods_support::pane::button(key, label, "pm-mock") }

/// Com o mouse ligado, o rótulo repetido na árvore leva ao teclado sem nenhuma ação de mouse: o mesmo
/// caminho de `keyboard_reaches_the_pane_presses_and_comes_back`.
async fn press_by_keyboard_with_mouse_on(tree: Value) {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm_with_mr(tree));
    to_mr(&pane);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "mr-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-mr", "mr-a")]);
    back_from_mr(&pane);
    press(&mods, &pane, "pm-mock-mr", "mr-a").await.unwrap();
    assert!(pane.actions().iter().all(|a| a.starts_with("keys ")), "nenhuma ação de mouse: {:?}", pane.actions());
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "Tab", "Enter", "C-x Tab", "C-x Tab"]);
}

#[tokio::test]
async fn a_homonym_off_the_screen_sends_the_click_to_the_keyboard() {
    // Um segundo "xxxxx" no painel, fora da área visível: a tela mostra um só, a árvore tem dois.
    press_by_keyboard_with_mouse_on(json!({"type": "Box", "children": [mr_button("mr-a", "xxxxx"), mr_button("mr-b", "xxxxx")]})).await;
}

#[tokio::test]
async fn a_text_that_contains_the_label_sends_the_click_to_the_keyboard() {
    press_by_keyboard_with_mouse_on(json!({"type": "Box", "children": [
        {"type": "Text", "children": ["xxxxx de outro item"]}, mr_button("mr-a", "xxxxx")]})).await;
}

#[tokio::test]
async fn close_rereads_the_tab_right_before_the_click() {
    // O MR na frente na primeira leitura e o Jenkins logo depois: o `✕` da mesma célula fecharia o Jenkins.
    let (mods, pane) = setup("tmux-02-apos-clicar-mr-150", pm());
    pane.queue(&["tmux-02-apos-clicar-mr-150", "tmux-01-tres-paineis-150"]);
    pane.on_click((0, 148), vec![CloseAll]);
    assert_eq!(code(close(&mods, &pane, "pm-mock-mr").await), "erro_mod_clique_sem_resposta");
    assert!(pane.actions().is_empty(), "{:?}", pane.actions());
}

#[tokio::test]
async fn a_new_life_in_the_middle_of_the_click_stops_the_actions_and_cleans_up() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    let pane = Arc::new(pane);
    pane.mouse(false);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-5-faixa")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1"), NewLife(2)]);
    let (task, answer) = click::spawn(parts(&mods, &pane), press_call("pm-mock-mr", "mr-a"), far());
    assert!(answer.await.unwrap().is_err());
    task.await.unwrap();
    // O executor da vida antiga morreu com ela: nenhuma tecla depois da troca, nem a da volta ao prompt.
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab"]);
}

/// Pressiona o MR pelo teclado numa tarefa, sem a volta ao prompt preparada.
fn keyboard_press_without_return(mods: &Mods, pane: &Arc<FakePane>) -> (tokio::task::JoinHandle<()>, tokio::sync::oneshot::Receiver<Result<Value, ModsError>>) {
    pane.mouse(false);
    to_mr(pane);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "mr-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-mr", "mr-a")]);
    click::spawn(parts(mods, pane), press_call("pm-mock-mr", "mr-a"), far())
}

/// A primeira reserva que a limpeza manda, logo depois do `Enter`, e o tempo que ela cobre.
fn renewal_after_enter(pane: &FakePane) -> u64 {
    let log = pane.log();
    let enter = log.iter().position(|a| a == "keys Enter").unwrap();
    log[enter + 1].strip_prefix("hold ").expect("a limpeza renova a reserva antes de qualquer volta").parse().unwrap()
}

/// Com a volta ao prompt no teto dela (9 s), a folga não some no corte do executor (10 s): o prazo da volta
/// encolhe para a reserva caber inteira.
#[tokio::test]
async fn the_cleanup_hold_fits_the_executor_ceiling() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    let pane = Arc::new(pane);
    pane.mouse(false);
    to_mr(&pane);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "mr-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-mr", "mr-a")]);
    // Custo que, somado aos 9 s da volta e à folga, passa dos 10 s do executor.
    pane.cost(Duration::from_millis(550));
    let parts = Parts { limits: Limits { ring_step: Duration::from_secs(2), ..Limits::quick() }, ..parts(&mods, &pane) };
    let (task, answer) = click::spawn(parts, press_call("pm-mock-mr", "mr-a"), far());
    assert_eq!(answer.await.unwrap().unwrap(), json!({}));
    task.await.unwrap();
    let log = pane.log();
    let enter = log.iter().position(|a| a == "keys Enter").unwrap();
    let holds: Vec<u64> = log[enter..].iter().filter_map(|a| a.strip_prefix("hold ")?.parse().ok()).collect();
    assert!(holds.iter().all(|ms| *ms <= 10_000), "{holds:?}");
    assert!(renewal_after_enter(&pane) >= 9_000, "{holds:?}");
}

#[tokio::test]
async fn a_failed_return_keeps_the_pane_held() {
    // A tela não sai do painel: a limpeza renova a reserva, tenta até o teto e não solta o pane.
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    let pane = Arc::new(pane);
    let (task, answer) = keyboard_press_without_return(&mods, &pane);
    assert_eq!(answer.await.unwrap().unwrap(), json!({}));
    task.await.unwrap();
    assert!(renewal_after_enter(&pane) >= click::UNDO_MAX.as_millis() as u64);
    assert!(pane.held(), "o teclado ficou num painel: a fila não pode entregar");
    assert!(!pane.log().contains(&"release".to_string()), "{:?}", pane.log());
    assert_eq!(mods.armed_focus(S), None);
}

#[tokio::test]
async fn a_return_that_fails_once_is_retried_before_releasing() {
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    let pane = Arc::new(pane);
    // A primeira volta inteira (o anel e mais um) não sai do painel; a nova tentativa chega ao prompt.
    let (task, answer) = keyboard_press_without_return(&mods, &pane);
    for _ in 0..6 { pane.on_keys("C-x Tab", vec![]); }
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-9-prompt")]);
    assert_eq!(answer.await.unwrap().unwrap(), json!({}));
    task.await.unwrap();
    assert_eq!(keys(&pane).iter().filter(|k| *k == "C-x Tab").count(), 3 + 7);
    assert!(!pane.held());
    assert_eq!(pane.log().last().map(String::as_str), Some("release"));
}

#[tokio::test]
async fn a_hold_that_would_expire_in_the_cleanup_is_renewed() {
    // A reserva do pedido cobre o prazo (1 s) e mais `UNDO_MAX`. O `Tab` fica sem resposta até o prazo,
    // e a primeira tecla da volta, até o fim do prazo da limpeza: a nova tentativa cairia depois da reserva
    // do pedido. Renovada, a reserva não vence antes de o teclado voltar ao prompt e o pane ser solto.
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    let pane = Arc::new(pane);
    pane.mouse(false);
    to_mr(&pane);
    pane.on_keys("Tab", vec![]);
    back_from_mr(&pane);
    pane.stall_on("keys Tab");
    let (task, answer) = click::spawn(parts(&mods, &pane), press_call("pm-mock-mr", "mr-a"),
        Instant::now() + Duration::from_millis(1000));
    until(|| keys(&pane).contains(&"Tab".to_string())).await;
    pane.stall_on("keys C-x Tab");
    assert_eq!(code(answer.await.unwrap()), "erro_mod_clique_sem_resposta");
    task.await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "Tab", "C-x Tab", "C-x Tab"]);
    assert!(!pane.log().contains(&"hold vencida".to_string()), "{:?}", pane.log());
    assert_eq!(pane.log().last().map(String::as_str), Some("release"));
    assert!(!pane.held());
}

#[tokio::test]
async fn a_rename_in_the_middle_stops_the_actions_but_the_cleanup_still_goes_back() {
    // Mesmo processo e mesmo pane, nome novo: o pedido para na ação seguinte (a vida já não é a do nome), e a
    // limpeza volta o teclado ao prompt pelo executor, sem conferir pelo nome.
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    let pane = Arc::new(pane);
    pane.mouse(false);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-5-faixa")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1"), Rename]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-7-painel-2")]);
    back_from_mr(&pane);
    let (task, answer) = click::spawn(parts(&mods, &pane), press_call("pm-mock-mr", "mr-a"), far());
    assert_eq!(code(answer.await.unwrap()), "erro_mod_painel_inexistente");
    task.await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "C-x Tab", "C-x Tab"]);
    assert!(!pane.held());
    assert_eq!(pane.log().last().map(String::as_str), Some("release"));
}

#[tokio::test]
async fn a_late_band_focus_refuses_instead_of_pressing_with_the_focus_ahead() {
    // O `ui.focus` do primeiro botão chega depois do `focus_wait`: sem evento no prazo, recusa. Antes, ele era
    // achado na tecla seguinte, com o teclado já no painel, e o `Enter` saía.
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    // O caminho inteiro da faixa ao prompt: a recusa sai na faixa, e a limpeza volta por ele.
    to_mr(&pane);
    back_from_mr(&pane);
    let late = {
        let mods = mods.clone();
        tokio::spawn(async move {
            until(|| mods.armed_focus(S).is_some()).await;
            tokio::time::sleep(Duration::from_millis(150)).await;
            if let Some(attempt) = mods.armed_focus(S) { mods.focused(S, &attempt, "above-prompt", None, Some("pm-abrir"), false); }
        })
    };
    assert_eq!(code(press(&mods, &pane, "above-prompt", "pm-abrir").await), "erro_mod_clique_sem_resposta");
    late.await.unwrap();
    assert!(!keys(&pane).contains(&"Enter".to_string()), "{:?}", keys(&pane));
}

#[tokio::test]
async fn the_last_focus_after_the_tab_decides_not_the_first() {
    // O `Tab` dá dois focos: o alvo e logo outro elemento. O teclado está no segundo: nenhum `Enter`.
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    to_mr(&pane);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "mr-a", false), Focus("pm-mock-mr", "outro", false)]);
    back_from_mr(&pane);
    assert_eq!(code(press(&mods, &pane, "pm-mock-mr", "mr-a").await), "erro_mod_clique_sem_resposta");
    assert!(!keys(&pane).contains(&"Enter".to_string()), "{:?}", keys(&pane));
}

#[tokio::test]
async fn a_rewrite_on_the_ctrl_x_tab_into_the_pane_skips_the_tab() {
    // O hook reescreve já no `ctrl+x tab` que dá o teclado ao painel: o alvo tem o foco, e o `Tab` o tiraria.
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-5-faixa")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-7-painel-2"), Focus("pm-mock-mr", "mr-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-mr", "mr-a")]);
    back_from_mr(&pane);
    press(&mods, &pane, "pm-mock-mr", "mr-a").await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "Enter", "C-x Tab", "C-x Tab"]);
}

#[tokio::test]
async fn the_last_focus_wins_when_the_target_comes_after_another() {
    // Um foco atrasado em outro elemento e logo o do alvo: o teclado está no alvo, e o `Enter` sai.
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm());
    pane.mouse(false);
    to_mr(&pane);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "outro", false), Focus("pm-mock-mr", "mr-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-mr", "mr-a")]);
    back_from_mr(&pane);
    press(&mods, &pane, "pm-mock-mr", "mr-a").await.unwrap();
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "Tab", "Enter", "C-x Tab", "C-x Tab"]);
}

/// Os intervalos entre os cliques de mouse, na ordem.
fn click_gaps(pane: &FakePane) -> Vec<Duration> {
    let clicks: Vec<Instant> = pane.stamps().into_iter().filter(|(_, a)| a.starts_with("click ")).map(|(at, _)| at).collect();
    clicks.windows(2).map(|w| w[1] - w[0]).collect()
}

#[tokio::test]
async fn a_click_after_another_waits_the_gap_also_in_the_next_request() {
    // A aba e o botão são dois cliques seguidos: mais perto que o intervalo, o Claude Code os toma por duplo
    // clique e engole o segundo. O pedido seguinte no mesmo pane (fechar) também espera o intervalo.
    let (mods, pane) = setup("tmux-400-vitrine-abas-150", vitrine());
    pane.on_click((0, 87), vec![Show("tmux-402-vitrine-texto-150")]);
    pane.on_click((1, 88), vec![Pressed("vitrine-texto", "V04-vitrine-texto")]);
    pane.on_click((0, 148), vec![CloseAll]);
    // Os três cliques ficam longe da janela do duplo clique: o título e o botão logo abaixo dele são vizinhos
    // (até 450 ms o Claude Code engole o segundo), e com prazo de sobra o intervalo é sempre o maior.
    let gap = Duration::from_millis(300);
    let (limits, clicked) = (Limits { click_gap: Duration::from_millis(100), click_gap_near: gap, ..Limits::quick() }, Mutex::default());
    for call in [press_call("vitrine-texto", "V04-vitrine-texto"), ModsCall::Close { site: "vitrine-texto".into() }] {
        let undo = Undo::default();
        let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: far(), undo: &undo, life: 1, clicked: &clicked };
        click::dispatch(&ctx, call).await.unwrap();
        click::finish(&ctx).await;
    }
    assert_eq!(pane.actions(), ["click 0 87", "click 1 88", "click 0 148"]);
    assert!(click_gaps(&pane).iter().all(|g| *g >= gap), "{:?}", click_gaps(&pane));
}

#[tokio::test]
async fn the_click_gap_counts_against_the_deadline() {
    // Depois do clique na aba sobra menos que o intervalo, a confirmação (300 ms) e a folga (300 ms): o
    // clique no botão não sai, em vez de sair colado no primeiro.
    let (mods, pane) = setup("tmux-400-vitrine-abas-150", vitrine());
    pane.on_click((0, 87), vec![Show("tmux-402-vitrine-texto-150")]);
    pane.on_click((1, 88), vec![Pressed("vitrine-texto", "V04-vitrine-texto")]);
    // O anel caro demais tira o teclado da conta: o caminho é o do mouse até o fim.
    let (limits, undo, clicked) = (Limits { click_gap: Duration::from_millis(500), click_gap_near: Duration::from_millis(500),
        ring_step: Duration::from_secs(60), ..Limits::quick() },
        Undo::default(), Mutex::default());
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: Instant::now() + Duration::from_millis(900), undo: &undo,
        life: 1, clicked: &clicked };
    let result = click::dispatch(&ctx, press_call("vitrine-texto", "V04-vitrine-texto")).await;
    click::finish(&ctx).await;
    assert_eq!(code(result), "erro_mod_clique_sem_resposta");
    assert_eq!(pane.actions(), ["click 0 87"]);
}

#[tokio::test]
async fn the_keyboard_ring_with_a_dozen_band_buttons_fits_the_deadline() {
    // Doze botões na faixa antes dos painéis, com o custo de uma operação no psmux (um processo: cerca de
    // 70 ms na prova, aqui 100 ms) e os tempos de verdade: cada passo do anel é a tecla e uma leitura, sem
    // reler o tamanho, e o `Enter` sai dentro dos 7,5 s do pedido.
    let mut faixa = pm();
    faixa.above = json!({"type": "Box", "children": (0..12).map(|i| mods_support::pane::button(&format!("b{i}"), &format!("Botão {i}"), "pm-mock"))
        .collect::<Vec<_>>()});
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", faixa);
    pane.mouse(false);
    pane.cost(Duration::from_millis(100));
    // Na tela de verdade cada botão da faixa muda o inverso; aqui duas telas se alternam para a leitura ver a
    // mudança, e nenhuma delas tem o foco num painel.
    for i in 0..12 { pane.on_keys("C-x Tab", vec![Show(if i % 2 == 0 { "tmux-14-ciclo-5-faixa" } else { "tmux-14-ciclo-4-prompt" })]); }
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-7-painel-2")]);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "mr-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-mr", "mr-a")]);
    back_from_mr(&pane);
    let started = Instant::now();
    let (limits, undo, clicked) = (Limits::default(), Undo::default(), Mutex::default());
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: started + Duration::from_millis(7500), undo: &undo, life: 1,
        clicked: &clicked };
    let result = click::dispatch(&ctx, press_call("pm-mock-mr", "mr-a")).await;
    click::finish(&ctx).await;
    result.unwrap();
    assert_eq!(keys(&pane).iter().filter(|k| *k == "C-x Tab").count(), 14 + 2, "o anel inteiro e a volta ao prompt");
    assert!(keys(&pane).contains(&"Enter".to_string()));
}

/// O MR com o botão `mr-a` fora da área visível: a tela não o mostra, e a roda não chega a ele.
fn pm_with_far_button() -> TerminalView { pm_with_mr(json!({"type": "Box", "children": [mr_button("mr-a", "Botão lá embaixo")]})) }

/// Como o teclado chega ao MR a partir da tela com ele na frente e o prompt com o teclado.
fn keyboard_to_mr(pane: &FakePane) {
    to_mr(pane);
    pane.on_keys("Tab", vec![Focus("pm-mock-mr", "mr-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-mr", "mr-a")]);
    back_from_mr(pane);
}

#[tokio::test]
async fn a_button_beyond_the_wheel_goes_to_the_keyboard() {
    // Com terminal ligado não se estica a janela; cada evento da roda rola uma linha, e o botão está a ~40.
    // No teto da roda, o clique passa à reserva por teclado, cujo `Tab` rola o painel até o botão.
    let (mods, pane) = setup("tmux-02-apos-clicar-mr-150", pm_with_far_button());
    for offset in 1..=40 { pane.on_wheel(vec![Scroll("pm-mock-mr", offset)]); }
    keyboard_to_mr(&pane);
    let (limits, undo, clicked) = (Limits { wheel_events: 16, ..Limits::quick() }, Undo::default(), Mutex::default());
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: far(), undo: &undo, life: 1, clicked: &clicked };
    let result = click::dispatch(&ctx, press_call("pm-mock-mr", "mr-a")).await;
    click::finish(&ctx).await;
    result.unwrap();
    assert_eq!(pane.actions().iter().filter(|a| a.starts_with("wheel ")).count(), 16);
    assert_eq!(keys(&pane), ["C-x Tab", "C-x Tab", "C-x Tab", "Tab", "Enter", "C-x Tab", "C-x Tab"]);
}

#[tokio::test]
async fn the_wheel_stops_in_time_for_the_keyboard() {
    // A roda lenta não vai até o teto dela: para quando sobra só o que a reserva por teclado precisa.
    let (mods, pane) = setup("tmux-02-apos-clicar-mr-150", pm_with_far_button());
    for offset in 1..=200 { pane.on_wheel(vec![Scroll("pm-mock-mr", offset)]); }
    keyboard_to_mr(&pane);
    let (limits, undo, clicked) = (Limits { wheel_gap: Duration::from_millis(50), ..Limits::quick() }, Undo::default(), Mutex::default());
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: Instant::now() + Duration::from_millis(2500), undo: &undo,
        life: 1, clicked: &clicked };
    let result = click::dispatch(&ctx, press_call("pm-mock-mr", "mr-a")).await;
    click::finish(&ctx).await;
    result.unwrap();
    let wheels = pane.actions().iter().filter(|a| a.starts_with("wheel ")).count();
    assert!(wheels > 0 && wheels < limits.wheel_events, "{wheels} eventos");
    assert!(keys(&pane).contains(&"Enter".to_string()), "{:?}", keys(&pane));
}

#[tokio::test]
async fn the_wheel_stops_in_time_for_a_short_ring_on_psmux() {
    // Anel curto (um botão na faixa, o MR no segundo painel) com o custo de uma operação no psmux e os tempos
    // de verdade: a roda para com tempo para a última volta dela, a leitura do anel e a espera ao entrar no
    // painel, e o `Enter` ainda sai.
    let (mods, pane) = setup("tmux-02-apos-clicar-mr-150", pm_with_far_button());
    pane.cost(Duration::from_millis(70));
    for offset in 1..=200 { pane.on_wheel(vec![Scroll("pm-mock-mr", offset)]); }
    keyboard_to_mr(&pane);
    let (limits, undo, clicked) = (Limits::default(), Undo::default(), Mutex::default());
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: Instant::now() + Duration::from_millis(7500), undo: &undo,
        life: 1, clicked: &clicked };
    let result = click::dispatch(&ctx, press_call("pm-mock-mr", "mr-a")).await;
    click::finish(&ctx).await;
    result.unwrap();
    assert!(pane.actions().iter().any(|a| a.starts_with("wheel ")), "a roda tentou antes");
    assert!(keys(&pane).contains(&"Enter".to_string()), "{:?}", keys(&pane));
}

#[tokio::test]
async fn the_cleanup_covers_a_long_ring_back_to_the_prompt() {
    // Doze botões na faixa e dez painéis: depois do botão da faixa, a volta ao prompt passa por 22 paradas
    // do anel. Com o custo do psmux, os 2 s de antes cobriam poucos passos e a limpeza desistia com o teclado
    // num painel; o prazo da volta passa a acompanhar o tamanho do anel.
    let ids: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
    let mut grande = view(&ids.iter().map(|id| (id.as_str(), id.as_str(), "k", "x")).collect::<Vec<_>>());
    grande.above = json!({"type": "Box", "children": (0..12).map(|i| mods_support::pane::button(&format!("b{i}"), &format!("Botão {i}"), "pm-mock"))
        .collect::<Vec<_>>()});
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", grande);
    pane.mouse(false);
    pane.cost(Duration::from_millis(60));
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-5-faixa"), Focus("above-prompt", "b0", false)]);
    pane.on_keys("Enter", vec![Pressed("above-prompt", "b0")]);
    for i in 0..21 { pane.on_keys("C-x Tab", vec![Show(if i % 2 == 0 { "tmux-14-ciclo-6-painel-1" } else { "tmux-14-ciclo-5-faixa" })]); }
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-9-prompt")]);
    let pane = Arc::new(pane);
    let parts = Parts { limits: Limits::default(), ..parts(&mods, &pane) };
    let (task, answer) = click::spawn(parts, press_call("above-prompt", "b0"), Instant::now() + Duration::from_millis(7500));
    assert_eq!(answer.await.unwrap().unwrap(), json!({}));
    task.await.unwrap();
    assert_eq!(keys(&pane).iter().filter(|k| *k == "C-x Tab").count(), 1 + 22);
    assert_eq!(pane.log().last().map(String::as_str), Some("release"), "o teclado voltou e o pane foi solto");
}

#[tokio::test]
async fn a_far_click_short_of_time_uses_the_shorter_gap_but_a_near_one_does_not() {
    // Com pouco prazo, o clique longe do anterior espera só o intervalo curto; o vizinho exige o longo, e sem
    // tempo para ele não sai.
    let (mods, pane) = setup("tmux-400-vitrine-abas-150", vitrine());
    pane.on_click((0, 87), vec![Show("tmux-402-vitrine-texto-150")]);
    pane.on_click((1, 88), vec![Pressed("vitrine-texto", "V04-vitrine-texto")]);
    let limits = Limits { click_gap: Duration::from_millis(100), click_gap_near: Duration::from_millis(600), ring_step: Duration::from_secs(60),
        ..Limits::quick() };
    let clicked = Mutex::new(Some((Instant::now(), (30, 10))));
    let undo = Undo::default();
    // Longe (linha 30): sobra para 100 ms, não para 600 ms.
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: Instant::now() + Duration::from_millis(1000), undo: &undo,
        life: 1, clicked: &clicked };
    let started = Instant::now();
    let result = click::dispatch(&ctx, press_call("vitrine-texto", "V04-vitrine-texto")).await;
    click::finish(&ctx).await;
    // O título saiu com o intervalo curto; o botão, vizinho do título, já não tinha tempo para o longo.
    assert_eq!(pane.actions().first().map(String::as_str), Some("click 0 87"));
    assert!(started.elapsed() < Duration::from_millis(600), "{:?}", started.elapsed());
    assert_eq!(code(result), "erro_mod_clique_sem_resposta");
    assert_eq!(pane.actions(), ["click 0 87"]);
}

/// O espelho `pm()` com doze botões na faixa e o `pm-a` do primeiro painel fora da área visível.
fn pm_with_band_and_far_button() -> TerminalView {
    let mut v = pm();
    v.panes[0].tree = json!({"type": "Box", "children": [mods_support::pane::button("pm-a", "Botão lá embaixo", "pm-mock")]});
    v.above = json!({"type": "Box", "children": (0..12).map(|i| mods_support::pane::button(&format!("b{i}"), &format!("Botão {i}"), "pm-mock"))
        .collect::<Vec<_>>()});
    v
}

#[tokio::test]
async fn with_the_band_collapsed_the_wheel_runs_before_the_keyboard() {
    // Faixa recolhida: os doze botões dela saem do anel e não contam no que o teclado precisa. A roda tenta
    // primeiro (o botão está a ~40 linhas, uma por evento) e, no tempo que o teclado ainda cabe, passa a ele.
    let (mods, pane) = setup("tmux-140-faixa-recolhida-150", pm_with_band_and_far_button());
    for offset in 1..=200 { pane.on_wheel(vec![Scroll("pm-mock-pm", offset)]); }
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1")]);
    pane.on_keys("Tab", vec![Focus("pm-mock-pm", "pm-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-pm", "pm-a")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-9-prompt")]);
    let (limits, undo, clicked) = (Limits::default(), Undo::default(), Mutex::default());
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: Instant::now() + Duration::from_millis(7500), undo: &undo,
        life: 1, clicked: &clicked };
    let result = click::dispatch(&ctx, press_call("pm-mock-pm", "pm-a")).await;
    click::finish(&ctx).await;
    result.unwrap();
    assert!(pane.actions().iter().filter(|a| a.starts_with("wheel ")).count() > 5, "{:?}", pane.actions());
    assert_eq!(keys(&pane), ["C-x Tab", "Tab", "Enter", "C-x Tab"]);
}

#[tokio::test]
async fn the_ring_does_not_wait_the_screen_on_band_buttons_that_are_not_drawn() {
    // A faixa da árvore não aparece na tela (a âncora dela não está lá): cada `ctrl+x tab` pelos doze botões
    // não muda a tela, e o passo termina no `ui.focus` do botão, não na espera inteira da tela (300 ms).
    let (mods, pane) = setup("tmux-14-ciclo-4-prompt", pm_with_band_and_far_button());
    pane.mouse(false);
    for i in 0..12 { pane.on_keys("C-x Tab", vec![Focus("above-prompt", ["b0","b1","b2","b3","b4","b5","b6","b7","b8","b9","b10","b11"][i], false)]); }
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-6-painel-1")]);
    pane.on_keys("Tab", vec![Focus("pm-mock-pm", "pm-a", false)]);
    pane.on_keys("Enter", vec![Pressed("pm-mock-pm", "pm-a")]);
    pane.on_keys("C-x Tab", vec![Show("tmux-14-ciclo-9-prompt")]);
    let (limits, undo, clicked) = (Limits::default(), Undo::default(), Mutex::default());
    let started = Instant::now();
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: started + Duration::from_millis(7500), undo: &undo, life: 1,
        clicked: &clicked };
    let result = click::dispatch(&ctx, press_call("pm-mock-pm", "pm-a")).await;
    click::finish(&ctx).await;
    result.unwrap();
    assert!(started.elapsed() < Duration::from_millis(12 * 300), "{:?}", started.elapsed());
}

#[tokio::test]
async fn another_tab_in_front_and_a_far_button_keep_time_for_the_keyboard() {
    // Terminal ligado, outra aba na frente, o botão longe e a faixa da árvore sem desenho (doze botões no
    // anel). A escolha é feita antes de ativar a aba: a roda tenta até a hora em que o teclado ainda cabe, e o
    // anel passa pelos botões invisíveis sem esperar a tela, até o painel, dentro do prazo.
    let mut v = pm_with_far_button();
    v.above = pm_with_band_and_far_button().above;
    let (mods, pane) = setup("tmux-01-tres-paineis-150", v);
    pane.cost(Duration::from_millis(60));
    pane.on_click((0, 104), vec![Show("tmux-02-apos-clicar-mr-150")]);
    for offset in 1..=200 { pane.on_wheel(vec![Scroll("pm-mock-mr", offset)]); }
    for i in 0..12 { pane.on_keys("C-x Tab", vec![Focus("above-prompt", ["b0","b1","b2","b3","b4","b5","b6","b7","b8","b9","b10","b11"][i], false)]); }
    keyboard_to_mr(&pane);
    let (limits, undo, clicked) = (Limits::default(), Undo::default(), Mutex::default());
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: Instant::now() + Duration::from_millis(7500), undo: &undo,
        life: 1, clicked: &clicked };
    let result = click::dispatch(&ctx, press_call("pm-mock-mr", "mr-a")).await;
    click::finish(&ctx).await;
    result.unwrap();
    assert_eq!(pane.actions()[0], "click 0 104", "a aba foi ativada pelo mouse");
    assert!(pane.actions().iter().any(|a| a.starts_with("wheel ")), "a roda tentou antes");
    assert!(keys(&pane).contains(&"Enter".to_string()), "{:?}", keys(&pane));
}

#[tokio::test]
async fn a_pane_the_wheel_does_not_roll_goes_to_the_keyboard() {
    // A roda não rola o painel nem para baixo nem para cima (painel em caixa de poucas linhas): com tempo para
    // o teclado, o `Tab` dele rola até o botão, em vez de recusar.
    let (mods, pane) = setup("tmux-02-apos-clicar-mr-150", pm_with_far_button());
    keyboard_to_mr(&pane);
    let (limits, undo, clicked) = (Limits::quick(), Undo::default(), Mutex::default());
    let ctx = Ctx { name: S, pane: &pane, mods: &mods, limits: &limits, until: far(), undo: &undo, life: 1, clicked: &clicked };
    let result = click::dispatch(&ctx, press_call("pm-mock-mr", "mr-a")).await;
    click::finish(&ctx).await;
    result.unwrap();
    assert_eq!(pane.actions().iter().filter(|a| a.starts_with("wheel ")).count(), 2, "para baixo e para cima");
    assert!(keys(&pane).contains(&"Enter".to_string()));
}

#[tokio::test]
async fn the_same_key_twice_from_one_mod_on_the_terminal_triggers_neither() {
    // O mesmo mod com a mesma `key` duas vezes no painel: nenhuma ação chega ao pane, e a resposta é a do item
    // ausente.
    let mut v = pm();
    v.panes[1].tree = json!({"type": "Box", "children": [mods_support::pane::button("mr-a", "Abrir", "pm-mock"),
        mods_support::pane::button("mr-a", "Outro", "pm-mock")]});
    let (mods, pane) = setup("tmux-02-apos-clicar-mr-150", v);
    assert_eq!(code(press(&mods, &pane, "pm-mock-mr", "mr-a").await), "erro_mod_botao_inexistente");
    assert!(pane.actions().is_empty(), "{:?}", pane.actions());
}

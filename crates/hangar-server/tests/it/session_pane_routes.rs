//! R3: `pane`, `peer-address`, `limits`, `then` e `recarregar` respondidos pelo Rust, num tmux isolado com
//! panes reais (o `Rig` de `session_write_unowned`). O esperado vem do Python em c22f75dc8 (`api.py`,
//! `test_api.py`, `test_recarregar.py`, `test_tmux.py`) e das decisões 2 e 9; nada chega ao Python.
#![cfg(target_os = "linux")]
use serde_json::{Value, json};

use crate::fake::{OWNER, client};
use crate::session_write_unowned::{BLIND, GUEST, Rig};

fn detail(code: &str, msg: &str) -> Value { json!({"detail": {"code": code, "params": {}, "msg": msg}}) }

async fn call(rig: &Rig, method: reqwest::Method, path: &str, token: &str) -> (u16, Value) {
    let response = client().request(method, format!("http://{}/api/sessions/{path}", rig.server)).header("content-type", "application/json")
        .bearer_auth(token).body("{}").send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

async fn get(rig: &Rig, path: &str) -> (u16, Value) { call(rig, reqwest::Method::GET, path, OWNER).await }

/// Capture do próprio tmux, o oráculo: o mesmo `capture-pane -p` que o Python rodava.
fn oracle(rig: &Rig, name: &str, lines: u32) -> String { rig.tmux(&["capture-pane", "-p", "-t", &format!("={name}:"), "-S", &format!("-{lines}")]) }

/// 300 linhas coloridas num shell: histórico real acima da tela de 45 linhas.
async fn printed(rig: &Rig, name: &str) {
    rig.pane(name, r#"bash --norc --noprofile -c 'for i in $(seq 1 300); do printf "\033[31mlinha-%s\033[0m\n" $i; done; exec sleep 100000'"#);
    rig.screen_shows(name, "linha-300").await;
}

fn mute(rig: &Rig, on: bool) {
    let (live, off) = (rig.dir.path().join("tmux"), rig.dir.path().join("tmux.mudo"));
    if on { std::fs::rename(live, off).unwrap() } else { std::fs::rename(off, live).unwrap() }
}

// R3-01, R3-02, R3-06 (`api.py:2729-2744`, `tmux.py:1075-1091`): o texto cru do `capture-pane -p`, sem ANSI,
// com `lines` padrão 200 preso em 50..5000.
#[tokio::test]
async fn pane_r3_01_02_06_text_is_the_raw_capture_with_the_clamped_scrollback() {
    let rig = Rig::new().await;
    printed(&rig, "p1").await;
    let text = |reply: &Value| reply["text"].as_str().unwrap().to_owned();
    let (status, default) = get(&rig, "p1/pane").await;
    assert_eq!(status, 200, "{default}");
    assert_eq!(text(&default), oracle(&rig, "p1", 200));
    assert!(!text(&default).contains('\u{1b}'), "sem ANSI: {:?}", text(&default));
    for (query, clamped) in [("lines=10", 50), ("lines=-5", 50), ("lines=50", 50), ("lines=99999", 5000), ("lines=1_000", 1000)] {
        let (status, reply) = get(&rig, &format!("p1/pane?{query}")).await;
        assert_eq!((status, text(&reply)), (200, oracle(&rig, "p1", clamped)), "{query}");
    }
    let (_, short) = get(&rig, "p1/pane?lines=10").await;
    assert_ne!(text(&short), text(&default), "50 linhas não são as 200");
    let (_, whole) = get(&rig, "p1/pane?lines=99999").await;
    assert_eq!(text(&whole).lines().next(), Some("linha-1"), "5000 alcança o histórico inteiro");
    assert_eq!(rig.python.hits_to("/api/sessions/p1/pane"), 0);
}

// R3-03 (`lines: int` do FastAPI): o 422 `int_parsing` com o valor que veio.
#[tokio::test]
async fn pane_r3_03_a_non_integer_lines_is_the_fastapi_422() {
    let rig = Rig::new().await;
    rig.pane("p1", "bash --norc --noprofile");
    for raw in ["abc", "", "1.5"] {
        assert_eq!(get(&rig, &format!("p1/pane?lines={raw}")).await, (422, json!({"detail": [{"type": "int_parsing", "loc": ["query", "lines"],
            "msg": "Input should be a valid integer, unable to parse string as an integer", "input": raw}]})), "{raw:?}");
    }
    assert_eq!(rig.python.hits_to("/api/sessions/p1/pane"), 0);
}

// R3-04 (`api.py:2738`, `tmux.py:550-557`): sem sessão no tmux é 404; o alvo é exato, nunca o prefixo de outra.
#[tokio::test]
async fn pane_r3_04_a_name_without_tmux_session_is_404_even_with_a_prefix_sibling() {
    let rig = Rig::new().await;
    rig.pane("pfx-2", "bash --norc --noprofile");
    for name in ["ghost", "pfx"] {
        assert_eq!(get(&rig, &format!("{name}/pane")).await, (404, detail("erro_sessao_inexistente", "sessao nao encontrada")), "{name}");
        assert_eq!(rig.python.hits_to(&format!("/api/sessions/{name}/pane")), 0);
    }
}

// R3-05 (`tmux._pane_target` → `agentpane.resolve_target`): a janela nova do usuário não troca o que o espelho lê.
#[tokio::test]
async fn pane_r3_05_reads_the_agent_pane_not_the_active_one() {
    let rig = Rig::new().await;
    rig.pane("ag", "bash --norc --noprofile -c 'echo AGENTE-TELA; ( exec -a claude sleep 100000 ); exec sleep 100000'");
    rig.agent_runs("ag", "claude").await;
    rig.tmux(&["split-window", "-t", "=ag:", "bash --norc --noprofile -c 'echo SHELL-TELA; exec sleep 100000'"]);
    rig.screen_shows("ag", "SHELL-TELA").await;
    let (status, reply) = get(&rig, "ag/pane").await;
    let text = reply["text"].as_str().unwrap_or_default();
    assert_eq!(status, 200, "{reply}");
    assert!(text.contains("AGENTE-TELA") && !text.contains("SHELL-TELA"), "leu o pane ativo: {text:?}");
}

// R3-07 (`tmux.py:1054-1072`): `scrollback` é o histórico do tmux, 0 na tela alternada (o Claude).
#[tokio::test]
async fn pane_r3_07_scrollback_is_the_history_and_zero_on_the_alternate_screen() {
    let rig = Rig::new().await;
    printed(&rig, "p1").await;
    rig.pane("alt", r#"bash --norc --noprofile -c 'printf "\033[?1049h"; echo tela-alternada; exec sleep 100000'"#);
    rig.screen_shows("alt", "tela-alternada").await;
    let history: u64 = rig.tmux(&["display", "-p", "-t", "=p1:", "#{history_size}"]).trim().parse().unwrap();
    assert!(history > 0);
    assert_eq!(get(&rig, "p1/pane").await.1["scrollback"], history);
    assert_eq!(get(&rig, "alt/pane").await.1["scrollback"], 0);
}

// R3-08 (bug 3 do inventário; decisão 9): captura que falha com a sessão viva é erro com código, nunca 200 vazio.
#[tokio::test]
async fn pane_r3_08_a_failed_capture_is_an_error_with_a_code() {
    let rig = Rig::new().await;
    rig.pane("p1", "bash --norc --noprofile");
    rig.flag("fail-capture", true);
    let (status, reply) = get(&rig, "p1/pane").await;
    assert_eq!((status, reply["detail"]["code"].as_str()), (503, Some("capture_refused")), "{reply}");
    assert_eq!(rig.python.hits_to("/api/sessions/p1/pane"), 0);
}

// R3-09 (bug 2 do inventário; decisão 9): multiplexador que não responde é 503 com código, não "sessão não encontrada".
#[tokio::test]
async fn pane_r3_09_a_silent_multiplexer_is_503_not_404() {
    let rig = Rig::new().await;
    rig.pane("p1", "bash --norc --noprofile");
    mute(&rig, true);
    let (status, reply) = get(&rig, "p1/pane").await;
    mute(&rig, false);
    assert_eq!((status, reply["detail"]["code"].as_str()), (503, Some("capture_mux_no_answer")), "{reply}");
    assert_eq!(rig.python.hits_to("/api/sessions/p1/pane"), 0);
}

// R3-11 (decisão 1): o convidado que vê a sessão lê o espelho pelo mesmo caminho; quem não vê leva 403, e
// o desconhecido o 401 do Rust.
#[tokio::test]
async fn pane_r3_11_guest_reads_through_the_owner_path() {
    let rig = Rig::new().await;
    rig.pane("p1", "bash --norc --noprofile -c 'echo CONVIDADO-VE; exec sleep 100000'");
    rig.screen_shows("p1", "CONVIDADO-VE").await;
    let (status, reply) = call(&rig, reqwest::Method::GET, "p1/pane", GUEST).await;
    assert_eq!(status, 200, "{reply}");
    assert!(reply["text"].as_str().unwrap().contains("CONVIDADO-VE"));
    assert_eq!(call(&rig, reqwest::Method::GET, "p1/pane", BLIND).await, (403, detail("erro_fora_do_convidado", "fora do acesso do convidado")));
    assert_eq!(call(&rig, reqwest::Method::GET, "p1/pane", "errado").await, (401, detail("erro_nao_autorizado", "unauthorized")));
    assert_eq!(rig.python.hits_to("/api/sessions/p1/pane"), 0);
}

// R3-12..17 (decisão 9): `peer-address` saiu; quem alcança a sessão recebe o 404 de rota inexistente.
#[tokio::test]
async fn peer_address_r3_12_17_is_a_gone_route() {
    let rig = Rig::new().await;
    rig.pane("p1", "bash --norc --noprofile");
    let gone = (404, json!({"detail": "Not Found"}));
    for name in ["p1", "ghost"] {
        assert_eq!(get(&rig, &format!("{name}/peer-address")).await, gone, "{name}");
        assert_eq!(rig.python.hits_to(&format!("/api/sessions/{name}/peer-address")), 0);
    }
    assert_eq!(call(&rig, reqwest::Method::GET, "p1/peer-address", GUEST).await, gone);
    assert_eq!(call(&rig, reqwest::Method::GET, "p1/peer-address", "errado").await.0, 401);
}

// R3-19 (`test_limits_claude_rejected_with_400`) e R3-20 (decisão 9): sessão que não é Codex é o 400 da
// rota; nome que não existe é 404, não "só Codex".
#[tokio::test]
async fn limits_r3_19_20_not_codex_is_400_and_a_missing_name_is_404() {
    let rig = Rig::new().await;
    rig.pane("sh1", "bash --norc --noprofile");
    assert_eq!(get(&rig, "sh1/limits").await, (400, detail("erro_limits_so_codex", "limits so existe pra sessoes Codex")));
    assert_eq!(get(&rig, "ghost/limits").await, (404, detail("erro_sessao_inexistente", "sessão não encontrada")));
    assert_eq!(rig.python.hits_to("/api/sessions/sh1/limits") + rig.python.hits_to("/api/sessions/ghost/limits"), 0);
}

// R3-24: o convidado nunca arma nem limpa o `then` (alcança outra sessão do dono).
#[tokio::test]
async fn then_r3_24_the_guest_never_sets_nor_clears_the_link() {
    let rig = Rig::new().await;
    rig.pane("p1", "bash --norc --noprofile");
    for method in [reqwest::Method::PUT, reqwest::Method::DELETE] {
        assert_eq!(call(&rig, method.clone(), "p1/then", GUEST).await.0, 403, "{method}");
    }
    assert_eq!(rig.python.hits_to("/api/sessions/p1/then"), 0);
}

// R3-27, R3-28 (`test_reload_route_is_rust_only`): nome sem sessão é 404; pane de outro agente ou shell é o
// 409 da recarga; R3-30 (decisão 9): multiplexador calado é 503 com o código, nunca `session_open_needs_rust_server`.
#[tokio::test]
async fn reload_r3_27_28_30_what_no_rust_agent_serves_is_answered_in_rust() {
    let rig = Rig::new().await;
    rig.pane("sh1", "bash --norc --noprofile");
    let post = |path: &'static str| call(&rig, reqwest::Method::POST, path, OWNER);
    assert_eq!(post("ghost/recarregar").await, (404, detail("erro_sessao_inexistente", "sessão não encontrada")));
    let only_headless = (409, detail("erro_recarregar_so_sem_terminal", "recarregar só vale para sessão Claude sem terminal"));
    assert_eq!(post("sh1/recarregar").await, only_headless);
    mute(&rig, true);
    let (status, reply) = post("sh1/recarregar").await;
    mute(&rig, false);
    assert_eq!((status, reply["detail"]["code"].as_str()), (503, Some("erro_mux_indisponivel")), "{reply}");
    assert_eq!(rig.python.hits_to("/api/sessions/ghost/recarregar") + rig.python.hits_to("/api/sessions/sh1/recarregar"), 0);
}

// R3-31, R3-32 (decisão 2): o convidado completo recarrega pelo caminho do dono, e o diário diz quem pediu;
// quem não vê a sessão leva 403.
#[tokio::test]
async fn reload_r3_31_32_the_guest_reloads_through_the_owner_path_and_the_diary_names_it() {
    let rig = Rig::new().await;
    rig.pane("sh1", "bash --norc --noprofile");
    assert_eq!(call(&rig, reqwest::Method::POST, "sh1/recarregar", GUEST).await,
        (409, detail("erro_recarregar_so_sem_terminal", "recarregar só vale para sessão Claude sem terminal")));
    assert_eq!(call(&rig, reqwest::Method::POST, "sh1/recarregar", BLIND).await, (403, detail("erro_fora_do_convidado", "fora do acesso do convidado")));
    let python = rig.python.clone();
    crate::fake::wait_until(move || python.diag().iter().any(|line| line["evento"] == "sessao.recarga_pedida"
        && line["codigo"] == "session_reload_requested_by_guest" && line["sessao"] == "sh1")).await;
    assert_eq!(rig.python.hits_to("/api/sessions/sh1/recarregar"), 0);
}

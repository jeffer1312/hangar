//! As escritas que não são texto num nome sem agente do Rust (`Write::Unowned`), num tmux isolado com
//! panes reais (o `Rig` de `session_write_unowned`): nome sem sessão, shell, `claude login` e o Codex cru.
//! O status e o código vêm da rota Python (`api.py` em c22f75dc8, inventário R2-18..26); a decisão 6
//! troca a digitação no pane do login e do Codex cru por recusa, e a 7 recusa pergunta sem identidade.
#![cfg(target_os = "linux")]
use std::process::Command;

use serde_json::{Value, json};

use crate::fake::{OWNER, client};
use crate::session_write_unowned::Rig;

const MSG_UNBOUND: &str = "vínculo gerenciado indisponível; escrita suspensa";

fn detail(code: &str, msg: &str, params: Value) -> Value { json!({"detail": {"code": code, "params": params, "msg": msg}}) }

fn unbound() -> (u16, Value) { (502, detail("erro_envio_falhou", MSG_UNBOUND, json!({"erro": MSG_UNBOUND}))) }

async fn request(rig: &Rig, method: reqwest::Method, path: &str, body: &str) -> (u16, Value) {
    let response = client().request(method, format!("http://{}/api/sessions/{path}", rig.server)).header("content-type", "application/json")
        .bearer_auth(OWNER).body(body.to_owned()).send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

async fn post(rig: &Rig, path: &str, body: &str) -> (u16, Value) { request(rig, reqwest::Method::POST, path, body).await }

const ANSWER: &str = r#"{"answers":[{"question":"q","answer":"a"}],"interaction_id":"i-1"}"#;

/// As rotas de pane e de pergunta, com o corpo mínimo que cada uma aceita.
const PANE_ROUTES: [(&str, &str); 6] = [("interrupt", ""), ("interrupt?clear=true", ""), ("keys", r#"{"key":"Escape"}"#),
    ("term-input", r#"{"text":"ls"}"#), ("select/submit", r#"{"interaction_id":"i-1"}"#), ("answer", ANSWER)];

// R2-25, R2-26 (`api.py:1395-1419`; `test_queue_panel_routes_answer_unavailable_when_rust_does_not_serve`):
// fila sem ator não tem entrada a tirar nem a mexer, com ou sem pane.
#[tokio::test]
async fn queue_routes_without_a_rust_owner_are_the_python_refusals() {
    let rig = Rig::new().await;
    rig.pane("sh1", "bash --norc --noprofile");
    for name in ["ghost", "sh1"] {
        for path in [format!("{name}/queue/e1"), format!("{name}/queue/reorder"), format!("{name}/queue/e1?only_waiting=true")] {
            assert_eq!(request(&rig, reqwest::Method::DELETE, &path, "").await,
                (404, detail("erro_fila_entrada_nao_encontrada", "entrada não está na fila", json!({}))), "{path}");
        }
        let unavailable = (409, detail("queue_action_unavailable", "esta sessão não permite mexer na fila agora", json!({})));
        assert_eq!(request(&rig, reqwest::Method::PATCH, &format!("{name}/queue/e1"), r#"{"text":"x"}"#).await, unavailable, "{name}");
        assert_eq!(post(&rig, &format!("{name}/queue/reorder"), r#"{"id":"a","before_id":null}"#).await, unavailable, "{name}");
        assert_eq!(post(&rig, &format!("{name}/queue/e1/steer"), "").await, unavailable, "{name}");
        // O Python não olhava o corpo no painel da fila.
        assert_eq!(post(&rig, &format!("{name}/queue/e1/steer"), "não é json").await, unavailable, "{name}");
    }
    assert_eq!(rig.python.hits_to("/api/sessions/ghost/queue/e1") + rig.python.hits_to("/api/sessions/sh1/queue/e1"), 0);
}

// R2-18..24, u1 (nome sem pane nem sidecar): 404 onde o Python conferia a sessão, 502 onde ele caía no vínculo.
#[tokio::test]
async fn writes_to_a_name_without_session_are_refused_with_the_python_codes() {
    let rig = Rig::new().await;
    assert_eq!(post(&rig, "ghost/steer", "").await, (404, detail("erro_sessao_inexistente", "sessão não encontrada", json!({}))));
    let missing = (404, detail("erro_sessao_opcao_nao_enviada", "sessão não encontrada — opção NÃO enviada", json!({})));
    assert_eq!(post(&rig, "ghost/select", r#"{"option":2,"interaction_id":"i-1"}"#).await, missing, "test_select_404_when_session_missing");
    assert_eq!(post(&rig, "ghost/select/submit", r#"{"interaction_id":"i-1"}"#).await, missing);
    for (route, body) in [("interrupt", ""), ("keys", r#"{"key":"Escape"}"#), ("term-input", r#"{"text":"ls"}"#), ("answer", ANSWER)] {
        assert_eq!(post(&rig, &format!("ghost/{route}"), body).await, unbound(), "{route}");
    }
    for route in ["steer", "select", "select/submit", "interrupt", "keys", "term-input", "answer"] {
        assert_eq!(rig.python.hits_to(&format!("/api/sessions/ghost/{route}")), 0, "{route}");
    }
}

// Decisão 7 (B5): `select`, `select/submit` e `answer` sem `interaction_id` são 409 sem identidade antes
// de olhar o nome; corpo que não é objeto é o 422 do Rust (decisão 11).
#[tokio::test]
async fn questions_without_an_interaction_id_are_409_before_the_name() {
    let rig = Rig::new().await;
    rig.pane("sh1", "bash --norc --noprofile");
    for name in ["ghost", "sh1"] {
        for (route, body) in [("select", r#"{"option":1}"#), ("select", r#"{"option":1,"interaction_id":null}"#), ("select", r#"{"option":1,"interaction_id":""}"#),
            ("select/submit", ""), ("select/submit", "{}"), ("answer", r#"{"answers":[{"question":"q","answer":"a"}]}"#)] {
            let (status, reply) = post(&rig, &format!("{name}/{route}"), body).await;
            assert_eq!((status, reply["detail"]["code"].as_str()), (409, Some("erro_interacao_sem_identidade")), "{name} {route} {body}");
        }
        for (route, body) in [("select", "[]"), ("answer", "não é json"), ("keys", "5"), ("term-input", "\"ls\"")] {
            let (status, reply) = post(&rig, &format!("{name}/{route}"), body).await;
            assert_eq!((status, reply["detail"]["code"].as_str()), (422, Some("erro_corpo_invalido")), "{name} {route} {body}");
        }
    }
}

// R2-18..24, u2 (shell): o 502 do vínculo; o `select` com o 503 da opção não enviada. Nada é digitado.
#[tokio::test]
async fn writes_to_a_shell_pane_are_refused_and_nothing_is_typed() {
    let rig = Rig::new().await;
    rig.pane("sh1", "bash --norc --noprofile");
    // O prompt do bash aparece quando ele quer: espera-o, para a tela de antes já tê-lo.
    rig.screen_shows("sh1", "$").await;
    let before = rig.tmux(&["capture-pane", "-p", "-t", "=sh1:"]);
    assert_eq!(post(&rig, "sh1/steer", r#"{"text":"oi"}"#).await, unbound());
    assert_eq!(post(&rig, "sh1/select", r#"{"option":1,"interaction_id":"i-1"}"#).await,
        (503, detail("erro_opcao_nao_convergiu", "não consegui responder pelo terminal — opção NÃO enviada", json!({"detalhe": MSG_UNBOUND}))));
    for (route, body) in PANE_ROUTES {
        assert_eq!(post(&rig, &format!("sh1/{route}"), body).await, unbound(), "{route}");
    }
    let after = rig.tmux(&["capture-pane", "-p", "-t", "=sh1:"]);
    assert!(!after.contains("ls") && !after.contains("oi"), "nada digitado no shell: {after:?}");
    assert_eq!(after, before, "a tela não mudou");
}

// R2-19..24, u3 e u4 (B6, decisão 6): o Python digitava no `claude login` e no Codex cru (Esc, número,
// setas, texto); o Rust recusa com o 502 do vínculo e a TUI não lê nada.
#[tokio::test]
async fn writes_to_claude_login_or_raw_codex_never_type_into_the_pane() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    let cwd = rig.dir.path().join("w");
    std::fs::create_dir_all(&cwd).unwrap();
    assert!(Command::new("mkfifo").arg(cwd.join("login")).status().unwrap().success());
    rig.pane("lg1", "bash --norc --noprofile -c '( exec -a claude cat login ); exec sleep 100000'");
    rig.agent_runs("lg1", "claude").await;
    let login = rig.tmux(&["capture-pane", "-p", "-t", "=lg1:"]);
    for name in ["cx", "lg1"] {
        assert_eq!(post(&rig, &format!("{name}/steer"), "").await, unbound(), "{name} steer");
        // R2-20: o código e o detalhe da opção não enviada, não só o status.
        assert_eq!(post(&rig, &format!("{name}/select"), r#"{"option":1,"interaction_id":"i-1"}"#).await,
            (503, detail("erro_opcao_nao_convergiu", "não consegui responder pelo terminal — opção NÃO enviada", json!({"detalhe": MSG_UNBOUND}))), "{name} select");
        for (route, body) in PANE_ROUTES {
            assert_eq!(post(&rig, &format!("{name}/{route}"), body).await, unbound(), "{name} {route}");
        }
    }
    assert_eq!(rig.tui("cx", "keys.jsonl"), Vec::<Value>::new(), "a TUI do Codex cru não leu nada");
    assert_eq!(rig.tmux(&["capture-pane", "-p", "-t", "=lg1:"]), login, "nada digitado no login");
    assert_eq!(rig.queue("cx"), None, "nenhuma linha na fila");
}

// R2-23, R2-24 (`api.py:2746,2760`, `key not allowed`): num nome sem dono no Rust (sem sessão, shell, Codex
// cru), tecla fora da lista é o 400 da validação antes da recusa do vínculo, e nada é digitado.
#[tokio::test]
async fn a_key_outside_the_list_on_a_name_without_owner_is_400_and_nothing_is_typed() {
    let rig = Rig::new().await;
    rig.pane("sh1", "bash --norc --noprofile");
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    rig.screen_shows("sh1", "$").await;
    let shell = rig.tmux(&["capture-pane", "-p", "-t", "=sh1:"]);
    let refused = (400, json!({"detail": "tecla não permitida"}));
    for name in ["ghost", "sh1", "cx"] {
        assert_eq!(post(&rig, &format!("{name}/keys"), r#"{"key":"Nope"}"#).await, refused, "{name} keys");
        assert_eq!(post(&rig, &format!("{name}/term-input"), r#"{"text":"ls","key":"Nope"}"#).await, refused, "{name} term-input");
        // Tecla da lista segue na recusa do vínculo.
        assert_eq!(post(&rig, &format!("{name}/keys"), r#"{"key":"Escape"}"#).await, unbound(), "{name}");
        assert_eq!(rig.python.hits_to(&format!("/api/sessions/{name}/keys")) + rig.python.hits_to(&format!("/api/sessions/{name}/term-input")), 0, "{name}");
    }
    assert_eq!(rig.tui("cx", "keys.jsonl"), Vec::<Value>::new(), "a TUI do Codex cru não leu nada");
    assert_eq!(rig.tmux(&["capture-pane", "-p", "-t", "=sh1:"]), shell, "nada digitado no shell");
}

// R2-17 (decisão da rodada 3): Codex com terminal cujo arquivo de sessão o Rust não adota. Ler o modo de
// permissão vem do arquivo (200); trocar o modo e implementar o plano seguem recusados, e a TUI não lê tecla.
#[tokio::test]
async fn a_terminal_codex_rust_does_not_adopt_reads_its_mode_from_the_file_and_types_nothing() {
    let rig = Rig::build_with(true, false, &[]).await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    let sidecars = rig.home.join(".hangar/codex-sessions");
    std::fs::create_dir_all(&sidecars).unwrap();
    std::fs::write(sidecars.join("cx.json"), json!({"name": "cx", "provider": "codex", "headless": false, "key": "k-cx",
        "permission_mode": "Approve for me"}).to_string()).unwrap();
    let (status, modes) = request(&rig, reqwest::Method::GET, "cx/codex-permissions", "").await;
    assert_eq!((status, modes["current"].as_str()), (200, Some("Approve for me")), "{modes}");
    let marked: Vec<_> = modes["modes"].as_array().unwrap().iter().filter(|m| m["atual"] == true).map(|m| m["nome"].clone()).collect();
    assert_eq!(marked, [json!("Approve for me")]);
    let control = (409, detail("erro_codex_controle", "O Codex não aceitou a alteração; atualize a sessão e tente novamente.", json!({})));
    assert_eq!(post(&rig, "cx/codex-permissions", r#"{"mode":"Full Access"}"#).await, control);
    assert_eq!(post(&rig, "cx/codex/plan/implement", "").await, control);
    assert_eq!(rig.tui("cx", "keys.jsonl"), Vec::<Value>::new(), "a TUI do Codex não leu nada");
    assert_eq!(rig.python.hits_to("/api/sessions/cx/codex-permissions") + rig.python.hits_to("/api/sessions/cx/codex/plan/implement"), 0);
}

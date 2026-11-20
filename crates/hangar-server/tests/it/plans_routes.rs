//! Rotas de plano e o catálogo de customizações no Rust: os casos dos testes Python que saíram
//! (`test_planprog_api.py`, `test_plano_claude.py` e os de pin do `test_api.py`), com os mesmos
//! códigos e corpos. Sessões Claude de mentira (`list_support`); a `s0` trabalha em `w0`.
#![cfg(unix)]
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::fake::{OWNER, client};
use crate::list_support;
use serde_json::{Value, json};

const PLAN: &str = "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: verificação manual**\n";

async fn call(addr: SocketAddr, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut req = client().request(method.parse().unwrap(), format!("http://{addr}{path}")).bearer_auth(OWNER);
    if let Some(body) = body { req = req.header("content-type", "application/json").body(body.to_string()); }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

fn code(v: &Value) -> &str { v["detail"]["code"].as_str().unwrap_or("") }

/// Repositório em `w0` com `.git` e o plano de exemplo; devolve a pasta de planos.
fn repo(root: &Path) -> PathBuf {
    let plans = root.join("w0/docs/superpowers/plans");
    std::fs::create_dir_all(&plans).unwrap();
    std::fs::create_dir_all(root.join("w0/.git")).unwrap();
    std::fs::write(plans.join("2026-07-29-plano.md"), PLAN).unwrap();
    plans
}

#[tokio::test(flavor = "multi_thread")]
async fn plan_detail_carries_stem_index_manual_and_markdown() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_support::server(dir.path(), 1).await;
    let (status, body) = call(srv.addr, "GET", "/api/sessions/s0/plan", None).await;
    assert_eq!((status, code(&body)), (404, "erro_sem_plano_ativo"));
    let (status, body) = call(srv.addr, "GET", "/api/sessions/s0/plans", None).await;
    assert_eq!((status, code(&body)), (404, "erro_sem_pasta_planos"));
    let (status, body) = call(srv.addr, "GET", "/api/sessions/nada/plan", None).await;
    assert_eq!((status, code(&body)), (404, "erro_sessao_inexistente"));

    let plans = repo(dir.path());
    let (status, j) = call(srv.addr, "GET", "/api/sessions/s0/plan", None).await;
    assert_eq!(status, 200, "{j}");
    assert_eq!((j["name"].as_str(), j["stem"].as_str()), (Some("plano"), Some("2026-07-29-plano")));
    assert_eq!(j["path"], json!(plans.join("2026-07-29-plano.md")));
    assert_eq!((j["done"].as_u64(), j["total"].as_u64(), j["complete"].as_bool()), (Some(1), Some(2), Some(false)));
    assert_eq!((j["task"].as_u64(), j["task_total"].as_u64()), (Some(1), Some(1)));
    assert_eq!(j["tasks"], json!([{"title": "Task 1: X", "done": 1, "total": 2, "steps": [
        {"title": "Step 1: A", "done": true, "manual": false, "idx": 0},
        {"title": "Step 2: verificação manual", "done": false, "manual": true, "idx": 1}]}]));
    assert_eq!(j["markdown"], PLAN);
    let (status, j) = call(srv.addr, "GET", "/api/sessions/s0/plans", None).await;
    assert_eq!((status, j), (200, json!({"plans": [{"stem": "2026-07-29-plano", "name": "plano", "done": 1, "total": 2, "complete": false}], "pinned": null})));
}

#[tokio::test(flavor = "multi_thread")]
async fn pin_validates_the_stem_and_writes_the_pin() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_support::server(dir.path(), 1).await;
    let plans = repo(dir.path());
    std::fs::write(dir.path().join("w0/segredo.md"), "x").unwrap();
    let pin_file = dir.path().join("w0/.git/cp-plan-pin");
    let (status, body) = call(srv.addr, "POST", "/api/sessions/s0/plan-pin", Some(json!({"stem": "../../../segredo"}))).await;
    assert_eq!((status, code(&body)), (400, "erro_nome_plano_invalido"));
    assert_eq!(body["detail"]["params"], json!({"nome": "../../../segredo"}));
    assert!(!pin_file.exists());
    let (status, body) = call(srv.addr, "POST", "/api/sessions/s0/plan-pin", Some(json!({"stem": "2026-07-30-nada"}))).await;
    assert_eq!((status, code(&body)), (404, "erro_plano_nao_encontrado"));
    let (status, body) = call(srv.addr, "POST", "/api/sessions/s0/plan-pin", Some(json!({"stem": "2026-07-29-plano"}))).await;
    assert_eq!((status, body), (200, json!({"pinned": "2026-07-29-plano"})));
    assert_eq!(std::fs::read_to_string(&pin_file).unwrap(), "2026-07-29-plano\n");
    let (_, j) = call(srv.addr, "GET", "/api/sessions/s0/plans", None).await;
    assert_eq!(j["pinned"], "2026-07-29-plano");
    let (status, body) = call(srv.addr, "POST", "/api/sessions/s0/plan-pin", Some(json!({"stem": "!none"}))).await;
    assert_eq!((status, body), (200, json!({"pinned": "!none"})));
    let (status, body) = call(srv.addr, "GET", "/api/sessions/s0/plan", None).await;
    assert_eq!((status, code(&body)), (404, "erro_sem_plano_ativo"), "escondido não cai na eleição");
    let (status, body) = call(srv.addr, "POST", "/api/sessions/s0/plan-pin", Some(json!({}))).await;
    assert_eq!((status, body), (200, json!({"pinned": null})));
    assert!(!pin_file.exists());
    let (status, _) = call(srv.addr, "POST", "/api/sessions/s0/plan-pin", Some(json!({"stem": "x", "outro": 1}))).await;
    assert_eq!(status, 422, "campo desconhecido, como o _StrictBody");
    let _ = plans;
}

#[tokio::test(flavor = "multi_thread")]
async fn step_marks_and_answers_the_new_progress() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_support::server(dir.path(), 1).await;
    let plans = repo(dir.path());
    std::fs::write(dir.path().join("w0/fora.md"), PLAN).unwrap();
    let (status, body) = call(srv.addr, "POST", "/api/sessions/s0/plan-step", Some(json!({"stem": "2026-07-29-plano", "idx": 1, "done": true}))).await;
    assert_eq!((status, body), (200, json!({"done": 2, "total": 2, "complete": true})));
    assert!(std::fs::read_to_string(plans.join("2026-07-29-plano.md")).unwrap().contains("- [x] **Step 2:"));
    let (status, body) = call(srv.addr, "POST", "/api/sessions/s0/plan-step", Some(json!({"stem": "2026-07-29-plano", "idx": 99, "done": true}))).await;
    assert_eq!((status, code(&body)), (409, "erro_marcar_step"));
    assert!(body["detail"]["params"]["erro"].as_str().unwrap().contains("99"));
    let (status, body) = call(srv.addr, "POST", "/api/sessions/s0/plan-step", Some(json!({"stem": "../../../fora", "idx": 0, "done": false}))).await;
    assert_eq!((status, code(&body)), (409, "erro_marcar_step"));
    assert!(std::fs::read_to_string(dir.path().join("w0/fora.md")).unwrap().contains("- [x] **Step 1:"));
    let (status, _) = call(srv.addr, "POST", "/api/sessions/s0/plan-step", Some(json!({"stem": "2026-07-29-plano", "idx": "um", "done": true}))).await;
    assert_eq!(status, 422);
}

#[tokio::test(flavor = "multi_thread")]
async fn archive_moves_to_done_and_refuses_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_support::server(dir.path(), 1).await;
    let plans = repo(dir.path());
    let (status, body) = call(srv.addr, "POST", "/api/sessions/s0/plan-archive", Some(json!({"stem": "nada"}))).await;
    assert_eq!((status, code(&body)), (409, "erro_arquivar_plano"));
    let (status, body) = call(srv.addr, "POST", "/api/sessions/s0/plan-archive", Some(json!({"stem": "2026-07-29-plano"}))).await;
    assert_eq!((status, body), (200, json!({"moved": ["2026-07-29-plano.md"]})));
    assert!(plans.join("feitos/2026-07-29-plano.md").is_file() && !plans.join("2026-07-29-plano.md").exists());
    let (status, body) = call(srv.addr, "GET", "/api/sessions/s0/plan", None).await;
    assert_eq!((status, code(&body)), (404, "erro_sem_plano_ativo"), "arquivado sai do painel na hora");
}

/// O transcript da `s0`, trocado pelo do teste; a conta é `home/.claude`.
fn transcript(root: &Path, lines: &[Value]) -> PathBuf {
    let proj = root.join("home/.claude/projects").join(hangar_workspace::worktrees::sanitize_cwd(root.join("w0").to_str().unwrap()));
    let path = proj.join("00000000-0000-0000-0000-000000000000.jsonl");
    std::fs::write(&path, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn preview_shows_the_native_plan_and_its_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_support::server(dir.path(), 1).await;
    let (status, body) = call(srv.addr, "GET", "/api/sessions/s0/plan-preview", None).await;
    assert_eq!((status, body), (200, Value::Null), "sem proposta");
    let plan = hangar_workspace::real(&dir.path().join("home/.claude/plans/escolhido.md"));
    let write = json!({"type": "assistant", "uuid": "a-write", "isSidechain": false, "message": {"content": [
        {"type": "tool_use", "id": "w1", "name": "Write", "input": {"file_path": plan}}]}});
    let done = json!({"type": "user", "uuid": "r1", "isSidechain": false, "message": {"content": [
        {"type": "tool_result", "tool_use_id": "w1", "content": "ok"}]}});
    let reply = json!({"type": "assistant", "uuid": "a-plan", "isSidechain": false, "message": {"content": [{"type": "text", "text": "Plano."}]}});
    transcript(dir.path(), &[write, done, reply]);
    let (status, body) = call(srv.addr, "GET", "/api/sessions/s0/plan-preview?content=false", None).await;
    assert_eq!((status, body), (200, json!({"name": "escolhido", "path": plan, "anchor_id": "a-plan"})));
    let (status, body) = call(srv.addr, "GET", "/api/sessions/s0/plan-preview", None).await;
    assert_eq!((status, code(&body)), (404, "erro_plano_removido"));
    std::fs::create_dir_all(plan.parent().unwrap()).unwrap();
    std::fs::write(&plan, "# Escolhido").unwrap();
    let (status, body) = call(srv.addr, "GET", "/api/sessions/s0/plan-preview", None).await;
    assert_eq!((status, body), (200, json!({"name": "escolhido", "path": plan, "anchor_id": "a-plan", "markdown": "# Escolhido"})));
    let (status, _) = call(srv.addr, "GET", "/api/sessions/s0/plan-preview?content=talvez", None).await;
    assert_eq!(status, 422);
    let (status, body) = call(srv.addr, "GET", "/api/sessions/nada/plan-preview", None).await;
    assert_eq!((status, code(&body)), (404, "erro_sessao_inexistente"));
}

#[tokio::test(flavor = "multi_thread")]
async fn customization_catalog_validates_folder_and_account() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_support::server(dir.path(), 1).await;
    let (status, body) = call(srv.addr, "GET", "/api/claude/customizations?cwd=/nao/existe/mesmo", None).await;
    assert_eq!((status, code(&body)), (400, "erro_cwd_inexistente"));
    assert_eq!(body["detail"]["params"], json!({"cwd": "/nao/existe/mesmo"}));
    let cwd = dir.path().join("w0");
    let (status, body) = call(srv.addr, "GET", &format!("/api/claude/customizations?cwd={}&config_dir=/conta/inventada", cwd.display()), None).await;
    assert_eq!((status, code(&body)), (400, "erro_config_dir_invalido"));
    let (status, _) = call(srv.addr, "GET", "/api/claude/customizations", None).await;
    assert_eq!(status, 422, "cwd obrigatório");
}

#[tokio::test(flavor = "multi_thread")]
async fn strangers_get_401_from_rust() {
    let dir = tempfile::tempdir().unwrap();
    let srv = list_support::server(dir.path(), 1).await;
    repo(dir.path());
    // R5-09: rota que o Python também tinha, com `require_auth`: o mesmo 401, agora do Rust.
    let resp = client().get(format!("http://{}/api/sessions/s0/plan", srv.addr)).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 401);
}

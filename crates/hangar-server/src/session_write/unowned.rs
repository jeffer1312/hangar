//! Texto para sessão que o Rust não adotou (`adopt_now` sem dono): nome sem pane nem sidecar, pane de
//! shell ou programa qualquer, `claude login` e o Codex cru (`command codex`, sem sidecar). Só o Codex
//! cru recebe digitação, pela mesma condução com prova do Pi/omp/Kimi (`other_terminal::drive`), sem
//! ator, e só com a caixa de digitar vazia e pronta: fora disso é 409 com código, sem fila (ninguém a
//! drenaria). O resto é recusa com código, na hora. Respostas no formato do `_send_one` do Python.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::detail_body;
use crate::list::discover::{PaneSession, Resolver, discover_panes};
use crate::list::discover_other::Dirs;
use crate::list::mux::Pane;
use crate::routes::AppState;
use crate::runtime::other_terminal::{OtherIo, OtherOptions, OtherTarget, ProcessOtherIo, drive};
use crate::terminal_input::{Disposition, TerminalBinding};

const MSG_UNBOUND: &str = "vínculo gerenciado indisponível; escrita suspensa";
const MSG_LOGIN: &str = "sessão Claude sem vínculo no Rust";
const MSG_CONTROL: &str = "control characters not allowed in prompt";
const MSG_PARTIAL: &str = "envio incompleto: parte do texto ficou no composer da sessao e nada foi submetido. Confira o terminal antes de reenviar.";
/// O teto do `PromptQueue` do Python.
const QUEUE_MAX: usize = 1000;
/// Espera máxima pela vez de digitar na sessão, abaixo do prazo de 3 s da entrega adiada do Connect.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Uma digitação por sessão: dois recados no mesmo tty sairiam concatenados. A chave leva o servidor
/// (a lista dele): dois servidores no mesmo processo não disputam o mesmo nome.
static LOCKS: LazyLock<Mutex<HashMap<(usize, String), Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Default::default);

/// `Missing`: nem pane nem sidecar. `Answer`: status e o corpo do `_send_one` (`{ok, error, delivered?, native?}`).
pub(crate) enum Sent { Missing, Answer(StatusCode, Value) }

/// O que a digitação precisa, possuído: ela segue numa tarefa própria.
#[derive(Clone)]
struct Deps {
    list: Arc<crate::list::bridge::ListBridge>,
    terminal: crate::terminal_control::TerminalPool,
    interactions: Arc<crate::interactions::InteractionStore>,
    diag: crate::diag::DiagClient,
}

/// O envio inteiro, numa tarefa à parte: quem pediu pode desistir (cliente fechou), a digitação começada
/// termina e a fila fica gravada, como o `asyncio.shield` do Python.
pub(crate) async fn send(st: &AppState, name: &str, text: &str) -> Sent {
    let deps = Deps { list: st.list.clone(), terminal: st.terminal.clone(), interactions: st.interactions.clone(), diag: st.diag.clone() };
    let (name, text) = (name.to_owned(), text.to_owned());
    match tokio::spawn(async move { send_owned(deps, &name, &text).await }).await {
        Ok(sent) => sent,
        Err(error) => {
            tracing::error!(code = "unowned_send_aborted", panicked = error.is_panic(), "a tarefa do envio caiu");
            refused("erro_envio_falhou", "o envio parou no meio", json!({"erro": "unowned_send_aborted"}), false)
        }
    }
}

/// O `/input`: 404 sem sessão, o recado entregue ou adiado, ou a recusa com o `detail` dela.
pub(crate) fn input_answer(sent: Sent) -> (StatusCode, Value) {
    match sent {
        Sent::Missing => (StatusCode::NOT_FOUND, detail_body("erro_sessao_recado_nao_enfileirado", "sessão não encontrada — recado NÃO enfileirado", json!({}))),
        Sent::Answer(status, body) if status.is_success() => (StatusCode::OK, json!({"ok": true, "delivered": body["delivered"] == true, "steered": false, "native": false})),
        Sent::Answer(status, body) => (status, json!({"detail": body["error"]})),
    }
}

/// A linha do nome no `/api/broadcast`.
pub(crate) fn broadcast_result(sent: Sent) -> Value {
    match sent {
        Sent::Missing => json!({"ok": false, "error": detail_body("erro_sessao_inexistente", "sessão não encontrada", json!({}))["detail"], "delivered": false}),
        Sent::Answer(_, body) => body,
    }
}

fn refused(code: &str, msg: &str, params: Value, delivered: bool) -> Sent {
    let mut body = json!({"ok": false, "error": detail_body(code, msg, params)["detail"]});
    if delivered { body["delivered"] = json!(false); }
    Sent::Answer(StatusCode::BAD_REQUEST, body)
}

fn unbound(deps: &Deps, name: &str, code: &str) -> Sent {
    deps.diag.report("runtime.send_failed", name, code, "pane sem agente que o Rust escreva; nada foi digitado");
    refused("erro_envio_falhou", MSG_UNBOUND, json!({"erro": MSG_UNBOUND}), false)
}

async fn send_owned(deps: Deps, name: &str, text: &str) -> Sent {
    let context = deps.list.runtime_context(name).await.ok();
    let Some(context) = context.filter(|context| !context.panes.is_empty()) else { return no_pane(&deps, name).await };
    let (panes, children, procs, projects) = (context.panes.clone(), context.children.clone(), deps.list.env().procs.clone(), context.dirs.claude.join("projects"));
    let found = tokio::task::spawn_blocking(move || {
        let row = discover_panes(&panes, &*procs, &children, &projects, &mut Resolver::default(), &|_| false).into_iter().next();
        let argv = row.as_ref().and_then(|row| row.agent_pid).map(|pid| procs.argv(pid)).unwrap_or_default();
        (row, argv)
    }).await;
    let Ok((Some(row), argv)) = found else { return unbound(&deps, name, "unowned_pane_unreadable") };
    match (row.provider, row.agent_pid) {
        ("codex", Some(_)) => codex(&deps, name, text, &context.panes, &context.children, &context.dirs, &context.config_dirs, &row).await,
        ("claude", Some(_)) if crate::runtime::claude_terminal::login(&argv) => refused("erro_envio_falhou", MSG_LOGIN, json!({}), true),
        _ => unbound(&deps, name, "unowned_pane"),
    }
}

/// Sem pane: sidecar que o Rust não adotou é sessão estragada, com código; sem sidecar, não existe.
async fn no_pane(deps: &Deps, name: &str) -> Sent {
    let Some(home) = deps.list.env().dirs.as_ref().map(|dirs| dirs.home.clone()) else { return Sent::Missing };
    let owned = name.to_owned();
    let sidecar = tokio::task::spawn_blocking(move || {
        use crate::runtime::codex_sidecar::{dir, path};
        if path(&dir(&home), &owned).exists() { Some("codex_sidecar_invalid") }
        else if path(&home.join(".hangar").join("claude-headless"), &owned).exists() { Some("claude_sidecar_invalid") } else { None }
    }).await.ok().flatten();
    match sidecar {
        Some(code) => {
            deps.diag.report("runtime.send_failed", name, code, "sidecar sem dono no Rust; nada foi enviado");
            Sent::Answer(StatusCode::SERVICE_UNAVAILABLE, json!({"ok": false, "error": detail_body(code, "o arquivo da sessão não abre no servidor; feche e abra a sessão", json!({}))["detail"], "delivered": false}))
        }
        None => Sent::Missing,
    }
}

#[allow(clippy::too_many_arguments)]
async fn codex(deps: &Deps, name: &str, text: &str, panes: &[Pane], children: &crate::list::procs::ChildrenMap, dirs: &Dirs, config_dirs: &[PathBuf], row: &PaneSession) -> Sent {
    if text.chars().any(|c| u32::from(c) < 32 && c != '\t' && c != '\n') {
        return refused("erro_envio_falhou", &format!("falha ao enviar: {MSG_CONTROL}"), json!({"erro": MSG_CONTROL}), false);
    }
    // Pane do Codex com sidecar é do `codex --remote` gerenciado: o Rust o adota, nunca digita cru nele.
    let sidecar = crate::runtime::codex_sidecar::path(&crate::runtime::codex_sidecar::dir(&dirs.home), name);
    if tokio::task::spawn_blocking(move || sidecar.exists()).await.unwrap_or(true) {
        deps.diag.report("runtime.send_failed", name, "codex_sidecar_invalid", "Codex gerenciado que o Rust não adotou; nada foi digitado");
        return Sent::Answer(StatusCode::SERVICE_UNAVAILABLE, json!({"ok": false, "error": detail_body("codex_sidecar_invalid",
            "o arquivo da sessão não abre no servidor; feche e abra a sessão", json!({}))["detail"], "delivered": false}));
    }
    let key = (Arc::as_ptr(&deps.list) as usize, name.to_owned());
    let lock = LOCKS.lock().unwrap_or_else(|e| e.into_inner()).entry(key.clone()).or_default().clone();
    // Fila curta atrás de outro recado: esperar além do prazo de quem pediu (3 s no Connect) faria o
    // reenvio dele entrar depois deste, em dobro.
    let sent = match tokio::time::timeout(LOCK_WAIT, lock.clone().lock_owned()).await {
        Ok(held) => {
            let sent = codex_locked(deps, name, text, panes, children, dirs, config_dirs, row).await;
            drop(held);
            sent
        }
        Err(_) => Sent::Answer(StatusCode::CONFLICT, json!({"ok": false, "error": detail_body("erro_envio_em_andamento",
            "outro recado ainda está sendo digitado nesta sessão; tente de novo em instantes", json!({}))["detail"], "delivered": false})),
    };
    let mut locks = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    // Só o mapa e esta cópia: ninguém mais espera a vez, a entrada sai.
    if Arc::strong_count(&lock) <= 2 { locks.remove(&key); }
    sent
}

#[allow(clippy::too_many_arguments)]
async fn codex_locked(deps: &Deps, name: &str, text: &str, panes: &[Pane], children: &crate::list::procs::ChildrenMap, dirs: &Dirs, config_dirs: &[PathBuf], row: &PaneSession) -> Sent {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let Some(target) = target(deps, name, panes, children, dirs, config_dirs, row) else { return unbound(deps, name, "unowned_codex_unproved") };
    let io = ProcessOtherIo::new(&target, &OtherOptions::new(deps.terminal.clone(), crate::provider_models::inbox::INBOX.clone(),
        deps.list.env().procs.clone(), deps.interactions.clone()));
    let result = drive::prompt(&target, &io, text, None).await;
    io.release().await;
    match result.disposition {
        Disposition::Accepted => {}
        // Fora da caixa vazia e pronta nada é digitado nem guardado: a pessoa vê o erro e reenvia.
        Disposition::Deferred => return not_at_composer(deps, name, &result.code),
        Disposition::Rejected => return refused("erro_envio_falhou", &format!("falha ao enviar: {}", result.code), json!({"erro": result.code}), false),
        // Parou com texto na caixa: como o Python no Codex, sem limpar e sem Enter.
        Disposition::Unknown => {
            deps.diag.report("rust.unowned_send_partial", name, &result.code, "texto ficou pela metade no Codex cru; Enter não enviado");
            return refused("erro_envio_incompleto_composer", MSG_PARTIAL, json!({}), false);
        }
    }
    let queue = crate::locate::queue_path(&dirs.claude, name);
    let stripped = text.trim_start();
    if stripped.starts_with('/') {
        // Comando de barra não tem linha na fila; o `/clear` que rodou leva a fila junto (a conversa é outra).
        if stripped.split_whitespace().next() == Some("/clear") {
            let _ = tokio::task::spawn_blocking(move || clear_queue(&queue)).await;
        }
    } else {
        let row = json!({"id": crate::mods::state::random_hex(16), "text": text, "ts": ts, "delivered": true});
        let written = tokio::task::spawn_blocking(move || append(&queue, row)).await.map_err(std::io::Error::other).and_then(|r| r);
        if let Err(error) = written {
            // Digitado: o recado chegou; só a fila ficou sem a linha.
            tracing::error!(session = %name, %error, "fila indisponível depois de digitar no Codex cru; o recado foi entregue sem linha na fila");
        }
    }
    Sent::Answer(StatusCode::OK, json!({"ok": true, "error": null, "delivered": true, "native": false}))
}

/// 409 com código: menu ou diálogo aberto (`codex_menu_open`) ou qualquer outra tela que não é a caixa
/// vazia e pronta (`codex_composer_busy`), com o motivo da condução.
fn not_at_composer(deps: &Deps, name: &str, reason: &str) -> Sent {
    let (code, msg) = if reason == "codex_menu_open" {
        ("codex_menu_open", "há um menu ou diálogo aberto no terminal do Codex; nada foi enviado. Feche-o e mande de novo.")
    } else {
        ("codex_composer_busy", "a caixa de digitar do Codex não está vazia e pronta; nada foi enviado. Mande de novo.")
    };
    deps.diag.report("runtime.send_failed", name, code, "Codex cru fora da caixa vazia; nada foi digitado nem guardado");
    Sent::Answer(StatusCode::CONFLICT, json!({"ok": false, "error": detail_body(code, msg, json!({"motivo": reason}))["detail"], "delivered": false}))
}

/// A vida do Codex cru, montada da descoberta como a dos atores do Pi/omp/Kimi, sem fila nem conversa.
fn target(deps: &Deps, name: &str, panes: &[Pane], children: &crate::list::procs::ChildrenMap, dirs: &Dirs, config_dirs: &[PathBuf], row: &PaneSession) -> Option<OtherTarget> {
    let agent = row.agent_pid?;
    let pane = panes.iter().find(|pane| pane.pid.is_some_and(|shell| {
        let shell = i64::from(shell);
        shell == agent || crate::list::discover::descendants(shell, children).contains(&agent)
    }))?;
    let (created, pane_pid) = (pane.session_created?, i64::from(pane.pid?));
    let birth = deps.list.env().procs.start_time(agent).filter(|birth| birth.is_finite() && *birth > 0.0)?;
    let queue_dir = dirs.claude.join(".hangar-queue");
    Some(OtherTarget {
        key: format!("unowned-codex:{name}"), generation: 0, name: name.to_owned(), provider: hangar_transcript::Provider::Codex,
        binding: TerminalBinding { name: name.to_owned(), pane: pane.pane_id.clone(), conversation: String::new(), generation: 0, created,
            mux_argv: vec![deps.list.env().capture_program.to_string_lossy().into_owned()], windows: cfg!(windows),
            clipboard_lock_path: Some(queue_dir.join("runtime").join("clipboard.lock")) },
        pane_address: pane.target()?, dirs: dirs.clone(), cwd: pane.cwd.clone(), pane_pid, agent_pid: agent, agent_birth: birth,
        transcript: PathBuf::new(), state_path: PathBuf::new(), lease_path: PathBuf::new(), projection_dir: queue_dir,
        config_dirs: config_dirs.to_vec(), created: birth.max(created as f64),
    })
}

/// `PromptQueue.append`: linhas JSON, teto de 1000, escrita atômica; linha ilegível fica de fora.
fn append(path: &Path, row: Value) -> std::io::Result<()> {
    let mut rows: Vec<Value> = match crate::runtime::queue::read_replaced(path) {
        Ok(raw) => String::from_utf8_lossy(&raw).lines().filter_map(|line| serde_json::from_str(line.trim()).ok()).collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error),
    };
    rows.push(row);
    let keep = rows.len().saturating_sub(QUEUE_MAX);
    let mut out = Vec::new();
    for row in &rows[keep..] { serde_json::to_writer(&mut out, row)?; out.push(b'\n'); }
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    crate::runtime::queue::atomic_write(path, &out)
}

fn clear_queue(path: &Path) {
    for file in [path.to_path_buf(), path.with_extension("jsonl.tmp")] {
        if let Err(error) = std::fs::remove_file(&file) && error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %file.display(), %error, "fila do Codex cru não apagada no /clear");
        }
    }
}

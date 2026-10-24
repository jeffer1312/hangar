//! Loop runner (objetivo → conferência → re-prompt até passar) e vínculo `then` (quando a sessão
//! termina o turno, um texto vai para outra), servidos e acionados pelo Rust. Os sidecars ficam no
//! formato do Python, lidos no lugar; o fim de turno vem das transições do vigia de avisos.
pub mod check;
pub mod http;
pub mod refine;
pub mod store;
pub mod tick;
pub mod watch;

use crate::routes::AppState;
use futures_util::future::BoxFuture;
use hangar_api::session::SessionRow;
use std::path::Path;
use std::sync::Arc;

/// A pasta do Claude que a lista lê (os badges de loop e `then` saem dela).
fn claude_dir(st: &AppState) -> std::path::PathBuf {
    st.list.env().dirs.as_ref().map_or_else(|| st.accounts.env.claude_base.clone(), |d| d.claude.clone())
}

pub fn store(st: &AppState) -> store::Store { store::Store::new(&claude_dir(st)) }

/// A chave mestra das automações (`automations_enabled` do Python).
pub fn automations(st: &AppState) -> bool {
    crate::push::watch::Settings::read(&claude_dir(st), &crate::push::server_env()).automations
}

pub fn notify(st: &Arc<AppState>, name: &str, status: &str, reason: &str) {
    let sender = crate::push::send::Sender::new(st.clone(), crate::push::server_env());
    sender.notify(name, crate::push::send::Notice::Loop { status: status.into(), reason: reason.into() });
}

/// Texto na fila durável da sessão Claude, esperando a vez (`delivered: false`); devolve o id.
pub async fn append(st: &AppState, name: &str, text: &str) -> Result<String, crate::runtime::protocol::RuntimeError> {
    let runtime = st.state.runtime.get()
        .ok_or_else(|| crate::runtime::protocol::RuntimeError { code: "runtime_unavailable".into(), message: "runtime indisponível".into() })?;
    runtime.adopt_claude_terminal(name).await?;
    let id = crate::mods::state::random_hex(16);
    runtime.queue_action(name, format!("loop-append:{id}"), crate::runtime::queue::Action::Append {
        text: text.into(), delivered: false, ts: None, pre_transcript: false, entry_id: Some(id.clone()) }).await?;
    Ok(id)
}

/// Entrega já, se a sessão aceita; senão a fila entrega no próximo fim de turno.
pub async fn drain(st: &AppState, name: &str) {
    let Some(runtime) = st.state.runtime.get() else { return };
    if let Err(e) = runtime.drain(name).await {
        tracing::warn!(session = name, code = %e.code, "loop: drain falhou; a entrada fica na fila");
    }
}

/// Branch da worktree onde o agente trabalha (a mesma que a lista mostra).
pub fn branch(row: &SessionRow) -> Option<String> {
    let dir = row.git_cwd.as_deref().or(row.cwd.as_deref())?;
    let root = hangar_workspace::worktrees::repo_root_of(dir);
    hangar_workspace::git::head_info(Some(root.as_deref().unwrap_or(dir))).0
}

/// Texto da última resposta do assistente (`transcript.last_assistant_text`).
pub fn last_assistant(jsonl: &Path) -> Option<String> {
    use std::io::BufRead;
    let file = std::io::BufReader::new(std::fs::File::open(jsonl).ok()?);
    let mut parser = crate::transcript::LineParser::new(crate::transcript::Provider::Claude);
    let mut last = None;
    for line in file.split(b'\n') {
        let Ok(line) = line else { break };
        for e in parser.feed(&line, 0) {
            if matches!(e.kind, hangar_api::chat::ChatKind::AssistantMsg) && e.text.as_deref().is_some_and(|t| !t.is_empty()) { last = e.text; }
        }
    }
    last
}

/// O tique de verdade de uma sessão, com a linha resolvida na hora em que foi agendado.
pub struct Live { pub st: Arc<AppState>, pub row: SessionRow }

impl tick::Env for Live {
    fn now(&self) -> f64 { crate::nav_requests::now() }
    fn automations(&self) -> bool { automations(&self.st) }
    fn branch(&self) -> Option<String> { branch(&self.row) }
    fn entry_delivered(&self, entry_id: Option<String>) -> BoxFuture<'_, Option<bool>> {
        Box::pin(async move {
            let id = entry_id?;
            let runtime = self.st.state.runtime.get()?;
            let action = crate::runtime::queue::Action::EntryDelivered { entry_id: id };
            match runtime.queue_action(&self.row.name, format!("loop-anchor:{}", crate::mods::state::random_hex(8)), action).await {
                Ok(v) => v.as_bool(),
                // Sem a fila não dá para provar a entrega: espera o próximo fim de turno.
                Err(e) => { tracing::warn!(session = %self.row.name, code = %e.code, "loop: entrega do objetivo ilegível"); Some(false) }
            }
        })
    }
    fn run_check(&self, cmd: String) -> BoxFuture<'_, (i64, String)> {
        Box::pin(async move { check::run(&cmd, Path::new(self.row.cwd.as_deref().unwrap_or("")), check::TIMEOUT).await })
    }
    fn last_assistant(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async move {
            let jsonl = self.row.jsonl.clone()?;
            tokio::task::spawn_blocking(move || last_assistant(Path::new(&jsonl))).await.ok().flatten()
        })
    }
    fn enqueue(&self, text: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            append(&self.st, &self.row.name, &text).await.map_err(|e| e.code)?;
            drain(&self.st, &self.row.name).await;
            Ok(())
        })
    }
    fn notify(&self, status: &str, reason: &str) { notify(&self.st, &self.row.name, status, reason); }
}

#[cfg(test)]
pub(crate) mod tests {
    pub(crate) fn golden() -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/loop/cases.json");
        serde_json::from_slice(&std::fs::read(path).expect("golden do Python (gen_loop.py)")).unwrap()
    }

    #[test]
    fn push_bodies_match_the_python_in_each_locale() {
        for c in golden()["push_bodies"].as_array().unwrap() {
            let n = crate::push::send::Notice::Loop { status: c["status"].as_str().unwrap().into(), reason: c["reason"].as_str().unwrap().into() };
            assert_eq!(n.body("pt"), c["pt"].as_str().unwrap(), "{c}");
            assert_eq!(n.body("en"), c["en"].as_str().unwrap(), "{c}");
        }
    }

    #[test]
    fn last_assistant_skips_empty_text_like_python() {
        let dir = tempfile::tempdir().unwrap();
        let j = dir.path().join("t.jsonl");
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"oi"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"primeira"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"trabalho feito. LOOP_DONE"}]}}"#,
        ];
        std::fs::write(&j, lines.join("\n")).unwrap();
        assert!(super::last_assistant(&j).unwrap().contains("LOOP_DONE"));
        assert!(super::last_assistant(&dir.path().join("nada.jsonl")).is_none());
    }
}

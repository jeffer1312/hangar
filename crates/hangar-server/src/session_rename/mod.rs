//! `POST /api/sessions/{name}/rename`, inteiro no Rust. Tudo o que pode recusar é conferido antes do
//! primeiro efeito; a troca segura as portas de entrada dos dois nomes e a reserva do nome novo; falha
//! no meio desfaz o que já mudou, em ordem inversa, e o que não voltou sai na resposta e no diário.
pub mod files;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};

use crate::list::discover_other::sanitize_session_name;
use crate::routes::{AppState, cors, pass, who};
use crate::runtime::gateway::{Detached, RuntimeRegistry};
use crate::session_write::{busy_body, detail_body, json_response, session_name};
use crate::state::capture::{MuxProcess, TIMEOUT};

/// Mesmo teto da criação (`session_open::resolve::NAME_MAX`).
const NAME_MAX: usize = 64;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Body { new: String }

fn reply(headers: &HeaderMap, status: StatusCode, body: Value) -> Response {
    let mut response = json_response(status, body);
    cors(headers, response.headers_mut());
    response
}

fn refuse(headers: &HeaderMap, status: StatusCode, code: &str, msg: &str, params: Value) -> Response {
    reply(headers, status, detail_body(code, msg, params))
}

pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, caller) = who(&st, peer, &req);
    let name = session_name(req.uri().path()).filter(|n| caller.as_ref().is_some_and(|c| c.can(n, req.method())));
    // Quem não é o dono nem convidado com acesso completo: o Python atende (convidado com conta) ou recusa.
    let Some(name) = name else { return pass(&st, req, &fwd).await };
    let (parts, body) = req.into_parts();
    let headers = parts.headers;
    let Some(runtime) = st.state.runtime.get().cloned() else {
        return refuse(&headers, StatusCode::SERVICE_UNAVAILABLE, "rename_needs_rust_server", "o rename precisa do servidor Rust", json!({}));
    };
    let Some(dirs) = st.list.env().dirs.clone() else {
        return refuse(&headers, StatusCode::SERVICE_UNAVAILABLE, "list_dirs_missing", "as pastas das sessões não foram configuradas", json!({}));
    };
    let bytes = to_bytes(body, 64 * 1024).await.ok();
    let ctx = Ctx { st: st.clone(), runtime, home: dirs.home, claude: dirs.claude, mux: MuxProcess::new(st.list.env().capture_program.clone(), TIMEOUT) };
    // Cliente que cai no meio não interrompe a troca: ela termina (ou desfaz) numa tarefa própria.
    let task = tokio::spawn(async move { ctx.rename(&name, bytes.as_deref()).await });
    match task.await {
        Ok(Ok(new)) => reply(&headers, StatusCode::OK, json!({"ok": true, "name": new})),
        Ok(Err(Refusal(status, body))) => reply(&headers, status, body),
        Err(_) => refuse(&headers, StatusCode::INTERNAL_SERVER_ERROR, "sessao_falha_renomear", "falha ao renomear", json!({"etapa": "task"})),
    }
}

struct Refusal(StatusCode, Value);

fn no(status: StatusCode, code: &str, msg: &str) -> Refusal { Refusal(status, detail_body(code, msg, json!({}))) }
fn in_use() -> Refusal { no(StatusCode::CONFLICT, "erro_nome_em_uso", "ja existe uma sessao com esse nome") }
fn mux_down() -> Refusal { no(StatusCode::SERVICE_UNAVAILABLE, "erro_mux_indisponivel", "o multiplexador não respondeu") }

struct Ctx { st: Arc<AppState>, runtime: Arc<RuntimeRegistry>, home: PathBuf, claude: PathBuf, mux: MuxProcess }

/// O que já mudou, na ordem: o desfazer percorre ao contrário.
enum Done { Group, Mux, Sidecar(PathBuf), File(PathBuf, PathBuf), ParkedQueue(String), Guest, Spec, Transfer, Share, Pairs, Shortcuts(Vec<String>), HiddenShell }

#[derive(PartialEq)]
enum Kind { ClaudeHeadless, CodexHeadless, Terminal }

fn claude_dir(home: &Path) -> PathBuf { home.join(".hangar").join("claude-headless") }
fn codex_dir(home: &Path) -> PathBuf { crate::runtime::codex_sidecar::dir(home) }

async fn blocking<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(job).await.unwrap_or_else(|error| std::panic::resume_unwind(error.into_panic()))
}

impl Ctx {
    async fn rename(&self, old: &str, body: Option<&[u8]>) -> Result<String, Refusal> {
        match crate::groups::orq::is_orchestrator(&self.st, &[old.to_owned()]).await {
            Ok(found) if found.is_empty() => {},
            Ok(_) => return Err(no(StatusCode::CONFLICT, "erro_sessao_orq", "o orquestrador não recebe mensagens; fale com o árbitro")),
            Err(failed) => return Err(Refusal(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed}))),
        }
        // A troca de agente em curso recusa antes de olhar o corpo, como a guarda do Python.
        self.transfer_free(old).await?;
        let body: Body = body.and_then(|b| serde_json::from_slice(b).ok())
            .ok_or_else(|| no(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "Pedido em formato inválido para esta rota; nada foi enviado."))?;
        let new = sanitize_session_name(&body.new);
        if new.is_empty() { return Err(no(StatusCode::BAD_REQUEST, "erro_nome_invalido", "nome invalido")); }
        if new.len() > NAME_MAX { return Err(no(StatusCode::BAD_REQUEST, "erro_nome_invalido", &format!("nome invalido: use até {NAME_MAX} caracteres"))); }
        let kind = self.kind(old).await?;
        if new == old { return Ok(new); }
        self.transfer_free(&new).await?;
        self.check_free(&new).await?;
        // Criação simultânea com o nome novo, e outro rename da mesma sessão, esperam por aqui.
        let _claims = (crate::session_open::open::NameClaim::take(&self.home, &new).map_err(|_| in_use())?,
            crate::session_open::open::NameClaim::take(&self.home, old).map_err(|_| Refusal(StatusCode::CONFLICT, busy_body()))?);
        let mut names = [old.to_owned(), new.clone()];
        names.sort();
        let gates = self.runtime.ingress();
        let mut closed = Vec::new();
        for name in &names {
            // Exclusiva: a troca de agente (porta retida) recusa os dois lados enquanto o rename durar.
            closed.push(crate::runtime::lifecycle::ClosedIngress::acquire_exclusive(gates, name).await
                .map_err(|e| if e.code == "session_transfer_busy" { Refusal(StatusCode::CONFLICT, busy_body()) }
                    else { no(StatusCode::CONFLICT, "ingress_busy", "a sessão está ocupada com outra escrita; tente de novo") })?);
        }
        // De novo, já com as portas fechadas e o nome reservado: a troca que começou antes delas.
        self.transfer_free(old).await?;
        self.transfer_free(&new).await?;
        self.check_free(&new).await?;
        let mut done = Vec::new();
        let mut detached = None;
        match self.apply(kind, old, &new, &mut done, &mut detached).await {
            Ok(()) => {
                self.after(old, &new, &done).await;
                Ok(new)
            },
            // O grupo é o primeiro passo: recusado, nada mudou.
            Err(("group", code)) => Err(Refusal(StatusCode::SERVICE_UNAVAILABLE,
                detail_body("erro_grupo_indisponivel", "os grupos estão indisponíveis agora", json!({"detalhe": code})))),
            Err((step, code)) => Err(self.undo(old, &new, done, detached, step, &code).await),
        }
    }

    async fn transfer_free(&self, name: &str) -> Result<(), Refusal> {
        let (home, name) = (self.home.clone(), name.to_owned());
        match blocking(move || files::transfer_busy(&home, &name)).await {
            Ok(false) => Ok(()),
            Ok(true) => Err(Refusal(StatusCode::CONFLICT, busy_body())),
            Err(_) => Err(no(StatusCode::CONFLICT, "erro_criacao_sessao", "índice de transferência inválido")),
        }
    }

    async fn sidecar_exists(&self, dir: PathBuf, name: &str) -> bool {
        let path = crate::runtime::codex_sidecar::path(&dir, name);
        blocking(move || path.exists()).await
    }

    async fn kind(&self, old: &str) -> Result<Kind, Refusal> {
        if self.sidecar_exists(claude_dir(&self.home), old).await { return Ok(Kind::ClaudeHeadless); }
        let path = crate::runtime::codex_sidecar::path(&codex_dir(&self.home), old);
        // Verdade do Python: `bool(meta.get("headless"))`.
        let headless = blocking(move || crate::runtime::codex_sidecar::load(&path)).await
            .is_some_and(|meta| meta.get("headless").is_some_and(truthy));
        if headless { return Ok(Kind::CodexHeadless); }
        match self.mux.has_session(old).await {
            Some(true) => Ok(Kind::Terminal),
            Some(false) => Err(no(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "sessao nao encontrada")),
            None => Err(mux_down()),
        }
    }

    async fn check_free(&self, new: &str) -> Result<(), Refusal> {
        if self.sidecar_exists(claude_dir(&self.home), new).await || self.sidecar_exists(codex_dir(&self.home), new).await { return Err(in_use()); }
        match self.mux.has_session(new).await {
            Some(false) => Ok(()),
            Some(true) => Err(in_use()),
            None => Err(mux_down()),
        }
    }

    /// Cada passo grava em `done` assim que muda algo. `Err((etapa, código))`.
    async fn apply(&self, kind: Kind, old: &str, new: &str, done: &mut Vec<Done>, detached: &mut Option<Detached>) -> Result<(), (&'static str, String)> {
        self.group(old, new).await.map_err(|code| ("group", code))?;
        done.push(Done::Group);
        *detached = self.runtime.detach_for_rename(old).await.map_err(|e| ("runtime_close", e.code))?;
        if kind == Kind::Terminal {
            if !self.mux_rename(old, new).await { return Err(("mux", "sessao_falha_renomear".into())); }
            done.push(Done::Mux);
        }
        for dir in [claude_dir(&self.home), codex_dir(&self.home)] {
            let (d, o, n) = (dir.clone(), old.to_owned(), new.to_owned());
            if blocking(move || files::move_sidecar(&d, &o, &n)).await.map_err(|e| ("sidecar", io_code(&e)))? { done.push(Done::Sidecar(dir)); }
        }
        if detached.is_none() {
            let parked = match self.parked_queue_key(new).await {
                Some(key) => self.runtime.rename_parked_queue(&key, new).await.map_err(|e| ("queue", e.code))?.then_some(key),
                None => None,
            };
            match parked {
                Some(key) => done.push(Done::ParkedQueue(key)),
                // Sem estado indexado, a fila é só a projeção no nome.
                None => self.move_file(files::queue_file(&self.claude, old), files::queue_file(&self.claude, new), done).await.map_err(|c| ("queue", c))?,
            }
        }
        self.move_file(files::then_file(&self.claude, old), files::then_file(&self.claude, new), done).await.map_err(|c| ("then", c))?;
        self.move_file(files::bastao_file(&self.claude, old), files::bastao_file(&self.claude, new), done).await.map_err(|c| ("bastao", c))?;
        let (claude, o, n) = (self.claude.clone(), old.to_owned(), new.to_owned());
        if blocking(move || files::move_guest(&claude, &o, &n)).await.map_err(|e| ("guest", io_code(&e)))? { done.push(Done::Guest); }
        let (home, o, n) = (self.home.clone(), old.to_owned(), new.to_owned());
        if blocking(move || files::move_spec(&home, &o, &n)).await.map_err(|e| ("spec", io_code(&e)))? { done.push(Done::Spec); }
        let (meta, life) = self.identity(new).await;
        let (home, o, n) = (self.home.clone(), old.to_owned(), new.to_owned());
        if blocking(move || files::move_transfer(&home, &o, &n, life.as_deref(), meta.as_ref())).await.map_err(|e| ("transfer", io_code(&e)))? { done.push(Done::Transfer); }
        if let Some(share) = self.st.share.clone() {
            // Registrado antes de conferir: a falha deixa a memória no nome novo e o desfazer regrava.
            done.push(Done::Share);
            share.store.rename(old, new).map_err(|e| ("share", io_code(&e)))?;
            done.push(Done::Pairs);
            share.pairs.rename_local(old, new).map_err(|e| ("share_pairs", io_code(&e)))?;
        }
        let owned = self.shortcuts(old, new).await.map_err(|(owned, code)| { done.push(Done::Shortcuts(owned)); ("shortcuts", code) })?;
        done.push(Done::Shortcuts(owned));
        if self.hidden_shell(old, new).await.map_err(|c| ("hidden_shell", c))? { done.push(Done::HiddenShell); }
        if let Some(d) = detached.clone() {
            // Falhou: o desfazer reabre pelo nome velho com a mesma entrada (a fila volta junto).
            self.runtime.reattach_after_rename(d, new).await.map_err(|error| ("runtime_open", error.code))?;
            *detached = None;
        }
        Ok(())
    }

    /// `rename-session` cujo prazo estourado não prova nada: o nome real é relido antes de decidir.
    async fn mux_rename(&self, from: &str, to: &str) -> bool {
        match self.mux.rename_session(from, to).await {
            Some(renamed) => renamed,
            None => self.mux.has_session(to).await == Some(true) && self.mux.has_session(from).await == Some(false),
        }
    }

    async fn move_file(&self, src: PathBuf, dst: PathBuf, done: &mut Vec<Done>) -> Result<(), String> {
        let (s, d) = (src.clone(), dst.clone());
        if blocking(move || files::move_file(&s, &d)).await.map_err(|e| io_code(&e))? { done.push(Done::File(src, dst)); }
        Ok(())
    }

    /// A chave da sessão sem terminal parada (sem entrada aqui), pelo sidecar já no nome novo.
    async fn parked_queue_key(&self, new: &str) -> Option<String> {
        for dir in [claude_dir(&self.home), codex_dir(&self.home)] {
            let path = crate::runtime::codex_sidecar::path(&dir, new);
            if let Some(meta) = blocking(move || crate::runtime::codex_sidecar::load(&path)).await {
                return meta.get("key").and_then(Value::as_str).filter(|k| !k.is_empty()).map(str::to_owned);
            }
        }
        None
    }

    /// O sidecar já no nome novo e a vida da sessão (`k:<chave>` ou `t:<nascimento no multiplexador>`).
    async fn identity(&self, new: &str) -> (Option<Value>, Option<String>) {
        for dir in [codex_dir(&self.home), claude_dir(&self.home)] {
            let path = crate::runtime::codex_sidecar::path(&dir, new);
            if let Some(meta) = blocking(move || crate::runtime::codex_sidecar::load(&path)).await {
                let life = meta.get("key").and_then(Value::as_str).filter(|k| !k.is_empty()).map(|k| format!("k:{k}"));
                return (Some(Value::Object(meta)), life);
            }
        }
        let created = self.mux.run(&["display-message", "-p", "-t", &format!("={new}:"), "#{session_created}"]).await.ok()
            .filter(|out| out.status.success()).map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned()).filter(|c| !c.is_empty());
        (None, created.map(|c| format!("t:{c}")))
    }

    async fn group(&self, old: &str, new: &str) -> Result<(), String> {
        let Some(groups) = self.st.groups.clone() else { return Ok(()) };
        match crate::groups::fed_exit::rename(&self.st, &groups, old, new).await {
            Some(done) => done,
            None => groups.rename(old, new).await,
        }.map_err(|e| match e { crate::groups::service::GroupError::Store(_) => "groups_store_failed".to_owned(), other => other.to_string() })?;
        crate::groups::link_return::renamed(&self.st, old, new).await;
        Ok(())
    }

    /// Terminais de atalho cujo dono é a sessão (`@cp_shortcut_owner`). `Err` leva os já trocados.
    async fn shortcuts(&self, old: &str, new: &str) -> Result<Vec<String>, (Vec<String>, String)> {
        let out = self.mux.run(&["list-sessions", "-F", "#{session_name}\t#{@cp_shortcut_owner}"]).await.map_err(|_| (Vec::new(), "mux_unavailable".to_owned()))?;
        let mut owned = Vec::new();
        if !out.status.success() {
            // Sem servidor do multiplexador não há atalho; outra recusa é não saber quais trocar.
            if crate::list::mux::is_absence(&String::from_utf8_lossy(&out.stderr)) { return Ok(owned); }
            return Err((owned, "mux_unavailable".into()));
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Some((session, owner)) = line.split_once('\t') else { continue };
            if owner != old { continue; }
            // Antes de trocar: com o prazo estourado a troca pode ter acontecido, e o desfazer tenta voltar.
            owned.push(session.to_owned());
            if !self.set_owner(session, new).await { return Err((owned, "shortcut_owner_unchanged".into())); }
        }
        Ok(owned)
    }

    async fn set_owner(&self, session: &str, owner: &str) -> bool {
        self.mux.run(&["set-option", "-t", &format!("={session}:"), "@cp_shortcut_owner", owner]).await.is_ok_and(|out| out.status.success())
    }

    /// O shell escondido do painel (`term-<velho>`), só com a marca `@cp_hidden`, vira `term-<novo>`. Com
    /// `term-<novo>` ocupado o velho morre, mas só no fim (`after`): matar não se desfaz.
    async fn hidden_shell(&self, old: &str, new: &str) -> Result<bool, String> {
        let (from, to) = (format!("term-{old}"), format!("term-{new}"));
        if !self.mux.is_hidden(&from).await { return Ok(false); }
        match self.mux.has_session(&to).await {
            Some(true) => Ok(false),
            Some(false) if self.mux_rename(&from, &to).await => Ok(true),
            Some(false) => Err("hidden_shell_unrenamed".into()),
            None => Err("mux_unavailable".into()),
        }
    }

    /// Depois que tudo mudou: o que não se desfaz e o que só limpa.
    async fn after(&self, old: &str, new: &str, done: &[Done]) {
        let from = format!("term-{old}");
        if !done.iter().any(|d| matches!(d, Done::HiddenShell)) && self.mux.is_hidden(&from).await && self.mux.kill_session(&from).await != Some(true) {
            tracing::warn!(session = %new, code = "hidden_shell_kept", "o shell escondido do nome antigo não fechou no rename");
            self.st.diag.report("sessao.rename_shell", new, "hidden_shell_kept", "o shell escondido do nome antigo ficou aberto");
        }
        self.st.list.rename(old, new);
        self.st.list.invalidate();
    }

    /// Volta o que mudou, do último ao primeiro. O que não voltou vai na resposta e no diário.
    async fn undo(&self, old: &str, new: &str, done: Vec<Done>, detached: Option<Detached>, step: &str, code: &str) -> Refusal {
        let mut left = Vec::new();
        for item in done.into_iter().rev() {
            let (what, ok) = match item {
                Done::Group => ("group", self.group(new, old).await.is_ok()),
                // Cada um foi gravado só quando moveu algo: voltar é mover de novo (`Ok(true)`).
                Done::Mux => ("mux", self.mux_rename(new, old).await),
                Done::Sidecar(dir) => {
                    let (o, n) = (old.to_owned(), new.to_owned());
                    ("sidecar", matches!(blocking(move || files::move_sidecar(&dir, &n, &o)).await, Ok(true)))
                },
                Done::File(src, dst) => ("file", matches!(blocking(move || files::move_file(&dst, &src)).await, Ok(true))),
                Done::ParkedQueue(key) => ("queue", matches!(self.runtime.rename_parked_queue(&key, old).await, Ok(true))),
                Done::Guest => {
                    let (c, o, n) = (self.claude.clone(), old.to_owned(), new.to_owned());
                    ("guest", matches!(blocking(move || files::move_guest(&c, &n, &o)).await, Ok(true)))
                },
                Done::Spec => {
                    let (h, o, n) = (self.home.clone(), old.to_owned(), new.to_owned());
                    ("spec", matches!(blocking(move || files::move_spec(&h, &n, &o)).await, Ok(true)))
                },
                Done::Transfer => {
                    let (meta, life) = self.identity(old).await;
                    let (h, o, n) = (self.home.clone(), old.to_owned(), new.to_owned());
                    ("transfer", matches!(blocking(move || files::move_transfer(&h, &n, &o, life.as_deref(), meta.as_ref())).await, Ok(true)))
                },
                Done::Share => ("share", self.st.share.as_ref().is_none_or(|s| s.store.rename(new, old).is_ok())),
                Done::Pairs => ("share_pairs", self.st.share.as_ref().is_none_or(|s| s.pairs.rename_local(new, old).is_ok())),
                Done::Shortcuts(owned) => {
                    let mut ok = true;
                    for session in owned { ok &= self.set_owner(&session, old).await; }
                    ("shortcuts", ok)
                },
                Done::HiddenShell => ("hidden_shell", self.mux_rename(&format!("term-{new}"), &format!("term-{old}")).await),
            };
            if !ok { left.push(what); }
        }
        if let Some(d) = detached && let Err(error) = self.runtime.reattach_after_rename(d, old).await {
            tracing::warn!(session = %old, code = %error.code, "a sessão não reabriu no nome antigo depois do rename desfeito");
            left.push("runtime_open");
        }
        tracing::warn!(session = %old, new = %new, step, code, not_undone = ?left, "rename falhou no meio e foi desfeito");
        let reason = if left.is_empty() { "o rename falhou no meio e foi desfeito" } else { "o rename falhou no meio e parte ficou com o nome novo" };
        self.st.diag.report("sessao.rename_falhou", old, code, reason);
        let msg = if left.is_empty() { "falha ao renomear".to_owned() }
            else { format!("falha ao renomear; ficaram com o nome novo ({new}): {}", left.join(", ")) };
        let status = if code == "erro_nome_em_uso" { StatusCode::CONFLICT } else { StatusCode::INTERNAL_SERVER_ERROR };
        let main = if code == "erro_nome_em_uso" { "erro_nome_em_uso" } else { "sessao_falha_renomear" };
        Refusal(status, detail_body(main, &msg, json!({"etapa": step, "codigo": code, "nao_desfeito": left})))
    }
}

fn io_code(error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::AlreadyExists { "erro_nome_em_uso".into() } else { format!("io_{:?}", error.kind()).to_lowercase() }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

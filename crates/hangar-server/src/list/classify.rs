//! Estado das linhas Claude da lista (`registry.list_with_state`): marcador do hook primeiro, pane
//! só quando ele não basta, statusline com teto de capturas e radar de limite da travada. Pura
//! sobre as linhas, os arquivos de fatos, os quadros e o relógio que `CaptureSource` entrega.
//! Codex, Pi, omp e Kimi ficam de fora: o estado deles vem dos fatos do Python.
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use hangar_api::session::SessionRow;
use serde_json::Value;

use super::capped::SESSION_CAP;
use super::facts_files::{self, HookStates};
use crate::state::capture::MuxProcess;
use crate::state::published::Published;
use crate::terminal_state::{analyze, PaneAnalysis};
use hangar_api::state::StateEvent;

/// Escrita no transcript até 1 s depois do idle é o resumo pós-Stop, não turno novo.
pub const IDLE_STALE_S: f64 = 1.0;
/// O Notification de "idle 60s" chega depois do Stop; antes disso o menu pode ainda não ter desenhado.
pub const AWAITING_DEMOTE_GRACE_S: f64 = 10.0;
pub const STATUS_TTL_S: f64 = 20.0;
pub const STATUS_BUDGET: usize = 2;
pub const LIMIT_TTL_S: f64 = 30.0;
/// Intervalo da segunda captura: spinner que não mudou nele está congelado no scrollback.
pub const SPINNER_RECHECK: Duration = Duration::from_millis(150);
/// Sessão sem terminal que o retrato do runtime não trouxe: o estado sai do marcador, e a linha diz.
pub const RUNTIME_ABSENT: &str = "list_runtime_absent";

/// Capturas simultâneas numa rodada: cada uma é um processo do multiplexador.
const CAPTURE_PARALLEL: usize = 4;

/// Os quadros na ordem dos nomes, no máximo `CAPTURE_PARALLEL` processos de uma vez.
/// ID do menu do pane da sessão Claude com terminal do Rust, para os botões do card responderem pelo app.
/// `before`: o vínculo lido antes da captura; mudou no meio, a leitura não vale.
async fn menu_interaction<C: CaptureSource>(runtime: &crate::runtime::gateway::RuntimeRegistry, io: &C, row: &mut SessionRow,
    before: Option<&crate::interactions::InteractionBinding>, pane: Result<&str, ()>) -> Result<Option<String>, String> {
    // `claude` aberto à mão, sem entrada no Rust: não há resposta a identificar, nem problema.
    if runtime.hand_opened(&row.name).await { return Ok(None); }
    let binding = runtime.interaction_binding_readonly(&row.name).await.map_err(|e| e.code)?;
    if before != Some(&binding) { return Err("erro_interacao_desatualizada".into()); }
    let target = runtime.terminal_target(&row.name).await.map_err(|e| e.code)?.ok_or("terminal_target_missing")?;
    if io.target(&row.name).as_deref() != Some(target.binding.pane.as_str()) { return Err("terminal_capture_target_changed".into()); }
    let jsonl = row.jsonl.as_deref().map(Path::new).ok_or("askq_transcript_missing")?;
    let pane = pane.map_err(|()| "list_capture_failed")?;
    let store = runtime.plugin().map_err(|e| e.code)?;
    let question = crate::state::ask::observe_terminal(store.interactions(), &binding, jsonl, pane)?;
    let (menu, _) = crate::state::ask::observe_screen_menu(&store, &binding, jsonl, pane)?;
    if runtime.interaction_binding_readonly(&row.name).await.map_err(|e| e.code)? != binding { return Err("erro_interacao_desatualizada".into()); }
    if let Some(question) = question {
        row.pending_questions = row.pending_questions.max(1);
        // O formulário vai à linha com o ID dele: o card responde pelo `/answer`, como o passo a passo do chat.
        row.ask_form = serde_json::to_value(&question).ok();
    }
    Ok(menu)
}

async fn capture_all<C: CaptureSource>(io: &C, names: Vec<&str>) -> Vec<Result<String, CaptureFailed>> {
    // Os futuros montados antes: um closure dentro do stream tira o `Send` da rodada.
    let pending: Vec<_> = names.into_iter().map(|n| io.capture(n)).collect();
    futures_util::stream::iter(pending).buffered(CAPTURE_PARALLEL).collect().await
}

/// Captura que não devolveu quadro; `code` vai ao log, nunca a saída do multiplexador.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureFailed { pub code: &'static str }

/// Quadros do pane e o relógio da rodada. `wall` é época em s; `mono` só serve para idade de cache.
pub trait CaptureSource: Sync {
    fn capture(&self, name: &str) -> impl Future<Output = Result<String, CaptureFailed>> + Send;
    fn pause(&self, d: Duration) -> impl Future<Output = ()> + Send;
    fn wall(&self) -> f64;
    fn mono(&self) -> f64;
    fn target(&self, _name: &str) -> Option<String> { None }
}

/// O que a rodada pede a quem é dono do registro (o Python, a partir da Fase B).
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// O pane contradisse um marcador `awaiting_input` vencido: rebaixar para idle no mapa e no sidecar.
    /// `ts` é a versão do estado rebaixado (o `statusUpdatedAt` do registro nativo).
    DemoteAwaiting { sid: String, ts: f64 },
}

/// Fatos lidos fora daqui para esta rodada. Os arquivos (transcript, pergunta aberta, sidecar da
/// statusline) são lidos durante a rodada: quem chama roda `classify` fora da thread do runtime.
pub struct Facts<'a> {
    pub hooks: &'a HookStates,
    pub alive: &'a (dyn Fn(i64) -> bool + Sync),
    pub config_dirs: &'a [PathBuf],
    /// Retrato do runtime das sessões sem terminal, por nome (`RuntimeRegistry::snapshots`). `None`:
    /// servidor sem runtime ligado; sessão fora de um retrato fornecido está parada.
    pub headless: Option<&'a BTreeMap<String, Value>>,
    /// Código de problema do runtime, por nome.
    pub problems: &'a BTreeMap<String, String>,
    pub stall_seconds: f64,
    /// Último estado de cada `Monitor` vivo.
    pub monitors: &'a Published,
}

/// Caches que atravessam rodadas.
#[derive(Default)]
pub struct Classifier {
    plugin_runtime: Option<std::sync::Arc<crate::runtime::gateway::RuntimeRegistry>>,
    permissions: Option<crate::state::permission::PermissionStore>,
    idle_checked: BTreeMap<String, f64>,
    status: BTreeMap<String, (f64, Option<String>)>,
    label: BTreeMap<String, Option<String>>,
    limit: BTreeMap<String, (f64, Option<String>)>,
}

fn sid(row: &SessionRow) -> Option<String> {
    row.jsonl.as_deref().and_then(|j| Path::new(j).file_stem()).and_then(|s| s.to_str()).map(str::to_owned)
}

fn mtime(name: &str, jsonl: Option<&str>) -> Option<f64> {
    match std::fs::metadata(jsonl?).and_then(|m| m.modified()) {
        Ok(t) => t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs_f64()),
        // Ausente é o transcript que ainda não nasceu; o resto deixa a atividade cega e precisa de rastro.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            if crate::warn_limit::allow(Some(name), "list_transcript_unreadable") {
                tracing::warn!(session = name, kind = ?e.kind(), "lista: mtime do transcript ilegível");
            }
            None
        }
    }
}

/// Sem terminal: o runtime vivo responde; parado (fora do retrato ou `alive: false`), vale o marcador
/// `working`/`idle`. Sem retrato nenhum, também o marcador, com a ausência na linha: sessão parada
/// calada esconderia que o runtime não foi consultado.
fn headless_state(row: &mut SessionRow, retrato: Option<&BTreeMap<String, Value>>, marker: Option<&facts_files::Marker>) {
    // Candidata do sidecar: só vira adormecida onde o runtime confirma a sessão parada, abaixo.
    let candidate = std::mem::take(&mut row.asleep);
    let Some(retrato) = retrato else {
        row.problema = Some(RUNTIME_ABSENT.into());
        row.state = marker.map(|m| m.state.as_str()).filter(|s| ["working", "idle"].contains(s)).unwrap_or("idle").into();
        return;
    };
    let snapshot = retrato.get(&row.name);
    // Ausente ou `alive: false` é a sessão parada; erro ou retrato sem estado é falha do runtime.
    let broken = snapshot.is_some_and(|s| !s["error"].is_null()
        || (s["view"]["alive"] == true && !s["view"]["public_state"]["state"].is_string()));
    if broken {
        // Retrato que não veio ou vista ilegível é acompanhamento (diário); erro durável do ator é falha real, a
        // mesma que o chat mostra como `runtime_falhou`.
        let actor_error = snapshot.and_then(|s| s["error"].as_str())
            .filter(|code| !matches!(*code, "runtime_live_unavailable" | "runtime_snapshot_timeout") && !crate::problem_gate::is_diagnostic(code));
        row.problema = Some(if actor_error.is_some() { "runtime_falhou" } else { "list_runtime_unavailable" }.into());
    }
    let live = snapshot.filter(|s| !broken && s["view"]["alive"] == true).map(|s| &s["view"]["public_state"]);
    let Some(state) = live else {
        // Retrato quebrado (ator subindo, erro): o turno pode estar rodando, vale o marcador.
        // No Codex, sem marcador, fica o fato do Python, como antes.
        if broken {
            if row.provider != "codex" {
                row.state = marker.map(|m| m.state.as_str()).filter(|s| ["working", "idle"].contains(s)).unwrap_or("idle").into();
            }
            return;
        }
        // Sem ator não há turno: o marcador fica em "working" quando o turno acaba sem o hook final (Esc,
        // limite de uso, queda), e o chat (feed estacionado) já diz parada.
        row.state = "idle".into();
        (row.label, row.question, row.options, row.interaction_id) = (None, None, None, None);
        row.asleep = candidate;
        return;
    };
    let text = |k: &str| state[k].as_str().map(str::to_owned);
    row.state = text("state").unwrap_or_default();
    row.label = text("label");
    row.question = text("question");
    row.options = state["options"].as_array().map(|o| o.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect());
    row.interaction_id = text("interaction_id");
    row.status_line = text("status_line");
    // O problema da vista (queda do socket, falha do runtime) aparece também na lista.
    if row.problema.is_none() { row.problema = text("problema"); }
    codex_question(row, state["codex_question"].as_object());
    codex_pending(row, Some(retrato));
}

/// Pergunta do Codex na linha: com o turno rodando a assíncrona não troca "trabalhando" por espera
/// (o motor já decide o estado), como na lista do Python.
fn codex_question(row: &mut SessionRow, question: Option<&serde_json::Map<String, Value>>) {
    let Some(q) = question else { return };
    if row.state != "working" { row.state = "awaiting_input".into(); }
    row.question = q.get("questions").and_then(|qs| qs[0]["question"].as_str()).map(str::to_owned);
}

/// Contagem de perguntas assíncronas sem resposta, do ator do Rust (os fatos do Python não o veem).
fn codex_pending(row: &mut SessionRow, retrato: Option<&BTreeMap<String, Value>>) {
    if let Some(count) = retrato.and_then(|r| r.get(&row.name)).filter(|s| s["error"].is_null())
        .and_then(|s| s["view"]["pending_questions"].as_u64()) {
        row.pending_questions = u32::try_from(count).unwrap_or(u32::MAX);
    }
}

/// O que o `Monitor` vivo publicou: estado, pergunta, statusline e limite; o problema dele só
/// quando a lista não tem outro.
fn from_monitor(row: &mut SessionRow, ev: &StateEvent) {
    row.state.clone_from(&ev.state);
    row.label.clone_from(&ev.label);
    row.question.clone_from(&ev.question);
    row.options.clone_from(&ev.options);
    row.interaction_id.clone_from(&ev.interaction_id);
    row.status_line.clone_from(&ev.status_line);
    row.limit_reset.clone_from(&ev.limit_reset);
    row.limited = ev.limited;
    if row.problema.is_none() { row.problema.clone_from(&ev.problema); }
}

fn apply(row: &mut SessionRow, a: &PaneAnalysis) {
    row.state = a.state.clone();
    row.label = a.label.clone();
    row.question = a.question.clone();
    row.options = a.options.clone();
}

/// Última adoção tentada pela lista, por sessão: a prova lê os processos, e a lista roda a cada 1,5 s.
static ADOPTIONS: std::sync::Mutex<BTreeMap<String, std::time::Instant>> = std::sync::Mutex::new(BTreeMap::new());
const ADOPTION_EVERY: Duration = Duration::from_secs(30);

pub(crate) fn adoption_due(name: &str, now: std::time::Instant) -> bool {
    let mut tried = ADOPTIONS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if tried.get(name).is_some_and(|at| now.duration_since(*at) < ADOPTION_EVERY) { return false; }
    tried.insert(name.to_owned(), now);
    true
}

/// O último código anotado por sessão: a mesma falha vai ao diário uma vez, e de novo só depois de um acerto.
static NOTED_CODES: std::sync::Mutex<BTreeMap<String, String>> = std::sync::Mutex::new(BTreeMap::new());

fn first_note(name: &str, code: &str) -> bool {
    NOTED_CODES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(name.to_owned(), code.to_owned()).as_deref() != Some(code)
}

fn clear_notes(name: &str) { NOTED_CODES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(name); }

impl Classifier {
    pub fn set_plugin_runtime(&mut self, runtime: std::sync::Arc<crate::runtime::gateway::RuntimeRegistry>) { self.plugin_runtime = Some(runtime); }

    pub fn set_permissions(&mut self, permissions: crate::state::permission::PermissionStore) { self.permissions = Some(permissions); }

    async fn observe_permission(&self, name: &str, pane: &str, expected: Option<&crate::interactions::InteractionBinding>, revision: Option<crate::state::permission::MemoryRevision>) {
        let (Some(runtime), Some(memory), Some(mode)) = (&self.plugin_runtime, &self.permissions, crate::state::permission::parse_permission_mode(pane)) else { return };
        let mut bound = runtime.interaction_binding_readonly(name).await;
        // O Claude com terminal sem ator (aberto ou restaurado antes de qualquer escrita) é adotado, com teto.
        if bound.as_ref().is_err_and(|e| e.code == "runtime_binding") && adoption_due(name, std::time::Instant::now()) {
            match runtime.adopt_claude_terminal(name).await {
                Ok(()) => bound = runtime.interaction_binding_readonly(name).await,
                Err(error) => bound = Err(error),
            }
        }
        // O vínculo esperado (o da captura) e o visto no runtime: separa vida trocada de conversa trocada.
        let side = |b: Option<&crate::interactions::InteractionBinding>| b.map(|b| format!("geração {} conversa {}", b.generation, b.conversation));
        let failed = |code: String, reason: &'static str, fields: serde_json::Value| if first_note(name, &code) {
            super::facts::note_with("rust.list_permission_failed", name, code, reason, fields)
        };
        let base = serde_json::json!({"esperado": side(expected), "modo": mode, "revisao_captura": revision.is_some().to_string()});
        match bound {
            Ok(binding) if expected == Some(&binding) => match memory.lock() {
                Ok(mut memory) => match revision.ok_or("permission_frame_unavailable").and_then(|revision| memory.observe_capture(&binding.conversation, mode, revision)) {
                    Ok(_) => clear_notes(name),
                    Err(code) => failed(code.into(), "captura de permissão recusada", base),
                },
                Err(_) => failed("permission_store_unavailable".into(), "memória de permissão indisponível", base),
            },
            Err(error) => {
                let mut fields = base;
                fields["detalhe"] = error.message.chars().take(200).collect::<String>().into();
                failed(error.code, "vínculo de permissão não confirmado", fields)
            }
            Ok(binding) => {
                let mut fields = base;
                fields["visto"] = side(Some(&binding)).into();
                failed("permission_binding_changed".into(), "captura pertence a outra vida", fields)
            }
        }
    }

    /// Caches expostos para o contrato com o Python comparar tique a tique.
    pub fn idle_checked(&self) -> &BTreeMap<String, f64> { &self.idle_checked }
    pub fn status_cache(&self) -> &BTreeMap<String, (f64, Option<String>)> { &self.status }
    pub fn label_cache(&self) -> &BTreeMap<String, Option<String>> { &self.label }
    pub fn limit_cache(&self) -> &BTreeMap<String, (f64, Option<String>)> { &self.limit }

    /// Nome reusado por outra sessão não herda a statusline, o spinner nem o limite da anterior.
    pub fn forget(&mut self, name: &str) {
        // O Python não limpa o `_idle_conferido`; aqui limpa, para o nome reusado não herdá-lo.
        self.idle_checked.remove(name);
        self.status.remove(name);
        self.label.remove(name);
        self.limit.remove(name);
    }

    /// Teto dos mapas por nome: acima dele, quem não está nas linhas da rodada sai. O caminho
    /// normal é o `forget` de quem fecha a sessão.
    fn cap(&mut self, rows: &[SessionRow]) {
        let over = [self.idle_checked.len(), self.status.len(), self.label.len(), self.limit.len()]
            .into_iter().any(|n| n > SESSION_CAP);
        if !over {
            return;
        }
        let live: HashSet<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        self.idle_checked.retain(|n, _| live.contains(n.as_str()));
        self.status.retain(|n, _| live.contains(n.as_str()));
        self.label.retain(|n, _| live.contains(n.as_str()));
        self.limit.retain(|n, _| live.contains(n.as_str()));
    }

    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(v) = self.idle_checked.remove(old) { self.idle_checked.insert(new.into(), v); }
        if let Some(v) = self.status.remove(old) { self.status.insert(new.into(), v); }
        if let Some(v) = self.label.remove(old) { self.label.insert(new.into(), v); }
        if let Some(v) = self.limit.remove(old) { self.limit.insert(new.into(), v); }
    }

    /// Decide estado, rótulo, pergunta, statusline, travada e limite das linhas Claude.
    pub async fn classify<C: CaptureSource>(&mut self, rows: &mut [SessionRow], facts: &Facts<'_>, io: &C) -> Vec<Effect> {
        let effects = self.classify_some(rows, facts, io).await;
        self.cap(rows);
        effects
    }

    /// `classify` de parte das linhas (a sessão cujo arquivo mudou): sem o teto dos caches, que
    /// tiraria as demais por não estarem na rodada.
    pub async fn classify_some<C: CaptureSource>(&mut self, rows: &mut [SessionRow], facts: &Facts<'_>, io: &C) -> Vec<Effect> {
        let mut effects = Vec::new();
        let mut pending = Vec::new();
        // Linhas com `Monitor` vivo: nenhuma captura, nem a da statusline nem a do limite.
        let mut watched = HashSet::new();
        for (i, row) in rows.iter_mut().enumerate() {
            // Codex com terminal cujo ator está no Rust: o pane só diz que a TUI existe; quem publica é o ator.
            let rust_codex_terminal = row.provider == "codex" && !row.headless
                && facts.headless.is_some_and(|r| r.contains_key(&row.name));
            // Codex sem terminal com chat aberto: o feed do hub e o chat dizem o mesmo; sem ele, vale o
            // fato do Python (o mesmo espelho da vista do Rust). Contagem, conta e atividade seguem dos fatos.
            // Sem o cliente Python, o fato dele infere "hooks não aprovados" de um turno no rollout sem marcador;
            // o ator do Rust sabe o turno pelo app-server e não usa o marcador.
            let rust_codex = row.provider == "codex" && facts.headless.is_some_and(|r| r.contains_key(&row.name));
            if rust_codex && row.problema.as_deref() == Some("codex_hooks_nao_aprovados") { row.problema = None; }
            if row.provider == "codex" && (row.headless || rust_codex_terminal)
                && let Some(ev) = facts.monitors.get(&row.name, sid(row).as_deref())
            {
                row.asleep = false;
                from_monitor(row, &ev);
                codex_question(row, ev.codex_question.as_ref());
                codex_pending(row, facts.headless);
                continue;
            }
            // Sem feed, o estado sai da vista do ator: o Python perdeu o cliente e o fato dele ficou para trás.
            if rust_codex_terminal || (row.provider == "codex" && row.headless && facts.headless.is_some()) {
                if row.headless { codex_pending(row, facts.headless); }
                // A vista do ator nasce sem linha de status até o primeiro turno; a do rollout (fato) cobre.
                let from_rollout = row.status_line.take();
                headless_state(row, facts.headless, None);
                if row.status_line.is_none() { row.status_line = from_rollout; }
                continue;
            }
            if row.provider == "codex" && row.headless { codex_pending(row, facts.headless); }
            // Sem feed, a linha de status sai da vista do ator, a mesma do chat: a do fato vem do
            // rollout, com cache, e só vê o modelo novo no turno seguinte.
            if row.provider == "codex" && row.headless
                && let Some(line) = facts.headless.and_then(|r| r.get(&row.name))
                    .filter(|s| s["error"].is_null() && s["view"]["alive"] == true)
                    .and_then(|s| s["view"]["public_state"]["status_line"].as_str())
            {
                row.status_line = Some(line.to_owned());
            }
            if row.provider != "claude" {
                // Sem retrato do runtime a candidata não se confirma.
                row.asleep = false;
                continue;
            }
            let sid = sid(row);
            let marker = facts.hooks.get_state(sid.as_deref(), facts.alive);
            if row.headless {
                row.last_activity = mtime(&row.name, row.jsonl.as_deref());
                headless_state(row, facts.headless, marker.as_ref());
                // O problema registrado pelo adaptador é mais preciso que a falha genérica do retrato; a
                // ausência do retrato fica, senão a linha passaria por consultada.
                if let Some(p) = facts.problems.get(&row.name)
                    && row.problema.as_deref() != Some(RUNTIME_ABSENT) { row.problema = Some(p.clone()); }
                continue;
            }
            if row.problema.is_none() {
                row.problema = facts.problems.get(&row.name).cloned();
            }
            if let Some(ev) = facts.monitors.get(&row.name, sid.as_deref()) {
                from_monitor(row, &ev);
                row.last_activity = mtime(&row.name, row.jsonl.as_deref());
                watched.insert(row.name.clone());
                continue;
            }
            let Some(m) = marker else { pending.push(i); continue };
            let mtime = mtime(&row.name, row.jsonl.as_deref());
            if m.state == "idle" && mtime.is_some_and(|t| t > m.ts + IDLE_STALE_S)
                && self.idle_checked.get(&row.name).copied() != mtime {
                // Transcript escrito depois do idle é turno aberto sem UserPromptSubmit (agente de fundo).
                pending.push(i);
            } else if m.state != "awaiting_input" {
                row.state = m.state;
                row.last_activity = mtime;
                if row.state != "working" { self.label.remove(&row.name); }
            } else {
                // O marcador não carrega a pergunta: só o pane tem as opções.
                pending.push(i);
            }
        }

        if !pending.is_empty() {
            let mut capture_bindings = BTreeMap::new();
            if let Some(runtime) = &self.plugin_runtime {
                for &i in &pending { capture_bindings.insert(rows[i].name.clone(), runtime.interaction_binding_readonly(&rows[i].name).await); }
            }
            let mut permission_revisions = BTreeMap::new();
            if let Some(memory) = &self.permissions && let Ok(memory) = memory.lock() {
                for (name, binding) in &capture_bindings {
                    if let Ok(binding) = binding { permission_revisions.insert(name.clone(), memory.revision(&binding.conversation)); }
                }
            }
            let frames = capture_all(io, pending.iter().map(|&i| rows[i].name.as_str()).collect()).await;
            let mut analyses: Vec<Option<PaneAnalysis>> = frames.iter().map(|f| f.as_ref().ok().map(|t| analyze(t))).collect();
            let spinning: Vec<usize> = (0..pending.len())
                .filter(|&k| analyses[k].as_ref().is_some_and(|a| a.state == "working")).collect();
            if !spinning.is_empty() {
                io.pause(SPINNER_RECHECK).await;
                let again = capture_all(io, spinning.iter().map(|&k| rows[pending[k]].name.as_str()).collect()).await;
                for (k, frame) in spinning.into_iter().zip(again) {
                    let a = analyses[k].as_mut().unwrap();
                    match frame {
                        Ok(text) => if analyze(&text).spinner.as_ref().is_none_or(|s| Some(s) == a.spinner.as_ref()) {
                            (a.state, a.label, a.question, a.options) = ("idle".into(), None, None, None);
                        },
                        Err(e) => { analyses[k] = None; self.capture_failed(&rows[pending[k]].name, e); }
                    }
                }
            }
            let wall = io.wall();
            for (k, &i) in pending.iter().enumerate() {
                let row = &mut rows[i];
                let marker = facts.hooks.get_state(sid(row).as_deref(), facts.alive);
                row.last_activity = mtime(&row.name, row.jsonl.as_deref());
                let Some(a) = &analyses[k] else {
                    // Sem quadro não há prova de nada: fica o marcador, sem rebaixar nem conferir o idle.
                    if let Err(e) = &frames[k] { self.capture_failed(&row.name, *e); }
                    row.state = marker.map_or_else(|| "idle".into(), |m| m.state);
                    row.problema.get_or_insert_with(|| "list_capture_failed".into());
                    // O spinner guardado é de antes da falha: preenchido de volta, viraria rótulo fantasma.
                    self.label.remove(&row.name);
                    continue;
                };
                apply(row, a);
                if let Ok(pane) = &frames[k] { self.observe_permission(&row.name, pane, capture_bindings.get(&row.name).and_then(|result| result.as_ref().ok()), permission_revisions.get(&row.name).copied()).await; }
                if let Some(runtime) = &self.plugin_runtime {
                    let before = capture_bindings.get(&row.name).and_then(|result| result.as_ref().ok()).cloned();
                    let pane = frames[k].as_ref().map(String::as_str).map_err(|_| ());
                    match menu_interaction(runtime, io, row, before.as_ref(), pane).await { Ok(id) => row.interaction_id = id, Err(code) => row.problema = Some(code) }
                }
                match row.last_activity {
                    Some(t) if a.state == "idle" => { self.idle_checked.insert(row.name.clone(), t); }
                    _ => { self.idle_checked.remove(&row.name); }
                }
                if let (Some(m), Some(sid)) = (&marker, sid(row)) {
                    if m.state == "awaiting_input" && a.state != "awaiting_input" && wall - m.ts > AWAITING_DEMOTE_GRACE_S {
                        effects.push(Effect::DemoteAwaiting { sid, ts: m.ts });
                    }
                }
                row.limit_reset = a.limit_reset.clone();
                row.limited = row.limit_reset.is_some();
                let now = io.mono();
                self.limit.insert(row.name.clone(), (now, a.limit_reset.clone()));
                self.status.insert(row.name.clone(), (now, a.status_line.clone()));
                self.label.insert(row.name.clone(), a.label.clone());
            }
        }

        // Pergunta que o pane não mostra: o menu que rolou para fora e o marcador não carrega. A
        // permissão que o hook do plugin segura vem do `plugin::Store` depois daqui (`ListBridge::project_plugins`).
        for row in rows.iter_mut() {
            if row.provider != "claude" || row.headless || row.state == "awaiting_input"
                || row.options.as_ref().is_some_and(|o| !o.is_empty()) || watched.contains(&row.name) {
                continue;
            }
            if row.jsonl.is_none() {
                continue;
            }
            if let Some(q) = facts_files::open_question(sid(row).as_deref(), facts.config_dirs) {
                row.state = "awaiting_input".into();
                row.label = None;
                row.question = Some(q.question);
                row.options = Some(q.options);
                // O app responde (decisão do usuário): o quadro desta rodada não foi lido, então lê agora e dá ao
                // card o ID do menu do pane, que o `/select` responde pelas teclas como no chat.
                if let Some(runtime) = &self.plugin_runtime && !runtime.hand_opened(&row.name).await {
                    let before = runtime.interaction_binding_readonly(&row.name).await.ok();
                    let frame = io.capture(&row.name).await;
                    if let Ok(pane) = &frame && let Some(options) = analyze(pane).options.filter(|o| !o.is_empty()) { row.options = Some(options); }
                    match menu_interaction(runtime, io, row, before.as_ref(), frame.as_deref().map_err(|_| ())).await {
                        Ok(id) => row.interaction_id = id,
                        Err(code) => row.problema = Some(code),
                    }
                }
            }
        }

        // Statusline das que não foram raspadas: no máximo STATUS_BUDGET capturas, das mais velhas.
        let mut scraped: HashSet<String> = pending.iter().map(|&i| rows[i].name.clone()).collect();
        scraped.extend(watched.iter().cloned());
        let now = io.mono();
        let age = |c: &BTreeMap<String, (f64, Option<String>)>, n: &str| c.get(n).map_or(0.0, |v| v.0);
        let mut stale: Vec<String> = rows.iter()
            .filter(|r| r.provider == "claude" && !r.headless && !scraped.contains(&r.name)
                && now - age(&self.status, &r.name) > STATUS_TTL_S)
            .map(|r| r.name.clone()).collect();
        stale.sort_by(|a, b| age(&self.status, a).total_cmp(&age(&self.status, b)));
        stale.truncate(STATUS_BUDGET);
        for name in stale {
            let binding = match &self.plugin_runtime { Some(runtime) => runtime.interaction_binding_readonly(&name).await.ok(), None => None };
            let revision = self.permissions.as_ref().and_then(|memory| memory.lock().ok())
                .and_then(|memory| binding.as_ref().map(|binding| memory.revision(&binding.conversation)));
            match io.capture(&name).await {
                Ok(text) => {
                    self.observe_permission(&name, &text, binding.as_ref(), revision).await;
                    let a = analyze(&text);
                    let now = io.mono();
                    self.status.insert(name.clone(), (now, a.status_line.clone()));
                    self.limit.insert(name.clone(), (now, a.limit_reset.clone()));
                    // Uma captura só não separa spinner vivo de congelado: só guarda o que parece vivo.
                    if a.state == "working" { self.label.insert(name, a.label); } else { self.label.remove(&name); }
                }
                Err(e) => {
                    // Statusline velha ainda é verdadeira; spinner velho vira fantasma.
                    self.capture_failed(&name, e);
                    let prev = self.status.get(&name).and_then(|v| v.1.clone());
                    self.status.insert(name.clone(), (io.mono(), prev));
                    self.label.remove(&name);
                }
            }
        }

        let wall = io.wall();
        for row in rows.iter_mut().filter(|r| r.provider == "claude") {
            let published = || facts_files::published_status(sid(row).as_deref(), facts.config_dirs, wall).map(|p| p.line);
            if row.headless {
                if row.status_line.is_none() { row.status_line = published(); }
                continue;
            }
            if watched.contains(&row.name) {
                continue;
            }
            // Sidecar antes do pane: a captura traz a linha cortada na largura da janela.
            row.status_line = published().or_else(|| self.status.get(&row.name).and_then(|v| v.1.clone()));
            if row.state == "working" && row.label.is_none() {
                row.label = self.label.get(&row.name).cloned().flatten();
            }
        }
        for row in rows.iter_mut().filter(|r| r.provider == "claude") {
            row.stalled = row.state == "working" && row.last_activity.is_some_and(|t| wall - t > facts.stall_seconds);
        }
        self.limit_radar(rows, &scraped, io).await;
        effects
    }

    /// Travada (`working` sem o transcript andar) é quem espera o limite voltar: só ela paga a
    /// captura que o marcador poupou, uma vez a cada LIMIT_TTL_S.
    async fn limit_radar<C: CaptureSource>(&mut self, rows: &mut [SessionRow], scraped: &HashSet<String>, io: &C) {
        let now = io.mono();
        let targets: Vec<usize> = (0..rows.len()).filter(|&i| {
            let r = &rows[i];
            r.provider == "claude" && !r.headless && r.stalled && !scraped.contains(&r.name)
        }).collect();
        let fresh: Vec<usize> = targets.iter().copied()
            .filter(|&i| now - self.limit.get(&rows[i].name).map_or(0.0, |v| v.0) > LIMIT_TTL_S).collect();
        let frames = capture_all(io, fresh.iter().map(|&i| rows[i].name.as_str()).collect()).await;
        for (&i, frame) in fresh.iter().zip(frames) {
            let name = rows[i].name.clone();
            let reset = match frame {
                Ok(text) => analyze(&text).limit_reset,
                Err(e) => {
                    // Captura falhou: fica o último valor bom.
                    self.capture_failed(&name, e);
                    self.limit.get(&name).and_then(|v| v.1.clone())
                }
            };
            self.limit.insert(name, (now, reset));
        }
        for i in targets {
            let row = &mut rows[i];
            row.limit_reset = self.limit.get(&row.name).and_then(|v| v.1.clone());
            row.limited = row.limit_reset.is_some();
        }
    }

    fn capture_failed(&self, name: &str, e: CaptureFailed) {
        if crate::warn_limit::allow(Some(name), e.code) {
            tracing::warn!(session = name, code = e.code, "lista: captura do pane falhou");
        }
    }
}

/// Captura avulsa pelo multiplexador: a sessão sem chat aberto não tem cliente `-C` para alugar.
pub struct MuxCapture {
    mux: std::sync::Arc<MuxProcess>,
    /// Pane do agente por nome, da descoberta; sem ele vale o ativo da sessão (`=nome:`).
    pub targets: std::sync::Arc<BTreeMap<String, String>>,
}

/// Uma origem por processo: os caches do `Classifier` atravessam rodadas e instâncias de captura.
pub(crate) fn process_start() -> std::time::Instant {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *START.get_or_init(std::time::Instant::now)
}

impl MuxCapture {
    pub fn new(program: impl Into<OsString>, timeout: Duration, targets: std::sync::Arc<BTreeMap<String, String>>) -> Self {
        Self { mux: std::sync::Arc::new(MuxProcess::new(program, timeout)), targets }
    }
}

impl CaptureSource for MuxCapture {
    fn target(&self, name: &str) -> Option<String> { self.targets.get(name).cloned() }
    fn capture(&self, name: &str) -> impl Future<Output = Result<String, CaptureFailed>> + Send {
        let target = self.targets.get(name).cloned().unwrap_or_else(|| format!("={name}:"));
        let (mux, name) = (self.mux.clone(), name.to_owned());
        async move { mux.capture_checked(&name, &target).await.map_err(|code| CaptureFailed { code }) }
    }
    fn pause(&self, d: Duration) -> impl Future<Output = ()> + Send { tokio::time::sleep(d) }
    fn wall(&self) -> f64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
    }
    fn mono(&self) -> f64 { process_start().elapsed().as_secs_f64() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const IDLE: &str = "● pronto\n────────────\n❯\n────────────\n🤖 Opus 4.5";

    struct Fixed { frame: Result<String, CaptureFailed>, wall: f64, calls: Mutex<u32> }

    impl CaptureSource for Fixed {
        async fn capture(&self, _: &str) -> Result<String, CaptureFailed> {
            *self.calls.lock().unwrap() += 1;
            self.frame.clone()
        }
        async fn pause(&self, _: Duration) {}
        fn wall(&self) -> f64 { self.wall }
        fn mono(&self) -> f64 { self.wall }
    }

    fn setup(marker: &str, ts: f64) -> (tempfile::TempDir, Vec<PathBuf>, SessionRow) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".claude");
        std::fs::create_dir_all(cfg.join(".hangar-state")).unwrap();
        std::fs::write(cfg.join(".hangar-state/abc.json"), format!(r#"{{"state":"{marker}","ts":{ts}}}"#)).unwrap();
        let row: SessionRow = serde_json::from_value(serde_json::json!({
            "name": "s", "jsonl": dir.path().join("abc.jsonl").to_str().unwrap()})).unwrap();
        (dir, vec![cfg], row)
    }

    async fn run(row: &mut SessionRow, dirs: &[PathBuf], io: &Fixed, headless: Option<&BTreeMap<String, Value>>) -> Vec<Effect> {
        let hooks = HookStates::load(dirs);
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: dirs, headless,
                            problems: &BTreeMap::new(), stall_seconds: 300.0, monitors: &Published::default() };
        Classifier::default().classify(std::slice::from_mut(row), &facts, io).await
    }

    #[tokio::test]
    async fn permission_card_after_bash_is_awaiting() {
        // Com o app aberto, o hook do plugin segura a permissão do Bash: a TUI não desenha cartão,
        // o registro nativo fica `busy` e o marcador `working`. Só a pergunta segurada no `plugin::Store`
        // diz que espera, e ela entra depois da classificação, como em `ListBridge::project_plugins`.
        let (_d, dirs, mut row) = setup("working", 990.0);
        let io = Fixed { frame: Ok(IDLE.into()), wall: 1000.0, calls: Mutex::new(0) };
        let hooks = HookStates::load(&dirs);
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &dirs, headless: Some(&BTreeMap::new()),
                            problems: &BTreeMap::new(), stall_seconds: 300.0, monitors: &Published::default() };
        Classifier::default().classify(std::slice::from_mut(&mut row), &facts, &io).await;
        assert_eq!(row.state, "working", "a classificação não inventa a pergunta segurada");
        let mut state = hangar_api::state::StateEvent { state: row.state.clone(), label: row.label.clone(), question: row.question.clone(),
            options: row.options.clone(), interaction_id: row.interaction_id.clone(), ..Default::default() };
        let held = [serde_json::json!({"id": "perm:t1", "interaction_id": "shown", "questions": [], "tool": "Bash", "resumo": "ls"})];
        crate::state::facts::project_plugin_state(&mut state, &held).unwrap();
        assert_eq!((state.state.as_str(), state.question.as_deref(), state.interaction_id.as_deref()), ("awaiting_input", Some("Bash: ls"), Some("shown")));
        assert_eq!(state.options, Some(vec!["Yes".into(), "No".into()]));
        assert_eq!(state.label, None);
        // Sem pergunta segurada, o marcador responde como antes.
        let mut row2 = setup("working", 990.0).2;
        row2.jsonl = row.jsonl.clone();
        run(&mut row2, &dirs, &io, Some(&BTreeMap::new())).await;
        assert_eq!(row2.state, "working");
    }

    /// Parity M12: `claude` aberto à mão (sem entrada no Rust) não ganha o aviso `runtime_binding`.
    #[tokio::test]
    async fn hand_opened_claude_without_rust_entry_has_no_false_problem() {
        let (_d, dirs, mut row) = setup("awaiting_input", 900.0);
        let io = Fixed { frame: Ok(IDLE.into()), wall: 1000.0, calls: Mutex::new(0) };
        let hooks = HookStates::load(&dirs);
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &dirs, headless: Some(&BTreeMap::new()),
                            problems: &BTreeMap::new(), stall_seconds: 300.0, monitors: &Published::default() };
        let mut classifier = Classifier::default();
        classifier.set_plugin_runtime(std::sync::Arc::new(crate::runtime::gateway::RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "t".into(), "i".into())));
        classifier.classify(std::slice::from_mut(&mut row), &facts, &io).await;
        assert_eq!(row.problema, None);
    }

    #[tokio::test]
    async fn uses_monitor_state_when_alive() {
        // Marcador awaiting manda capturar; com `Monitor` vivo, vale o que ele publicou, sem captura.
        let (_d, dirs, mut row) = setup("awaiting_input", 900.0);
        let io = Fixed { frame: Ok(IDLE.into()), wall: 1000.0, calls: Mutex::new(0) };
        let hooks = HookStates::load(&dirs);
        let monitors = Published::default();
        monitors.set(1, "s", Some("abc".into()), std::sync::Arc::new(hangar_api::state::StateEvent {
            session: "s".into(), state: "awaiting_input".into(), question: Some("Bash: ls".into()),
            options: Some(vec!["Yes".into(), "No".into()]), status_line: Some("🤖 Haiku".into()),
            limited: true, limit_reset: Some("9:10pm".into()), ..Default::default() }));
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &dirs, headless: Some(&BTreeMap::new()),
                            problems: &BTreeMap::new(), stall_seconds: 300.0, monitors: &monitors };
        let effects = Classifier::default().classify(std::slice::from_mut(&mut row), &facts, &io).await;
        assert_eq!(*io.calls.lock().unwrap(), 0, "nenhuma captura com o Monitor vivo");
        assert_eq!(effects, vec![], "o Monitor não rebaixa o registro nativo pela lista");
        assert_eq!((row.state.as_str(), row.question.as_deref()), ("awaiting_input", Some("Bash: ls")));
        assert_eq!((row.status_line.as_deref(), row.limited, row.limit_reset.as_deref()), (Some("🤖 Haiku"), true, Some("9:10pm")));
        // Outra conversa (depois do `/clear`): o retrato do Monitor não vale e a lista captura.
        monitors.set(1, "s", Some("outra".into()), std::sync::Arc::new(hangar_api::state::StateEvent {
            state: "working".into(), ..Default::default() }));
        Classifier::default().classify(std::slice::from_mut(&mut row), &facts, &io).await;
        assert!(*io.calls.lock().unwrap() > 0);
        assert_eq!(row.state, "idle");
    }

    #[tokio::test]
    async fn asks_demote_after_grace() {
        // Marcador awaiting com 5 s: pane sem menu ainda não rebaixa; com 60 s, rebaixa.
        for (age, want) in [(5.0, vec![]), (60.0, vec![Effect::DemoteAwaiting { sid: "abc".into(), ts: 940.0 }])] {
            let (_d, dirs, mut row) = setup("awaiting_input", 1000.0 - age);
            let io = Fixed { frame: Ok(IDLE.into()), wall: 1000.0, calls: Mutex::new(0) };
            assert_eq!(run(&mut row, &dirs, &io, Some(&BTreeMap::new())).await, want, "idade {age}");
            assert_eq!(row.state, "idle");
        }
        // Sem quadro não há prova: o marcador fica, sem rebaixar, e a falha aparece na linha.
        let (_d, dirs, mut row) = setup("awaiting_input", 900.0);
        let io = Fixed { frame: Err(CaptureFailed { code: "capture_refused" }), wall: 1000.0, calls: Mutex::new(0) };
        assert_eq!(run(&mut row, &dirs, &io, Some(&BTreeMap::new())).await, vec![]);
        assert_eq!((row.state.as_str(), row.problema.as_deref()), ("awaiting_input", Some("list_capture_failed")));
    }

    /// Custo de um tique com 20 sessões, sem o subprocesso (quadro fixo). Medir em release:
    /// `cargo test --release -p hangar-server --lib list::classify::tests::tick_cost -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn tick_cost() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".claude");
        std::fs::create_dir_all(cfg.join(".hangar-state")).unwrap();
        let dirs = vec![cfg.clone()];
        let rows: Vec<SessionRow> = (0..20).map(|i| {
            let jsonl = dir.path().join(format!("s{i}.jsonl"));
            std::fs::write(&jsonl, "{}\n").unwrap();
            serde_json::from_value(serde_json::json!({"name": format!("s{i}"), "jsonl": jsonl.to_str().unwrap()})).unwrap()
        }).collect();
        let frame = format!("{}\n{IDLE}", "● linha de resposta comprida o bastante\n".repeat(190));
        for (label, marker) in [("marcador working", Some("working")), ("sem marcador (tudo no pane)", None),
                                ("marcador awaiting (tudo no pane)", Some("awaiting_input"))] {
            for i in 0..20 {
                let f = cfg.join(format!(".hangar-state/s{i}.json"));
                match marker {
                    Some(m) => std::fs::write(&f, format!(r#"{{"state":"{m}","ts":1.0}}"#)).unwrap(),
                    None => { let _ = std::fs::remove_file(&f); }
                }
            }
            let hooks = HookStates::load(&dirs);
            let (headless, problems) = (BTreeMap::new(), BTreeMap::new());
            let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &dirs, headless: Some(&headless),
                                problems: &problems, stall_seconds: 1e12, monitors: &Published::default() };
            let mut c = Classifier::default();
            let io = Fixed { frame: Ok(frame.clone()), wall: 1e9, calls: Mutex::new(0) };
            let n = 200;
            let t = std::time::Instant::now();
            for _ in 0..n {
                let mut r = rows.clone();
                c.classify(&mut r, &facts, &io).await;
            }
            let calls = *io.calls.lock().unwrap() as f64 / n as f64;
            println!("{label}: {:.0} µs/tique, {calls:.1} capturas/tique", t.elapsed().as_micros() as f64 / n as f64);
        }
    }

    /// Captura que cede a vez enquanto está "rodando", contando quantas há ao mesmo tempo.
    struct Slow { now: Mutex<usize>, peak: Mutex<usize> }

    impl CaptureSource for Slow {
        async fn capture(&self, _: &str) -> Result<String, CaptureFailed> {
            { let mut n = self.now.lock().unwrap(); *n += 1; let mut p = self.peak.lock().unwrap(); *p = (*p).max(*n); }
            for _ in 0..3 { tokio::task::yield_now().await; }
            *self.now.lock().unwrap() -= 1;
            Ok(IDLE.into())
        }
        async fn pause(&self, _: Duration) {}
        fn wall(&self) -> f64 { 1000.0 }
        fn mono(&self) -> f64 { 1000.0 }
    }

    #[tokio::test]
    async fn captures_have_a_ceiling() {
        let mut rows: Vec<SessionRow> = (0..20).map(|i| serde_json::from_value(serde_json::json!({"name": format!("s{i}")})).unwrap()).collect();
        let hooks = HookStates::default();
        let (headless, problems) = (BTreeMap::new(), BTreeMap::new());
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &[], headless: Some(&headless),
                            problems: &problems, stall_seconds: 300.0, monitors: &Published::default() };
        let io = Slow { now: Mutex::new(0), peak: Mutex::new(0) };
        Classifier::default().classify(&mut rows, &facts, &io).await;
        assert!(rows.iter().all(|r| r.state == "idle"));
        assert_eq!(*io.peak.lock().unwrap(), CAPTURE_PARALLEL, "20 linhas sem marcador, no máximo 4 capturas juntas");
    }

    #[test]
    fn classify_runs_in_a_spawned_task() {
        fn send<T: Send>(_: T) {}
        let hooks = HookStates::default();
        let (headless, problems) = (BTreeMap::new(), BTreeMap::new());
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &[], headless: Some(&headless),
                            problems: &problems, stall_seconds: 300.0, monitors: &Published::default() };
        let io = MuxCapture::new("tmux", Duration::from_secs(1), Default::default());
        send(Classifier::default().classify(&mut [], &facts, &io));
    }

    #[tokio::test]
    async fn headless_from_runtime() {
        let (_d, dirs, mut row) = setup("working", 990.0);
        row.headless = true;
        let io = Fixed { frame: Ok(String::new()), wall: 1000.0, calls: Mutex::new(0) };
        let snap = |alive: bool| BTreeMap::from([("s".to_owned(), serde_json::json!({"error": null, "view": {"alive": alive,
            "public_state": {"state": "awaiting_input", "label": null, "question": "Permitir Bash?",
                             "options": ["Permitir", "Negar"], "status_line": "🤖 Haiku 4.5"}}}))]);
        // Candidata do sidecar: vivo no runtime desfaz, parado confirma, erro e sem retrato não confirmam.
        row.asleep = true;
        run(&mut row, &dirs, &io, Some(&snap(true))).await;
        assert!(!row.asleep);
        row.asleep = true;
        run(&mut row, &dirs, &io, Some(&snap(false))).await;
        assert!(row.asleep);
        row.asleep = true;
        run(&mut row, &dirs, &io, None).await;
        assert!(!row.asleep);
        run(&mut row, &dirs, &io, Some(&snap(true))).await;
        assert_eq!((row.state.as_str(), row.question.as_deref()), ("awaiting_input", Some("Permitir Bash?")));
        assert_eq!(row.options, Some(vec!["Permitir".into(), "Negar".into()]));
        assert_eq!(row.status_line.as_deref(), Some("🤖 Haiku 4.5"));
        // Processo parado: sessão parada, mesmo com o marcador em "working". Pane nunca é raspado.
        row.question = None;
        run(&mut row, &dirs, &io, Some(&snap(false))).await;
        assert_eq!(row.state, "idle");
        assert_eq!(*io.calls.lock().unwrap(), 0);
        // Erro durável do ator não passa por sessão parada calada: é falha real, como o `runtime_falhou` do chat.
        let broken = BTreeMap::from([("s".to_owned(), serde_json::json!({"error": "cano_exited", "view": {"alive": true}}))]);
        run(&mut row, &dirs, &io, Some(&broken)).await;
        assert_eq!((row.state.as_str(), row.problema.as_deref()), ("working", Some("runtime_falhou")));
        // Retrato que não veio (acompanhamento) é diagnóstico.
        for code in ["runtime_live_unavailable", "runtime_snapshot_timeout"] {
            row.problema = None;
            let unread = BTreeMap::from([("s".to_owned(), serde_json::json!({"error": code}))]);
            run(&mut row, &dirs, &io, Some(&unread)).await;
            assert_eq!(row.problema.as_deref(), Some("list_runtime_unavailable"), "{code}");
        }
    }

    #[tokio::test]
    async fn codex_terminal_owned_by_rust_reads_the_actor_not_the_python_fact() {
        let row = || -> SessionRow { serde_json::from_value(serde_json::json!({"name": "cx", "provider": "codex", "headless": false,
            "state": "idle", "jsonl": "/x/rollout-abc.jsonl"})).unwrap() };
        let hooks = HookStates::default();
        let problems = BTreeMap::new();
        let io = Fixed { frame: Ok(String::new()), wall: 1000.0, calls: Mutex::new(0) };
        let owned = BTreeMap::from([("cx".to_owned(), serde_json::json!({"error": null, "view": {"alive": true, "headless": false,
            "public_state": {"state": "working", "label": null, "question": null, "options": null, "status_line": "gpt-5", "headless": false}}}))]);
        let (monitors, empty) = (Published::default(), BTreeMap::new());
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &[], headless: Some(&owned),
                            problems: &problems, stall_seconds: 1e12, monitors: &monitors };
        let mut rust = row();
        Classifier::default().classify(std::slice::from_mut(&mut rust), &facts, &io).await;
        assert_eq!((rust.state.as_str(), rust.status_line.as_deref()), ("working", Some("gpt-5")));
        // A queda do socket chega à lista pelo problema da vista.
        let dropped = BTreeMap::from([("cx".to_owned(), serde_json::json!({"error": null, "view": {"alive": true, "headless": false,
            "public_state": {"state": "idle", "problema": "runtime_falhou", "problema_detalhe": "a conexão de controle caiu", "headless": false}}}))]);
        let mut lost = row();
        Classifier::default().classify(std::slice::from_mut(&mut lost), &Facts { headless: Some(&dropped), ..facts }, &io).await;
        assert_eq!(lost.problema.as_deref(), Some("runtime_falhou"));
        // Sem ator no Rust, o pane continua sendo do Python: o fato dele vale.
        let facts = Facts { headless: Some(&empty), ..facts };
        let mut python = row();
        Classifier::default().classify(std::slice::from_mut(&mut python), &facts, &io).await;
        assert_eq!(python.state, "idle");
        assert_eq!(*io.calls.lock().unwrap(), 0, "Codex nunca é raspado");
    }

    #[tokio::test]
    async fn codex_terminal_of_the_rust_keeps_the_rollout_status_line_and_drops_the_stale_hooks_warning() {
        // Depois de reiniciar, a vista do ator só tem linha de status no primeiro turno: a do rollout (fato)
        // fica. O "hooks não aprovados" do fato vem de o Python não ter mais o cliente: o ator não usa o marcador.
        let row = serde_json::from_value::<SessionRow>(serde_json::json!({"name": "cx", "provider": "codex", "headless": false,
            "state": "idle", "jsonl": "/x/rollout-abc.jsonl", "status_line": "🤖 gpt (high) │ 💬 46k/95", "problema": "codex_hooks_nao_aprovados"})).unwrap();
        let hooks = HookStates::default();
        let problems = BTreeMap::new();
        let io = Fixed { frame: Ok(String::new()), wall: 1000.0, calls: Mutex::new(0) };
        let owned = BTreeMap::from([("cx".to_owned(), serde_json::json!({"error": null, "view": {"alive": true, "headless": false,
            "public_state": {"state": "working", "status_line": null, "headless": false}}}))]);
        let monitors = Published::default();
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &[], headless: Some(&owned),
                            problems: &problems, stall_seconds: 1e12, monitors: &monitors };
        let mut rust = row;
        Classifier::default().classify(std::slice::from_mut(&mut rust), &facts, &io).await;
        assert_eq!(rust.state, "working");
        assert_eq!(rust.status_line.as_deref(), Some("🤖 gpt (high) │ 💬 46k/95"));
        assert_eq!(rust.problema, None);
    }

    #[tokio::test]
    async fn codex_headless_reads_the_feed() {
        // Linha do Codex sem terminal com o feed vivo: estado e pergunta dele; sem feed, o fato do Python.
        let row = || -> SessionRow { serde_json::from_value(serde_json::json!({"name": "cx", "provider": "codex", "headless": true,
            "state": "idle", "pending_questions": 2, "jsonl": "/x/rollout-abc.jsonl"})).unwrap() };
        let hooks = HookStates::default();
        let (headless, problems) = (BTreeMap::new(), BTreeMap::new());
        let monitors = Published::default();
        let io = Fixed { frame: Ok(String::new()), wall: 1000.0, calls: Mutex::new(0) };
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &[], headless: Some(&headless),
                            problems: &problems, stall_seconds: 1e12, monitors: &monitors };
        // Sem chat aberto e sem ator, a sessão está parada: o fato do Python lê o marcador do hook, que
        // fica em "working" quando o turno acaba sem resposta.
        let mut without = row();
        without.state = "working".into();
        Classifier::default().classify(std::slice::from_mut(&mut without), &facts, &io).await;
        assert_eq!(without.state, "idle", "sem ator não há turno rodando");
        let question = serde_json::json!({"questions": [{"question": "Qual caminho?"}]});
        monitors.set(1, "cx", Some("rollout-abc".into()), std::sync::Arc::new(hangar_api::state::StateEvent {
            state: "working".into(), status_line: Some("gpt".into()), codex_question: question.as_object().cloned(), ..Default::default() }));
        let mut with = row();
        Classifier::default().classify(std::slice::from_mut(&mut with), &facts, &io).await;
        // Turno rodando: a pergunta assíncrona aparece, mas a sessão segue trabalhando (como no Python).
        assert_eq!((with.state.as_str(), with.question.as_deref(), with.status_line.as_deref()),
                   ("working", Some("Qual caminho?"), Some("gpt")));
        assert_eq!(with.pending_questions, without.pending_questions, "sem o ator no retrato, a contagem segue dos fatos");
        // Ocioso com a pergunta: espera.
        monitors.set(2, "cx", Some("rollout-abc".into()), std::sync::Arc::new(hangar_api::state::StateEvent {
            state: "idle".into(), codex_question: question.as_object().cloned(), ..Default::default() }));
        let mut idle = row();
        Classifier::default().classify(std::slice::from_mut(&mut idle), &facts, &io).await;
        assert_eq!(idle.state, "awaiting_input");
        // Com o ator no retrato, a contagem vem dele: os fatos do Python não enxergam sessão do Rust.
        let owned = BTreeMap::from([("cx".to_owned(), serde_json::json!({"error": null, "view": {"alive": true, "pending_questions": 1,
            "public_state": {"state": "idle"}}}))]);
        let mut counted = row();
        Classifier::default().classify(std::slice::from_mut(&mut counted), &Facts { headless: Some(&owned), ..facts }, &io).await;
        assert_eq!(counted.pending_questions, 1);
        assert_eq!(*io.calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn codex_headless_status_line_follows_the_runtime_within_a_life() {
        // Sem chat aberto (sem feed), a linha de status vem da vista do ator, a mesma do chat: o fato do
        // Python lê o rollout com cache e só vê o modelo novo no turno seguinte.
        let row = || -> SessionRow { serde_json::from_value(serde_json::json!({"name": "cx", "provider": "codex", "headless": true,
            "state": "idle", "jsonl": "/x/rollout-abc.jsonl", "status_line": "🤖 gpt-6.1-sol (high) │ 💬 46k/95 46k/828k"})).unwrap() };
        let hooks = HookStates::default();
        let problems = BTreeMap::new();
        let monitors = Published::default();
        let io = Fixed { frame: Ok(String::new()), wall: 1000.0, calls: Mutex::new(0) };
        let view = |line: &str| BTreeMap::from([("cx".to_owned(), serde_json::json!({"error": null,
            "view": {"alive": true, "public_state": {"state": "idle", "status_line": line}}}))]);
        let mut classifier = Classifier::default();
        for line in ["🤖 gpt-6-luna (medium) │ 💬 46k/95 46k/828k", "🤖 gpt-6-luna (medium) │ 💬 50k/5 50k/828k"] {
            let headless = view(line);
            let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &[], headless: Some(&headless),
                                problems: &problems, stall_seconds: 1e12, monitors: &monitors };
            let mut r = row();
            classifier.classify(std::slice::from_mut(&mut r), &facts, &io).await;
            assert_eq!(r.status_line.as_deref(), Some(line));
        }
        // Ator parado: fica o fato do Python.
        let stopped = BTreeMap::from([("cx".to_owned(), serde_json::json!({"error": null, "view": {"alive": false}}))]);
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &[], headless: Some(&stopped),
                            problems: &problems, stall_seconds: 1e12, monitors: &monitors };
        let mut r = row();
        classifier.classify(std::slice::from_mut(&mut r), &facts, &io).await;
        assert_eq!(r.status_line.as_deref(), Some("🤖 gpt-6.1-sol (high) │ 💬 46k/95 46k/828k"));
    }

    #[tokio::test]
    async fn headless_without_runtime_view_is_marked() {
        let (_d, dirs, mut row) = setup("working", 990.0);
        row.headless = true;
        let io = Fixed { frame: Ok(String::new()), wall: 1000.0, calls: Mutex::new(0) };
        // Sem retrato fornecido, o marcador responde, mas a linha diz que o runtime não foi consultado.
        run(&mut row, &dirs, &io, None).await;
        assert_eq!((row.state.as_str(), row.problema.as_deref()), ("working", Some(RUNTIME_ABSENT)));
        // Retrato fornecido sem a sessão: ela está parada, e o marcador "working" que o Esc ou o limite
        // deixaram para trás não a faz parecer trabalhando.
        let other = BTreeMap::from([("outra".to_owned(), serde_json::json!({"error": null, "view": {"alive": true}}))]);
        row.problema = None;
        run(&mut row, &dirs, &io, Some(&other)).await;
        assert_eq!((row.state.as_str(), row.problema.as_deref()), ("idle", None));
        let stopped = BTreeMap::from([("s".to_owned(), serde_json::json!({"error": null, "view": {"alive": false}}))]);
        run(&mut row, &dirs, &io, Some(&stopped)).await;
        assert_eq!((row.state.as_str(), row.problema.as_deref()), ("idle", None), "ator encerrado é sessão parada");
        assert_eq!(*io.calls.lock().unwrap(), 0);
        // O problema do adaptador não apaga a ausência do retrato.
        let hooks = HookStates::load(&dirs);
        let problems = BTreeMap::from([("s".to_owned(), "cano_exited".to_owned())]);
        let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &dirs, headless: None,
                            problems: &problems, stall_seconds: 300.0, monitors: &Published::default() };
        Classifier::default().classify(std::slice::from_mut(&mut row), &facts, &io).await;
        assert_eq!(row.problema.as_deref(), Some(RUNTIME_ABSENT));
    }
}

#[cfg(test)]
mod permission_notes {
    use super::*;

    /// A lista tenta adotar no máximo a cada 30 s por sessão, e a mesma falha vai ao diário uma vez.
    #[test]
    fn the_list_adopts_with_a_ceiling_and_notes_each_failure_once() {
        let t0 = std::time::Instant::now();
        assert!(adoption_due("pn-a", t0) && !adoption_due("pn-a", t0 + Duration::from_secs(29)) && adoption_due("pn-a", t0 + ADOPTION_EVERY));
        assert!(adoption_due("pn-b", t0), "por sessão");
        assert!(first_note("pn-a", "runtime_binding") && !first_note("pn-a", "runtime_binding"));
        assert!(first_note("pn-a", "permission_binding_changed"), "outro código");
        clear_notes("pn-a");
        assert!(first_note("pn-a", "permission_binding_changed"), "de novo depois de um acerto");
    }
}

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

// Os formatos da conversa vêm do crate que o hangar-server também usa; os nomes antigos ficam para o
// resto do app não mudar.
pub use hangar_api::chat::ChatEvent;
pub use hangar_api::preview::PreviewEvent as Preview;
pub use hangar_api::state::{ShellVivo as ShellAlive, StateEvent as SessionState};

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub name: String,
    pub cwd: Option<String>,
    pub jsonl: Option<String>,
    #[serde(default)] pub provider: String,
    #[serde(default)] pub headless: bool,
    #[serde(default)] pub state: String,
    pub tracked: Option<bool>,
    pub question: Option<String>,
    pub options: Option<Vec<String>>,
    /// Perguntas esperando resposta fora do terminal (as assíncronas do Codex); a aba mostra "? N".
    #[serde(default)] pub pending_questions: u32,
    pub problema: Option<String>,
    pub label: Option<String>,
    #[serde(default)] pub startup_steps: Vec<String>,
    pub last_activity: Option<f64>,
    pub branch: Option<String>,
    pub git_added: Option<i64>,
    pub git_removed: Option<i64>,
    pub git_dirty: Option<i64>,
    /// Commits a enviar / a trazer do upstream: o "↑N ↓M" da linha.
    pub git_ahead: Option<i64>,
    pub git_behind: Option<i64>,
    pub status_line: Option<String>,
    pub loop_status: Option<String>,
    pub loop_iter: Option<u32>,
    pub loop_max: Option<u32>,
    pub limited: Option<bool>,
    pub limit_reset: Option<String>,
    /// Sessão que recebe um prompt quando esta terminar (`PUT …/then`).
    pub then_target: Option<String>,
    /// Conta que a sessão usa ("claude:<pasta>", "codex:<pasta>"): selo da linha e id da lista de contas.
    pub conta: Option<String>,
    pub last_reply: Option<String>,
    pub last_reply_at: Option<f64>,
    pub worktree: Option<bool>,
    /// Membros do grupo de trabalho além dela; `srv::nome` é par de outro servidor.
    pub pair_peers: Option<Vec<String>>,
    /// Id estável do grupo: a lista junta num bloco quem tem o mesmo.
    pub pair_gid: Option<String>,
    /// Tarefa do grupo (ex: ABC-1234 …), o rótulo do cabeçalho do bloco.
    pub pair_task: Option<String>,
    /// Há convite ativo desta sessão (pendente ou já usado): o 🔗 da linha.
    #[serde(default)] pub shared: bool,
    /// Nome do convidado que criou a sessão; `None` = dono do servidor.
    pub owner: Option<String>,
    /// Só na linha `orq`: a sessão do árbitro atual, que o "Falar com o árbitro" abre.
    pub orq_arbiter: Option<String>,
    /// Task em andamento do plano que a sessão executa, e o total delas.
    pub plan_task: Option<u32>,
    pub plan_task_total: Option<u32>,
    /// Como o convidado vê esta sessão: "pair" = só leitura.
    #[serde(default)] pub guest_kind: Option<String>,
    /// Par com a sessão de outra pessoa (outra máquina, outro usuário).
    #[serde(default)] pub pair_external: Option<PairExternal>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct PairExternal { pub alias: String, pub owner: String, pub session: String }

impl SessionInfo {
    pub fn readable(&self) -> bool { self.tracked != Some(false) && self.jsonl.is_some() }
    pub fn display_state(&self) -> &str {
        if self.provider == "codex" && !self.readable() && !matches!(self.state.as_str(), "awaiting_input" | "dead") { "loading" }
        else { &self.state }
    }
    pub fn peers(&self) -> &[String] { self.pair_peers.as_deref().unwrap_or_default() }
    /// O orquestrador sem LLM: tem linha do tempo, mas não recebe mensagem, nome novo, fechar nem interromper.
    pub fn orq(&self) -> bool { self.provider == "orq" }
    /// Tem compositor: a linha `orq` lê a linha do tempo, mas ninguém escreve nela.
    pub fn takes_messages(&self) -> bool { self.readable() && !self.orq() && !self.read_only() }
    /// A sessão da outra pessoa num par externo: acompanha, sem escrever nem mexer nela.
    pub fn read_only(&self) -> bool { self.guest_kind.as_deref() == Some("pair") }
    /// O árbitro que esta linha `orq` aponta, entre as sessões da mesma lista.
    pub fn arbiter<'a>(&self, sessions: &'a [SessionInfo]) -> Option<&'a SessionInfo> {
        let name = self.orq_arbiter.as_deref()?;
        sessions.iter().find(|s| s.name == name)
    }
}

/// Resposta de juntar ou sair do grupo (`POST|DELETE …/pair`): o vínculo já mudou; `warning` diz quem não recebeu o aviso.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PairResult { pub warning: Option<String> }

impl PairResult {
    pub fn from_value(value: &Value) -> Self {
        let warning = value.get("warning").filter(|w| !w.is_null()).map(|w| match w {
            Value::String(text) => text.clone(),
            _ => w.get("msg").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| w.to_string()),
        });
        Self { warning }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SkillLoaded {
    pub name: String,
    #[serde(default)] pub body: String,
}

/// Linha da linha do tempo do orquestrador sem LLM, já enriquecida pelo backend (só sessão `orq`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqEntry {
    /// `advance` | `woke` | `would_drop` | `dropped` | `failed` | `notice`; vazio conta como `notice`.
    #[serde(default)] pub kind: String,
    pub task: Option<u32>,
    pub line: Option<OrqLine>,
    /// `notify` | `orchestrator`.
    pub origin: Option<String>,
    pub sender: Option<String>,
    pub mark: Option<String>,
    #[serde(default)] pub alarm: bool,
    pub rejected_round: Option<u32>,
    #[serde(default)] pub body: String,
    pub question: Option<String>,
    pub parecer: Option<String>,
    pub error: Option<String>,
    pub decided_by: Option<OrqDecidedBy>,
}

/// Frase curta pelo código; código que este app não conhece cai em `Unknown` em vez de derrubar a leitura.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum OrqLine {
    Opened { #[serde(default)] sessions: Vec<OrqSessionRef> },
    Integrated { #[serde(default)] merge: bool },
    Delivered { round: Option<u32>, commit: Option<String> },
    RedBack { executor: Option<String> },
    RedRetry,
    #[serde(other)] Unknown,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqSessionRef {
    #[serde(default)] pub name: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqDecidedBy {
    /// `rule` | `alarm` | `jev` | `regex`.
    #[serde(default)] pub source: String,
    /// `mark` | `orchestrator`.
    pub rule: Option<String>,
    pub jev: Option<OrqJev>,
    pub regex: Option<OrqRegex>,
    pub regex_agreed: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqJev {
    pub mode: Option<String>,
    pub choice: Option<String>,
    pub p: Option<f64>,
    #[serde(default)] pub probs: HashMap<String, Option<f64>>,
    /// Mapa aberto (`context`, `user`, `problem`, `deviation`, ...): o backend pode acrescentar chave.
    pub veto: Option<HashMap<String, Option<f64>>>,
    #[serde(default)] pub held: Vec<String>,
    pub would_drop: Option<bool>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqRegex {
    /// `drop` | `wake`.
    #[serde(default)] pub verdict: String,
    pub category: Option<String>,
}

/// `GET /api/sessions/{name}/orq/panel`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqPanel {
    #[serde(default)] pub run: String,
    pub metadata: Option<OrqRunMetadata>,
    #[serde(default)] pub gid: String,
    #[serde(default)] pub errors: Vec<OrqFileError>,
    #[serde(default)] pub empty: bool,
    #[serde(default)] pub timing: OrqTiming,
    #[serde(default)] pub tasks: OrqPanelTasks,
    #[serde(default)] pub team: Vec<OrqTeamMember>,
    #[serde(default)] pub decisions: Vec<OrqDecision>,
    #[serde(default)] pub automation: OrqAutomation,
    /// `null` enquanto o backend ainda soma os transcripts.
    pub consumption: Option<OrqConsumption>,
    #[serde(default)] pub integration: OrqIntegration,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqRunMetadata {
    #[serde(default)] pub title: String,
    #[serde(default)] pub plan: String,
    #[serde(default)] pub repo: String,
    pub total_tasks: Option<u32>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqFileError {
    #[serde(default)] pub file: String,
    #[serde(default)] pub error: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqPanelTasks {
    #[serde(default)] pub integrated: u32,
    #[serde(default)] pub total: u32,
    #[serde(default)] pub total_known: bool,
    #[serde(default)] pub rows: Vec<OrqPanelTask>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqPanelTask {
    #[serde(default)] pub n: u32,
    #[serde(default)] pub title: String,
    /// `queued` | `executing` | `in_review` | `rejected` | `approved` | `integrated` | `integration_red`.
    #[serde(default)] pub state: String,
    pub round: Option<u32>,
    #[serde(default)] pub timing: OrqTiming,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqTiming {
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub elapsed_seconds: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqTeamMember {
    #[serde(default)] pub name: String,
    /// `arbiter` | `executor` | `reviewer`.
    #[serde(default)] pub role: String,
    pub task: Option<u32>,
    #[serde(default)] pub current: bool,
    pub last: Option<OrqLast>,
}

/// Último gesto do membro do time: `started` | `delivered` | `approved` | `rejected` | `swapped_in`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqLast {
    #[serde(default)] pub code: String,
    pub round: Option<u32>,
    pub ts: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqDecision {
    pub task: Option<u32>,
    pub ts: Option<String>,
    #[serde(default)] pub question: String,
    pub parecer: Option<String>,
    #[serde(default)] pub event_id: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqAutomation {
    #[serde(default)] pub mode: OrqAutoMode,
    #[serde(default)] pub woke: OrqAutoWoke,
    #[serde(default)] pub alone: OrqAutoAlone,
    #[serde(default)] pub dropped_by_jev: u32,
    #[serde(default)] pub advanced: OrqAutoAdvanced,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqAutoMode {
    /// `on` | `shadow` | `off`.
    pub jev: Option<String>,
    pub regex: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqAutoWoke {
    #[serde(default)] pub total: u32,
    #[serde(default)] pub decisions: u32,
    #[serde(default)] pub alarms: u32,
    #[serde(default)] pub messages: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqAutoAlone {
    #[serde(default)] pub total: u32,
    #[serde(default)] pub opened: u32,
    #[serde(default)] pub integrated: u32,
    #[serde(default)] pub dropped: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqAutoAdvanced {
    #[serde(default)] pub would_drop: u32,
    #[serde(default)] pub disagree: u32,
    #[serde(default)] pub judged: u32,
    pub min_confidence: Option<OrqMinConfidence>,
    #[serde(default)] pub by_rule: u32,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqMinConfidence {
    pub p: Option<f64>,
    pub choice: Option<String>,
    pub ts: Option<String>,
    #[serde(default)] pub text: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqConsumption {
    pub computed_at: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    #[serde(default)] pub sessions: OrqConsumptionSessions,
    #[serde(default)] pub totals: OrqConsumptionTotals,
    #[serde(default)] pub providers: Vec<OrqConsumptionProvider>,
    #[serde(default)] pub missing_prices: Vec<String>,
    #[serde(default)] pub subagents: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqConsumptionSessions {
    #[serde(default)] pub team: u32,
    #[serde(default)] pub measured: u32,
    /// Nomes do time sem transcript.
    #[serde(default)] pub missing: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqConsumptionTotals {
    #[serde(default)] pub new: u64,
    #[serde(default)] pub cache_read: u64,
    /// `null` quando nenhum modelo tem preço.
    pub usd: Option<f64>,
    #[serde(default)] pub usd_partial: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqConsumptionProvider {
    #[serde(default)] pub provider: String,
    #[serde(default)] pub sessions: u32,
    #[serde(default)] pub new: u64,
    #[serde(default)] pub cache_read: u64,
    pub usd: Option<f64>,
    #[serde(default)] pub models: Vec<OrqConsumptionModel>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqConsumptionModel {
    #[serde(default)] pub model: String,
    #[serde(default)] pub sessions: u32,
    #[serde(default)] pub new: u64,
    #[serde(default)] pub cache_read: u64,
    pub usd: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqIntegration {
    /// `null` quando a execução não registrou o início ou o bloco falhou.
    pub branch: Option<String>,
    pub last: Option<OrqIntegrationLast>,
    /// `green` | `red` | `conflict` | `failed`.
    pub outcome: Option<String>,
    pub red_log: Option<String>,
    #[serde(default)] pub delivery_checks: OrqDeliveryChecks,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqIntegrationLast {
    pub task: Option<u32>,
    pub commit: Option<String>,
    pub ts: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OrqDeliveryChecks {
    #[serde(default)] pub ok: u32,
    #[serde(default)] pub total: u32,
    /// Números das Tasks com check vermelho.
    #[serde(default)] pub failing: Vec<u32>,
}

/// O que o desktop tira de uma mensagem; o formato em si é o do `hangar-api`.
pub trait ChatEventExt {
    fn queued(&self) -> bool;
    fn body(&self) -> String;
    /// Skill de um notice `skill_loaded`; sem `name` em texto fica sem skill, sem derrubar a mensagem.
    fn loaded_skill(&self) -> Option<SkillLoaded>;
    /// Entrada da linha do tempo de uma sessão `orq`; formato que este app não lê fica sem entrada.
    fn orq_entry(&self) -> Option<OrqEntry>;
}

impl ChatEventExt for ChatEvent {
    fn queued(&self) -> bool { self.id.starts_with("queued-") }
    fn body(&self) -> String {
        self.text.clone().or_else(|| self.result.clone()).unwrap_or_else(|| {
            self.tool_input.as_ref().map(|v| serde_json::to_string_pretty(v).unwrap_or_default()).unwrap_or_default()
        })
    }
    fn loaded_skill(&self) -> Option<SkillLoaded> {
        let skill = self.skill.as_ref()?;
        let name = skill.get("name")?.as_str()?.to_owned();
        let body = skill.get("body").and_then(Value::as_str).unwrap_or_default().to_owned();
        Some(SkillLoaded { name, body })
    }
    fn orq_entry(&self) -> Option<OrqEntry> {
        serde_json::from_value(Value::Object(self.orq.clone()?)).ok()
    }
}

/// Evento SSE `stats`: só turns/steps/in/out são garantidos; o resto aparece quando o backend mede.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Stats {
    #[serde(default)] pub turns: u64,
    #[serde(default)] pub steps: u64,
    #[serde(default)] pub in_tok: u64,
    #[serde(default)] pub out_tok: u64,
    pub llm_ms: Option<f64>,
    pub tool_ms: Option<f64>,
    pub tok_s: Option<f64>,
    pub cache_pct: Option<f64>,
    pub ttft_ms: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct PlanPending {
    #[serde(default)] pub plan: String,
    pub path: Option<String>,
}

/// Plano do Claude sem terminal esperando aprovação, lido do mapa que o estado traz.
pub fn plan_pending(state: &SessionState) -> Option<PlanPending> {
    let pending = state.claude_plan_pending.as_ref()?;
    let text = |key: &str| pending.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(PlanPending { plan: text("plan").unwrap_or_default(), path: text("path") })
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct AskOption {
    #[serde(default)] pub label: String,
    #[serde(default)] pub description: String,
    #[serde(default)] pub preview: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskItem {
    pub id: Option<String>,
    #[serde(default)] pub header: String,
    #[serde(default)] pub question: String,
    #[serde(default)] pub multi_select: bool,
    #[serde(default)] pub options: Vec<AskOption>,
    #[serde(default)] pub is_other: bool,
    #[serde(default)] pub is_secret: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AskPayload {
    pub provider: Option<String>,
    // Valor cru: o Codex recusa id com outro tipo JSON (número × texto).
    pub request_id: Option<Value>,
    #[serde(default)] pub is_async: bool,
    #[serde(default)] pub questions: Vec<AskItem>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Delivery {
    pub ok: bool,
    #[serde(default)] pub delivered: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommandInfo {
    pub name: String,
    #[serde(default)] pub display: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    #[serde(default)] pub source: String,
    #[serde(default)] pub destructive: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Uploaded {
    pub path: String,
    #[serde(default)] pub frames: Vec<String>,
    pub transcript: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UploadFile {
    pub filename: String,
    #[serde(default)] pub size: u64,
    #[serde(default)] pub mtime: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Steered {
    #[serde(default)] pub promoted: bool,
    #[serde(default)] pub confirmed: u32,
    #[serde(default)] pub queued_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{ChatEvent, ChatEventExt, OrqEntry, OrqLine, OrqPanel, SessionInfo, SessionState, plan_pending};
    use serde_json::json;

    #[test]
    fn orq_entry_reads_a_real_decision() {
        let entry: OrqEntry = serde_json::from_value(json!({"kind": "woke", "task": 4, "mark": "decisao",
            "parecer": "/home/jefferson/.hangar/orq/2026-09-29-cad3e6fe/pareceres/task-4-r1-revisor.md",
            "question": "Incluir `MessageList.tsx:104` em T4 (passar `ev.text` cru) ou deixar para T11?",
            "body": "…", "origin": "notify", "alarm": false,
            "decided_by": {"source": "rule", "rule": "mark", "jev": null, "regex": null, "regex_agreed": null}})).unwrap();
        assert_eq!(entry.task, Some(4));
        assert_eq!(entry.mark.as_deref(), Some("decisao"));
        let by = entry.decided_by.unwrap();
        assert_eq!(by.source, "rule");
        assert!(by.jev.is_none() && by.regex_agreed.is_none());
        assert!(entry.parecer.unwrap().ends_with("task-4-r1-revisor.md"));
    }

    #[test]
    fn orq_entry_reads_a_dropped_message_with_the_jev() {
        let entry: OrqEntry = serde_json::from_value(json!({"kind": "dropped", "task": null, "body": "ok", "alarm": false,
            "decided_by": {"source": "jev", "rule": null, "regex": {"verdict": "drop", "category": "janela"}, "regex_agreed": true,
                "jev": {"mode": "judge", "choice": "nothing", "p": 0.97, "probs": {"nothing": 0.97, "act": 0.03},
                    "veto": {"context": 0.1, "user": null, "problem": 0.0, "deviation": 0.0}, "held": [],
                    "would_drop": null, "error": null}}})).unwrap();
        let by = entry.decided_by.unwrap();
        let jev = by.jev.unwrap();
        assert_eq!(jev.probs["nothing"], Some(0.97));
        assert_eq!(jev.veto.as_ref().unwrap()["context"], Some(0.1));
        assert_eq!(jev.veto.unwrap()["user"], None);
        assert_eq!(jev.p, Some(0.97));
        assert_eq!(by.regex_agreed, Some(true));
        assert_eq!(by.regex.unwrap().category.as_deref(), Some("janela"));
    }

    #[test]
    fn orq_entry_tolerates_missing_kind_and_null_probability() {
        let entry: OrqEntry = serde_json::from_value(json!({"body": "x",
            "decided_by": {"source": "jev", "jev": {"probs": {"act": null}}}})).unwrap();
        assert_eq!(entry.kind, "");
        assert_eq!(entry.decided_by.unwrap().jev.unwrap().probs["act"], None);
    }

    #[test]
    fn orq_line_unknown_code_does_not_fail() {
        let line: OrqLine = serde_json::from_value(json!({"code": "novo"})).unwrap();
        assert!(matches!(line, OrqLine::Unknown));
        let opened: OrqLine = serde_json::from_value(json!({"code": "opened",
            "sessions": [{"name": "t1", "provider": "claude", "model": "opus"}]})).unwrap();
        assert!(matches!(&opened, OrqLine::Opened { sessions } if sessions[0].name == "t1"));
        let delivered: OrqLine = serde_json::from_value(json!({"code": "delivered", "round": 2, "commit": "3b799e6"})).unwrap();
        assert!(matches!(delivered, OrqLine::Delivered { round: Some(2), .. }));
        assert!(matches!(serde_json::from_value(json!({"code": "red_retry"})).unwrap(), OrqLine::RedRetry));
    }

    #[test]
    fn orq_panel_reads_the_contract() {
        let mut panel = json!({"run": "2026-09-29-cad3e6fe", "gid": "g1", "errors": [{"file": "x.json", "error": "torto"}], "empty": false,
            "tasks": {"integrated": 4, "total": 11, "total_known": true,
                "rows": [{"n": 1, "title": "Backend", "state": "integrated", "round": 1}, {"n": 5, "title": "Web", "state": "queued", "round": null}]},
            "team": [{"name": "arb", "role": "arbiter", "task": null, "current": true,
                "last": {"code": "delivered", "round": 2, "ts": "2026-09-29T21:00:00Z"}}, {"name": "t1", "role": "executor", "task": 5, "current": false, "last": null}],
            "decisions": [{"task": 4, "ts": "2026-09-29T21:00:00Z", "question": "?", "parecer": null, "event_id": "e1"}],
            "automation": {"mode": {"jev": "on", "regex": "shadow"}, "woke": {"total": 5, "decisions": 1, "alarms": 1, "messages": 3},
                "alone": {"total": 6, "opened": 2, "integrated": 3, "dropped": 1}, "dropped_by_jev": 4,
                "advanced": {"would_drop": 2, "disagree": 1, "judged": 7,
                    "min_confidence": {"p": 0.61, "choice": "act", "ts": "2026-09-29T21:00:00Z", "text": "oi"}, "by_rule": 2}},
            "consumption": {"computed_at": "2026-09-29T21:00:00Z", "since": "2026-09-29T18:00:00Z",
                "sessions": {"team": 11, "measured": 7, "missing": ["t1"]},
                "totals": {"new": 12345678901u64, "cache_read": 2, "usd": null, "usd_partial": true},
                "providers": [{"provider": "claude", "sessions": 3, "new": 5, "cache_read": 6, "usd": 1.5,
                    "models": [{"model": "opus", "sessions": 3, "new": 5, "cache_read": 6, "usd": null}]}],
                "missing_prices": ["x"], "subagents": true},
            "integration": {"branch": "main", "last": {"task": 4, "commit": "3b799e6", "ts": "2026-09-29T21:00:00Z"}, "outcome": "green",
                "red_log": null, "delivery_checks": {"ok": 2, "total": 3, "failing": [3, 5]}}});
        let read: OrqPanel = serde_json::from_value(panel.clone()).unwrap();
        assert_eq!(read.tasks.integrated, 4);
        assert_eq!((read.automation.mode.jev.as_deref(), read.automation.mode.regex.as_deref()), (Some("on"), Some("shadow")));
        assert_eq!(read.tasks.rows[1].round, None);
        assert_eq!(read.team[0].last.as_ref().unwrap().code, "delivered");
        assert_eq!(read.automation.advanced.min_confidence.unwrap().p, Some(0.61));
        assert_eq!(read.integration.delivery_checks.failing, [3, 5]);
        assert_eq!(read.integration.branch.as_deref(), Some("main"));
        let use_ = read.consumption.unwrap();
        assert_eq!(use_.totals.new, 12_345_678_901);
        assert!(use_.totals.usd.is_none() && use_.totals.usd_partial);
        assert_eq!(use_.sessions.missing, ["t1"]);
        assert!(use_.providers[0].models[0].usd.is_none());
        panel["consumption"] = json!(null);
        assert!(serde_json::from_value::<OrqPanel>(panel).unwrap().consumption.is_none());
    }

    #[test]
    fn orq_panel_accepts_the_fallback_blocks_with_nulls() {
        // O bloco que falha no backend sai com estes nulls; o estado de erro não pode virar erro de parse.
        let panel = json!({"run": "r", "gid": "", "errors": [{"file": "integration", "error": "x"}], "empty": false,
            "tasks": {"integrated": 0, "total": 0, "total_known": false, "rows": []}, "team": [], "decisions": [],
            "automation": {"mode": {"jev": null, "regex": null}, "woke": {"total": 0, "decisions": 0, "alarms": 0, "messages": 0},
                "alone": {"total": 0, "opened": 0, "integrated": 0, "dropped": 0}, "dropped_by_jev": 0,
                "advanced": {"would_drop": 0, "disagree": 0, "judged": 0, "min_confidence": null, "by_rule": 0}},
            "consumption": null,
            "integration": {"branch": null, "last": null, "outcome": null, "red_log": null,
                "delivery_checks": {"ok": 0, "total": 0, "failing": []}}});
        let read: OrqPanel = serde_json::from_value(panel).unwrap();
        assert!(read.integration.branch.is_none() && read.automation.mode.jev.is_none());
    }

    #[test]
    fn chat_event_without_orq_is_unchanged() {
        let event: ChatEvent = serde_json::from_value(json!({"kind": "notice", "id": "n1", "text": "aviso", "ts": 1.5})).unwrap();
        assert!(event.orq.is_none());
        assert_eq!(event.body(), "aviso");
        let with: ChatEvent = serde_json::from_value(json!({"kind": "notice", "id": "n2", "text": "x",
            "orq": {"kind": "notice", "body": "x"}})).unwrap();
        assert_eq!(with.orq_entry().unwrap().kind, "notice");
    }

    #[test]
    fn unreadable_codex_is_loading_except_for_user_question_or_failure() {
        let mut session: SessionInfo = serde_json::from_value(json!({"name": "codex", "provider": "codex", "state": "idle",
            "startup_steps": ["Preparing", "Starting"]})).unwrap();
        assert_eq!(session.startup_steps, ["Preparing", "Starting"]);
        for state in ["idle", "working"] { session.state = state.into(); assert_eq!(session.display_state(), "loading"); }
        for state in ["awaiting_input", "dead"] { session.state = state.into(); assert_eq!(session.display_state(), state); }
        session.state = "idle".into();
        session.jsonl = Some("/tmp/thread.jsonl".into());
        assert_eq!(session.display_state(), "idle");
        session.jsonl = None;
        session.provider = "claude".into();
        assert_eq!(session.display_state(), "idle");
    }

    #[test]
    fn orq_row_reads_the_arbiter_and_finds_it_in_the_list() {
        let orq: SessionInfo = serde_json::from_value(json!({"name": "g1-orq", "provider": "orq", "jsonl": "/r/timeline-x.jsonl",
            "orq_arbiter": "arb"})).unwrap();
        assert!(orq.orq() && orq.readable(), "a linha do tempo é lida como conversa");
        let list = [SessionInfo { name: "arb".into(), ..Default::default() }, orq.clone()];
        assert_eq!(orq.arbiter(&list).map(|s| s.name.as_str()), Some("arb"));
        let old: SessionInfo = serde_json::from_value(json!({"name": "a", "provider": "claude"})).unwrap();
        assert!(!old.orq() && old.orq_arbiter.is_none(), "backend sem o campo continua lendo");
        let gone = SessionInfo { orq_arbiter: Some("sumiu".into()), ..orq };
        assert!(gone.arbiter(&list).is_none(), "árbitro fora da lista: o botão fica desligado");
    }

    #[test]
    fn only_readable_non_orq_rows_take_the_composer_focus() {
        let orq = SessionInfo { provider: "orq".into(), jsonl: Some("/r/t.jsonl".into()), ..Default::default() };
        assert!(orq.readable() && !orq.takes_messages(), "o compositor da linha `orq` não é desenhado");
        let chat = SessionInfo { provider: "claude".into(), ..orq.clone() };
        assert!(chat.takes_messages());
        assert!(!SessionInfo { jsonl: None, ..chat }.takes_messages());
    }

    #[test]
    fn pair_session_is_read_only() {
        let chat = SessionInfo { provider: "claude".into(), jsonl: Some("/r/t.jsonl".into()), ..Default::default() };
        let ro = SessionInfo { guest_kind: Some("pair".into()), ..chat.clone() };
        assert!(ro.read_only() && !ro.takes_messages());
        assert!(!SessionInfo { guest_kind: Some("share".into()), ..chat }.read_only());
    }

    #[test]
    fn chat_event_from_the_crate_keeps_the_desktop_reading() {
        let event: ChatEvent = serde_json::from_value(json!({"kind": "tipo_futuro", "id": "k1",
            "skill": {"name": "pdf", "path": "/s/pdf/SKILL.md", "body": "# PDF"}, "tool_input": {"command": "ls"}})).unwrap();
        assert_eq!(event.kind, "tipo_futuro");
        assert_eq!(event.loaded_skill().map(|s| (s.name, s.body)), Some(("pdf".to_owned(), "# PDF".to_owned())));
        assert_eq!(event.body(), "{\n  \"command\": \"ls\"\n}");
        let bad_skill: ChatEvent = serde_json::from_value(json!({"kind": "notice", "id": "n", "skill": {"body": "x"}})).unwrap();
        assert!(bad_skill.loaded_skill().is_none(), "skill sem nome não derruba a mensagem");
        let state: SessionState = serde_json::from_value(json!({"session": "s", "state": "idle",
            "claude_plan_pending": {"plan": "# Plano", "path": "/p.md", "tool_use_id": "t"}})).unwrap();
        let plan = plan_pending(&state).unwrap();
        assert_eq!((plan.plan.as_str(), plan.path.as_deref()), ("# Plano", Some("/p.md")));
        assert!(plan_pending(&SessionState::default()).is_none());
    }
}

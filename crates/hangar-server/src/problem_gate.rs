//! Problema de estado observado só chega à tela se ainda valer depois de `HOLD`: troca de vida, Claude
//! subindo e leitura isolada passam sem aviso. Some na hora quando o estado vem sem ele. Erro de ação
//! (resposta de um clique ou envio) não passa por aqui.
use std::time::{Duration, Instant};

pub const HOLD: Duration = Duration::from_secs(3);

/// Falha de acompanhamento, dado velho ou condição que some sozinha: vai ao diário, nunca à tela.
const DIAGNOSTIC: [&str; 16] = [
    crate::state::facts::UNAVAILABLE, crate::state::monitor::OBSERVATION_FAILED, crate::state::monitor::PERMISSION_FAILED,
    crate::list::facts::UNAVAILABLE, "list_capture_failed", "list_runtime_unavailable", crate::list::classify::RUNTIME_ABSENT,
    "loop_life_changed", "loop_life_unavailable", "loop_list_unavailable", "loop_provider_unsupported", "codex_sem_conexao", crate::runtime::codex_terminal::CONTROL_LOST,
    crate::list::ports::UNAVAILABLE, crate::list::ports::UNREACHED, crate::list::ports::MAP_STALE,
];

/// Códigos de tela que a telemetria conhece (`problem_shown`); outro sai `other`.
pub const SCREEN: &[&str] = &[
    "headless_processo_caiu", "headless_caiu", "headless_nao_subiu", "headless_sem_login", "headless_sem_resposta",
    "headless_turno_erro", "headless_delivery_stalled", "codex_sem_login", "codex_hooks_nao_aprovados", "codex_limite_uso",
    "codex_headless_nao_subiu", "codex_conversa_nao_abriu", "codex_abertura_falhou", "codex_prompt_bloqueado",
    "codex_turno_cortado", "codex_esforco_nao_aplicado", "limit_switch_no_room", "limit_switch_failed", "limit_switch_no_target",
    "limit_switch_login_lost", "terminal_clear_not_applied", "terminal_input_stalled", "terminal_input_composer_busy",
    "terminal_input_delivery_unproved", "terminal_input_delivery_composer_busy", "terminal_out_of_memory", "terminal_died",
    "runtime_falhou", crate::list::bridge::INTERACTION_UNAVAILABLE,
];

/// O código vai só ao diário, qualquer que seja o estado da sessão.
pub fn is_diagnostic(code: &str) -> bool { DIAGNOSTIC.contains(&code) }

/// O código não pede a pessoa: sai da saída (lista e conversa) e vai ao diário. A pergunta sem identidade
/// só pede a pessoa enquanto a sessão espera a resposta dela.
pub fn diagnostic(code: &str, state: &str) -> bool {
    is_diagnostic(code) || (code == crate::list::bridge::INTERACTION_UNAVAILABLE && state != "awaiting_input")
}

/// Diagnóstico não cobre código de tela: `new` não entra no lugar de `current`, que o portão mostraria.
pub fn keeps(current: Option<&str>, new: &str) -> bool { current.is_some_and(|current| !is_diagnostic(current)) && is_diagnostic(new) }

/// Registro do aviso tirado da tela: o diário recebe o código e o detalhe que a tela teria mostrado (uma
/// vez por minuto por sessão e código), e o log também.
pub fn report_hidden(diag: &crate::diag::DiagClient, session: &str, code: &str, detail: Option<&str>) {
    if crate::warn_limit::allow(Some(session), &format!("problem_hidden:{code}")) {
        tracing::warn!(session, code, detail, "aviso de diagnóstico fora da tela");
        // No ritmo do log: cada estado publicado repetiria a contagem.
        if is_diagnostic(code) && let Some(telemetry) = crate::telemetry_hooks::installed() {
            use hangar_telemetry::events::{Code, Dims, Event};
            telemetry.count(Event::ProblemHidden, Dims::new().with("code", Code::known(code, &DIAGNOSTIC)), 1);
        }
        if let Some((code, cause)) = health(code, detail) {
            crate::telemetry_hooks::report_health(code, cause, hangar_usage_diary::Severity::Warn, Some(session), Vec::new(), None);
        }
    }
    diag.report_with("rust.problem_hidden", session, code, "aviso de diagnóstico fora da tela",
        serde_json::json!({"detalhe": detail, "via": "conversa"}));
}

/// A falha de saúde (`events::HEALTH`) que o aviso escondido é, e a causa tirada do detalhe. O detalhe
/// é um código fixo deste binário ou o texto fixo do `TerminalError`; o que não está aqui sai `other`.
pub fn health(code: &str, detail: Option<&str>) -> Option<(&'static str, &'static str)> {
    use crate::state::{facts::UNAVAILABLE, monitor::{OBSERVATION_FAILED, PERMISSION_FAILED}};
    let detail = detail.unwrap_or_default();
    // Vínculo da vida: igual nos três, só o nome do cofre de permissões muda.
    let binding = match detail {
        "plugin_runtime_absent" | "state_runtime_unbound" | "state_lifecycle_unbound" => Some("runtime_unbound"),
        "runtime_binding" | "state_lifecycle_binding_missing" => Some("binding_missing"),
        "erro_interacao_desatualizada" | "state_binding_changed" | "permission_binding_changed" => Some("binding_changed"),
        _ => None,
    };
    let (code, cause) = if code == UNAVAILABLE {
        (UNAVAILABLE, binding.unwrap_or(match detail {
            "plugin_store_unavailable" => "plugin_store_unavailable",
            "plugin_facts_invalid" => "plugin_facts_invalid",
            "plugin_question_invalid" => "plugin_question_invalid",
            "permission_store_unavailable" => "permission_store_poisoned",
            "askq_unreadable" | "askq_transcript_unreadable" | "askq_path" => "askq_sidecar_unreadable",
            "askq_malformed" | "askq_serialize" => "askq_sidecar_malformed",
            "askq_changed_during_read" => "askq_race",
            "askq_consumed_lock" | "askq_scan_lock" => "askq_lock_poisoned",
            "askq_frame_unavailable" => "askq_frame_missing",
            "askq_timeout" => "askq_timeout",
            "plan_notice_missing" => "plan_notice_missing",
            "erro_interacao_indisponivel" => "interaction_id_missing",
            "state_files_timeout" | "state_facts_timeout" => "files_timeout",
            "state_files_panic" | "state_files_cancelled" | "askq_task_failed" => "task_panic",
            "list_dirs_missing" => "files_dirs_missing",
            "state_plugin_binding_changed" | "state_feed_binding_changed" => "feed_binding_changed",
            lifecycle if lifecycle.starts_with("lifecycle_") => "dead_unproven",
            _ => "other",
        }))
    } else if code == OBSERVATION_FAILED {
        (OBSERVATION_FAILED, binding.unwrap_or(match detail {
            "permission_store_unavailable" => "permission_store_poisoned",
            "terminal capture timeout" | "capture_timeout" => "capture_timeout",
            "terminal startup timeout" | "terminal acquire timeout" => "startup_timeout",
            "terminal command timeout" | "terminal release timeout" => "command_timeout",
            "terminal observer closed" | "terminal observer exited" | "terminal observer EOF" => "observer_died",
            "terminal read failed" | "terminal write failed" => "observer_io",
            "tmux command failed" | "capture_refused" => "mux_command_failed",
            "control payload too large" | "control line too large" | "invalid frame UTF-8" | "control frame too large"
            | "invalid notification UTF-8" | "invalid control frame" | "unfinished control frame" | "unexpected control frame"
            | "terminal command marker mismatch" | "invalid terminal metadata" | "invalid terminal cursor" | "capture_undecodable" => "protocol_error",
            "terminal target changed" => "target_changed",
            "invalid terminal target" | "invalid capture request" | "capture_session_missing" => "target_invalid",
            "too many terminal consumers" | "too many terminal observers" | "too many terminal retries" | "terminal queue full"
            | "terminal event queue full" => "pool_saturated",
            "terminal control unavailable" | "terminal mux missing" | "capture_mux_no_answer" => "mux_unavailable",
            "cannot start terminal observer" | "cannot start terminal watch" | "capture_spawn_failed" => "observer_spawn_failed",
            "terminal watch without output" | "psmux_direct_panicked" => "mux_process_failed",
            _ => "other",
        }))
    } else if code == PERMISSION_FAILED {
        (PERMISSION_FAILED, binding.unwrap_or(match detail {
            "permission_frame_unavailable" => "frame_lock_poisoned",
            "permission_store_unavailable" => "store_poisoned",
            "permission_capture_stale" => "capture_stale",
            "permission_mode_missing" => "mode_unconfirmed",
            "permission_mode_unknown" => "mode_unknown",
            _ => "other",
        }))
    } else { return None };
    Some((code, cause))
}

/// A entrega incerta segura a fila com ou sem texto no campo: a troca do motivo não é problema novo.
fn same_problem(a: &str, b: &str) -> bool {
    const HELD: [&str; 2] = ["terminal_input_delivery_unproved", "terminal_input_delivery_composer_busy"];
    a == b || (HELD.contains(&a) && HELD.contains(&b))
}

/// O que fazer com o problema que o estado traz agora.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// Sem problema: nada a mostrar.
    Clear,
    /// Valeu pelo prazo: mostra.
    Show,
    /// Ainda no prazo: esconde e confere de novo em `Instant`.
    Hold(Instant),
}

/// Um por sessão; código diferente do anterior conta como problema novo.
#[derive(Default)]
pub struct ProblemGate { shown: Option<String>, pending: Option<(String, Instant)> }

/// O problema que a lista mostra de cada sessão: a conversa aberta depois começa do mesmo ponto, sem
/// esconder de novo pelo prazo o que a lista já mostra.
#[derive(Default)]
pub struct Shown(std::sync::Mutex<std::collections::HashMap<String, String>>);
impl Shown {
    pub fn set(&self, name: &str, code: Option<&str>) {
        let mut shown = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match code { Some(code) => { shown.insert(name.to_owned(), code.to_owned()); }, None => { shown.remove(name); } }
    }
    pub fn get(&self, name: &str) -> Option<String> { self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(name).cloned() }
    pub fn retain(&self, keep: impl Fn(&str) -> bool) { self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|name, _| keep(name)); }
}

impl ProblemGate {
    /// Já mostrando `code` (o que a lista mostra): o mesmo problema sai sem esperar de novo.
    pub fn showing(code: Option<String>) -> Self { Self { shown: code, pending: None } }

    pub fn admit(&mut self, code: Option<&str>, now: Instant, hold: Duration) -> Gate { self.admit_noting(code, now, hold).0 }

    /// `admit`, e se esta chamada passou o problema de esperando para mostrado: a vez que ele foi visto.
    pub fn admit_noting(&mut self, code: Option<&str>, now: Instant, hold: Duration) -> (Gate, bool) {
        let Some(code) = code else { self.shown = None; self.pending = None; return (Gate::Clear, false) };
        if self.shown.as_deref().is_some_and(|shown| same_problem(shown, code)) { self.shown = Some(code.to_owned()); return (Gate::Show, false); }
        self.shown = None;
        let since = match &self.pending { Some((pending, since)) if same_problem(pending, code) => *since, _ => now };
        if now.saturating_duration_since(since) >= hold {
            self.pending = None;
            self.shown = Some(code.to_owned());
            return (Gate::Show, true);
        }
        self.pending = Some((code.to_owned(), since));
        (Gate::Hold(since + hold), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_problem_shows_only_after_it_held_and_clears_at_once() {
        let (mut gate, t0) = (ProblemGate::default(), Instant::now());
        assert_eq!(gate.admit(None, t0, HOLD), Gate::Clear);
        assert_eq!(gate.admit(Some("runtime_binding"), t0, HOLD), Gate::Hold(t0 + HOLD));
        assert_eq!(gate.admit(Some("runtime_binding"), t0 + Duration::from_secs(2), HOLD), Gate::Hold(t0 + HOLD), "o prazo conta do primeiro");
        assert_eq!(gate.admit(Some("runtime_binding"), t0 + HOLD, HOLD), Gate::Show);
        assert_eq!(gate.admit(Some("runtime_binding"), t0 + Duration::from_secs(9), HOLD), Gate::Show, "mostrado segue mostrado");
        assert_eq!(gate.admit(None, t0 + Duration::from_secs(10), HOLD), Gate::Clear, "resolveu: some na hora");
        let t1 = t0 + Duration::from_secs(11);
        assert_eq!(gate.admit(Some("runtime_binding"), t1, HOLD), Gate::Hold(t1 + HOLD), "voltou: espera de novo");
        // A telemetria conta só a passagem de esperando para mostrado.
        assert_eq!(gate.admit_noting(Some("runtime_binding"), t1 + HOLD, HOLD), (Gate::Show, true));
        for tick in 2..12 { assert_eq!(gate.admit_noting(Some("runtime_binding"), t1 + HOLD * tick, HOLD), (Gate::Show, false)); }
        assert_eq!(ProblemGate::showing(Some("a".into())).admit_noting(Some("a"), t1, HOLD), (Gate::Show, false), "a lista já mostrava");
        for code in SCREEN { assert!(!is_diagnostic(code) && hangar_telemetry::wire::is_name(code), "{code}"); }
    }

    #[test]
    fn diagnostic_codes_stay_off_the_screen_and_screen_codes_stay_on() {
        for code in DIAGNOSTIC {
            assert!(diagnostic(code, "working"), "{code} é diagnóstico");
            assert!(diagnostic(code, "awaiting_input"), "{code} é diagnóstico em qualquer estado");
            assert!(keeps(Some("terminal_input_stalled"), code) && !keeps(Some(code), "terminal_input_stalled"), "{code} não cobre tela");
        }
        for code in ["headless_processo_caiu", "headless_caiu", "headless_nao_subiu", "headless_sem_login", "headless_sem_resposta",
            "headless_turno_erro", "codex_sem_login", "codex_hooks_nao_aprovados", "codex_limite_uso", "codex_headless_nao_subiu",
            "limit_switch_no_room", "limit_switch_failed", "limit_switch_no_target", "codex_conversa_nao_abriu", "codex_abertura_falhou",
            "codex_prompt_bloqueado", "codex_turno_cortado", "terminal_clear_not_applied", "terminal_input_stalled", "runtime_falhou",
            "codex_esforco_nao_aplicado", "headless_delivery_stalled"] {
            assert!(!diagnostic(code, "idle"), "{code} é de tela");
        }
        assert!(diagnostic("interacao_indisponivel", "working"), "P1: fora da espera, diário");
        assert!(diagnostic("interacao_indisponivel", "idle"));
        assert!(!diagnostic("interacao_indisponivel", "awaiting_input"), "P1: esperando a pessoa, tela");
        assert!(diagnostic("session_port_map_stale", "idle"), "mapa do túnel vencido: só log");
    }

    #[test]
    fn telemetry_hidden_warnings_become_health_with_a_closed_cause() {
        use crate::state::{facts::UNAVAILABLE, monitor::{OBSERVATION_FAILED, PERMISSION_FAILED}};
        let known: [(&str, &[&str]); 3] = [
            (UNAVAILABLE, &["plugin_runtime_absent", "state_runtime_unbound", "state_lifecycle_unbound", "runtime_binding",
                "state_lifecycle_binding_missing", "erro_interacao_desatualizada", "state_binding_changed", "plugin_store_unavailable",
                "plugin_facts_invalid", "plugin_question_invalid", "permission_store_unavailable", "askq_unreadable",
                "askq_transcript_unreadable", "askq_path", "askq_malformed", "askq_serialize", "askq_changed_during_read",
                "askq_consumed_lock", "askq_scan_lock", "askq_frame_unavailable", "askq_timeout", "plan_notice_missing",
                "erro_interacao_indisponivel", "state_files_timeout", "state_facts_timeout", "state_files_panic",
                "state_files_cancelled", "askq_task_failed", "list_dirs_missing", "lifecycle_closed_record",
                "lifecycle_binding_unavailable", "state_plugin_binding_changed", "state_feed_binding_changed"]),
            (OBSERVATION_FAILED, &["state_runtime_unbound", "runtime_binding", "state_binding_changed", "permission_store_unavailable",
                "terminal capture timeout", "capture_timeout", "terminal startup timeout", "terminal acquire timeout",
                "terminal command timeout", "terminal release timeout", "terminal observer closed", "terminal observer exited",
                "terminal observer EOF", "terminal read failed", "terminal write failed", "tmux command failed", "capture_refused",
                "invalid control frame", "unexpected control frame", "capture_undecodable", "terminal command marker mismatch",
                "terminal target changed", "invalid terminal target", "capture_session_missing", "too many terminal consumers",
                "terminal queue full", "terminal control unavailable", "terminal mux missing", "capture_mux_no_answer",
                "cannot start terminal observer", "capture_spawn_failed", "terminal watch without output", "psmux_direct_panicked"]),
            (PERMISSION_FAILED, &["state_runtime_unbound", "runtime_binding", "permission_binding_changed", "permission_frame_unavailable",
                "permission_store_unavailable", "permission_capture_stale", "permission_mode_missing", "permission_mode_unknown"]),
        ];
        for (code, details) in known {
            let causes = hangar_telemetry::events::HEALTH.iter().find(|h| h.code == code).unwrap().causes;
            for detail in details {
                let (health_code, cause) = health(code, Some(detail)).unwrap_or_else(|| panic!("{code} {detail}"));
                assert!(health_code == code && causes.contains(&cause), "{code} {detail} -> {cause}");
            }
        }
        assert_eq!(health(UNAVAILABLE, Some("askq_scan_lock")), Some((UNAVAILABLE, "askq_lock_poisoned")));
        assert_eq!(health(OBSERVATION_FAILED, Some("terminal read failed")), Some((OBSERVATION_FAILED, "observer_io")));
        assert_eq!(health(PERMISSION_FAILED, Some("permission_store_unavailable")), Some((PERMISSION_FAILED, "store_poisoned")));
        assert_eq!(health(UNAVAILABLE, Some("texto que ninguém conhece")), Some((UNAVAILABLE, "other")));
        assert_eq!(health(UNAVAILABLE, None), Some((UNAVAILABLE, "other")));
        assert_eq!(health("list_capture_failed", Some("capture_timeout")), None, "só os códigos de HEALTH");
    }

    #[test]
    fn the_held_delivery_switching_its_reason_stays_one_problem() {
        let (mut gate, t0) = (ProblemGate::default(), Instant::now());
        assert_eq!(gate.admit(Some("terminal_input_delivery_unproved"), t0, HOLD), Gate::Hold(t0 + HOLD));
        assert_eq!(gate.admit(Some("terminal_input_delivery_composer_busy"), t0 + HOLD, HOLD), Gate::Show, "o prazo segue do primeiro");
        assert_eq!(gate.admit(Some("terminal_input_delivery_unproved"), t0 + HOLD * 2, HOLD), Gate::Show, "texto apagado: segue mostrado");
    }

    #[test]
    fn a_transient_problem_never_shows_and_a_new_code_waits_again() {
        let (mut gate, t0) = (ProblemGate::default(), Instant::now());
        assert_eq!(gate.admit(Some("a"), t0, HOLD), Gate::Hold(t0 + HOLD));
        assert_eq!(gate.admit(None, t0 + Duration::from_secs(1), HOLD), Gate::Clear, "durou menos que o prazo");
        assert_eq!(gate.admit(Some("a"), t0 + Duration::from_secs(2), HOLD), Gate::Hold(t0 + Duration::from_secs(2) + HOLD));
        assert_eq!(gate.admit(Some("a"), t0 + Duration::from_secs(6), HOLD), Gate::Show);
        let t1 = t0 + Duration::from_secs(7);
        assert_eq!(gate.admit(Some("b"), t1, HOLD), Gate::Hold(t1 + HOLD), "código novo é problema novo");
    }
}

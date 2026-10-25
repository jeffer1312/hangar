//! Problema de estado observado só chega à tela se ainda valer depois de `HOLD`: troca de vida, Claude
//! subindo e leitura isolada passam sem aviso. Some na hora quando o estado vem sem ele. Erro de ação
//! (resposta de um clique ou envio) não passa por aqui.
use std::time::{Duration, Instant};

pub const HOLD: Duration = Duration::from_secs(3);

/// Falha de acompanhamento, dado velho ou condição que some sozinha: vai ao diário, nunca à tela.
const DIAGNOSTIC: [&str; 9] = [
    crate::state::facts::UNAVAILABLE, crate::state::monitor::OBSERVATION_FAILED, crate::state::monitor::PERMISSION_FAILED,
    crate::list::facts::UNAVAILABLE, "list_capture_failed", "list_runtime_unavailable", crate::list::classify::RUNTIME_ABSENT,
    "codex_sem_conexao", crate::runtime::codex_terminal::CONTROL_LOST,
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

/// Registro do aviso tirado da tela: o diário recebe o código (uma vez por minuto por sessão e código) e o
/// log, o detalhe, que o diário não aceita por ser texto livre.
pub fn report_hidden(diag: &crate::diag::DiagClient, session: &str, code: &str, detail: Option<&str>) {
    if crate::warn_limit::allow(Some(session), &format!("problem_hidden:{code}")) {
        tracing::warn!(session, code, detail, "aviso de diagnóstico fora da tela");
    }
    diag.report("rust.problem_hidden", session, code, "aviso de diagnóstico fora da tela");
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

    pub fn admit(&mut self, code: Option<&str>, now: Instant, hold: Duration) -> Gate {
        let Some(code) = code else { self.shown = None; self.pending = None; return Gate::Clear };
        if self.shown.as_deref() == Some(code) { return Gate::Show; }
        self.shown = None;
        let since = match &self.pending { Some((pending, since)) if pending == code => *since, _ => now };
        if now.saturating_duration_since(since) >= hold {
            self.pending = None;
            self.shown = Some(code.to_owned());
            return Gate::Show;
        }
        self.pending = Some((code.to_owned(), since));
        Gate::Hold(since + hold)
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

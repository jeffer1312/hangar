//! O que sai junto com o estado (porte do laço de `sse.merged_events`): a sugestão quando muda, a
//! borda "voltou a aceitar texto" que pede a entrega ao Python e o problema do runtime de entrada.
use hangar_api::state::StateEvent;

/// O que o ator de entrada terminal publica: `error` é o código do canal `problem` ou do retrato,
/// `message` a frase dele, `input_stalled` o código do escritor parado.
pub use crate::runtime::terminal_status::TerminalStatus as RuntimeView;

/// `runtime_adapter.runtime_problem`: `(código, detalhe)` do runtime para o `problema` do estado.
pub fn runtime_problem(view: &RuntimeView) -> Option<(String, String)> {
    // A entrega incerta e a sessão sem prova da conversa têm frase própria (o que fazer), não o erro
    // genérico que elas mesmas causam no runtime.
    if let Some(code) = view.error.as_deref().filter(|c| !c.is_empty() && !explained(view.input_stalled.as_deref(), c)) {
        let text = match view.message.as_deref().filter(|m| !m.is_empty()) {
            Some(message) => format!("{code}: {message}"),
            None => code.to_owned(),
        };
        return Some(("runtime_falhou".into(), text.chars().take(300).collect()));
    }
    let Some(stalled) = view.input_stalled.as_deref().filter(|s| !s.is_empty()) else { return view.limit.clone() };
    if stalled == "queue_batch_too_large" {
        return Some(("terminal_input_stalled".into(),"O lote é grande demais. Edite ou remova mensagens da fila para enviar.".into()));
    }
    let code = match stalled {
        "composer_busy" => "terminal_input_composer_busy",
        "composer_unreadable" => "terminal_input_composer_unreadable",
        "capture_failed" | "capture_utf8" => "terminal_input_capture_failed",
        "delivery_unproved" => "terminal_input_delivery_unproved",
        "delivery_composer_busy" => "terminal_input_delivery_composer_busy",
        "no_safe_delivery" => "terminal_input_no_safe_delivery",
        _ => "terminal_input_stalled",
    };
    Some((code.into(), stalled.chars().take(60).collect()))
}

/// O erro do runtime é consequência do motivo de entrada parada que já sai com frase própria.
pub(crate) fn explained(stalled: Option<&str>, error: &str) -> bool {
    match stalled {
        Some("delivery_unproved" | "delivery_composer_busy") => error == "terminal_delivery_unknown",
        Some("no_safe_delivery") => matches!(error, "terminal_facts" | "terminal_delivery_unknown"),
        _ => false,
    }
}

/// Problema que veio do runtime, e não da observação nem dos fatos.
pub fn is_runtime_problem(code: &str) -> bool { code == "runtime_falhou" || code.starts_with("terminal_input_") || code.starts_with("limit_switch_") }

/// O pane aceita texto livre: fora de pergunta e de menu.
pub fn deliverable(event: &StateEvent) -> bool {
    !matches!(event.state.as_str(), "awaiting_input" | "dead") && !event.overlay
}

/// Memória das bordas, que sobrevive ao `/clear`: a sugestão e a entrega não são da conversa.
#[derive(Default)]
pub struct Edges { suggestion: String, deliverable: bool }

impl Edges {
    /// A sugestão mudou desde a última que saiu. `None`: fato desconhecido, nada muda.
    pub fn suggestion(&mut self, now: Option<&str>) -> Option<String> {
        let now = now.filter(|s| *s != self.suggestion)?;
        self.suggestion = now.to_owned();
        Some(self.suggestion.clone())
    }

    /// Subiu a borda de entregável. Começa em falso: o primeiro estado entregável depois de nascer
    /// também entrega (o que ficou na fila durante um reinício).
    pub fn deliverable(&mut self, event: &StateEvent) -> bool {
        let now = deliverable(event);
        let edge = now && !self.deliverable;
        self.deliverable = now;
        edge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_problem_like_python() {
        let v = |error: Option<&str>, message: Option<&str>, stalled: Option<&str>| RuntimeView {
            error: error.map(Into::into), message: message.map(Into::into), input_stalled: stalled.map(Into::into), limit: None };
        assert_eq!(runtime_problem(&v(None, None, None)), None);
        assert_eq!(runtime_problem(&v(Some("queue_io"), Some("disco"), Some("composer_busy"))),
            Some(("runtime_falhou".into(), "queue_io: disco".into())), "erro vence a entrada parada");
        assert_eq!(runtime_problem(&v(Some("queue_io"), Some(""), None)), Some(("runtime_falhou".into(), "queue_io".into())));
        assert_eq!(runtime_problem(&v(Some(""), None, Some("composer_busy"))).unwrap().0, "terminal_input_composer_busy", "código vazio é falso no Python");
        assert_eq!(runtime_problem(&v(None, None, Some("capture_utf8"))),
            Some(("terminal_input_capture_failed".into(), "capture_utf8".into())));
        assert_eq!(runtime_problem(&v(None, None, Some("x".repeat(80).as_str()))).unwrap(),
            ("terminal_input_stalled".into(), "x".repeat(60)));
        assert!(is_runtime_problem("terminal_input_stalled") && !is_runtime_problem("terminal_observacao_falhou"));
        // Sem prova da conversa, os fatos falham na fila: a frase é a da causa, não o erro genérico.
        assert_eq!(runtime_problem(&v(Some("terminal_facts"), None, Some("no_safe_delivery"))),
            Some(("terminal_input_no_safe_delivery".into(), "no_safe_delivery".into())));
        assert_eq!(runtime_problem(&v(Some("queue_io"), None, Some("no_safe_delivery"))).unwrap().0, "runtime_falhou", "outro erro continua vencendo");
        // Trava da entrega incerta com texto no campo: a frase diz que é a pessoa quem destrava.
        assert_eq!(runtime_problem(&v(Some("terminal_delivery_unknown"), None, Some("delivery_composer_busy"))),
            Some(("terminal_input_delivery_composer_busy".into(), "delivery_composer_busy".into())));
    }

    #[test]
    fn the_failed_limit_switch_shows_with_its_own_code_behind_errors_and_stalls() {
        let limit = Some(("limit_switch_no_room".to_owned(), "17:30".to_owned()));
        let view = RuntimeView { limit: limit.clone(), ..Default::default() };
        assert_eq!(runtime_problem(&view), limit);
        assert!(is_runtime_problem("limit_switch_no_room") && is_runtime_problem("limit_switch_failed"), "sai e entra sem rodada nova");
        let stalled = RuntimeView { input_stalled: Some("composer_busy".into()), ..view };
        assert_eq!(runtime_problem(&stalled).unwrap().0, "terminal_input_composer_busy");
    }
}

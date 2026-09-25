//! Mensagens de usuário que viram cartão em vez de bolha. O que não casar volta `None` e segue na bolha de sempre;
//! o texto cru nunca some, quem o mostra é o cartão, num bloco fechado.
use serde_json::Value;

/// Notificação de subagente que o Codex grava como mensagem de usuário (`frontend/src/lib/subagenteCodex.ts`).
#[derive(Clone, Debug, PartialEq)]
pub struct CodexSubagent {
    /// Chave de `status` como o Codex a escreveu: `completed`, `errored`, ou outra que ele criar.
    pub status: String,
    pub agent_path: String,
    /// Corpo do status; markdown, no caso do relatório.
    pub report: String,
}

impl CodexSubagent {
    /// `errored` e `failed` são o mesmo desfecho para quem lê; o Codex usa os dois.
    pub fn failed(&self) -> bool { matches!(self.status.as_str(), "errored" | "failed") }
}

pub fn codex_subagent(text: &str) -> Option<CodexSubagent> {
    let inner = text.trim().strip_prefix("<subagent_notification>")?.strip_suffix("</subagent_notification>")?;
    let object: Value = serde_json::from_str(inner.trim()).ok()?;
    // O Codex grava uma chave só. Com mais de uma vale a primeira do texto, como no web: o serde_json deste build tem
    // `preserve_order`.
    // Lista também passa, como no `typeof … === 'object'` do web: a chave vira o índice "0".
    let (status, body) = match object.get("status")? {
        Value::Object(map) => map.iter().next().map(|(key, body)| (key.clone(), body))?,
        Value::Array(items) => ("0".to_owned(), items.first()?),
        _ => return None,
    };
    Some(CodexSubagent {
        status,
        agent_path: object.get("agent_path").and_then(Value::as_str).unwrap_or("").to_owned(),
        // Corpo que não é texto ainda vira cartão: o JSON legível é melhor que a bolha com o envelope inteiro.
        // Em bloco de código: fora dele o markdown juntaria as linhas do JSON numa só.
        report: match body {
            Value::String(s) => s.clone(),
            other => crate::conversation::fenced(&serde_json::to_string_pretty(other).unwrap_or_default()),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(json: &str) -> String { format!("<subagent_notification>\n{json}\n</subagent_notification>") }

    #[test]
    fn codex_notification_reads_status_path_and_report() {
        let card = codex_subagent(&wrap(r#"{"agent_path":"01a0b2c3d4e5","status":{"completed":"**feito**\n- a"}}"#)).unwrap();
        assert_eq!((card.status.as_str(), card.agent_path.as_str(), card.report.as_str()), ("completed", "01a0b2c3d4e5", "**feito**\n- a"));
        assert!(!card.failed());
        assert!(codex_subagent(&wrap(r#"{"status":{"errored":"x"}}"#)).unwrap().failed());
        assert!(codex_subagent(&wrap(r#"{"status":{"failed":"x"}}"#)).unwrap().failed());
        assert_eq!(codex_subagent(&wrap(r#"{"status":{"errored":"x"}}"#)).unwrap().agent_path, "");
    }

    #[test]
    fn codex_body_that_is_not_text_becomes_readable_json() {
        let card = codex_subagent(&wrap(r#"{"status":{"running":{"step":2}}}"#)).unwrap();
        assert_eq!((card.status.as_str(), card.report.as_str()), ("running", "```text\n{\n  \"step\": 2\n}\n```"));
        let listed = codex_subagent(&wrap(r#"{"status":["primeiro","segundo"]}"#)).unwrap();
        assert_eq!((listed.status.as_str(), listed.report.as_str()), ("0", "primeiro"));
        let first = codex_subagent(&wrap(r#"{"status":{"zeta":"a","alfa":"b"}}"#)).unwrap();
        assert_eq!((first.status.as_str(), first.report.as_str()), ("zeta", "a"));
    }

    #[test]
    fn anything_else_stays_a_bubble() {
        for text in [
            "oi",
            "antes <subagent_notification>{\"status\":{\"completed\":\"x\"}}</subagent_notification>",
            &wrap("não é json"),
            &wrap(r#"{"status":"completed"}"#),
            &wrap(r#"{"status":{}}"#),
            &wrap(r#"{"status":[]}"#),
            &wrap(r#"{"status":null}"#),
            &wrap(r#"["status"]"#),
        ] {
            assert_eq!(codex_subagent(text), None, "{text}");
        }
    }
}

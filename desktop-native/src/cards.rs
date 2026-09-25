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

/// Mensagem de usuário que vira cartão.
#[derive(Clone, Debug, PartialEq)]
pub enum Card { Codex(CodexSubagent), Baton(Baton) }

/// Kick-off que a sessão sucessora recebe na passagem de bastão (`frontend/src/lib/bastaoRecado.ts`).
#[derive(Clone, Debug, PartialEq)]
pub struct Baton {
    /// Sessão de onde o trabalho vem; continua viva, só parou de escrever.
    pub origin: String,
    /// Caminho do resumo `.md` gravado no disco do servidor.
    pub dossier: String,
    pub account: String,
    pub model: String,
}

pub const BATON_PREFIX: &str = "[hangar: passagem de bastão]";
// O cartão reescreve o recado em frases curtas; só é honesto enquanto o recado diz o mesmo. Mudou a instrução, estas
// marcas deixam de casar e a mensagem volta inteira, como bolha.
const BATON_MARKS: [&str; 3] = ["Leia o plano", "continua VIVA", "NÃO move esses vínculos"];

/// O texto entre crases logo depois de `label` (espaços no meio), na primeira ocorrência que tiver um.
fn ticked<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    text.match_indices(label).find_map(|(at, _)| {
        let rest = text[at + label.len()..].trim_start().strip_prefix('`')?;
        rest.find('`').map(|end| &rest[..end]).filter(|inside| !inside.is_empty())
    })
}

pub fn baton(text: &str) -> Option<Baton> {
    if !text.starts_with(BATON_PREFIX) || !BATON_MARKS.iter().all(|mark| text.contains(mark)) { return None; }
    let origin = ticked(text, "Você continua o trabalho da sessão")?;
    // Duas redações: o recado já chamou o arquivo de "dossiê" e hoje o chama de "resumo do trabalho"; vale a que vem antes.
    let dossier = ["o dossiê em", "o resumo do trabalho em"].iter()
        .filter_map(|label| text.match_indices(label).find_map(|(at, _)| ticked(&text[at..], label).map(|path| (at, path))))
        .min_by_key(|(at, _)| *at).map(|(_, path)| path)?;
    // "Ela vinha de <conta e modelo> —", na mesma linha.
    let from = text.split_once("Ela vinha de").and_then(|(_, rest)| rest.lines().next()?.split_once('—')).map_or("", |(from, _)| from.trim());
    Some(Baton {
        origin: origin.to_owned(),
        dossier: dossier.to_owned(),
        account: ticked(from, "conta").unwrap_or("").to_owned(),
        model: ticked(from, "modelo").unwrap_or("").to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // As linhas do `bastao.kickoff` do backend.
    fn kickoff(dossier_line: &str, from_line: &str) -> String {
        [
            "[hangar: passagem de bastão] Você continua o trabalho da sessão `origem-x` — não é tarefa nova, é a mesma.",
            dossier_line,
            "Leia o plano e o contrato citados no resumo ANTES de mexer em qualquer arquivo.",
            "A sessão `origem-x` continua VIVA, mas parou de escrever.",
            "Se o resumo mostrar par ou grupo, a continuação NÃO move esses vínculos: troque a linha.",
            from_line,
        ].join("\n")
    }
    const DOSSIER: &str = "Comece lendo, com um `Read`, o resumo do trabalho em `/srv/bastao/origem-x.md`: onde ele está.";
    const FROM: &str = "Ela vinha de conta `02-200` · modelo `opus/high` — você pode estar em outra.";

    #[test]
    fn baton_reads_origin_dossier_account_and_model() {
        let card = baton(&kickoff(DOSSIER, FROM)).unwrap();
        assert_eq!(card, Baton { origin: "origem-x".into(), dossier: "/srv/bastao/origem-x.md".into(), account: "02-200".into(), model: "opus/high".into() });
        // Redação antiga ("dossiê") e sem conta/modelo (a linha diz que estão no resumo).
        let old = baton(&kickoff("Comece lendo o dossiê em `/srv/velho.md`.", "A conta e o modelo de onde ela vinha estão na primeira seção do resumo.")).unwrap();
        assert_eq!((old.dossier.as_str(), old.account.as_str(), old.model.as_str()), ("/srv/velho.md", "", ""));
        let only_model = baton(&kickoff(DOSSIER, "Ela vinha de modelo `sonnet` — você pode estar em outra.")).unwrap();
        assert_eq!((only_model.account.as_str(), only_model.model.as_str()), ("", "sonnet"));
    }

    #[test]
    fn baton_without_prefix_marks_origin_or_dossier_stays_a_bubble() {
        let full = kickoff(DOSSIER, FROM);
        assert_eq!(baton(&format!("oi {full}")), None);
        assert_eq!(baton(&full.replace("continua VIVA", "continua viva")), None);
        assert_eq!(baton(&full.replace("Leia o plano", "Leia")), None);
        assert_eq!(baton(&full.replace("NÃO move esses vínculos", "não move")), None);
        assert_eq!(baton(&kickoff("Comece lendo o resumo.", FROM)), None);
        assert_eq!(baton(&full.replace("sessão `origem-x` —", "sessão origem-x —")), None);
    }

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

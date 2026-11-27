//! Balanceamento: a máquina que a Nova conversa marca e a página que grava as regras em todas as máquinas.
use super::*;
use super::create::ServerChoice;

mod page;
pub(super) use page::PlacementSettings;

/// Quanto cada máquina tem para responder: a que passa disso fica sem resposta e não trava a tela.
const WAIT: u64 = 5;

/// O identificador da máquina (o `CP_SERVER_ID`), o mesmo que a escolha devolve em `server`.
pub(super) async fn ident(api: &Api) -> Option<String> {
    let value = api.server_read(&["peers", "identificador"], &[], WAIT).await.ok()?;
    value.get("identificador")?.as_str().filter(|id| !id.is_empty()).map(str::to_owned)
}

/// A sugestão só vale com a tela em "sugerir" e o pedido dentro do balanceamento: a máquina e o motivo.
fn suggested(config: &Value, placement: &Value) -> Option<(String, String)> {
    if config["ui_mode"].as_str() != Some("suggest") { return None; }
    if placement.pointer("/reason/code").and_then(Value::as_str).is_none_or(|code| code == "not_balanced") { return None; }
    let server = placement["server"].as_str().filter(|s| !s.is_empty())?;
    Some((server.to_owned(), reason_text(placement)))
}

/// O motivo da escolha no idioma do app, montado dos códigos: o `text` do servidor sai sempre em português.
fn reason_text(placement: &Value) -> String {
    let str_of = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let machine = str_of(&placement["server"]);
    let exclusion = |why: &Value| match why.as_str().unwrap_or_default() {
        "missing" => tr("placement_exclusion_missing"),
        "not_capable" => tr("placement_exclusion_not_capable"),
        "schedule" => tr("placement_exclusion_schedule"),
        "memory" => tr("placement_exclusion_memory"),
        "working" => tr("placement_exclusion_working"),
        "cpu" => tr("placement_exclusion_cpu"),
        "no_clone_dir" => tr("placement_exclusion_no_clone_dir"),
        other => other.to_owned(),
    };
    let with_machine = |key: &str| tr(key).replace("{maquina}", &machine);
    let reason = &placement["reason"];
    let mut text = match reason["code"].as_str().unwrap_or_default() {
        "same_folder" => with_machine("placement_reason_same_folder"),
        "unpublished" => match reason["why"].as_str() {
            Some("dirty") => tr("placement_reason_unpublished_dirty"),
            Some("no_upstream") => tr("placement_reason_unpublished_no_upstream"),
            _ => tr("placement_reason_unpublished_ahead"),
        },
        "plain_dir" => with_machine("placement_reason_plain_dir"),
        "project_rule" => with_machine("placement_reason_project_rule")
            .replace("{projeto}", str_of(&reason["project"]).trim_start_matches("git:")),
        "local_within_limits" => with_machine("placement_reason_local_within_limits"),
        "least_loaded" => with_machine("placement_reason_least_loaded").replace("{motivo}", &exclusion(&reason["local_out"])),
        "no_candidate" => with_machine("placement_reason_no_candidate"),
        "not_balanced" => {
            let why = match reason["why"].as_str().unwrap_or_default() {
                "guest" => tr("placement_not_balanced_guest"),
                "prewarm" => tr("placement_not_balanced_prewarm"),
                "local_field" => tr("placement_not_balanced_local_field"),
                "link" => tr("placement_not_balanced_link"),
                "orq" => tr("placement_not_balanced_orq"),
                "mode_off" => tr("placement_not_balanced_mode_off"),
                other => other.to_owned(),
            };
            tr("placement_reason_not_balanced").replace("{motivo}", &why)
        }
        other => other.to_owned(),
    };
    for warning in placement["warnings"].as_array().into_iter().flatten() {
        let server = str_of(&warning["server"]);
        text.push_str("; ");
        text.push_str(&match warning["code"].as_str().unwrap_or_default() {
            "rule_server_out" => tr("placement_warning_rule_server_out").replace("{maquina}", &server)
                .replace("{motivo}", &exclusion(&warning["why"])),
            "rule_overridden" => tr("placement_warning_rule_overridden").replace("{maquina}", &server),
            _ => tr("placement_warning_all_over_limit"),
        });
    }
    if let Some(dir) = placement.pointer("/clone/dir").and_then(Value::as_str) {
        text.push_str("; ");
        text.push_str(&with_machine("placement_clone_note").replace("{dir}", dir));
    }
    text
}

/// A máquina da lista (`ServerChoice::key`) que a Nova conversa marca e o motivo, perguntados ao servidor ativo.
/// `Ok(None)`: nada a marcar, ou a sugerida não está na lista deste aparelho.
pub(super) async fn suggestion(active: Api, machines: Vec<ServerChoice>) -> Result<Option<(String, String)>, Failure> {
    let body = json!({"cwd": "~", "machine": "auto"});
    let (config, placement) = tokio::join!(
        active.server_read(&["placement", "config"], &[], WAIT),
        // A escolha espera até 2 s pelos fatos de cada máquina.
        active.server_send(reqwest::Method::POST, &["placement", "suggest"], Some(body), WAIT * 2));
    let Some((server, text)) = suggested(&config?, &placement?) else { return Ok(None) };
    let here = super::servers::norm(&active.identity());
    // Quase sempre a sugerida é a ativa: uma pergunta só.
    if ident(&active).await.as_deref() == Some(server.as_str()) { return Ok(Some((here, text))); }
    let others = machines.into_iter().filter(|m| m.key != here && !m.offline).collect::<Vec<_>>();
    let idents = futures::future::join_all(others.iter().map(|m| async {
        match Api::new(&m.address, &m.token) { Ok(api) => ident(&api).await, Err(_) => None }
    })).await;
    Ok(others.into_iter().zip(idents).find(|(_, id)| id.as_deref() == Some(server.as_str())).map(|(m, _)| (m.key, text)))
}

#[cfg(test)]
mod tests {
    use super::{reason_text, suggested};
    use serde_json::json;

    #[test]
    fn only_the_suggest_mode_and_a_balanced_request_mark_a_machine() {
        let suggest = json!({"ui_mode": "suggest"});
        let other = json!({"server": "nb", "reason": {"code": "least_loaded", "local_out": "working"}, "text": "nb tem menos"});
        assert_eq!(suggested(&suggest, &other).map(|(server, _)| server).as_deref(), Some("nb"));
        assert_eq!(suggested(&json!({"ui_mode": "off"}), &other), None);
        assert_eq!(suggested(&json!({"ui_mode": "ask"}), &other), None, "perguntar ainda não tem desenho na tela");
        let kept = json!({"server": "pc", "reason": {"code": "not_balanced", "why": "mode_off"}, "text": "fora"});
        assert_eq!(suggested(&suggest, &kept), None);
        assert_eq!(suggested(&suggest, &json!({"server": "", "reason": {"code": "local_within_limits"}})), None);
        assert_eq!(suggested(&suggest, &json!({"server": "pc"})), None, "resposta sem motivo não marca");
    }

    #[test]
    fn the_reason_comes_from_the_codes_in_the_app_language() {
        crate::i18n::english_in_this_thread(true);
        let placement = json!({"server": "nb", "reason": {"code": "least_loaded", "local_out": "working"}, "text": "texto do servidor",
            "warnings": [{"code": "rule_server_out", "server": "vm", "why": "missing"}], "clone": {"dir": "/h/Projetos"}});
        assert_eq!(reason_text(&placement), "the connected machine is out (too many sessions working); nb has fewer sessions working; \
            the rule's machine, vm, is out (off or not answering); nb doesn't have the repository: clone into /h/Projetos");
        let rule = json!({"server": "pc", "reason": {"code": "project_rule", "project": "git:github.com/a/b"}, "warnings": []});
        assert_eq!(reason_text(&rule), "the rule for github.com/a/b sends it to pc");
        let kept = json!({"server": "pc", "reason": {"code": "not_balanced", "why": "outro_codigo"}});
        assert_eq!(reason_text(&kept), "not balanced (outro_codigo)", "código sem texto aparece cru, não some");
    }
}

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
    Some((server.to_owned(), placement["text"].as_str().unwrap_or_default().to_owned()))
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
    use super::suggested;
    use serde_json::json;

    #[test]
    fn only_the_suggest_mode_and_a_balanced_request_mark_a_machine() {
        let suggest = json!({"ui_mode": "suggest"});
        let other = json!({"server": "nb", "reason": {"code": "least_loaded", "local_out": "working"}, "text": "nb tem menos"});
        assert_eq!(suggested(&suggest, &other), Some(("nb".into(), "nb tem menos".into())));
        assert_eq!(suggested(&json!({"ui_mode": "off"}), &other), None);
        assert_eq!(suggested(&json!({"ui_mode": "ask"}), &other), None, "perguntar ainda não tem desenho na tela");
        let kept = json!({"server": "pc", "reason": {"code": "not_balanced", "why": "mode_off"}, "text": "fora"});
        assert_eq!(suggested(&suggest, &kept), None);
        assert_eq!(suggested(&suggest, &json!({"server": "", "reason": {"code": "local_within_limits"}})), None);
        assert_eq!(suggested(&suggest, &json!({"server": "pc"})), None, "resposta sem motivo não marca");
    }
}

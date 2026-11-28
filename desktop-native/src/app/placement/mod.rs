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
pub(crate) fn reason_text(placement: &Value) -> String {
    let str_of = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let machine = str_of(&placement["server"]);
    let exclusion = |why: &Value| match why.as_str().unwrap_or_default() {
        "missing" => tr("placement_exclusion_missing"),
        "not_capable" => tr("placement_exclusion_not_capable"),
        "schedule" => tr("placement_exclusion_schedule"),
        "memory" => tr("placement_exclusion_memory"),
        "working" => tr("placement_exclusion_working"),
        "open" => tr("placement_exclusion_open"),
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

/// A escolha para a pasta escolhida aponta outra máquina: pela regra do projeto (`project`), a Nova conversa oferece abrir
/// lá; pela carga (`project` vazio), já marca lá. `server`: o id que a máquina informa (vai no `machine` do pedido);
/// `key`: a mesma máquina na lista deste aparelho; `cwd`: a pasta para a qual a oferta vale; `reason`: o motivo da escolha;
/// `taken`: a máquina da oferta é a que vai abrir.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RuleOffer {
    pub(super) server: String,
    pub(super) key: String,
    pub(super) project: Option<String>,
    pub(super) reason: String,
    pub(super) cwd: String,
    pub(super) taken: bool,
}

impl RuleOffer {
    pub(super) fn notice(&self, machine: &str) -> String {
        match &self.project {
            Some(project) => tr("placement_rule_offer").replace("{projeto}", project).replace("{maquina}", machine),
            None => tr("new_chat_machine_suggested").replace("{motivo}", &self.reason),
        }
    }

    pub(super) fn action(&self, machine: &str) -> String { tr("placement_rule_open").replace("{maquina}", machine) }
}

/// Conta, motor, perfil e reserva são desta máquina: com eles o servidor recusa abrir noutra. Lá vale o padrão de lá.
const LOCAL_FIELDS: [&str; 8] = ["config_dir", "force_account", "engine", "engine_account", "engine_binding", "codex_account", "omp_profile", "prewarm"];

/// O corpo de abertura que o servidor daqui encaminha à máquina da regra (`server`): ele troca a pasta pela do mesmo projeto lá.
pub(super) fn elsewhere(body: &mut Value, server: &str) {
    let Some(fields) = body.as_object_mut() else { return };
    for key in LOCAL_FIELDS { fields.remove(key); }
    fields.insert("machine".into(), json!(server));
}

/// A máquina da escolha e, pela regra de projeto, o projeto; só regra e carga apontam outra máquina para a pasta.
fn offer_of(placement: &Value) -> Option<(String, Option<String>)> {
    let server = placement["server"].as_str().filter(|s| !s.is_empty())?;
    match placement.pointer("/reason/code")?.as_str()? {
        "project_rule" => {
            let project = placement.pointer("/reason/project")?.as_str()?;
            Some((server.to_owned(), Some(project.trim_start_matches("git:").to_owned())))
        }
        "least_loaded" => Some((server.to_owned(), None)),
        _ => None,
    }
}

/// A oferta para `cwd`, perguntada à máquina marcada; com a tela em "desligado" nada é pedido. A da carga já vem marcada
/// (`taken`); a da regra espera o clique. `Ok(None)`: a escolha fica na própria marcada, ou a máquina está fora da lista
/// deste aparelho.
pub(super) async fn rule_offer(marked: Api, machines: Vec<ServerChoice>, cwd: String) -> Result<Option<RuleOffer>, Failure> {
    let config = marked.server_read(&["placement", "config"], &[], WAIT).await?;
    if config["ui_mode"].as_str().is_none_or(|mode| mode == "off") { return Ok(None); }
    let body = json!({"cwd": cwd, "machine": "auto"});
    let placement = marked.server_send(reqwest::Method::POST, &["placement", "suggest"], Some(body), WAIT * 2).await?;
    let Some((server, project)) = offer_of(&placement) else { return Ok(None) };
    let here = super::servers::norm(&marked.identity());
    let (reason, taken) = (reason_text(&placement), project.is_none());
    Ok(key_of(&marked, machines, &server).await.filter(|key| *key != here)
        .map(|key| RuleOffer { server, key, project, reason, cwd, taken }))
}

/// A máquina da lista (`ServerChoice::key`) cujo id é `server`; quase sempre é a própria `active`, uma pergunta só.
async fn key_of(active: &Api, machines: Vec<ServerChoice>, server: &str) -> Option<String> {
    let here = super::servers::norm(&active.identity());
    if ident(active).await.as_deref() == Some(server) { return Some(here); }
    let others = machines.into_iter().filter(|m| m.key != here && !m.offline).collect::<Vec<_>>();
    let idents = futures::future::join_all(others.iter().map(|m| async {
        match Api::new(&m.address, &m.token) { Ok(api) => ident(&api).await, Err(_) => None }
    })).await;
    others.into_iter().zip(idents).find(|(_, id)| id.as_deref() == Some(server)).map(|(m, _)| m.key)
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
    Ok(key_of(&active, machines, &server).await.map(|key| (key, text)))
}

#[cfg(test)]
mod tests {
    use super::{RuleOffer, elsewhere, offer_of, reason_text, suggested};
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
    fn a_project_rule_and_the_load_point_the_folder_to_another_machine() {
        let rule = json!({"server": "vm", "reason": {"code": "project_rule", "project": "git:github.com/a/b"}, "warnings": []});
        assert_eq!(offer_of(&rule), Some(("vm".into(), Some("github.com/a/b".into()))));
        let loaded = json!({"server": "vm", "reason": {"code": "least_loaded", "local_out": "open"}});
        assert_eq!(offer_of(&loaded), Some(("vm".into(), None)));
        for code in ["same_folder", "unpublished", "plain_dir", "local_within_limits", "no_candidate", "not_balanced"] {
            assert_eq!(offer_of(&json!({"server": "vm", "reason": {"code": code}})), None, "{code} fica na marcada");
        }
        assert_eq!(offer_of(&json!({"server": "", "reason": {"code": "least_loaded", "local_out": "open"}})), None);
        assert_eq!(offer_of(&json!({"server": "vm", "reason": {"code": "project_rule"}})), None, "regra sem projeto não tem o que dizer");
    }

    #[test]
    fn the_body_sent_elsewhere_has_the_machine_and_nothing_of_this_one() {
        let mut body = json!({"name": "s", "cwd": "/home/j/hangar", "provider": "claude", "model": "opus", "config_dir": "/home/j/.claude-x",
            "force_account": true, "engine": null, "engine_account": "a", "engine_binding": {"id": 1}, "codex_account": "", "omp_profile": "p",
            "prewarm": "r1", "headless": true, "branch": "feat"});
        elsewhere(&mut body, "vm");
        assert_eq!(body, json!({"name": "s", "cwd": "/home/j/hangar", "provider": "claude", "model": "opus", "headless": true, "branch": "feat",
            "machine": "vm"}));
    }

    #[test]
    fn the_offer_names_the_project_and_the_machine_in_the_app_language() {
        crate::i18n::english_in_this_thread(true);
        let offer = RuleOffer { server: "vm".into(), key: "k".into(), project: Some("github.com/a/b".into()), reason: String::new(),
            cwd: "/x".into(), taken: false };
        assert_eq!(offer.notice("VM"), "github.com/a/b has a rule for VM");
        assert_eq!(offer.action("VM"), "Open on VM");
        let load = RuleOffer { project: None, reason: "too many sessions open".into(), taken: true, ..offer };
        assert!(load.notice("VM").contains("too many sessions open"), "{}", load.notice("VM"));
    }

    #[test]
    fn the_reason_comes_from_the_codes_in_the_app_language() {
        crate::i18n::english_in_this_thread(true);
        let placement = json!({"server": "nb", "reason": {"code": "least_loaded", "local_out": "working"}, "text": "texto do servidor",
            "warnings": [{"code": "rule_server_out", "server": "vm", "why": "missing"}], "clone": {"dir": "/h/Projetos"}});
        assert_eq!(reason_text(&placement), "the connected machine is out (too many sessions working); nb has fewer sessions open; \
            the rule's machine, vm, is out (off or not answering); nb doesn't have the repository: clone into /h/Projetos");
        let rule = json!({"server": "pc", "reason": {"code": "project_rule", "project": "git:github.com/a/b"}, "warnings": []});
        assert_eq!(reason_text(&rule), "the rule for github.com/a/b sends it to pc");
        let kept = json!({"server": "pc", "reason": {"code": "not_balanced", "why": "outro_codigo"}});
        assert_eq!(reason_text(&kept), "not balanced (outro_codigo)", "código sem texto aparece cru, não some");
    }
}

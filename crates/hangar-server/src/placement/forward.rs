//! Abrir noutra máquina pela API de lá. O pedido vai pronto: com a máquina fixada (lá não se escolhe de
//! novo, nem volta para cá), sem criadora (lá ela não existe) e com a pasta de lá.
use std::time::Duration;

use reqwest::Method;
use serde_json::{Value, json};
use tokio::time::Instant;

use hangar_placement::{MachineFacts, OpenContext};

use crate::groups::peers::{OPEN_TIMEOUT, PeerClient, PeerError};
use crate::session_open::{OpenError, SessionOpenRequest};
use crate::session_write::detail_body;

/// A parte do prazo da abertura que a busca no remoto pode gastar: o resto é da criação lá.
const FETCH_SHARE: Duration = Duration::from_secs(60);

/// O que viria da criadora, lido nesta máquina: lá nada herda.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inherited {
    pub provider: Option<String>,
    pub permission_mode: Option<String>,
    pub headless: Option<bool>,
}

/// O corpo do `POST /api/sessions` em `dest`. A pasta vira a raiz do mesmo projeto lá, na branch da
/// daqui; sem projeto, a home de lá. A branch nova parte da base pedida ou da branch daqui. `None`: `dest`
/// não tem o projeto (precisa de clone), ou a pasta é um repositório sem projeto, que só existe aqui.
pub fn body(request: &SessionOpenRequest, ctx: &OpenContext, dest: &MachineFacts, inherited: &Inherited) -> Option<Value> {
    let mut out = request.clone();
    out.cwd = match &ctx.cwd_project {
        Some(project) => {
            let root = dest.repos.iter().find(|r| &r.project == project)?.root.clone();
            if out.new_branch { out.base = out.base.or_else(|| ctx.branch.clone()); }
            out.branch = out.branch.or_else(|| ctx.branch.clone());
            root
        }
        None if ctx.cwd_root.is_some() => return None,
        // Pasta sem git (só na máquina pedida explicitamente): pode ser a de lá, vai como veio.
        None if ctx.plain_dir_not_home => out.cwd,
        None => "~".into(),
    };
    out.machine = Some(dest.server_id.clone());
    out.placed = true;
    // A ligação é feita daqui, onde a criadora existe; lá a filha nasce solta.
    out.link = false;
    out.creator = None;
    out.inherit = None;
    let fill = |asked: &mut Option<String>, from: &Option<String>| { if asked.is_none() { asked.clone_from(from); } };
    fill(&mut out.provider, &inherited.provider);
    fill(&mut out.permission_mode, &inherited.permission_mode);
    out.headless = out.headless.or(inherited.headless);
    serde_json::to_value(&out).ok()
}

/// `POST /api/sessions` lá. `fetch`: antes, busca no remoto da raiz de lá, para a branch daqui existir
/// lá; a busca que falha só vira aviso, e a abertura diz se a branch faltou. Busca e abertura dividem um prazo
/// só (`OPEN_TIMEOUT`), e a busca nunca leva a parte da abertura.
pub async fn open(peers: &PeerClient, server: &str, body: &Value, fetch: bool) -> Result<Value, PeerError> {
    let deadline = Instant::now() + OPEN_TIMEOUT;
    let root = body["cwd"].as_str().filter(|cwd| fetch && *cwd != "~" && body.get("branch").is_some());
    let mut warning = None;
    if let Some(root) = root {
        let fetched = tokio::time::timeout(FETCH_SHARE,
            peers.call_opening(server, Method::POST, "/api/worktrees/fetch", Some(&json!({"repo": root})))).await;
        let failed = match fetched {
            Ok(Ok(_)) => None,
            Ok(Err(error)) => Some(error.text(server)),
            Err(_) => Some(format!("passou de {} s", FETCH_SHARE.as_secs())),
        };
        warning = failed.map(|why| format!("a busca no remoto em {server} falhou ({why}); a branch pode estar desatualizada lá"));
    }
    let opened = tokio::time::timeout_at(deadline, peers.call_opening(server, Method::POST, "/api/sessions", Some(body))).await
        .map_err(|_| PeerError::Transport(format!("{server} não terminou a abertura em {} s", OPEN_TIMEOUT.as_secs())))?;
    let mut info = opened?.unwrap_or(Value::Null);
    add_avisos(&mut info, warning.into_iter().collect());
    Ok(info)
}

/// Junta `items` aos `avisos` da resposta da abertura; sem itens, não cria a lista.
pub(crate) fn add_avisos(info: &mut Value, items: Vec<String>) {
    if items.is_empty() { return }
    let list = info.as_object_mut().and_then(|fields| fields.entry("avisos").or_insert_with(|| json!([])).as_array_mut());
    if let Some(list) = list { list.extend(items.into_iter().map(Value::from)); }
}

pub(crate) fn machine_unknown(server: &str) -> OpenError {
    OpenError::new(400, "placement_machine_unknown", format!("a máquina '{server}' não está no peers.json (ou sem base_url/token)"))
        .with_params(json!({"server": server}))
}

/// Lá respondeu: nada abriu, ou não está no `peers.json`. O resto (rede, prazo, corpo ilegível) é incerto.
fn certain(error: &PeerError) -> bool { matches!(error, PeerError::Refused { .. } | PeerError::Unknown) }

fn uncertain_text(server: &str) -> String {
    format!("{server} não respondeu à abertura: a sessão pode ter nascido lá. Confira em {server} (`hangar-send --list`) antes de tentar de novo")
}

/// Status e corpo da resposta de quem pediu, quando lá não abriu ou não se sabe; nunca abre aqui no lugar.
pub fn failure(server: &str, error: &PeerError) -> (u16, Value) {
    match error {
        PeerError::Refused { status, detail } => (*status, json!({"detail": detail})),
        PeerError::Unknown => (400, machine_unknown(server).body()),
        other => (504, detail_body("placement_uncertain", &uncertain_text(server), json!({"server": server, "cause": other.text(server)}))),
    }
}

/// O erro para o MCP, com o código na frente quando lá pode ter aberto.
pub fn failure_text(server: &str, error: &PeerError) -> String {
    if certain(error) { error.text(server) } else { format!("placement_uncertain: {} — {}", uncertain_text(server), error.text(server)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_core::project::ProjectId;
    use hangar_placement::KnownRepo;

    fn hangar() -> ProjectId { ProjectId("git:github.com/jeffer1312/hangar".into()) }

    fn nb() -> MachineFacts {
        MachineFacts { server_id: "nb".into(), version: "1".into(), capable: true, mem_total: 0, mem_available: 0, working: 0,
            cpu_pct: None, repos: vec![KnownRepo { project: hangar(), root: "/home/j/Projetos/hangar".into() }], clone_dir: None }
    }

    fn request(cwd: &str) -> SessionOpenRequest {
        SessionOpenRequest { name: Some("filha".into()), cwd: cwd.into(), machine: Some("auto".into()),
            creator: Some("pai".into()), inherit: Some(true), link: true, ..Default::default() }
    }

    #[test]
    fn the_folder_becomes_the_same_project_there_on_the_branch_from_here() {
        let ctx = OpenContext { local_id: "pc".into(), cwd_project: Some(hangar()), branch: Some("feat".into()), ..Default::default() };
        let body = body(&request("/home/j/pessoal/hangar-wt"), &ctx, &nb(), &Inherited::default()).unwrap();
        assert_eq!((&body["cwd"], &body["branch"]), (&json!("/home/j/Projetos/hangar"), &json!("feat")));
        // A máquina fixada impede escolher de novo lá; a criadora e o `inherit` não existem lá.
        assert_eq!((&body["machine"], &body["placed"]), (&json!("nb"), &json!(true)));
        assert!(body.get("creator").is_none() && body.get("inherit").is_none(), "{body}");
        // A ligação é feita daqui: lá, sem criadora, ela viraria `erro_link_sem_criador`.
        assert!(body.get("link").is_none(), "{body}");
    }

    #[test]
    fn a_requested_branch_stays_and_without_project_it_is_the_home_there() {
        let ctx = OpenContext { local_id: "pc".into(), cwd_project: Some(hangar()), branch: Some("feat".into()), ..Default::default() };
        let asked = SessionOpenRequest { branch: Some("nova".into()), new_branch: true, ..request("/x") };
        let sent = body(&asked, &ctx, &nb(), &Inherited::default()).unwrap();
        // A branch nova lá parte da daqui; a base pedida vence.
        assert_eq!((&sent["branch"], &sent["base"]), (&json!("nova"), &json!("feat")));
        let based = SessionOpenRequest { base: Some("main".into()), ..asked.clone() };
        assert_eq!(body(&based, &ctx, &nb(), &Inherited::default()).unwrap()["base"], "main");
        // Branch que já existe não leva base.
        assert!(body(&request("/x"), &ctx, &nb(), &Inherited::default()).unwrap().get("base").is_none());
        let home = OpenContext { local_id: "pc".into(), ..Default::default() };
        let body = body(&request("/home/j"), &home, &nb(), &Inherited::default()).unwrap();
        assert_eq!(body["cwd"], "~");
        assert!(body.get("branch").is_none());
    }

    #[test]
    fn a_repo_without_project_never_becomes_the_home_there() {
        let ctx = OpenContext { local_id: "pc".into(), cwd_root: Some("/home/j/solo".into()), plain_dir_not_home: true, ..Default::default() };
        assert_eq!(body(&request("/home/j/solo"), &ctx, &nb(), &Inherited::default()), None);
    }

    #[test]
    fn a_project_the_destination_lacks_has_no_body() {
        let ctx = OpenContext { local_id: "pc".into(), cwd_project: Some(ProjectId("git:github.com/a/b".into())), ..Default::default() };
        assert_eq!(body(&request("/x"), &ctx, &nb(), &Inherited::default()), None);
    }

    #[test]
    fn what_comes_from_the_creator_goes_explicit_and_the_asked_value_wins() {
        let inherited = Inherited { provider: Some("claude".into()), permission_mode: Some("bypassPermissions".into()), headless: Some(true) };
        let asked = SessionOpenRequest { permission_mode: Some("acceptEdits".into()), headless: Some(false), ..request("/home/j") };
        let home = OpenContext { local_id: "pc".into(), ..Default::default() };
        let body = body(&asked, &home, &nb(), &inherited).unwrap();
        assert_eq!((&body["provider"], &body["permission_mode"], &body["headless"]), (&json!("claude"), &json!("acceptEdits"), &json!(false)));
        let plain = super::body(&request("/home/j"), &home, &nb(), &inherited).unwrap();
        assert_eq!((&plain["permission_mode"], &plain["headless"]), (&json!("bypassPermissions"), &json!(true)));
    }

    #[test]
    fn no_answer_is_uncertain_and_never_a_clean_failure() {
        let (status, body) = failure("nb", &PeerError::Transport("prazo".into()));
        assert_eq!((status, body["detail"]["code"].as_str()), (504, Some("placement_uncertain")));
        assert!(body["detail"]["msg"].as_str().unwrap().contains("Confira em nb"), "{body}");
        assert!(failure_text("nb", &PeerError::Transport("prazo".into())).starts_with("placement_uncertain:"));
        // Lá recusou: o erro de lá passa como veio.
        let refused = PeerError::Refused { status: 409, detail: json!({"code": "erro_x", "msg": "não"}) };
        assert_eq!(failure("nb", &refused), (409, json!({"detail": {"code": "erro_x", "msg": "não"}})));
        assert!(!failure_text("nb", &refused).starts_with("placement_uncertain"));
    }
}

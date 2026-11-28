//! `GET /api/placement/facts`: o que esta máquina responde a quem escolhe onde abrir.
use std::{collections::BTreeMap, net::SocketAddr, path::Path, sync::Arc, time::Duration};

use axum::{extract::{ConnectInfo, Request, State}, http::StatusCode, response::Response};
use hangar_core::project::ProjectId;
use hangar_placement::{KnownRepo, MachineFacts};
use hangar_workspace::worktrees;
use serde_json::json;

use crate::{routes::{AppState, owner_gate}, session_write::{detail_body, json_response}};

/// Quem pergunta espera 2 s pela resposta inteira: a lista publicada tem só parte disso.
const LIST_WAIT: Duration = Duration::from_millis(1500);

pub async fn facts(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if let Err(response) = owner_gate(&state, peer, &request).await { return response; }
    let project = form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
        .find(|(key, _)| key == "project")
        .map(|(_, value)| ProjectId(value.into_owned()));
    match local(&state, project.as_ref(), true).await {
        Ok(facts) => json_response(StatusCode::OK, json!(facts)),
        Err(code) => {
            state.diag.report("rust.placement_facts_failed", "", code, "fatos do balanceamento indisponíveis");
            json_response(StatusCode::SERVICE_UNAVAILABLE,
                detail_body(code, "não foi possível ler os fatos desta máquina", json!({})))
        }
    }
}

/// Os fatos desta máquina; `project` deixa em `repos` só as raízes desse projeto. Sem `repos` (a escolha feita
/// aqui, que não olha os repositórios daqui) pula a varredura: `repos` e `clone_dir` vêm vazios.
pub(crate) async fn local(st: &Arc<AppState>, project: Option<&ProjectId>, with_repos: bool) -> Result<MachineFacts, &'static str> {
    // Assina só pela leitura: com lista aberta a resposta é a já publicada, sem rodada nova.
    let sub = st.hub.subscribe_internal(&st.list, &st.devices, &st.diag);
    let rows = sub.wait_rows(LIST_WAIT);
    let scan = {
        let st = st.clone();
        tokio::task::spawn_blocking(move || if with_repos { scan(&st) } else { Ok((Vec::new(), None, hangar_status::host::placement_load())) })
    };
    let (rows, scan) = tokio::join!(rows, scan);
    let rows = rows.ok_or("placement_facts_list_unavailable")?;
    let (mut roots, clone_dir, load) = scan.map_err(|_| "placement_facts_unavailable")??;
    let working = u32::try_from(rows.iter().filter(|row| row.state == "working").count()).unwrap_or(u32::MAX);
    // Sessão morta continua na lista até ser dispensada, mas não ocupa a máquina.
    let open = u32::try_from(rows.iter().filter(|row| row.state != "dead").count()).unwrap_or(u32::MAX);
    let folders = rows.iter().filter_map(|row| row.git_cwd.clone().filter(|c| !c.is_empty()).or_else(|| row.cwd.clone())).collect::<Vec<_>>();
    let project = project.cloned();
    let repos = if !with_repos { Vec::new() } else { tokio::task::spawn_blocking(move || {
        roots.extend(worktrees::main_repos(folders.iter().map(String::as_str)));
        roots.sort();
        roots.dedup();
        // Um `git config` por raiz, em paralelo (pool do rayon): em série, centenas de raízes passariam dos 2 s de quem pergunta.
        use rayon::prelude::*;
        roots.into_par_iter()
            .filter_map(|root| Some(KnownRepo { project: ProjectId::from_origin(&worktrees::origin_url(&root)?)?, root }))
            .filter(|repo| project.as_ref().is_none_or(|p| &repo.project == p))
            .collect::<Vec<_>>()
    }).await.map_err(|_| "placement_facts_unavailable")? };
    let version = crate::update::running_package().map_or_else(|| env!("CARGO_PKG_VERSION").to_owned(), |p| p.version.clone());
    Ok(MachineFacts {
        server_id: st.current_server_id(), version, capable: true,
        mem_total: load.mem_total, mem_available: load.mem_available, working, open: Some(open), cpu_pct: load.cpu_pct,
        repos, clone_dir,
    })
}

type Scan = (Vec<String>, Option<String>, hangar_status::host::PlacementLoad);

/// Raízes das pastas recentes e da varredura, a pasta de clone com um nível dela, e a carga.
fn scan(st: &AppState) -> Result<Scan, &'static str> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| "placement_facts_clock")?.as_secs_f64();
    let bases = crate::worktree_context::project_bases(st).map_err(|_| "placement_facts_config")?;
    let scan_roots = crate::worktree_context::roots(st).map_err(|_| "placement_facts_config")?;
    let recent = crate::archive_list::worktree_activities(st, &bases, now).into_iter()
        .filter(|(_, mtime)| *mtime >= now - crate::worktree_context::RECENT_SECS).map(|(cwd, _)| cwd).collect::<Vec<_>>();
    let mut roots = worktrees::main_repos(recent.iter().map(String::as_str));
    let discovery = worktrees::discover_repositories(&scan_roots);
    if discovery.partial {
        // A varredura parou no teto de pastas ou de tempo: `repos` pode faltar um repositório que esta máquina tem.
        st.diag.report("rust.placement_facts_partial", "", "placement_facts_scan_partial", "varredura de repositórios incompleta");
    }
    roots.extend(discovery.repos);
    let configured = match hangar_placement::store::load(&st.accounts.env.home) {
        Ok(config) => config.clone_dirs.get(&st.current_server_id()).cloned(),
        Err(_) => {
            // Regras ilegíveis aparecem na tela delas; aqui só perde a pasta escolhida.
            st.diag.report("rust.placement_facts_failed", "", "placement_config_unreadable", "regras do balanceamento ilegíveis");
            None
        }
    };
    let clone_dir = configured.or_else(|| common_parent(&roots));
    if let Some(dir) = &clone_dir {
        // A pasta de clone escrita na tela pode começar por `~`.
        let children = std::fs::read_dir(crate::worktree_context::expanded(dir, &st.accounts.env)).into_iter().flatten().flatten()
            .map(|entry| entry.path()).filter(|path| path.join(".git").exists())
            .map(|path| path.to_string_lossy().into_owned()).collect::<Vec<_>>();
        roots.extend(worktrees::main_repos(children.iter().map(String::as_str)));
    }
    Ok((roots, clone_dir, hangar_status::host::placement_load()))
}

/// A pasta-mãe da maioria das raízes; empate fica com a primeira em ordem alfabética.
fn common_parent(roots: &[String]) -> Option<String> {
    let mut count = BTreeMap::<&Path, usize>::new();
    for parent in roots.iter().filter_map(|root| Path::new(root).parent()) {
        *count.entry(parent).or_default() += 1;
    }
    let best = count.values().copied().max()?;
    count.into_iter().find(|(_, n)| *n == best).map(|(parent, _)| parent.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_dir_is_where_most_repos_live() {
        let roots = ["/h/Projetos/a", "/h/Projetos/b", "/h/pessoal/c"].map(String::from);
        assert_eq!(common_parent(&roots).as_deref(), Some("/h/Projetos"));
    }

    #[test]
    fn clone_dir_tie_is_alphabetical_and_empty_is_none() {
        let roots = ["/h/z/a", "/h/b/c"].map(String::from);
        assert_eq!(common_parent(&roots).as_deref(), Some("/h/b"));
        assert_eq!(common_parent(&[]), None);
    }
}

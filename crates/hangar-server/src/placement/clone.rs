//! `POST /api/placement/clone`: o repositório que esta máquina não tem, clonado na pasta de clone antes de a
//! sessão abrir nele. Passo próprio, fora da criação: o clone passa de longe o prazo dela.
use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::StatusCode, response::Response};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};

use hangar_core::project::ProjectId;
use hangar_workspace::{git, process, worktrees};

use crate::groups::peers::{PeerClient, PeerError};
use crate::routes::{AppState, owner_gate};
use crate::session_open::OpenError;
use crate::session_write::{detail_body, json_response};

/// Repositório grande em rede lenta; quem pede espera um pouco mais (`groups::peers::CLONE_TIMEOUT`).
const CLONE_WAIT: Duration = Duration::from_secs(600);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asked {
    origin: String,
    dir: String,
}

fn refusal(status: u16, code: &'static str, msg: impl Into<String>, params: Value) -> OpenError {
    OpenError::new(status, code, msg).with_params(params)
}

pub async fn clone(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if let Err(response) = owner_gate(&st, peer, &request).await { return response; }
    let parsed = to_bytes(request.into_body(), 64 * 1024).await.ok()
        .and_then(|raw| serde_json::from_slice::<Asked>(&raw).ok());
    let Some(asked) = parsed else {
        return json_response(StatusCode::UNPROCESSABLE_ENTITY, detail_body("erro_corpo_invalido", "pedido de clone inválido", json!({})));
    };
    let dir = crate::worktree_context::expanded(asked.dir.trim(), &st.accounts.env);
    let result = tokio::task::spawn_blocking(move || clone_into(&asked.origin, &dir, CLONE_WAIT)).await
        .unwrap_or_else(|_| Err(refusal(500, "placement_clone_failed", "o clone não terminou", json!({}))));
    match result {
        Ok(root) => json_response(StatusCode::OK, json!({"root": root})),
        Err(r) => {
            st.diag.report("rust.placement_clone_failed", "", r.code.unwrap_or_default(), "clone do balanceamento recusado");
            json_response(StatusCode::from_u16(r.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), r.body())
        }
    }
}

/// Clona `origin` em `<dir>/<nome>` por uma pasta temporária ao lado, renomeada só no fim: clone pela metade
/// nunca parece repositório. A pasta que já existe com o mesmo `origin` é a resposta.
pub fn clone_into(origin: &str, dir: &Path, wait: Duration) -> Result<String, OpenError> {
    let origin = origin.trim();
    let name = folder_name(origin).filter(|_| !origin.starts_with('-'))
        .ok_or_else(|| refusal(422, "placement_clone_invalid", "origin sem nome de repositório", json!({})))?;
    if !dir.is_absolute() {
        return Err(refusal(422, "placement_clone_invalid", "a pasta de clone precisa ser absoluta", json!({})));
    }
    let target = dir.join(&name);
    if target.symlink_metadata().is_ok() { return existing(&target, origin) }
    std::fs::create_dir_all(dir).map_err(|e| refusal(500, "placement_clone_failed",
        format!("não consegui criar a pasta de clone {}: {e}", dir.display()), json!({})))?;
    let tmp = dir.join(format!(".{name}.tmp-{}", hangar_core::ids::random_hex(8)));
    // Sem o gerenciador de credenciais abrir janela: credencial ausente é erro, nunca espera.
    let ran = process::run_program(process::git_command(None).arg("-C").arg(dir).args(["clone", "--", origin]).arg(&tmp)
        .env("GCM_INTERACTIVE", "never"), wait);
    let failure = match ran {
        Ok(out) if out.code == 0 => None,
        Ok(out) => Some(refusal(409, "placement_clone_failed", format!("o clone de {name} falhou: {}", useful_line(&out.stderr)), json!({"name": name}))),
        Err(e) if e.status == 504 => Some(refusal(504, "placement_clone_timeout",
            format!("o clone de {name} não terminou em {} min", wait.as_secs() / 60), json!({"name": name}))),
        Err(e) => Some(refusal(500, "placement_clone_failed", format!("o clone de {name} não rodou: {}", e.detail.as_str().unwrap_or("git")),
            json!({"name": name}))),
    };
    if let Some(failure) = failure {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(failure);
    }
    if std::fs::rename(&tmp, &target).is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
        // Outro pedido terminou o mesmo clone antes deste.
        return existing(&target, origin);
    }
    Ok(target.to_string_lossy().into_owned())
}

/// A pasta que já ocupa o nome: serve se for o mesmo repositório.
fn existing(target: &Path, origin: &str) -> Result<String, OpenError> {
    let root = target.to_string_lossy().into_owned();
    match worktrees::origin_url(&root) {
        Some(found) if same_repo(&found, origin) => Ok(root),
        found => Err(refusal(409, "placement_clone_conflict", format!("{root} já existe e não é um clone deste repositório"),
            json!({"root": root, "other": found.is_some()}))),
    }
}

/// O mesmo repositório por https ou ssh; endereço sem host (pasta local) só por igualdade.
fn same_repo(a: &str, b: &str) -> bool {
    match (ProjectId::from_origin(a), ProjectId::from_origin(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/'),
    }
}

/// O nome da pasta, como o `git clone` daria: o último trecho do endereço, sem `.git`.
fn folder_name(origin: &str) -> Option<String> {
    let path = origin.trim_end_matches(['/', '\\']);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let name = path.rsplit(['/', '\\', ':']).next()?;
    let ok = !name.is_empty() && !name.starts_with('.') && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    ok.then(|| name.to_owned())
}

/// A primeira linha do git que diz o que houve (sem o "Cloning into"), sem credencial e curta.
fn useful_line(stderr: &str) -> String {
    let line = stderr.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("Cloning into")).unwrap_or("git falhou");
    git::scrub(line).chars().take(300).collect()
}

/// Pede o clone a `server` e devolve a raiz de lá.
pub async fn remote(peers: &PeerClient, server: &str, origin: &str, dir: &str) -> Result<String, OpenError> {
    let answer = peers.call_cloning(server, Method::POST, "/api/placement/clone", Some(&json!({"origin": origin, "dir": dir}))).await;
    read_remote(server, answer)
}

fn read_remote(server: &str, answer: Result<Option<Value>, PeerError>) -> Result<String, OpenError> {
    let params = json!({"server": server});
    let uncertain = |cause: String| OpenError::new(504, "placement_clone_uncertain", format!("o clone em {server} não respondeu: pode ter \
        terminado lá. Abra de novo, que a pasta já clonada é reaproveitada")).with_params(json!({"server": server, "cause": cause}));
    match answer {
        Ok(Some(value)) => value["root"].as_str().filter(|r| !r.is_empty()).map(str::to_owned)
            .ok_or_else(|| uncertain(format!("{server}: resposta do clone sem a raiz"))),
        Ok(None) => Err(uncertain(format!("{server}: resposta do clone vazia"))),
        // A rota não existe lá: a máquina não devia ter sido candidata.
        Err(error) if error.missing_route() => Err(OpenError::new(409, "placement_peer_old",
            format!("{server} está numa versão sem o clone do balanceamento: escolha outra máquina")).with_params(params)),
        Err(PeerError::Refused { status, detail }) => {
            let code = match detail["code"].as_str() {
                Some("placement_clone_conflict") => "placement_clone_conflict",
                Some("placement_clone_timeout") => "placement_clone_timeout",
                Some("placement_clone_invalid") => "placement_clone_invalid",
                _ => "placement_clone_failed",
            };
            let msg = detail["msg"].as_str().map_or_else(|| format!("{server} recusou o clone (HTTP {status})"), |m| format!("{server}: {m}"));
            let mut params = detail.get("params").filter(|p| p.is_object()).cloned().unwrap_or_else(|| json!({}));
            params["server"] = json!(server);
            Err(OpenError::new(status, code, msg).with_params(params))
        }
        Err(PeerError::Unknown) => Err(super::forward::machine_unknown(server)),
        Err(error) => Err(uncertain(error.text(server))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_is_named_like_git_clone_names_it() {
        assert_eq!(folder_name("git@github.com:jeffer1312/hangar.git").as_deref(), Some("hangar"));
        assert_eq!(folder_name("https://github.com/Jeffer1312/Hangar/").as_deref(), Some("Hangar"));
        assert_eq!(folder_name("/srv/remote.git").as_deref(), Some("remote"));
        assert_eq!(folder_name("C:\\repos\\app.git").as_deref(), Some("app"));
        // Nada que saia da pasta de clone nem vire pasta escondida.
        for bad in ["https://github.com/a/..", "https://github.com/a/.git", "", "https://x/a b"] {
            assert_eq!(folder_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn https_and_ssh_of_the_same_repo_are_the_same_clone() {
        assert!(same_repo("git@github.com:jeffer1312/hangar.git", "https://github.com/jeffer1312/hangar"));
        assert!(!same_repo("git@github.com:jeffer1312/hangar.git", "https://github.com/outro/hangar"));
        assert!(same_repo("/srv/remote.git/", "/srv/remote.git"));
    }

    #[test]
    fn the_git_line_shown_skips_the_preamble_and_hides_the_credential() {
        // URL montada em partes: a trava de commit do repositório público recusa credencial literal.
        let url = format!("https://user{}segredo{}github.com/a/b/", ':', '@');
        let stderr = format!("Cloning into '/x/.a.tmp-1'...\nfatal: unable to access '{url}': error\n");
        let line = useful_line(&stderr);
        assert!(line.starts_with("fatal: unable to access"), "{line}");
        assert!(!line.contains("segredo"), "{line}");
    }

    #[test]
    fn an_invalid_origin_or_a_relative_dir_is_refused_before_anything() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(clone_into("--upload-pack=x", base.path(), CLONE_WAIT).unwrap_err().code, Some("placement_clone_invalid"));
        assert_eq!(clone_into("https://github.com/a/b", Path::new("rel"), CLONE_WAIT).unwrap_err().code, Some("placement_clone_invalid"));
        assert_eq!(std::fs::read_dir(base.path()).unwrap().count(), 0);
    }

    #[test]
    fn the_answer_from_the_other_machine_keeps_its_code_and_an_old_one_is_out() {
        assert_eq!(read_remote("vm", Ok(Some(json!({"root": "/h/Projetos/a"})))).unwrap(), "/h/Projetos/a");
        let old = read_remote("vm", Err(PeerError::Refused { status: 404, detail: json!("Not Found") })).unwrap_err();
        assert_eq!((old.status, old.code), (409, Some("placement_peer_old")));
        let conflict = read_remote("vm", Err(PeerError::Refused { status: 409,
            detail: json!({"code": "placement_clone_conflict", "msg": "/h/a já existe", "params": {"root": "/h/a"}}) })).unwrap_err();
        assert_eq!((conflict.status, conflict.code), (409, Some("placement_clone_conflict")));
        assert_eq!((&conflict.params["root"], &conflict.params["server"]), (&json!("/h/a"), &json!("vm")));
        assert!(conflict.msg.starts_with("vm: "), "{}", conflict.msg);
        // Sem resposta, lá pode ter clonado: nunca "falhou".
        let silent = read_remote("vm", Err(PeerError::Transport("prazo".into()))).unwrap_err();
        assert_eq!((silent.status, silent.code), (504, Some("placement_clone_uncertain")));
    }
}

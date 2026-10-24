//! `GET /api/atualizacao`, `POST /api/atualizacao/iniciar` e `POST /api/atualizacao/reiniciar`.
//! O Rust atende o dono só na máquina migrada: pacote (`current` com `manifest.json`) ou modo
//! desenvolvedor (`current` num checkout git sem manifesto). Máquina não migrada, convidado e quem
//! não é dono seguem ao motor `git` do Python, como antes.
//!
//! A resposta tem o formato do Python, campo por campo, que os clientes (PWA, nativo, Expo) leem.
//! No pacote os campos de git ficam neutros: `mudancas` e `passos` vazios, `sujo`/`ahead` 0,
//! `branch_de_trabalho` falso, `servidor_desatualizado` falso; `atras` é a distância no número da
//! versão (o último número é a contagem de commits).
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use hangar_package::{Manifest, PlatformEntry, Version};
use serde_json::{json, Map, Value};

use super::dev::Checkout;
use super::executor::{Args, StartError, Started, Task};
use super::restart::Topology;
use super::{Layout, Release};
use crate::routes::{cors, gate, pass_any, AppState};
use crate::session_write::{detail, json_response};

/// Como esta máquina roda o Hangar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// `current` → `versions/<v>` com `manifest.json` (caminho real).
    Package(PathBuf),
    /// `current` → checkout git sem manifesto (caminho real).
    Developer(PathBuf),
    /// Sem `current`: o motor `git` do Python.
    Unmigrated,
}

pub fn mode(home: &Path) -> Mode {
    let Ok(root) = std::fs::canonicalize(crate::install_root::current(home)).map(crate::list::links::without_verbatim) else { return Mode::Unmigrated };
    if super::is_package(&root) { return Mode::Package(root); }
    if root.join(".git").exists() { return Mode::Developer(root); }
    Mode::Unmigrated
}

/// O modo, quando o pedido é do dono numa máquina migrada; senão `None` (segue ao Python).
fn owned(st: &AppState, peer: SocketAddr, req: &Request) -> Option<Mode> {
    if !gate(st, peer, req).1 { return None; }
    Some(mode(&st.accounts.env.home)).filter(|m| *m != Mode::Unmigrated)
}

/// Só o que foi achado fica guardado: `manual` pode ser o `systemctl` falhando num instante.
fn keeps(topology: Topology) -> bool { topology != Topology::Manual }

/// Topologia achada fica para o processo (a tela pergunta a cada 2 s); `manual` é relida.
pub(crate) async fn topology() -> Topology {
    static FOUND: std::sync::OnceLock<Topology> = std::sync::OnceLock::new();
    if let Some(found) = FOUND.get() { return *found; }
    let read = tokio::task::spawn_blocking(|| {
        let runner = super::ShellRunner { timeout: Duration::from_secs(15) };
        Topology::detect(&runner, &std::env::temp_dir(), cfg!(windows))
    }).await.unwrap_or(Topology::Manual);
    if keeps(read) { let _ = FOUND.set(read); }
    read
}

/// Por quanto a entrada publicada vale sem `procurar` (o `_PUBLICADO_TTL_S` do Python).
const PUBLISHED_TTL: Duration = Duration::from_secs(300);

/// A entrada deste sistema no índice, guardada por índice; `procurar` relê. Falha vira `None` (e
/// também fica guardada: a tela não bate na rede a cada 2 s sem rede).
async fn published(release: &Release, refresh: bool) -> Option<PlatformEntry> {
    static CACHE: LazyLock<Mutex<std::collections::HashMap<String, (Instant, Option<PlatformEntry>)>>> = LazyLock::new(Default::default);
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| crate::http_client().build().unwrap_or_default());
    let url = release.index_url();
    if !refresh && let Some((at, entry)) = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&url)
        && at.elapsed() < PUBLISHED_TTL {
        return entry.clone();
    }
    let entry = super::entry(&CLIENT, release, &super::platform()).await
        .inspect_err(|e| tracing::warn!(code = e.code(), "update: índice da release ilegível: {e}")).ok();
    CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(url, (Instant::now(), entry.clone()));
    entry
}

/// `latest` é o canal da main; os outros têm o nome da branch.
fn target_branch(channel: &str) -> &str { if channel == "latest" { "main" } else { channel } }

/// Quantos commits a versão publicada está à frente: a diferença do último número quando o
/// `VERSION` é o mesmo; versão base nova conta 1; mesma ou mais velha, 0.
fn distance(installed: &str, remote: &str) -> u64 {
    let (Ok(remote), installed) = (remote.parse::<Version>(), installed.parse::<Version>().ok()) else { return 0 };
    match installed {
        Some(installed) if remote <= installed => 0,
        Some(installed) if remote.0[..3] == installed.0[..3] => remote.0[3] - installed.0[3],
        _ => 1,
    }
}

/// O `GET` no modo pacote.
pub fn package_body(installed: Option<&Manifest>, running: Option<&str>, remote: Option<&PlatformEntry>, topology: &str,
    estado: Map<String, Value>) -> Value {
    let disk = installed.map_or("", |m| m.version.as_str());
    let behind = remote.map_or(0, |r| distance(disk, &r.version));
    let branch = target_branch(installed.map_or("latest", |m| m.channel.as_str()));
    json!({
        "modo": "pacote",
        // Versão no ar desconhecida (servidor fora de pacote) é a do disco: vazia, o nativo
        // ofereceria reiniciar para sempre.
        "versoes": {"repo": disk, "backend": running.unwrap_or(disk)},
        "versao_legivel": {"repo": installed.map(|m| m.version.clone()), "backend": running.unwrap_or(disk), "remoto": remote.map(|r| r.version.clone())},
        "atras": behind,
        "atualizacao_disponivel": behind > 0,
        "servidor_desatualizado": false,
        "api_publicado": remote.map(|r| r.api).or(installed.map(|m| m.api)).unwrap_or(1),
        "mudancas": [],
        "passos": [],
        "pre_voo": {"pode": true, "faltando": [], "branch": branch, "alvo": branch, "sujo": 0, "ahead": 0, "ahead_incerto": false,
            "behind": behind, "divergiu": false, "branch_de_trabalho": false, "topologia": topology,
            "commit": installed.map_or("", |m| m.commit.as_str())},
        "estado": estado,
    })
}

/// O `GET` no modo desenvolvedor; `checkout` com erro é o `nao_e_repo` do Python.
pub fn dev_body(checkout: Result<&Checkout, &str>, boot: Option<&str>, topology: &str, estado: Map<String, Value>) -> Value {
    let pre = match checkout {
        Ok(c) => json!({"pode": true, "faltando": [], "branch": c.branch, "alvo": c.branch, "sujo": c.dirty, "ahead": c.ahead,
            "ahead_incerto": false, "behind": c.behind, "divergiu": c.ahead > 0 && c.behind > 0, "branch_de_trabalho": false,
            "topologia": topology, "commit": c.head}),
        Err(why) => json!({"pode": false, "faltando": [], "erro": "nao_e_repo", "detalhe": why, "topologia": topology}),
    };
    let c = checkout.ok();
    let head = c.map_or("", |c| c.head.as_str());
    json!({
        "modo": "desenvolvedor",
        "versoes": {"repo": head, "backend": boot.unwrap_or(head)},
        "versao_legivel": {"repo": c.map(|c| c.head.clone()), "backend": boot.unwrap_or(head), "remoto": c.and_then(|c| c.upstream_head.clone())},
        "atras": c.map_or(0, |c| c.behind),
        "atualizacao_disponivel": c.is_some_and(|c| c.behind > 0),
        "servidor_desatualizado": false,
        "api_publicado": hangar_api::CLIENT_API,
        "mudancas": c.map_or(vec![], |c| c.changes.iter().map(|(sha, title)| json!({"sha": sha, "titulo": title})).collect()),
        "passos": [],
        "pre_voo": pre,
        "estado": estado,
    })
}

fn with_cors(headers: &axum::http::HeaderMap, mut resp: Response) -> Response {
    cors(headers, resp.headers_mut());
    resp
}

/// `GET /api/atualizacao[?procurar=1]`.
pub async fn status(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let Some(mode) = owned(&st, peer, &req) else { return pass_any(State(st), ConnectInfo(peer), req).await };
    let refresh = req.uri().query().is_some_and(|q| q.split('&').any(|p| matches!(p, "procurar=1" | "procurar=true")));
    let (home, claude) = (st.accounts.env.home.clone(), st.accounts.env.claude_base.clone());
    let topology = topology().await.name();
    let running = super::running_package().map(|p| p.version.clone());
    let layout = Layout::new(&home);
    let estado = {
        let (layout, running) = (layout.clone(), running.clone());
        tokio::task::spawn_blocking(move || super::for_screen(&super::state_dir(&claude), &layout, running.as_deref())).await.unwrap_or_default()
    };
    let body = match mode {
        Mode::Package(_) if let Some(why) = super::broken_manifest(&layout) => {
            // Pacote sem o que o atualizador precisa: o botão fica preso com o motivo, sem ir ao Python.
            let mut body = package_body(None, running.as_deref(), None, topology, estado);
            body["pre_voo"]["pode"] = false.into();
            body["pre_voo"]["erro"] = "manifesto_invalido".into();
            body["pre_voo"]["detalhe"] = why.into();
            body
        }
        Mode::Package(_) => {
            let installed = super::installed(&layout);
            let channel = installed.as_ref().map_or_else(|| "latest".to_owned(), |m| m.channel.clone());
            let remote = published(&Release { base_url: st.update_release_base.clone(), channel }, refresh).await;
            package_body(installed.as_ref(), running.as_deref(), remote.as_ref(), topology, estado)
        }
        Mode::Developer(root) => {
            let checkout = tokio::task::spawn_blocking(move || super::dev::read(&root, refresh)).await.unwrap_or_else(|e| Err(e.to_string()));
            dev_body(checkout.as_ref().map_err(String::as_str), super::dev::boot_head(), topology, estado)
        }
        Mode::Unmigrated => unreachable!("filtrado em owned"),
    };
    with_cors(req.headers(), json_response(StatusCode::OK, body))
}

fn busy() -> Response { detail(StatusCode::CONFLICT, "erro_atualizacao_ja_rodando", "ja existe uma atualizacao rodando", json!({})) }

/// Lança o executor fora do runtime (a sondagem do escopo e o spawn bloqueiam).
async fn launch(st: &AppState, task: Task) -> Result<Started, StartError> {
    let (home, claude, args) = (st.accounts.env.home.clone(), st.accounts.env.claude_base.clone(), Args { task, listen: st.cfg.listen });
    let require_scope = topology().await == Topology::Systemd;
    tokio::task::spawn_blocking(move || super::executor::launch(&home, &claude, &args, require_scope))
        .await.unwrap_or_else(|e| Err(StartError::Launch(e.to_string())))
}

fn started(started: Result<Started, StartError>) -> Response {
    match started {
        Ok(Started { pid, request_id: None }) => json_response(StatusCode::OK, json!({"ok": true, "pid": pid})),
        Ok(Started { pid, request_id: Some(id) }) => json_response(StatusCode::OK, json!({"ok": true, "pid": pid, "request_id": id})),
        Err(StartError::Busy) => busy(),
        Err(StartError::Launch(why)) => detail(StatusCode::INTERNAL_SERVER_ERROR, "erro_atualizacao_lancamento", &why, json!({})),
    }
}

/// `POST /api/atualizacao/iniciar`: o pacote, ou o `git pull` do modo desenvolvedor.
pub async fn start(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let Some(mode) = owned(&st, peer, &req) else { return pass_any(State(st), ConnectInfo(peer), req).await };
    let task = match mode {
        Mode::Developer(root) => {
            // Recusa antes de lançar, com o motivo; o executor confere de novo.
            let checked = tokio::task::spawn_blocking(move || super::dev::read(&root, false)).await.unwrap_or_else(|e| Err(e.to_string()));
            if let Some(why) = checked.as_ref().map_or_else(|e| Some(e.clone()), super::dev::refusal) {
                return with_cors(req.headers(), detail(StatusCode::CONFLICT, "erro_atualizacao_desenvolvedor", &why, json!({})));
            }
            Task::Dev
        }
        _ => {
            let (home, claude) = (st.accounts.env.home.clone(), st.accounts.env.claude_base.clone());
            let broken = tokio::task::spawn_blocking(move || {
                let why = super::broken_manifest(&Layout::new(&home))?;
                super::StateFile::new(&super::state_dir(&claude))
                    .write(json!({"fase": "pronto", "ok": false, "erro": why, "voltou": false, "no_ar": true}));
                Some(why)
            }).await.ok().flatten();
            if let Some(why) = broken {
                return with_cors(req.headers(), detail(StatusCode::CONFLICT, "erro_atualizacao_manifesto", &why, json!({})));
            }
            Task::Update { channel: None }
        }
    };
    with_cors(req.headers(), started(launch(&st, task).await))
}

/// `POST /api/atualizacao/reiniciar`: só reinicia, destacado; na mão não há o que reiniciar.
pub async fn restart(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if owned(&st, peer, &req).is_none() { return pass_any(State(st), ConnectInfo(peer), req).await; }
    let topology = topology().await;
    if topology == Topology::Manual {
        return with_cors(req.headers(), detail(StatusCode::CONFLICT, "erro_reinicio_indisponivel",
            "esta maquina nao reinicia sozinha (topologia manual)", json!({"topologia": topology.name()})));
    }
    let request_id = crate::mods::state::random_hex(16);
    with_cors(req.headers(), started(launch(&st, Task::Restart { request_id }).await))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use hangar_package::MANIFEST_FILE;

    use super::*;

    fn manifest(version: &str, channel: &str) -> Manifest {
        Manifest { version: version.into(), commit: "abc123".into(), channel: channel.into(), protocol: 89, api: 4, os: "linux-x86_64".into(),
            dist_key: None, files: BTreeMap::new() }
    }

    fn entry(version: &str, api: u32) -> PlatformEntry {
        PlatformEntry { version: version.into(), asset: "a.tar.gz".into(), sha256: "0".repeat(64), protocol: 89, api, commit: "def".into(), dist_key: None }
    }

    /// Todos os campos que algum cliente lê (`packages/core/src/types.ts` `Atualizacao`,
    /// `desktop-native/src/update.rs`, `app/device.rs`), com o tipo que ele espera.
    fn assert_client_shape(body: &Value) {
        for key in ["versoes", "versao_legivel", "pre_voo", "estado"] { assert!(body[key].is_object(), "{key}: {body}"); }
        for key in ["repo", "backend"] { assert!(body["versoes"][key].is_string(), "versoes.{key}"); }
        for key in ["repo", "backend", "remoto"] { assert!(body["versao_legivel"].get(key).is_some(), "versao_legivel.{key}"); }
        for key in ["atras", "api_publicado"] { assert!(body[key].is_u64(), "{key}"); }
        for key in ["atualizacao_disponivel", "servidor_desatualizado"] { assert!(body[key].is_boolean(), "{key}"); }
        for key in ["mudancas", "passos"] { assert!(body[key].is_array(), "{key}"); }
        let pre = &body["pre_voo"];
        assert!(pre["pode"].is_boolean() && pre["faltando"].is_array() && pre["branch_de_trabalho"].is_boolean() && pre["divergiu"].is_boolean());
        for key in ["sujo", "ahead", "behind"] { assert!(pre[key].is_u64(), "pre_voo.{key}"); }
        for key in ["branch", "alvo", "commit"] { assert!(pre[key].is_string(), "pre_voo.{key}"); }
        assert!(["systemd", "windows", "manual"].contains(&pre["topologia"].as_str().unwrap()));
    }

    #[test]
    fn update_package_body_has_the_python_shape_with_package_values() {
        let installed = manifest("0.1.0.8200", "latest");
        let estado = json!({"fase": "pronto", "ok": true}).as_object().cloned().unwrap();
        let body = package_body(Some(&installed), Some("0.1.0.8200"), Some(&entry("0.1.0.8207", 5)), "systemd", estado.clone());
        assert_client_shape(&body);
        assert_eq!(body["atualizacao_disponivel"], true);
        assert_eq!(body["atras"], 7, "contagem de commits do número da versão");
        assert_eq!(body["pre_voo"]["behind"], 7);
        assert_eq!(body["api_publicado"], 5);
        assert_eq!((body["pre_voo"]["alvo"].as_str(), body["pre_voo"]["branch"].as_str()), (Some("main"), Some("main")), "canal latest = main");
        assert_eq!(body["versoes"], json!({"repo": "0.1.0.8200", "backend": "0.1.0.8200"}));
        assert_eq!(body["versao_legivel"]["remoto"], "0.1.0.8207");
        assert_eq!(body["estado"], Value::Object(estado.clone()));

        let same = package_body(Some(&installed), Some("0.1.0.8199"), Some(&entry("0.1.0.8200", 5)), "windows", estado.clone());
        assert_eq!((same["atualizacao_disponivel"].clone(), same["atras"].clone()), (json!(false), json!(0)));
        assert_ne!(same["versoes"]["repo"], same["versoes"]["backend"], "disco à frente do processo: a tela oferece reiniciar");
        let base = package_body(Some(&installed), None, Some(&entry("0.2.0.10", 5)), "manual", estado.clone());
        assert_eq!((base["atras"].clone(), base["atualizacao_disponivel"].clone()), (json!(1), json!(true)), "VERSION novo");
        let offline = package_body(Some(&manifest("0.1.0.8200", "beta")), Some("0.1.0.8200"), None, "systemd", estado);
        assert_client_shape(&offline);
        assert_eq!((offline["atualizacao_disponivel"].clone(), offline["api_publicado"].clone()), (json!(false), json!(4)), "sem índice: a API instalada");
        assert_eq!(offline["pre_voo"]["alvo"], "beta");
    }

    #[test]
    fn update_dev_body_has_the_python_shape_with_the_checkout_values() {
        let checkout = Checkout { branch: "main".into(), upstream: Some("origin/main".into()), dirty: 2, ahead: 0, behind: 3,
            head: "aaa1111".into(), upstream_head: Some("bbb2222".into()), changes: vec![("bbb2222".into(), "fix: x".into())] };
        let body = dev_body(Ok(&checkout), Some("aaa1111"), "systemd", Map::new());
        assert_client_shape(&body);
        assert_eq!((body["atras"].clone(), body["atualizacao_disponivel"].clone()), (json!(3), json!(true)));
        assert_eq!(body["pre_voo"]["sujo"], 2, "a tela segura o botão com edição local");
        assert_eq!(body["mudancas"], json!([{"sha": "bbb2222", "titulo": "fix: x"}]));
        let broken = dev_body(Err("git saiu com 128"), None, "manual", Map::new());
        assert_eq!((broken["pre_voo"]["pode"].clone(), broken["pre_voo"]["erro"].clone()), (json!(false), json!("nao_e_repo")));
    }

    #[test]
    fn update_mode_reads_the_current_target() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        assert_eq!(mode(home), Mode::Unmigrated);
        let target = home.join("alvo");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(home.join(".hangar")).unwrap();
        super::super::swap(&Layout::new(home), &target).unwrap();
        assert_eq!(mode(home), Mode::Unmigrated, "nem pacote nem checkout");
        std::fs::create_dir_all(target.join(".git")).unwrap();
        let real = crate::list::links::without_verbatim(std::fs::canonicalize(&target).unwrap());
        assert_eq!(mode(home), Mode::Developer(real.clone()));
        std::fs::write(target.join(MANIFEST_FILE), "{}").unwrap();
        assert_eq!(mode(home), Mode::Developer(real.clone()), "manifesto inválido não faz pacote, como no Python");
        std::fs::write(target.join(MANIFEST_FILE), r#"{"version": "x", "commit": "", "channel": "latest"}"#).unwrap();
        assert_eq!(mode(home), Mode::Developer(real.clone()), "commit vazio: o Python também recusa");
        // O critério do `diag.install_manifest()`: version, commit e channel não vazios; o resto não decide.
        std::fs::write(target.join(MANIFEST_FILE), r#"{"version": "x", "commit": "c", "channel": "latest"}"#).unwrap();
        assert_eq!(mode(home), Mode::Package(real.clone()), "incompleto para atualizar, mas pacote");
        std::fs::write(target.join(MANIFEST_FILE), serde_json::to_vec(&manifest("0.1.0.1", "latest")).unwrap()).unwrap();
        assert_eq!(mode(home), Mode::Package(real));
    }

    #[test]
    fn update_unknown_running_version_is_the_disk_one() {
        // Vazio faria o nativo oferecer "reiniciar" para sempre (`behind` compara repo com backend).
        let body = package_body(Some(&manifest("0.1.0.5", "latest")), None, None, "systemd", Map::new());
        assert_eq!(body["versoes"], json!({"repo": "0.1.0.5", "backend": "0.1.0.5"}));
        assert_eq!(body["versao_legivel"]["backend"], "0.1.0.5");
        let checkout = Checkout { branch: "main".into(), head: "aaa1111".into(), ..Checkout::default() };
        let dev = dev_body(Ok(&checkout), None, "systemd", Map::new());
        assert_eq!(dev["versoes"], json!({"repo": "aaa1111", "backend": "aaa1111"}));
    }

    #[test]
    fn update_only_a_found_topology_is_kept() {
        assert!(keeps(Topology::Systemd) && keeps(Topology::Windows));
        assert!(!keeps(Topology::Manual), "falha momentânea do systemctl não vira manual para sempre");
    }
}

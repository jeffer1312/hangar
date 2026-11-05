//! Rotas completas da orquestração; a autorização da porta é feita antes de chegar aqui.
use super::{array, context, plan, policy, roles, runs, text, truthy};
use crate::routes::AppState;
use axum::{
    body::to_bytes,
    extract::Request,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
#[derive(Debug, Clone)]
enum Route {
    List,
    Detail(String),
    HistoryPanel(String),
    Conductor(String),
    Policy,
    PolicyAccount(String),
    Team(String),
    Panel(String),
    Role(String),
    Roles(String),
    Group(String),
    Start(String),
}
fn decode(s: &str) -> Option<String> {
    percent_encoding::percent_decode_str(s)
        .decode_utf8()
        .ok()
        .map(|s| s.into_owned())
}
fn route(method: &Method, path: &str) -> Option<Route> {
    if method == Method::GET && path == "/api/orq" {
        return Some(Route::List);
    }
    if path == "/api/orquestracao/politica" && method == Method::GET {
        return Some(Route::Policy);
    }
    if method == Method::PUT {
        if let Some(id) = path.strip_prefix("/api/orquestracao/politica/") {
            return decode(id).map(Route::PolicyAccount);
        }
    }
    if let Some(rest) = path.strip_prefix("/api/orq/") {
        if method != Method::GET {
            return None;
        }
        let fields: Vec<_> = rest.split('/').collect();
        return match fields.as_slice() {
            [id] => decode(id).map(Route::Detail),
            [id, "panel"] => decode(id).map(Route::HistoryPanel),
            [id, "conductor"] => decode(id).map(Route::Conductor),
            _ => None,
        };
    }
    let rest = path.strip_prefix("/api/sessions/")?;
    let (name, rest) = rest.split_once('/')?;
    let name = decode(name)?;
    match (rest, method) {
        ("orq", &Method::GET) => Some(Route::Team(name)),
        ("orq/panel", &Method::GET) => Some(Route::Panel(name)),
        ("orq/papel", &Method::POST | &Method::DELETE) => Some(Route::Role(name)),
        ("orq/papeis", &Method::POST) => Some(Route::Roles(name)),
        ("orq/grupo", &Method::POST) => Some(Route::Group(name)),
        ("orq/comecar", &Method::POST) => Some(Route::Start(name)),
        _ => None,
    }
}
pub fn matches(method: &Method, path: &str) -> bool {
    route(method, path).is_some()
}
fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    super::codec::response(
        status,
        crate::session_write::detail_body(code, msg, json!({})),
    )
}
fn invalid() -> Response {
    fail(
        StatusCode::UNPROCESSABLE_ENTITY,
        "erro_corpo_invalido",
        "corpo da requisição inválido",
    )
}
fn body<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Response> {
    let value = super::body::parse(bytes)
        .map_err(|(status, value)| super::codec::response(status, value))?;
    let validated = super::body::validate(std::any::type_name::<T>(), &value);
    super::codec::discard(value);
    let value = validated
        .map_err(|v| super::codec::response(StatusCode::UNPROCESSABLE_ENTITY, v))?;
    serde_json::from_value(value).map_err(|_| invalid())
}
fn changed(e: &str) -> Response {
    fail(
        StatusCode::CONFLICT,
        if e.contains("mudou desde") {
            "erro_orq_arquivo_mudou"
        } else {
            "erro_orq_celula_invalida"
        },
        e,
    )
}
fn missing() -> Response {
    fail(
        StatusCode::NOT_FOUND,
        "erro_nao_encontrado",
        "execucao nao encontrada",
    )
}
fn good(v: Value) -> Response {
    super::codec::response(StatusCode::OK, v)
}
fn dirs(st: &AppState) -> (PathBuf, PathBuf, String) {
    let env = &st.accounts.env;
    let home = env.home.clone();
    let pair = st
        .groups
        .as_ref()
        .map(|g| g.pair_root().to_owned())
        .unwrap_or_else(|| env.claude_base.join(".hangar-pair"));
    let mux = st.list.env().capture_program.to_string_lossy().into_owned();
    (home, pair, mux)
}
async fn rows(st: &AppState) -> Result<Vec<Value>, Response> {
    st.list
        .discover(None)
        .await
        .map(|rows| {
            rows.iter()
                .filter_map(|r| serde_json::to_value(r).ok())
                .collect()
        })
        .map_err(|_| {
            fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "erro_lista_indisponivel",
                "não foi possível ler as sessões",
            )
        })
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Response> {
    tokio::task::spawn_blocking(f).await.map_err(|_| {
        fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "orq_read_failed",
            "não foi possível ler a orquestração",
        )
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyBody {
    provider: String,
    #[serde(default)]
    apelido: String,
    #[serde(default = "any_models")]
    modelos: Vec<String>,
    #[serde(default = "yes")]
    trocar: bool,
    #[serde(default = "yes")]
    ligada: bool,
    #[serde(deserialize_with = "super::body::deserialize_number")]
    mtime: f64,
}
fn any_models() -> Vec<String> {
    vec!["*".into()]
}
fn yes() -> bool {
    true
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleBody {
    papel: String,
    #[serde(default)]
    sessao: String,
    provider: String,
    conta: String,
    #[serde(default)]
    modelo: String,
    #[serde(default)]
    esforco: String,
    #[serde(default)]
    vez: String,
    #[serde(default)]
    headless: Option<bool>,
    #[serde(default)]
    permissao: String,
    #[serde(default)]
    motor: String,
    #[serde(default)]
    jev: bool,
    #[serde(default)]
    subagente: String,
    #[serde(default)]
    perfil: String,
    #[serde(default)]
    janela: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RolesBody {
    papeis: Vec<RoleBody>,
    #[serde(deserialize_with = "super::body::deserialize_number")]
    mtime: f64,
    #[serde(default = "yes", rename = "avisar")]
    _notify: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OneRoleBody {
    papel: String,
    #[serde(default)]
    sessao: String,
    provider: String,
    conta: String,
    #[serde(default)]
    modelo: String,
    #[serde(default)]
    esforco: String,
    #[serde(default)]
    headless: Option<bool>,
    #[serde(default)]
    permissao: String,
    #[serde(default)]
    motor: String,
    #[serde(default)]
    jev: bool,
    #[serde(default)]
    subagente: String,
    #[serde(default)]
    perfil: String,
    #[serde(deserialize_with = "super::body::deserialize_number")]
    mtime: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveBody {
    papel: String,
    #[serde(default)]
    vez: String,
    #[serde(deserialize_with = "super::body::deserialize_number")]
    mtime: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupBody {
    gid: String,
    #[serde(deserialize_with = "super::body::deserialize_number")]
    mtime: f64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartBody {
    #[serde(default, rename = "mtime")]
    #[serde(deserialize_with = "super::body::deserialize_number")]
    _mtime: f64,
}
fn validate_role(st: &AppState, r: &roles::Role) -> Result<(), Response> {
    let error = |code: &str, msg: &str| {
        fail(
            StatusCode::BAD_REQUEST,
            code,
            &format!("{msg}: {}", r.papel),
        )
    };
    if r.headless == Some(true) && !matches!(r.provider.as_str(), "claude" | "codex") {
        return Err(error(
            "erro_orq_headless_provider",
            "sem terminal só vale para claude ou codex",
        ));
    }
    if r.headless == Some(true) && r.abertura_extra.contains("--read-only") {
        return Err(error(
            "erro_orq_headless_read_only",
            "sem terminal não aceita --read-only (o backend recusa a sessão): desligue o sem terminal",
        ));
    }
    if !r.motor.is_empty() {
        if r.provider != "claude" {
            return Err(error(
                "erro_motor_sem_claude",
                "motor so vale para provider claude",
            ));
        }
        let file = st
            .accounts
            .env
            .base
            .get("CP_ENGINES_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| st.accounts.env.claude_base.join("engines.json"));
        let engines = runs::json(&file).unwrap_or_default();
        if engines.get(&r.motor).is_none() {
            return Err(error("erro_motor_invalido", "motor invalido"));
        }
    }
    if !r.permissao.is_empty() {
        let valid = if r.provider == "codex" && r.headless == Some(true) {
            matches!(
                r.permissao.as_str(),
                "Ask for approval" | "Approve for me" | "Full Access"
            )
        } else if r.provider == "claude" {
            matches!(
                r.permissao.as_str(),
                "acceptEdits" | "auto" | "bypassPermissions" | "manual" | "dontAsk" | "plan"
            )
        } else {
            return Err(error(
                "erro_permissao_so_claude",
                "modo de permissao so vale para claude",
            ));
        };
        if !valid {
            return Err(error(
                "erro_permissao_invalida",
                "modo de permissao invalido",
            ));
        }
    }
    if !r.subagente.is_empty() {
        if r.provider != "claude" || !r.motor.is_empty() {
            return Err(error(
                "erro_subagente_so_claude",
                "modelo dos subagentes so vale para claude sem motor",
            ));
        }
        if crate::model_picker::normalize(Some(&r.subagente), None).is_err() {
            return Err(error(
                "erro_orq_celula_invalida",
                "modelo dos subagentes inválido",
            ));
        }
    }
    if !r.perfil.is_empty() {
        if r.provider != "omp" {
            return Err(error(
                "erro_perfil_so_omp",
                "perfil so vale para provider omp",
            ));
        }
        let mut env = st.accounts.env.base.clone();
        env.insert("OMP_PROFILE".into(), r.perfil.clone());
        if crate::accounts::secondary_auth::omp_directories(&st.accounts.env.home, &env).is_err() {
            return Err(error("erro_orq_celula_invalida", "perfil do omp inválido"));
        }
    }
    Ok(())
}
pub async fn serve(st: &Arc<AppState>, req: Request) -> Response {
    let Some(route) = route(req.method(), req.uri().path()) else {
        return missing();
    };
    if let Route::Conductor(_) = route {
        return crate::orq_conductor::serve(st, req).await;
    }
    let bulk_roles = matches!(&route, Route::Roles(_));
    let method = req.method().clone();
    let content_type = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let json_body = content_type.is_empty()
        || content_type.split(';').next().is_some_and(|mime| {
            mime == "application/json"
                || mime.starts_with("application/") && mime.ends_with("+json")
        });
    let bytes = match to_bytes(req.into_body(), 100 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response(),
    };
    let bytes = if json_body {
        bytes.to_vec()
    } else {
        serde_json::to_vec(&String::from_utf8_lossy(&bytes)).unwrap()
    };
    let (home, pair, mux) = dirs(st);
    let session = match &route {
        Route::Role(n) | Route::Roles(n) | Route::Group(n) | Route::Start(n)
            if method != Method::GET =>
        {
            Some(n.clone())
        }
        _ => None,
    };
    let _pass = if let (Some(name), Some(runtime)) = (session, st.state.runtime.get()) {
        match runtime.ingress().enter(&name, st.write_gate_wait).await {
            Ok(p) => Some(p),
            Err(_) => {
                return super::codec::response(
                    StatusCode::CONFLICT,
                    crate::session_write::busy_body(),
                );
            }
        }
    } else {
        None
    };
    match route {
        Route::List => {
            match blocking(move || {
                let mut execs = runs::list(&home);
                let cards = runs::cards(&execs);
                let units = runs::units();
                for e in &mut execs {
                    let id = text(&e["id"]).to_owned();
                    let dir = runs::root(&home).join(id);
                    let metadata = plan::metadata(&dir, text(&e["plano"]), &home);
                    e.as_object_mut().unwrap().remove("eventos_execucao");
                    if let Some(tasks) = e["tasks"].as_array_mut() {
                        for t in tasks {
                            t.as_object_mut().unwrap().remove("eventos");
                        }
                    }
                    e["watchdog"] = runs::watchdog(&dir, units.as_ref(), runs::now());
                    e["metadata"] = metadata;
                }
                json!({"execucoes":execs,"fichas":cards})
            })
            .await
            {
                Ok(v) => good(v),
                Err(e) => e,
            }
        }
        Route::Detail(id) => match blocking(move || {
            runs::detail(&home, &id).map(|mut e| {
                e["metadata"] =
                    plan::metadata(&runs::root(&home).join(&id), text(&e["plano"]), &home);
                e
            })
        })
        .await
        {
            Ok(Some(v)) => good(v),
            Ok(None) => missing(),
            Err(e) => e,
        },
        Route::HistoryPanel(id) => {
            let Some(dir) =
                runs::exec_dir(&home, &id).filter(|d| d.join("eventos.jsonl").is_file())
            else {
                return missing();
            };
            let live = rows(st).await.unwrap_or_default();
            let costs = st.costs.clone();
            match blocking(move || super::panel::panel(&dir, &home, live, costs)).await {
                Ok(v) => good(v),
                Err(e) => e,
            }
        }
        Route::Panel(name) => {
            let info = match st.list.find_row(&name).await {
                Ok(Some(row)) if row.provider == "orq" && row.jsonl.is_some() => row,
                _ => return missing(),
            };
            let Some(path) = info.jsonl else {
                return missing();
            };
            let Some(dir) = Path::new(&path).parent().map(Path::to_owned) else {
                return missing();
            };
            let live = rows(st).await.unwrap_or_default();
            let costs = st.costs.clone();
            match blocking(move || super::panel::panel(&dir, &home, live, costs)).await {
                Ok(v) => good(v),
                Err(e) => e,
            }
        }
        Route::Policy => {
            let path = policy::path(&home);
            let (content, mtime) = match blocking(move || super::read(&path)).await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    return fail(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "orq_policy_read_failed",
                        &e.to_string(),
                    );
                }
                Err(e) => return e,
            };
            let inventory = policy::inventory(st).await;
            good(
                json!({"arquivo":policy::path(&home),"mtime":mtime,"politica":policy::read(&content),"inventario":inventory}),
            )
        }
        Route::PolicyAccount(account) => {
            let b = match body::<PolicyBody>(&bytes) {
                Ok(b) => b,
                Err(e) => return e,
            };
            let inventory = policy::inventory(st).await;
            let Some(item) = inventory.iter().find(|i| {
                i["provider"] == b.provider
                    && super::markdown::normalize(text(&i["conta"]))
                        == super::markdown::normalize(&account)
            }) else {
                return fail(
                    StatusCode::BAD_REQUEST,
                    "erro_orq_conta_desconhecida",
                    &format!(
                        "conta {:?} ({}) não existe nesta máquina",
                        account, b.provider
                    ),
                );
            };
            let mut models: Vec<String> = b
                .modelos
                .iter()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            if models.is_empty() {
                models.push("*".into())
            }
            if !models.iter().any(|s| s == "*")
                && item["reduced"] != true
                && !array(&item["modelos"]).is_empty()
            {
                let bad: Vec<_> = models
                    .iter()
                    .filter(|m| {
                        !array(&item["modelos"])
                            .iter()
                            .any(|known| known["id"] == m.as_str())
                    })
                    .collect();
                if !bad.is_empty() {
                    return super::codec::response(
                        StatusCode::BAD_REQUEST,
                        crate::session_write::detail_body(
                            "erro_orq_modelo_desconhecido",
                            &format!(
                                "modelo(s) fora do catálogo da conta: {}",
                                bad.iter()
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            json!({"modelos":bad}),
                        ),
                    );
                }
            }
            for v in [&account, &b.apelido].into_iter().chain(models.iter()) {
                if let Err(e) = super::markdown::validate(v) {
                    return fail(StatusCode::BAD_REQUEST, "erro_orq_celula_invalida", &e);
                }
            }
            let item_account = text(&item["conta"]).to_owned();
            let result = blocking(move || {
                let _lock = context::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                let path = policy::path(&home);
                let (content, _) = super::read(&path).map_err(|e| e.to_string())?;
                let p = policy::Policy {
                    conta: item_account.clone(),
                    provider: b.provider,
                    apelido: b.apelido,
                    modelos: models,
                    trocar: b.trocar,
                };
                let content = policy::update(
                    &content,
                    if b.ligada { Some(&p) } else { None },
                    &item_account,
                    &inventory,
                )?;
                super::write(&path, &content, Some(b.mtime))
            })
            .await;
            match result {
                Ok(Ok(mtime)) => good(json!({"ok":true,"mtime":mtime})),
                Ok(Err(e)) => changed(&e),
                Err(e) => e,
            }
        }
        Route::Team(name) => {
            let live = match rows(st).await {
                Ok(v) => v,
                Err(e) => return e,
            };
            let base = st.accounts.env.claude_base.clone();
            match blocking(move||{let _lock=context::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());let ctx=context::resolve(&home,&pair,&name,&mux)?;let(content,mtime)=super::read(&ctx.path).map_err(|e|e.to_string())?;let roles=roles::read(&content);let arbiter=roles.iter().find(|r|super::markdown::normalize(&r.papel)=="arbitro").and_then(|r|roles::live(r,&live));let cwd=live.iter().find(|r|r["name"]==name).and_then(|r|r["cwd"].as_str());let ready=plan::readiness(cwd,ctx.grouped,!roles.is_empty());let roles:Vec<_>=roles.iter().map(|r|{let mut v=serde_json::to_value(r).unwrap();v["viva"]=json!(roles::live(r,&live));v["id_cota"]=json!(policy::quota_id(&home,&base,&r.provider,&r.conta));v}).collect();Ok::<_,String>(json!({"gid":ctx.gid,"grouped":ctx.grouped,"session_prefix":ctx.prefix,"session_identity":context::identity(&home,&name,&mux).ok(),"arquivo":ctx.path,"mtime":mtime,"prontidao":ready,"arbitro":arbiter,"papeis":roles}))}).await{Ok(Ok(v))=>good(v),Ok(Err(e))=>changed(&e),Err(e)=>e}
        }
        Route::Roles(name) | Route::Role(name) if method == Method::POST => {
            let b = if bulk_roles {
                match body::<RolesBody>(&bytes) {
                    Ok(b) => b,
                    Err(e) => return e,
                }
            } else {
                let b = match body::<OneRoleBody>(&bytes) {
                    Ok(b) => b,
                    Err(e) => return e,
                };
                RolesBody {
                    papeis: vec![RoleBody {
                        papel: b.papel,
                        sessao: b.sessao,
                        provider: b.provider,
                        conta: b.conta,
                        modelo: b.modelo,
                        esforco: b.esforco,
                        vez: String::new(),
                        headless: b.headless,
                        permissao: b.permissao,
                        motor: b.motor,
                        jev: b.jev,
                        subagente: b.subagente,
                        perfil: b.perfil,
                        janela: None,
                    }],
                    mtime: b.mtime,
                    _notify: true,
                }
            };
            if b.papeis.is_empty() {
                return fail(
                    StatusCode::BAD_REQUEST,
                    "erro_orq_celula_invalida",
                    "nenhum papel",
                );
            }
            let policy_path = policy::path(&home);
            let policies = match blocking(move || super::read(&policy_path)).await {
                Ok(Ok((content, _))) => policy::read(&content),
                Ok(Err(e)) => {
                    return fail(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "orq_policy_read_failed",
                        &e.to_string(),
                    );
                }
                Err(e) => return e,
            };
            let kimi = crate::provider_models::kimi_catalog(&home);
            let lock_home = home.clone();
            let lock_pair = pair.clone();
            let lock_name = name.clone();
            let lock_mux = mux.clone();
            let prepared = match blocking(move || {
                let ctx = context::resolve(&lock_home, &lock_pair, &lock_name, &lock_mux)?;
                let (content, _) = super::read(&ctx.path).map_err(|e| e.to_string())?;
                Ok::<_, String>((ctx, content))
            })
            .await
            {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => return changed(&e),
                Err(e) => return e,
            };
            let (ctx, mut content) = prepared;
            let old = roles::read(&content);
            let mut new = Vec::new();
            for item in b.papeis {
                let current = old.iter().find(|r| {
                    super::markdown::normalize(&r.papel) == super::markdown::normalize(&item.papel)
                        && super::markdown::normalize(&r.vez)
                            == super::markdown::normalize(item.vez.trim())
                });
                let r = roles::Role {
                    papel: item.papel.trim().into(),
                    sessao: if item.sessao.is_empty() {
                        current
                            .map(|r| r.sessao.trim().to_owned())
                            .unwrap_or_default()
                    } else {
                        item.sessao.trim().into()
                    },
                    provider: item.provider.trim().to_lowercase(),
                    conta: item.conta.trim().into(),
                    modelo: item.modelo.trim().into(),
                    esforco: item.esforco.trim().into(),
                    vez: item.vez.trim().into(),
                    headless: item.headless,
                    permissao: item.permissao.trim().into(),
                    motor: item.motor.trim().into(),
                    jev: item.jev,
                    subagente: item.subagente.trim().into(),
                    perfil: item.perfil.trim().into(),
                    abertura_extra: current
                        .map(|r| r.abertura_extra.clone())
                        .unwrap_or_default(),
                    janela: item
                        .janela
                        .map(|s| s.trim().trim_end_matches('%').trim().to_owned())
                        .unwrap_or_else(|| current.map(|r| r.janela.clone()).unwrap_or_default()),
                };
                if let Some(code) = policy::permitted(
                    &r.provider,
                    &r.conta,
                    &r.modelo,
                    &r.esforco,
                    &policies,
                    kimi.as_ref(),
                ) {
                    return fail(
                        StatusCode::BAD_REQUEST,
                        code,
                        &format!("a política de contas não permite esta escolha: {}", r.papel),
                    );
                }
                if let Err(e) = validate_role(st, &r) {
                    return e;
                }
                content = match roles::write(&content, &r) {
                    Ok(s) => s,
                    Err(e) => return fail(StatusCode::BAD_REQUEST, "erro_orq_celula_invalida", &e),
                };
                new.push(r);
            }
            match blocking(move || {
                let _lock = context::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                context::write(&home, &pair, &ctx, &content, b.mtime)
            })
            .await
            {
                Ok(Ok(mtime)) => good(
                    json!({"papeis":new,"papel":new[0],"mtime":mtime,"arbitro":null,"aviso":"proxima_sessao","erro":null}),
                ),
                Ok(Err(e)) => changed(&e),
                Err(e) => e,
            }
        }
        Route::Role(name) if method == Method::DELETE => {
            let b = match body::<RemoveBody>(&bytes) {
                Ok(b) => b,
                Err(e) => return e,
            };
            let missing_role=format!("não há linha para {} nesta configuração",crate::transcript::py::py_repr(&json!(b.papel)));
            match blocking(move || {
                let _lock = context::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                let ctx = context::resolve(&home, &pair, &name, &mux)?;
                let (content, _) = super::read(&ctx.path).map_err(|e| e.to_string())?;
                let content = roles::remove(&content, &b.papel, &b.vez)?;
                let mtime = context::write(&home, &pair, &ctx, &content, b.mtime)?;
                Ok::<_, String>(json!({"papeis":roles::read(&content),"mtime":mtime}))
            })
            .await
            {
                Ok(Ok(v)) => good(v),
                Ok(Err(e)) if e == "erro_orq_papel_inexistente" => fail(
                    StatusCode::NOT_FOUND,
                    &e,
                    &missing_role,
                ),
                Ok(Err(e)) => changed(&e),
                Err(e) => e,
            }
        }
        Route::Group(name) => {
            let b = match body::<GroupBody>(&bytes) {
                Ok(b) => b,
                Err(e) => return e,
            };
            let Some(groups) = &st.groups else {
                return fail(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "groups_need_rust_server",
                    "a associação exige o servidor Rust",
                );
            };
            let result = groups
                .locked(async {
                    blocking(move || {
                        let _lock = context::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                        context::associate(&home, &pair, &name, &b.gid, b.mtime, &mux)
                    })
                    .await
                })
                .await;
            match result {
                Ok(Ok(v)) => good(v),
                Ok(Err(context::AssociateError::Invalid(e))) => {
                    fail(StatusCode::CONFLICT, "erro_orq_celula_invalida", &e)
                }
                Ok(Err(context::AssociateError::Changed(e))) => {
                    fail(StatusCode::CONFLICT, "erro_orq_arquivo_mudou", &e)
                }
                Err(e) => e,
            }
        }
        Route::Start(name) => {
            if let Err(e) = body::<StartBody>(&bytes) {
                return e;
            }
            let live = match rows(st).await {
                Ok(v) => v,
                Err(e) => return e,
            };
            let Some(row) = live.iter().find(|r| r["name"] == name).cloned() else {
                return fail(
                    StatusCode::NOT_FOUND,
                    "erro_sessao_inexistente",
                    "sessao nao encontrada",
                );
            };
            let name2 = name.clone();
            let ready = match blocking(move || {
                let _lock = context::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                let ctx = context::resolve(&home, &pair, &name2, &mux)?;
                let (content, _) = super::read(&ctx.path).map_err(|e| e.to_string())?;
                let ready = plan::readiness(
                    row["cwd"].as_str(),
                    ctx.grouped,
                    !roles::read(&content).is_empty(),
                );
                Ok::<_, String>((ready, ctx.path))
            })
            .await
            {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => return changed(&e),
                Err(e) => return e,
            };
            let (ready, path) = ready;
            let message = plan::kickoff(&ready, &path.to_string_lossy());
            let (status, reply) =
                crate::session_write::input::deliver_reply(st, &name, &message).await;
            if !status.is_success() {
                return super::codec::response(
                    StatusCode::CONFLICT,
                    crate::session_write::detail_body(
                        "erro_orq_comecar_falhou",
                        "não deu pra avisar a sessão",
                        json!({"erro":reply.get("detail").unwrap_or(&reply)}),
                    ),
                );
            }
            good(
                json!({"ok":true,"entregue":truthy(&reply["delivered"]),"fase":ready["phase"],"plano":ready["plan"]["path"].as_str().map(|s|Path::new(s).file_name().unwrap_or_default().to_string_lossy().into_owned()).unwrap_or_default()}),
            )
        }
        _ => missing(),
    }
}

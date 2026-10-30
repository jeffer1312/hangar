//! O login decide a pasta antes de executar; nenhum consumidor Python decide sua identidade.
use super::{
    body::{self, Schema},
    terminal::{Service, Spec},
};
use crate::{login_guests::store::Guest, orchestration::codec, routes::AppState};
use axum::{
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

fn error(status: u16, code: &str, msg: &str, params: Value) -> Response {
    crate::engines::body::json_error(
        StatusCode::from_u16(status).unwrap(),
        json!({"detail":{"code":code,"msg":msg,"params":params}}),
    )
}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn fail(code: &str, msg: &str) -> Response {
    error(400, code, msg, json!({}))
}
fn unavailable() -> Response {
    error(
        503,
        "shortcut_context_unavailable",
        "contexto do terminal indisponível",
        json!({}),
    )
}

pub async fn display_environment(
    env: &crate::accounts::environment::AccountEnvironment,
) -> BTreeMap<String, String> {
    let mut result = env.base.clone();
    if !cfg!(windows) {
        let mut command = crate::terminal_input::child_command("systemctl");
        command
            .args(["--user", "show-environment"])
            .env_clear()
            .envs(&env.base)
            .kill_on_drop(true);
        if let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(3), command.output()).await
        {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some((key, value)) = line.split_once('=') {
                    if [
                        "DISPLAY",
                        "WAYLAND_DISPLAY",
                        "XDG_CURRENT_DESKTOP",
                        "HYPRLAND_INSTANCE_SIGNATURE",
                    ]
                    .contains(&key)
                    {
                        result.entry(key.into()).or_insert_with(|| value.into());
                    }
                }
            }
        }
    }
    result
}
async fn folder(st: &AppState, cwd: &str, requested: Option<&str>) -> Result<PathBuf, Response> {
    let Some(requested) = requested else {
        return Ok(PathBuf::from(cwd));
    };
    let requested = crate::transcript::py::strip(requested);
    let requested = Path::new(requested);
    let path = if requested.is_absolute() {
        requested.to_owned()
    } else {
        let base = Path::new(cwd);
        if !base.is_dir() {
            let message = format!("pasta da sessao nao existe: {cwd}");
            return Err(error(
                409,
                "erro_project_shortcuts_projeto",
                &message,
                json!({"detalhe":message}),
            ));
        }
        let mut command = crate::terminal_input::child_command("git");
        command
            .args([
                "-C",
                cwd,
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
                "--show-toplevel",
            ])
            .env_clear()
            .envs(&st.accounts.env.base)
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(3), command.output()).await;
        let (root, failure) = match output {
            Ok(Ok(output)) => {
                let raw = String::from_utf8_lossy(&output.stdout);
                let lines: Vec<_> = raw.trim().lines().collect();
                let root = if output.status.success() && lines.len() == 2 {
                    Some(PathBuf::from(lines[1]))
                } else {
                    None
                };
                (
                    root,
                    String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                )
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                (None, "git nao encontrado".into())
            }
            Ok(Err(error)) => (None, format!("git falhou: {error}")),
            Err(_) => (None, "git timeout".into()),
        };
        let failure = if failure.is_empty() {
            "sem resposta"
        } else {
            &failure
        };
        let root = match root {
            Some(root) => root,
            None if base.ancestors().any(|p| p.join(".git").exists()) => {
                return Err(error(
                    409,
                    "erro_project_shortcuts_projeto",
                    &format!("git falhou em {cwd}: {failure}"),
                    json!({"detalhe":format!("git falhou em {cwd}: {failure}")}),
                ));
            }
            None => base.to_owned(),
        };
        root.join(requested)
    };
    if !path.is_dir() {
        let msg = format!("pasta nao existe: {}", path.display());
        return Err(error(
            400,
            "erro_shortcut_pasta",
            &msg,
            json!({"detalhe":msg}),
        ));
    }
    Ok(path)
}

pub async fn intercept(
    st: &Arc<AppState>,
    guest: &Guest,
    path: &str,
    request: Request,
) -> Result<Request, Response> {
    if request.method() != axum::http::Method::POST {
        return Ok(request);
    }
    let Some(tail) = path.strip_prefix("/api/sessions/") else {
        return Ok(request);
    };
    let Some((name, operation)) = tail.split_once('/') else {
        return Ok(request);
    };
    if !matches!(operation, "shortcut-shell" | "run-code") {
        return Ok(request);
    }
    let run_code = operation == "run-code";
    let (mut parts, body) = request.into_parts();
    let bytes = crate::http_body_limit::read(body).await.map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "There was an error parsing the body",
        )
            .into_response()
    })?;
    if crate::http_body_input::is_json(&parts.headers) && !bytes.is_empty() {
        let syntax = crate::orchestration::body::parse(&bytes)
            .map_err(|(status, value)| crate::engines::body::json_error(status, value))?;
        codec::discard(syntax);
    }
    let _pass = match st.state.runtime.get() {
        Some(runtime) => Some(
            runtime
                .ingress()
                .enter(name, Duration::ZERO)
                .await
                .map_err(|_| {
                    crate::session_write::json_response(
                        StatusCode::CONFLICT,
                        crate::session_write::busy_body(),
                    )
                })?,
        ),
        None => None,
    };
    let home = st.accounts.env.home.clone();
    let transfer_name = name.to_owned();
    if tokio::task::spawn_blocking(move || {
        crate::session_rename::files::transfer_busy(&home, &transfer_name)
    })
    .await
    .map_err(|_| unavailable())?
    .map_err(|_| unavailable())?
    {
        return Err(crate::session_write::json_response(
            StatusCode::CONFLICT,
            crate::session_write::busy_body(),
        ));
    }
    let mut value = body::parse(
        if run_code {
            Schema::RunCode
        } else {
            Schema::Shell
        },
        &bytes,
        &parts.headers,
    )?;
    let result = plan(st, guest, name, run_code, &mut value).await;
    match result {
        Err(response) => {
            codec::discard(value);
            Err(response)
        }
        Ok(Some(response)) => {
            codec::discard(value);
            Err(response)
        }
        Ok(None) => {
            let encoded = codec::dumps(&value, false);
            codec::discard(value);
            parts.headers.insert(
                header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );
            parts.headers.remove(header::CONTENT_LENGTH);
            Ok(Request::from_parts(parts, Body::from(encoded)))
        }
    }
}
async fn plan(
    st: &Arc<AppState>,
    guest: &Guest,
    name: &str,
    run_code: bool,
    value: &mut Value,
) -> Result<Option<Response>, Response> {
    let command = text(value, "command");
    let mut shell = None;
    if run_code {
        if command.chars().count() > 4096 {
            return Err(fail("erro_run_code_longo", "comando longo demais"));
        }
        if command.contains('\0') {
            return Err(fail("erro_run_code_invalido", "comando invalido"));
        }
        let key = text(value, "key");
        if !key.is_empty()
            && (!key.starts_with("run-code:")
                || !key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-')))
        {
            return Err(fail("erro_run_code_invalido", "identificador invalido"));
        }
        let language = crate::transcript::py::strip(text(value, "language")).to_lowercase();
        let unix = ["bash", "sh", "zsh", "fish"].contains(&language.as_str());
        let powershell = ["powershell", "ps1", "pwsh"].contains(&language.as_str());
        if !language.is_empty() && !unix && !powershell && language != "shell" {
            return Err(fail(
                "erro_run_code_linguagem",
                "linguagem de terminal invalida",
            ));
        }
        if (cfg!(windows) && unix) || (!cfg!(windows) && powershell) {
            return Err(error(
                409,
                "erro_run_code_shell_incompativel",
                "o bloco nao combina com o sistema deste servidor",
                json!({"linguagem":language,"sistema":if cfg!(windows){"Windows"}else{"Linux"}}),
            ));
        }
        if (!cfg!(windows) && !matches!(language.as_str(), "" | "shell"))
            || (cfg!(windows) && language == "pwsh")
        {
            let executable = if cfg!(windows) { "pwsh.exe" } else { &language };
            shell = find_shell(executable, &st.accounts.env.base);
            if shell.is_none() {
                return Err(error(
                    409,
                    "erro_run_code_shell_ausente",
                    "interpretador nao instalado",
                    json!({"linguagem":language}),
                ));
            }
        }
    } else if text(value, "runs_in") == "hangar" {
        return Err(error(
            403,
            "erro_shortcut_hangar_convidado",
            "convidado nao roda atalho No Hangar",
            json!({}),
        ));
    }
    let row = st
        .list
        .find_row(name)
        .await
        .map_err(|_| unavailable())?
        .ok_or_else(|| {
            error(
                404,
                "erro_sessao_inexistente",
                "sessao nao encontrada",
                json!({}),
            )
        })?;
    let cwd = row.cwd.filter(|p| !p.is_empty()).ok_or_else(|| {
        error(
            404,
            "erro_sessao_inexistente",
            "sessao nao encontrada",
            json!({}),
        )
    })?;
    let command = crate::transcript::py::strip(command).to_owned();
    if command.is_empty() {
        return Err(fail("erro_shortcut_vazio", "comando vazio"));
    }
    let cwd = folder(
        st,
        &cwd,
        if run_code {
            None
        } else {
            value["pasta"].as_str()
        },
    )
    .await?;
    let real = confined_folder(&cwd, Path::new(&guest.root))?;
    if !run_code {
        value["pasta"] = json!(real.to_string_lossy());
        return Ok(None);
    }
    if let Some(missing) = super::has_placeholder(&command) {
        return Err(error(
            400,
            "erro_shortcut_segredo",
            &format!("preencha a credencial {missing} antes de usar"),
            json!({"nome":missing}),
        ));
    }
    let service = Arc::new(Service {
        mux: Arc::new(crate::session_open::terminal::ProcessMux::default()),
        home: st.accounts.env.home.clone(),
        env: display_environment(&st.accounts.env).await,
        program: vec![st.list.env().capture_program.to_string_lossy().into_owned()],
        windows: cfg!(windows),
        encode: super::terminal_windows::encode,
        id: || crate::mods::state::random_hex(3),
        sequence: || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        },
    });
    let terminal = service
        .clone()
        .start(Spec {
            owner: name.into(),
            cwd: real.to_string_lossy().into_owned(),
            command,
            label: "Terminal".into(),
            key: text(value, "key").into(),
            origin: String::new(),
            ask: true,
            powershell: true,
            shell,
        })
        .await
        .map_err(|_| crate::engines::body::internal_error())?
        .ok_or_else(|| {
            error(
                500,
                "erro_shortcut_shell",
                "o multiplexador recusou criar o terminal",
                json!({}),
            )
        })?;
    let limit = tokio::time::Instant::now() + Duration::from_secs(2);
    let (alive, code) = loop {
        let state = service.status(&terminal.tmux).await;
        if !state.0 || tokio::time::Instant::now() >= limit {
            break state;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let public = json!({"id":terminal.id,"label":terminal.label,"alive":alive,"exit_code":code});
    if alive || code == Some(0) {
        return Ok(Some(crate::engines::body::json_error(
            StatusCode::ACCEPTED,
            json!({"ok":true,"terminal":public}),
        )));
    }
    let output = service.output(&terminal.tmux).await;
    let lines: Vec<_> = output.trim().lines().rev().take(5).collect();
    let tail = lines
        .into_iter()
        .rev()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    let tail = if tail.chars().count() > 400 {
        format!(
            "…{}",
            tail.chars()
                .skip(tail.chars().count() - 400)
                .collect::<String>()
        )
    } else {
        tail
    };
    let mut msg = format!(
        "o comando saiu com o código {}",
        code.map(|v| v.to_string()).unwrap_or_else(|| "None".into())
    );
    if !tail.is_empty() {
        msg.push_str(&format!(": {tail}"))
    }
    Err(error(
        422,
        "erro_shortcut_falhou",
        &msg,
        json!({"codigo":code,"saida":tail,"terminal":public}),
    ))
}

fn confined_folder(cwd: &Path, root: &Path) -> Result<PathBuf, Response> {
    let real = std::fs::canonicalize(cwd).map_err(|_| unavailable())?;
    let root = std::fs::canonicalize(root).map_err(|_| unavailable())?;
    if !real.starts_with(&root) {
        return Err(error(403, "erro_fora_da_pasta",
            "o convidado só abre sessão dentro da pasta dele", json!({})));
    }
    Ok(real)
}

#[cfg(test)]
mod root_tests {
    #[test]
    fn registered_root_and_canonical_path_share_the_same_physical_boundary() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("convidado");
        let child = root.join("projeto");
        std::fs::create_dir_all(&child).unwrap();
        let registered = crate::login_guests::store::valid_root(root.to_str().unwrap()).unwrap();
        let real = std::fs::canonicalize(&child).unwrap();
        assert_eq!(super::confined_folder(&real, std::path::Path::new(&registered)).unwrap(), real);
        assert_eq!(super::confined_folder(home.path(), std::path::Path::new(&registered)).unwrap_err().status(), 403);
    }
}

fn find_shell(name: &str, env: &BTreeMap<String, String>) -> Option<String> {
    std::env::split_paths(env.get("PATH").map(String::as_str).unwrap_or(""))
        .map(|p| p.join(name))
        .find(|p| {
            if !p.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                p.metadata()
                    .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
        .map(|p| p.to_string_lossy().into_owned())
}

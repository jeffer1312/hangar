use super::*;
fn reply(ok: bool, message: impl Into<String>) -> Value {
    json!({"ok":ok,"message":message.into()})
}
fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}
pub fn terminal_command(term: &str, args: &[String], message: &str) -> Vec<String> {
    let command = std::iter::once("tmux".to_owned())
        .chain(args.iter().cloned())
        .map(|value| shell_quote(&value))
        .collect::<Vec<_>>()
        .join(" ");
    vec![
        term.into(),
        "-e".into(),
        "sh".into(),
        "-c".into(),
        format!(
            "{command} || {{ printf \"\\n%s\\nEnter pra fechar. \" {}; read _; }}",
            shell_quote(message)
        ),
    ]
}
impl Panel {
    fn scan_roots(&self) -> Result<Vec<String>,String> {
        let raw = self.env("CP_SCAN_ROOTS")?;
        let raw = if raw.is_empty() {
            "~/pessoal,~/sistemas"
        } else {
            &raw
        };
        let mut roots = vec![];
        for entry in raw
            .split(',')
            .map(crate::transcript::py::strip)
            .filter(|s| !s.is_empty())
        {
            let expanded = if entry == "~" {
                self.environment.home.clone()
            } else if let Some(rest) = entry.strip_prefix("~/") {
                self.environment.home.join(rest)
            } else {
                PathBuf::from(entry)
            };
            let path = self.realpath(&expanded.to_string_lossy());
            if Path::new(&path).is_dir() && !roots.contains(&path) {
                roots.push(path);
            }
        }
        Ok(roots)
    }
    async fn project_open(&self, name: &str) -> Result<Value, String> {
        let projects = self.api(name, "GET", "/api/projects", None, 8).await?;
        let target = projects
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["name"] == name)
            .and_then(|row| row["tmux"].as_str())
            .unwrap_or("");
        if target.is_empty() {
            return Ok(reply(
                false,
                format!("projeto '{name}' não está no projects.json"),
            ));
        }
        let Some(term) = ["kitty", "foot", "alacritty"]
            .into_iter()
            .find(|name| self.binary(name).is_some())
        else {
            return Ok(reply(
                false,
                "nenhum terminal conhecido (kitty/foot/alacritty)",
            ));
        };
        let live = self
            .command(
                &[
                    "tmux".into(),
                    "-L".into(),
                    "cppkt-run".into(),
                    "has-session".into(),
                    "-t".into(),
                    format!("={target}"),
                ],
                5,
            )
            .map_err(|error| {
                format!("nao consegui perguntar ao tmux se o log de '{name}' existe: {error}")
            })?;
        if live.code != 0 {
            return Ok(reply(
                false,
                format!("a sessao de log de '{name}' nao existe (projeto parado?)"),
            ));
        }
        let args = terminal_command(
            term,
            &[
                "-L".into(),
                "cppkt-run".into(),
                "attach-session".into(),
                "-t".into(),
                format!("={target}"),
            ],
            &format!(
                "hangar-panel-action: a sessao de log de '{name}' ({target}) nao existe mais — o projeto provavelmente nao esta rodando."
            ),
        );
        std::process::Command::new(self.binary(&args[0]).unwrap_or_else(||args[0].clone().into()))
            .args(&args[1..])
            .envs(&self.environment.base)
            .current_dir(&self.cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| format!("nao consegui abrir {term}: {}",spawn_error(&error,term)))?;
        Ok(reply(true, "log aberto no terminal"))
    }
    pub(super) async fn project_action(
        &self,
        name: &str,
        operation: &str,
        rest: &[String],
    ) -> Result<Value, String> {
        let first = rest.first().map(String::as_str).unwrap_or("");
        Ok(match operation {
            "project" => match first {
                "start" => {
                    let mut project = self
                        .api(
                            name,
                            "POST",
                            "/api/projects/{name}/start",
                            Some(json!({})),
                            30,
                        )
                        .await?;
                    { let mut out=reply(true,format!("iniciado ({})",project.get("state").map(codec::python_string).unwrap_or_else(||"?".into())));out["project"]=std::mem::take(&mut project.0);out }
                }
                "stop" => {
                    self.api(
                        name,
                        "POST",
                        "/api/projects/{name}/stop",
                        Some(json!({})),
                        30,
                    )
                    .await?;
                    reply(true, "parado")
                }
                "pane" => {
                    let value = self
                        .api(name, "GET", "/api/projects/{name}/pane", None, 8)
                        .await?;
                    { let mut out=reply(true,"");out["pane"]=value.get("pane").map(codec::clone_value).unwrap_or(json!(""));out }
                }
                "open" => return self.project_open(name).await,
                _ => reply(
                    false,
                    "uso: hangar-panel-action <projeto> project start|stop|pane|open",
                ),
            },
            "project-add" => {
                let mut body = json!({"name":name,"cwd":first,"command":rest.get(1).map(String::as_str).unwrap_or("")});
                if let Some(port) = rest
                    .get(2)
                    .filter(|s| !crate::transcript::py::strip(s).is_empty())
                {
                    let number = crate::orchestration::literal_integer(
                        crate::transcript::py::strip(port),
                    )
                    .ok_or_else(|| {
                        format!(
                            "porta inválida: {}",
                            crate::transcript::py::py_repr(&json!(port))
                        )
                    })?;
                    body["port"] = json!(number);
                }
                let mut project = self
                    .api(name, "POST", "/api/projects", Some(body), 30)
                    .await?;
                { let mut out=reply(true,format!("'{name}' salvo"));out["project"]=std::mem::take(&mut project.0);out }
            }
            "project-del" => {
                self.api(name, "DELETE", "/api/projects/{name}", None, 30)
                    .await?;
                reply(true, format!("'{name}' removido"))
            }
            "import-candidates" => {
                let remote = self.api(name, "GET", "/api/projects", None, 8).await?;
                let Some(projects) = remote.as_array() else {
                    return Ok(reply(false, format!("resposta inesperada de {name}")));
                };
                let roots = self.scan_roots()?;
                let mut candidates = vec![];
                let mut matched_count = 0;
                for project in projects {
                    let cwd = project
                        .get("cwd")
                        .map(codec::python_string)
                        .unwrap_or_default();
                    let command = project
                        .get("command")
                        .map(codec::python_string)
                        .unwrap_or_default();
                    let base = Path::new(cwd.trim_end_matches('/'))
                        .file_name()
                        .unwrap_or_default();
                    let mut hits = vec![];
                    for root in &roots {
                        let candidate =
                            self.realpath(&Path::new(root).join(base).to_string_lossy());
                        if Path::new(&candidate).is_dir() && !hits.contains(&candidate) {
                            hits.push(candidate);
                        }
                    }
                    let matched = hits.len() == 1
                        && !command
                            .split_whitespace()
                            .any(|token| token.starts_with('/') && !Path::new(token).exists());
                    if matched {
                        matched_count += 1;
                    }
                    candidates.push(Value::Object(serde_json::Map::from_iter([("name".into(),codec::clone_value(&project["name"])),("cwd".into(),json!(hits.first().cloned().unwrap_or(cwd))),("command".into(),json!(command)),("port".into(),codec::clone_value(&project["port"])),("matched".into(),json!(matched))])));
                }
                {let mut out=reply(true,format!("{matched_count}/{} casados",candidates.len()));out["candidates"]=Value::Array(candidates);out}
            }
            "pick-folder" => {
                let start = if first.is_empty() {
                    self.scan_roots()?
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| self.environment.home.display().to_string())
                } else {
                    first.into()
                };
                let args = if self.binary("kdialog").is_some() {
                    vec!["kdialog".into(), "--getexistingdirectory".into(), start]
                } else if self.binary("zenity").is_some() {
                    vec![
                        "zenity".into(),
                        "--file-selection".into(),
                        "--directory".into(),
                        format!("--filename={start}/"),
                    ]
                } else {
                    return Ok(reply(
                        false,
                        "nenhum seletor de pasta (instale kdialog ou zenity)",
                    ));
                };
                let result = self.command(&args, 600)?;
                let path = crate::transcript::py::strip(&result.stdout);
                if result.code == 0 && !path.is_empty() {
                    json!({"ok":true,"path":path})
                } else {
                    reply(false, "seleção cancelada")
                }
            }
            "scan-folder" => {
                if first.is_empty() || !Path::new(first).is_dir() {
                    return Ok(reply(false, "pasta inválida"));
                }
                let (existing, failed) = match self.api("x", "GET", "/api/projects", None, 8).await
                {
                    Ok(value) => (
                        value
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|row| codec::clone_value(&row["cwd"]))
                            .collect::<Vec<_>>(),
                        false,
                    ),
                    Err(_) => (vec![], true),
                };
                let mut subdirs = std::fs::read_dir(first)
                    .map_err(|e| e.to_string())?
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        !entry.file_name().to_string_lossy().starts_with('.')
                            && entry.path().is_dir()
                    })
                    .collect::<Vec<_>>();
                subdirs.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());
                let entries=subdirs.into_iter().map(|entry|{let cwd=self.realpath(&entry.path().to_string_lossy());json!({"name":entry.file_name().to_string_lossy(),"cwd":cwd,"command":projection::detect_command(Path::new(&cwd)),"already":existing.contains(&json!(cwd))})}).collect::<Vec<_>>();
                let mut message = format!("{} subpastas em {first}", entries.len());
                if failed {
                    message.push_str(" — não checei duplicados (backend fora?)");
                }
                json!({"ok":true,"entries":entries,"message":message})
            }
            _ => unreachable!(),
        })
    }
}

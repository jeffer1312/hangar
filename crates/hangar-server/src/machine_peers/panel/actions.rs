use super::*;
use futures_util::future::join_all;
fn result(ok: bool, message: impl Into<String>) -> Value {
    json!({"ok":ok,"message":message.into()})
}
fn list(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or(&[])
}
impl Panel {
    pub async fn action(&self, args: &[String]) -> (u8, Value) {
        if args.len() < 2 {
            return (
                2,
                result(
                    false,
                    "uso: hangar-panel-action <endereço> <operação> [args]",
                ),
            );
        }
        let output = self
            .action_result(&args[0], &args[1], &args[2..])
            .await
            .unwrap_or_else(|e| result(false, e));
        (if truthy(&output["ok"]) { 0 } else { 1 }, output)
    }
    async fn action_result(
        &self,
        address: &str,
        operation: &str,
        rest: &[String],
    ) -> Result<Value, String> {
        let first = rest.first().map(String::as_str).unwrap_or("");
        Ok(match operation {
            "rename" => {
                if crate::transcript::py::strip(first).is_empty() {
                    result(false, "nome vazio")
                } else {
                    let value = self
                        .api(
                            address,
                            "POST",
                            "/api/sessions/{name}/rename",
                            Some(json!({"new":first})),
                            8,
                        )
                        .await?;
                    result(
                        true,
                        format!(
                            "renomeada para {}",
                            value
                                .get("name")
                                .map(codec::python_string)
                                .unwrap_or_else(|| first.into())
                        ),
                    )
                }
            }
            "kill" => {
                self.api(address, "DELETE", "/api/sessions/{name}", None, 8)
                    .await?;
                result(true, "sessão encerrada")
            }
            "push" => {
                self.api(address, "POST", "/api/sessions/{name}/git/push", None, 30)
                    .await?;
                result(true, "push feito")
            }
            "git" => {
                let action = if first.is_empty() { "status" } else { first };
                let value = self
                    .api(
                        address,
                        "POST",
                        "/api/sessions/{name}/git",
                        Some(json!({"action":action})),
                        8,
                    )
                    .await?;
                let lines = value["output"]
                    .as_str()
                    .unwrap_or("")
                    .lines()
                    .filter(|line| !crate::transcript::py::strip(line).is_empty())
                    .collect::<Vec<_>>();
                let ok = truthy(&value["ok"]);
                let mut message = lines.first().copied().unwrap_or(action).to_owned();
                if ok && action == "status" && lines.len() > 1 {
                    message.push_str(&format!(" · {} arquivo(s)", lines.len() - 1));
                }
                result(ok, message.chars().take(140).collect::<String>())
            }
            "info" => {
                let sessions = self.api(address, "GET", "/api/sessions", None, 8).await?;
                let name = address.rsplit("::").next().unwrap_or(address);
                let branch = list(&sessions)
                    .iter()
                    .find(|row| row["name"] == name)
                    .map(|row| codec::clone_value(&row["branch"]))
                    .filter(truthy)
                    .unwrap_or(json!("?"));
                match self
                    .api(address, "GET", "/api/sessions/{name}/git/files", None, 8)
                    .await
                {
                    Ok(files) => {
                        let count = if files.is_array() {
                            list(&files).len()
                        } else {
                            list(&files["files"]).len()
                        };
                        { let message=format!("{} · {count} modificado(s)",codec::python_string(&branch));let mut out=serde_json::Map::new();out.insert("ok".into(),json!(true));out.insert("branch".into(),branch);out.insert("changed".into(),json!(count));out.insert("git".into(),json!(true));out.insert("message".into(),json!(message));Value::Object(out) }
                    }
                    Err(error) if error.contains("not a git repository") => {
                        json!({"ok":true,"branch":"","changed":0,"git":false,"message":"sem repositório git"})
                    }
                    Err(error) => return Err(error),
                }
            }
            "commit" | "commit-push" => {
                let files = self
                    .api(address, "GET", "/api/sessions/{name}/git/files", None, 8)
                    .await?;
                let rows = if files.is_array() {
                    list(&files)
                } else {
                    list(&files["files"])
                };
                let paths = rows
                    .iter()
                    .map(|row| {
                        if row.is_object() {
                            codec::clone_value(&row["path"])
                        } else {
                            codec::clone_value(row)
                        }
                    })
                    .collect::<Vec<_>>();
                if paths.is_empty() {
                    return Ok(result(false, "nada modificado pra commitar"));
                }
                let message = rest.join(" ");
                let count = paths.len();
                self.api(
                    address,
                    "POST",
                    "/api/sessions/{name}/git/commit",
                    Some(
                        Value::Object(serde_json::Map::from_iter([("message".into(),json!(if message.is_empty(){"wip"}else{&message})),("paths".into(),Value::Array(paths))])),
                    ),
                    30,
                )
                .await?;
                if operation == "commit-push" {
                    match self
                        .api(address, "POST", "/api/sessions/{name}/git/push", None, 30)
                        .await
                    {
                        Ok(_) => result(true, format!("commit de {count} arquivo(s) + push")),
                        Err(error) => result(
                            false,
                            format!("commit local OK ({count} arquivo(s)), push falhou: {error}"),
                        ),
                    }
                } else {
                    result(true, format!("commit de {count} arquivo(s)"))
                }
            }
            "peer" => {
                if !matches!(first, "on" | "off") {
                    return Ok(result(
                        false,
                        "uso: hangar-panel-action <servidor> peer on|off",
                    ));
                }
                book_write::toggle(self,address,first=="on")?;
                result(
                    true,
                    format!(
                        "{address} {}",
                        if first == "on" { "ligado" } else { "desligado" }
                    ),
                )
            }
            "project" | "project-add" | "project-del" | "import-candidates" | "pick-folder"
            | "scan-folder" => return self.project_action(address, operation, rest).await,
            _ => result(false, format!("operação desconhecida: {operation}")),
        })
    }
    pub async fn snapshot(&self) -> Value {self.snapshot_result().await.unwrap_or_else(|message|json!({"ok":false,"message":message}))}
    async fn snapshot_result(&self) -> Result<Value,String> {
        let mut errors = vec![];
        let peers = match self.peers() {
            Ok(peers) => peers,
            Err(error) => {
                errors.push(json!(error));
                ValueGuard(json!({}))
            }
        };
        let mut targets = vec![(
            None,
            self.local_base()?,
            self.env("CP_AUTH_TOKEN")?,
            String::new(),
        )];
        let mut servers = vec![];
        for (id, entry) in peers.as_object().unwrap() {
            if entry.is_object() && entry.get("app").is_some() && !truthy(&entry["base_url"])
            {
                continue;
            }
            let on = projection::enabled(&entry);
            let mut row = json!({"id":id,"enabled":on,"ok":false,"error":""});
            if on {
                let (base, token, web) = projection::fields(&entry);
                if base.is_empty() || token.is_empty() {
                    errors.push(json!(format!(
                        "{id}: entrada sem base_url/token válidos no peers.json"
                    )));
                    row["error"] = json!("entrada inválida no peers.json");
                } else {
                    targets.push((
                        Some(id.clone()),
                        base.clone(),
                        token,
                        if web.is_empty() { base } else { web },
                    ));
                }
            }
            servers.push(row);
        }
        let attached = self
            .command(
                &[
                    "tmux".into(),
                    "list-clients".into(),
                    "-F".into(),
                    "#{client_control_mode}\t#{session_name}".into(),
                ],
                3,
            )
            .ok()
            .filter(|out| out.code == 0)
            .map(|out| projection::attached(&out.stdout))
            .unwrap_or_default();
        let attached = &attached;
        let futures = targets.iter().map(|(server, base, token, web)| async move {
            let response = self
                .transport
                .request(base, token, "GET", "/api/sessions", None, 4)
                .await;
            let rows = response.map(ValueGuard).and_then(|value| {
                value
                    .as_array()
                    .ok_or_else(|| "resposta de sessões não é uma lista".into())
                    .and_then(|rows| {
                        rows.iter()
                            .map(|row| self.slim(row, server.as_deref(), web, &attached))
                            .collect::<Result<Vec<_>, _>>()
                    })
            });
            (server, rows)
        });
        let base = self.local_base()?;
        let token = self.env("CP_AUTH_TOKEN")?;
        let (rows, projects) = tokio::join!(
            join_all(futures),
            self.transport
                .request(&base, &token, "GET", "/api/projects", None, 4)
        );
        let mut sessions = vec![];
        for (server, rows) in rows {
            let error = match rows {
                Ok(rows) => {
                    sessions.extend(rows);
                    String::new()
                }
                Err(error) => {
                    errors.push(json!(format!(
                        "{}: {error}",
                        server.as_deref().unwrap_or("local")
                    )));
                    error
                }
            };
            if let Some(server) = server
                && let Some(row) = servers.iter_mut().find(|row| row["id"] == *server)
            {
                row["ok"] = json!(error.is_empty());
                row["error"] = json!(error);
            }
        }
        sessions.sort_by_key(|row| {
            let server = row["server"].as_str();
            let rank = match row["state"].as_str() {
                Some("awaiting_input") => 0,
                Some("working") => 1,
                _ => 2,
            };
            (
                server.is_some(),
                server.unwrap_or("").to_owned(),
                rank,
                row["name"].as_str().unwrap_or("").to_owned(),
            )
        });
        let mut projects = match projects {
            Ok(Value::Array(rows)) => rows,
            Ok(value) => {codec::discard(value);vec![]},
            Err(error) => {
                errors.push(json!(format!("projetos: {error}")));
                vec![]
            }
        };
        for project in &mut projects {
            if project.is_object() {
                let cwd = project["cwd"].as_str().unwrap_or("");
                let resolved = json!(if cwd.is_empty() {
                    String::new()
                } else {
                    self.realpath(cwd)
                });
                codec::discard(std::mem::replace(&mut project["cwd_real"],resolved));
            }
        }
        Ok(Value::Object(serde_json::Map::from_iter([("sessions".into(),Value::Array(sessions)),("errors".into(),Value::Array(errors)),("servers".into(),Value::Array(servers)),("projects".into(),Value::Array(projects))])))
    }
    pub fn slim(
        &self,
        row: &Value,
        server: Option<&str>,
        web: &str,
        attached: &std::collections::BTreeSet<String>,
    ) -> Result<Value, String> {
        if !row.is_object() {
            return Err("sessão não é um objeto".into());
        }
        let name = row["name"].as_str().ok_or("sessão sem nome")?;
        let mut out = serde_json::Map::new();
        for key in projection::KEEP {
            out.insert((*key).into(), codec::clone_value(&row[*key]));
        }
        let cwd = row["cwd"].as_str().unwrap_or("");
        out.extend(json!({"server":server,"address":server.map(|id|format!("{id}::{name}")).unwrap_or_else(||name.into()),"local":server.is_none(),"web_url":if web.is_empty(){String::new()}else{format!("{}/#/{name}",web.trim_end_matches('/'))},"attached":server.is_none()&&attached.contains(name),"group":server.unwrap_or("local"),"cwd_real":if cwd.is_empty(){String::new()}else{self.realpath(cwd)}}).as_object().unwrap().clone());
        Ok(Value::Object(out))
    }
}

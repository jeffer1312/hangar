use super::*;
use std::fs;
pub const DISABLED_CONTEXT_IDS: [&str; 2] = [
    "context-file:project:AGENTS.md",
    "context-file:user:AGENTS.md",
];
fn rule_info(content: &str) -> (String, bool) {
    let text = content.replace("\r\n", "\n").replace('\r', "\n");
    static COMMENTS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"<!--[\s\S]*?-->").unwrap());
    let comments = &*COMMENTS;
    let text = comments.replace_all(&text, "");
    static FRONTMATTER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\A---[ \t]*\n([\s\S]*?)\n---[ \t]*(?:\n|$)([\s\S]*)\z").unwrap()
    });
    let re = &*FRONTMATTER;
    let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some(c) = re.captures(&text) else {
        return (normalize(&text), false);
    };
    let body = normalize(&c[2]);
    let Ok(v) = serde_saphyr::from_str::<Value>(&c[1]) else {
        return (body, false);
    };
    let Some(m) = v.as_object() else {
        return (body, false);
    };
    let mut values = BTreeMap::new();
    static KEBAB: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"-([a-z])").unwrap());
    let kebab = &*KEBAB;
    for (k, v) in m {
        let k = kebab
            .replace_all(k, |c: &regex::Captures| c[1].to_uppercase())
            .to_string();
        if values.insert(k, v).is_some() {
            return (body, false);
        }
    }
    if values.get("alwaysApply") != Some(&&Value::Bool(true)) {
        return (body, false);
    }
    for (name, v) in values {
        let valid = match name.as_str() {
            "alwaysApply" | "enabled" => *v == Value::Bool(true),
            "name" | "description" => v.is_string() || v.is_null(),
            _ => {
                v.is_null()
                    || v == ""
                    || v.as_array().is_some_and(Vec::is_empty)
                    || v.as_object().is_some_and(Map::is_empty)
            }
        };
        if !valid {
            return (body, false);
        }
    }
    (body, true)
}
fn points_to(p: &Path, target: &Path) -> bool {
    if !io::symlink(p) {
        return false;
    }
    let Ok(link) = fs::read_link(p) else {
        return false;
    };
    let linked = if link.is_absolute() {
        link
    } else {
        p.parent().unwrap_or(Path::new(".")).join(link)
    };
    resolved(&linked) == resolved(target)
}
fn resolved(p: &Path) -> PathBuf {
    if let Ok(p) = p.canonicalize() {
        return p;
    }
    let mut tail = Vec::new();
    let mut prefix = p;
    while !prefix.exists() {
        if let Some(name) = prefix.file_name() {
            tail.push(name.to_os_string());
        }
        let Some(parent) = prefix.parent() else {
            return p.to_owned();
        };
        if parent == prefix {
            break;
        }
        prefix = parent;
    }
    let mut result = prefix.canonicalize().unwrap_or_else(|_| prefix.to_owned());
    for part in tail.into_iter().rev() {
        result.push(part);
    }
    result
}
fn preflight(
    agent: &Path,
    global: &Path,
    template: &Path,
    body: &str,
) -> Result<Option<PathBuf>, String> {
    let append = agent.join("APPEND_SYSTEM.md");
    let rules = agent.join("rules");
    let canonical = rules.join(template.file_name().ok_or("OSError")?);
    if (append.exists() || io::symlink(&append)) && !points_to(&append, global) {
        return Err(err(
            "APPEND_SYSTEM.md personalizado preservado; configuração não aplicada",
        ));
    }
    if io::symlink(&rules) || (rules.exists() && !rules.is_dir()) {
        return Err(err("Diretório de regras externo ou inválido preservado"));
    }
    let mut entries: BTreeMap<String, Vec<(PathBuf, String, bool)>> = BTreeMap::new();
    let mut eligible = vec![];
    let mut restricted = false;
    if rules.is_dir() {
        let mut files = fs::read_dir(&rules)
            .map_err(io::os_error)?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(io::os_error)?;
        files.sort();
        for p in files {
            if !p.is_file() || !matches!(p.extension().and_then(|e| e.to_str()), Some("md" | "mdc"))
            {
                continue;
            }
            let text = fs::read_to_string(&p).map_err(io::os_error)?;
            let (b, active) = rule_info(&text);
            entries
                .entry(stem(&p))
                .or_default()
                .push((p.clone(), b.clone(), active));
            if b == body {
                if active {
                    eligible.push(p)
                } else {
                    restricted = true
                }
            }
        }
    }
    if let Some(selected) = eligible.first() {
        let names = eligible.iter().map(|p| stem(p)).collect::<BTreeSet<_>>();
        if names.len() != 1 {
            return Err(err(
                "Mais de uma identidade de regra equivalente; política não aplicada",
            ));
        }
        if entries[&stem(selected)]
            .iter()
            .any(|(_, b, a)| b != body || !*a)
        {
            return Err(err(
                "Identidade da regra disputada por outro arquivo; preservada",
            ));
        }
        return Ok(Some(selected.clone()));
    }
    if restricted {
        return Err(err(
            "Regra equivalente desativada ou restrita; preferência preservada",
        ));
    }
    if canonical.exists() || io::symlink(&canonical) || entries.contains_key(&stem(&canonical)) {
        return Err(err(
            "Regra de contexto personalizada preservada; configuração não aplicada",
        ));
    }
    Ok(None)
}
fn stem(p: &Path) -> String {
    p.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
async fn command(
    s: &Synchronizer,
    action: &str,
    key: &str,
    value: Option<&Value>,
) -> Result<Value, String> {
    let mut args = vec!["omp".into(), "config".into(), action.into(), key.into()];
    if let Some(v) = value {
        args.push(v.to_string())
    }
    args.push("--json".into());
    let out = s
        .runner
        .run(args, &s.home, &s.env, Duration::from_secs(30))
        .await?;
    if out.code != 0 {
        return Err(format!(
            "Configuração nativa OMP falhou (código {})",
            out.code
        ));
    }
    let v: Value = serde_json::from_str(&out.stdout).map_err(|_| err("JSONDecodeError"))?;
    let values = &v["value"];
    if !values
        .as_array()
        .is_some_and(|a| a.iter().all(Value::is_string))
    {
        return Err(err("Controle nativo de regras inválido"));
    }
    Ok(values.clone())
}
fn blocked(controls: &Value, name: &str) -> bool {
    controls["disabledProviders"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v == "native"))
        || controls["disabledExtensions"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == &format!("rule:{name}")))
        || controls["ttsr.disabledRules"].as_array().is_some_and(|a| {
            a.iter()
                .any(|v| v.as_str().is_some_and(|s| s.trim() == name))
        })
}
/// Configuração opt-in feita pelo CLI nativo, preservando arquivos e bloqueios pessoais.
pub async fn configure(s: &Synchronizer, template: &Path, enabled: bool) -> Value {
    let mut result = json!({"enabled":enabled,"changed":false,"items":[],"errors":[]});
    if !enabled {
        return result;
    }
    let action:Result<(),String>=async{
  let agent=s.directories()?.agent_dir;let global=s.claude_dir.join("CLAUDE.md");let append=agent.join("APPEND_SYSTEM.md");let rules=agent.join("rules");let canonical=rules.join(template.file_name().ok_or("OSError")?);let (body,valid)=rule_info(&fs::read_to_string(template).map_err(io::os_error)?);if !valid{return Err(err("Template de contexto inválido"))}preflight(&agent,&global,template,&body)?;fs::create_dir_all(&agent).map_err(io::os_error)?;let lock=agent.join(".hangar-claude-context.lock");if io::symlink(&lock){return Err(err("Trava simbólica de configuração recusada"))}let _guard=io::lock(&lock).await?;preflight(&agent,&global,template,&body)?;
  let (a,b,c)=tokio::join!(command(s,"get","disabledExtensions",None),command(s,"get","ttsr.disabledRules",None),command(s,"get","disabledProviders",None));let mut controls=json!({"disabledExtensions":a?,"ttsr.disabledRules":b?,"disabledProviders":c?});let mut selected=preflight(&agent,&global,template,&body)?;let name=stem(selected.as_deref().unwrap_or(&canonical));if blocked(&controls,&name){return Err(err("Política bloqueada pela configuração pessoal; bloqueio preservado"))}let previous=controls["disabledExtensions"].clone();let mut merged=previous.as_array().unwrap().clone();for id in DISABLED_CONTEXT_IDS{if !merged.iter().any(|v|v==id){merged.push(json!(id))}}let merged=json!(merged);if merged!=previous{if command(s,"set","disabledExtensions",Some(&merged)).await?!=merged{return Err(err("CLI não confirmou a configuração solicitada"))}result["changed"]=json!(true)}controls["disabledExtensions"]=command(s,"get","disabledExtensions",None).await?;selected=preflight(&agent,&global,template,&body)?;if blocked(&controls,&stem(selected.as_deref().unwrap_or(&canonical))){return Err(err("Política bloqueada pela configuração pessoal; bloqueio preservado"))}if DISABLED_CONTEXT_IDS.iter().any(|id|!controls["disabledExtensions"].as_array().unwrap().iter().any(|v|v==id)){return Err(err("Configuração de contexto mudou durante a operação"))}
  result["global_context"]=json!(if global.is_file(){"available"}else{"missing"});if global.is_file()&&!io::symlink(&append){preflight(&agent,&global,template,&body)?;symlink(&global,&append)?;result["changed"]=json!(true)}if selected.is_none(){preflight(&agent,&global,template,&body)?;fs::create_dir_all(&rules).map_err(io::os_error)?;if preflight(&agent,&global,template,&body)?.is_some(){return Err(err("Regra mudou durante a configuração"))}symlink(template,&canonical)?;selected=Some(canonical.clone());result["changed"]=json!(true)}let effective=preflight(&agent,&global,template,&body)?;if effective.as_ref().map(|p|stem(p))!=selected.as_ref().map(|p|stem(p)){return Err(err("Regra efetiva mudou durante a configuração"))}let selected=selected.unwrap();result["items"]=json!([{"item":"context-files","state":"configured"},{"item":"rule","state":if selected==canonical{"configured"}else{"reused"},"name":stem(&selected)}]);Ok(())
 }.await;
    if let Err(e) = action {
        result["errors"].as_array_mut().unwrap().push(json!(e))
    }
    result
}
fn symlink(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).map_err(io::os_error)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link).map_err(io::os_error)
    }
}

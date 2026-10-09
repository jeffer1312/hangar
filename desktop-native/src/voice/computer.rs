//! Ferramenta `computer` da voz: o MCP do hangar-computer-control (HCC) em stdio, só para programas fora do Hangar.
use super::rpc::{Rpc, RpcError};
use serde_json::{Value, json};
use std::{ffi::OsString, path::{Path, PathBuf}, time::Duration};

/// O `objetivo` do HCC desiste sozinho em 240 s; a folga cobre a subida do servidor e do agente.
const OBJECTIVE_DEADLINE: Duration = Duration::from_secs(300);
pub const JEV_KEY: &str = "TYPESAFE_API_KEY";
/// Só o fallback com imagem usa; sem elas o laço segue pela árvore de acessibilidade.
const OPTIONAL_KEYS: [&str; 4] = ["LLM_PROXY_KEY", "LLM_PROXY_URL", "LLM_MODEL", "LLM_EFFORT"];
const ENTRY: &str = "hangar-computer-control";

pub struct Launch { pub program: PathBuf, pub args: Vec<OsString>, pub env: Vec<(String, OsString)> }

/// Sem pasta do usuário os caminhos viravam relativos à pasta do app: é erro, não `""`.
fn resolve_home(home: Option<PathBuf>) -> Result<PathBuf, String> {
    home.filter(|h| h.is_absolute()).ok_or_else(|| "Não achei a pasta do usuário (HOME/USERPROFILE).".to_owned())
}

fn home() -> Result<PathBuf, String> { resolve_home(std::env::home_dir()) }

/// Arquivo ausente é normal; ilegível ou quebrado vai ao log (só nome e tipo do erro), senão a falta da chave engana.
fn read_json(path: &Path) -> Value {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| { super::log(format!("computer {name} unreadable json kind={:?}", e.classify())); Value::Null }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => { super::log(format!("computer {name} unreadable kind={:?}", e.kind())); Value::Null }
    }
}

/// A entrada que o Hangar registrou: a ativa no `~/.claude.json`, senão a guardada ao desligar.
fn registered_entry(home: &Path) -> Option<Value> {
    let active = read_json(&home.join(".claude.json"))["mcpServers"][ENTRY].clone();
    let parked = || read_json(&home.join(".hangar").join("computer-control.json"));
    let usable = |e: &Value| e["command"].as_str().is_some_and(|c| !c.is_empty());
    Some(active).filter(usable).or_else(|| Some(parked()).filter(usable))
}

/// No Windows o agente desta máquina é o primeiro alvo `local` que não seja o do Linux; nunca o remoto das sessões.
pub fn pick_local_target(configs: &[(String, Value)]) -> Option<&str> {
    configs.iter().find(|(name, config)| name != "linux-agent.json" && config["transport"] == "local").map(|(name, _)| name.as_str())
}

/// A entrada das sessões aponta o `HCC_AGENT_CONFIG` para o alvo delas (às vezes uma VM); a voz troca pelo desta máquina.
fn local_agent(env: &mut Vec<(String, OsString)>, windows: bool) -> Result<(), String> {
    let configured = env.iter().find(|(k, _)| k == "HCC_AGENT_CONFIG").map(|(_, v)| PathBuf::from(v));
    env.retain(|(k, _)| k != "HCC_AGENT_CONFIG");
    // Vazio, não ausente: o filho herdaria o do app; o HCC trata vazio como não definido e cai no alvo automático.
    if !windows { env.push(("HCC_AGENT_CONFIG".to_owned(), OsString::new())); return Ok(()); }
    let dir = env.iter().find(|(k, v)| k == "HCC_AGENTS_DIR" && !v.is_empty()).map(|(_, v)| PathBuf::from(v))
        .or_else(|| configured.as_deref().and_then(Path::parent).filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf))
        .ok_or("Sem pasta de alvos do controle do computador (HCC_AGENTS_DIR). Ative em Configurações > Controle do Windows.")?;
    let mut configs: Vec<(String, Value)> = std::fs::read_dir(&dir).map_err(|e| format!("Não consegui ler {}: {e}", dir.display()))?
        .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with("-agent.json"))
        .map(|name| { let config = read_json(&dir.join(&name)); (name, config) }).collect();
    configs.sort_by(|a, b| a.0.cmp(&b.0));
    let name = pick_local_target(&configs)
        .ok_or_else(|| format!("Sem configuração de agente local em {} (um *-agent.json com transport local).", dir.display()))?;
    env.push(("HCC_AGENT_CONFIG".to_owned(), dir.join(name).into_os_string()));
    Ok(())
}

/// Roda o comando da entrada como está (binário ou uvx/python antigo); só a chave do Jev ganha os fallbacks do backend.
pub fn launch_from(home: &Path, windows: bool, process_env: impl Fn(&str) -> Option<String>) -> Result<Launch, String> {
    let entry = registered_entry(home).ok_or("O controle do computador não está registrado. Ative em Configurações > Controle do Windows.")?;
    let program = PathBuf::from(entry["command"].as_str().unwrap_or_default());
    let args = entry["args"].as_array().map(|a| a.iter().map(|v| v.as_str().map(OsString::from)).collect::<Option<Vec<_>>>()).unwrap_or(Some(vec![]))
        .ok_or("A entrada do hangar-computer-control tem argumento que não é texto. Ative de novo em Configurações > Controle do Windows.")?;
    // O env da entrada vai como está: vazio é de propósito (VIRTUAL_ENV); só a chave do Jev trata vazio como ausente.
    let mut env: Vec<(String, OsString)> = entry["env"].as_object().map(|vars| vars.iter()
        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), OsString::from(v)))).collect()).unwrap_or_default();
    let has = |env: &[(String, OsString)], key: &str| env.iter().any(|(k, v)| k == key && !v.is_empty());
    if !has(&env, JEV_KEY) {
        let settings = read_json(&home.join(".claude").join("settings.json"))["env"][JEV_KEY].as_str().map(str::to_owned);
        let key = settings.filter(|v| !v.is_empty()).or_else(|| process_env(JEV_KEY).filter(|v| !v.is_empty()))
            .ok_or_else(|| format!("Falta a chave {JEV_KEY} (Jev): configure em Configurações > Controle do Windows ou no ambiente do app."))?;
        env.retain(|(k, _)| k != JEV_KEY);
        env.push((JEV_KEY.to_owned(), key.into()));
    }
    local_agent(&mut env, windows)?;
    let missing: Vec<&str> = OPTIONAL_KEYS.into_iter().filter(|k| !has(&env, k) && process_env(k).is_none_or(|v| v.is_empty())).collect();
    if !missing.is_empty() { super::log(format!("computer optional keys missing: {}", missing.join(","))); }
    Ok(Launch { program, args, env })
}

/// Bloqueia (lê arquivos): quem chama faz fora da thread da tela.
pub fn launch() -> Result<Launch, String> { launch_from(&home()?, cfg!(windows), |key| std::env::var(key).ok()) }

pub fn initialize_params() -> Value {
    json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hangar-native-voice", "version": "1"}})
}

pub fn objective_params(objective: &str) -> Value { json!({"name": "objetivo", "arguments": {"texto": objective}}) }

/// Texto do `tools/call`; `isError` ou resposta vazia vira `Err`.
pub fn call_text(result: &Value) -> Result<String, String> {
    let text: String = result["content"].as_array().map(|parts| parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    if result["isError"] == true { Err(if text.is_empty() { "O hangar-computer-control falhou sem dizer o motivo.".into() } else { text }) }
    // Sem texto não há o que relatar ao usuário: tratar como sucesso diria "feito" sem prova.
    else if text.trim().is_empty() { Err("O controle do computador respondeu sem resultado".into()) } else { Ok(text) }
}

fn rpc_text(step: &str, error: RpcError) -> String {
    match error {
        RpcError::Spawn => "Não consegui iniciar o hangar-computer-control.".into(),
        RpcError::Closed => format!("O hangar-computer-control fechou durante {step}."),
        RpcError::Timeout => format!("O hangar-computer-control não respondeu a tempo ({step})."),
        RpcError::Server(message) => format!("O hangar-computer-control recusou {step}: {message}"),
    }
}

/// O receptor volta junto: largado, o leitor do Rpc para na primeira notificação e as respostas nunca chegam.
async fn session(launch: &Launch) -> Result<(Rpc, async_channel::Receiver<super::rpc::Incoming>), String> {
    let args: Vec<&std::ffi::OsStr> = launch.args.iter().map(OsString::as_os_str).collect();
    let env: Vec<(&str, OsString)> = launch.env.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let (rpc, incoming) = Rpc::spawn_program(&launch.program, &args, &env).await.map_err(|e| rpc_text("a subida", e))?;
    rpc.request("initialize", initialize_params()).await.map_err(|e| rpc_text("initialize", e))?;
    rpc.notify("notifications/initialized", json!({})).await.map_err(|e| rpc_text("initialize", e))?;
    Ok((rpc, incoming))
}

/// Um processo por objetivo: largar o future (parar a chamada) derruba o servidor e o agente dele (grupo/árvore no Drop do Rpc).
pub async fn run_objective(launch: Launch, objective: &str) -> Result<String, String> {
    let (rpc, _incoming) = session(&launch).await?;
    let result = rpc.request_within("tools/call", objective_params(objective), OBJECTIVE_DEADLINE).await.map_err(|e| rpc_text("o objetivo", e))?;
    call_text(&result)
}

pub async fn list_tools(launch: Launch) -> Result<Vec<String>, String> {
    let (rpc, _incoming) = session(&launch).await?;
    let result = rpc.request("tools/list", json!({})).await.map_err(|e| rpc_text("tools/list", e))?;
    Ok(result["tools"].as_array().map(|t| t.iter().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    fn home_with(name: &str, files: &[(&str, Value)]) -> PathBuf {
        let home = std::env::temp_dir().join(format!("hangar-computer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        for (rel, content) in files {
            let path = home.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content.to_string()).unwrap();
        }
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    fn active(entry: Value) -> (&'static str, Value) { (".claude.json", json!({"mcpServers": {"hangar-computer-control": entry}})) }
    fn no_env(_: &str) -> Option<String> { None }
    fn get<'a>(launch: &'a Launch, key: &str) -> Option<&'a OsString> { launch.env.iter().find(|(k, _)| k == key).map(|(_, v)| v) }

    #[test]
    fn launch_uses_active_entry_command_args_env() {
        let home = home_with("active", &[active(json!({"command": "/opt/hcc/hangar-computer-control", "args": ["--x"],
            "env": {JEV_KEY: "k", "LLM_MODEL": "m", "HCC_AGENTS_DIR": "/t"}})),
            (".hangar/computer-control.json", json!({"command": "/parked", "args": [], "env": {JEV_KEY: "p"}}))]);
        let launch = launch_from(&home, false, no_env).unwrap();
        assert_eq!(launch.program, PathBuf::from("/opt/hcc/hangar-computer-control"));
        assert_eq!(launch.args, vec![OsString::from("--x")]);
        assert_eq!(get(&launch, JEV_KEY), Some(&OsString::from("k")));
        assert_eq!(get(&launch, "LLM_MODEL"), Some(&OsString::from("m")));
        assert_eq!(get(&launch, "HCC_AGENTS_DIR"), Some(&OsString::from("/t")));
    }

    #[test]
    fn falls_back_to_parked_entry() {
        let home = home_with("parked", &[(".claude.json", json!({"mcpServers": {}})),
            (".hangar/computer-control.json", json!({"command": "/parked/hangar-computer-control", "args": [], "env": {JEV_KEY: "p"}}))]);
        let launch = launch_from(&home, false, no_env).unwrap();
        assert_eq!(launch.program, PathBuf::from("/parked/hangar-computer-control"));
        assert!(launch.args.is_empty());
        assert_eq!(get(&launch, JEV_KEY), Some(&OsString::from("p")));
    }

    #[test]
    fn legacy_uvx_entry_launches_as_is() {
        let args = ["--from", "git+https://example.invalid/hcc@v0.1.1", "hangar-computer-control"];
        let home = home_with("legacy", &[active(json!({"command": "/usr/bin/uvx", "args": args, "env": {JEV_KEY: "k", "VIRTUAL_ENV": ""}}))]);
        let launch = launch_from(&home, false, no_env).unwrap();
        assert_eq!(launch.program, PathBuf::from("/usr/bin/uvx"));
        assert_eq!(get(&launch, "VIRTUAL_ENV"), Some(&OsString::new()), "vazio de propósito: não herda o venv do app");
        assert_eq!(launch.args, args.map(OsString::from).to_vec());
    }

    #[test]
    fn no_python_paths_in_env() {
        let home = home_with("nopython", &[active(json!({"command": "/opt/hcc/hangar-computer-control", "args": [], "env": {JEV_KEY: "k"}}))]);
        let launch = launch_from(&home, false, no_env).unwrap();
        assert!(get(&launch, "PYTHONPATH").is_none() && get(&launch, "VIRTUAL_ENV").is_none(), "{:?}", launch.env);
    }

    #[test]
    fn linux_without_agent_config_relies_on_automatic_target() {
        let home = home_with("linuxagent", &[active(json!({"command": "/opt/hcc/hangar-computer-control", "args": [],
            "env": {JEV_KEY: "k", "HCC_AGENT_CONFIG": "/t/vm-a-agent.json", "HCC_AGENTS_DIR": "/t"}}))]);
        let launch = launch_from(&home, false, no_env).unwrap();
        assert_eq!(get(&launch, "HCC_AGENT_CONFIG"), Some(&OsString::new()), "vazio: nem o alvo remoto da entrada nem o herdado do app");
        assert_eq!(launch.env.iter().filter(|(k, _)| k == "HCC_AGENT_CONFIG").count(), 1);
        assert_eq!(get(&launch, "HCC_AGENTS_DIR"), Some(&OsString::from("/t")));
    }

    #[test]
    fn windows_points_agent_config_to_the_local_target() {
        let targets = home_with("wintargets", &[("vm-a-agent.json", json!({"transport": "ssh"})),
            ("linux-agent.json", json!({"transport": "local"})), ("pc-agent.json", json!({"transport": "local"}))]);
        let entry = |dir: &Path| active(json!({"command": "C:/hcc/windows-agent.exe", "args": [],
            "env": {JEV_KEY: "k", "HCC_AGENT_CONFIG": dir.join("vm-a-agent.json").to_string_lossy(), "HCC_AGENTS_DIR": dir.to_string_lossy()}}));
        let launch = launch_from(&home_with("winlocal", &[entry(&targets)]), true, no_env).unwrap();
        assert_eq!(get(&launch, "HCC_AGENT_CONFIG"), Some(&targets.join("pc-agent.json").into_os_string()));
        assert_eq!(launch.env.iter().filter(|(k, _)| k == "HCC_AGENT_CONFIG").count(), 1);
        let remote_only = home_with("winremote", &[("vm-a-agent.json", json!({"transport": "ssh"}))]);
        let error = launch_from(&home_with("winnolocal", &[entry(&remote_only)]), true, no_env).err().unwrap();
        assert!(error.starts_with("Sem configuração de agente local em"), "{error}");
    }

    #[test]
    fn agent_config_is_this_machine() {
        let configs = vec![("vm-a-agent.json".to_owned(), json!({"transport": "ssh"})), ("linux-agent.json".to_owned(), json!({"transport": "local"})),
            ("pc-agent.json".to_owned(), json!({"transport": "local"}))];
        assert_eq!(pick_local_target(&configs), Some("pc-agent.json"));
        assert_eq!(pick_local_target(&configs[..1]), None, "alvo remoto nunca é o desta máquina");
    }

    #[test]
    fn missing_entry_error_points_to_settings() {
        let error = launch_from(&home_with("noentry", &[]), false, no_env).err().unwrap();
        assert!(error.contains("Ative em Configurações > Controle do Windows"), "{error}");
    }

    #[test]
    fn jev_key_falls_back_to_settings_then_process_env_and_is_named_when_missing() {
        let entry = active(json!({"command": "/opt/hcc/hangar-computer-control", "args": [], "env": {JEV_KEY: ""}}));
        let home = home_with("jevsettings", &[entry.clone(), (".claude/settings.json", json!({"env": {JEV_KEY: "s"}}))]);
        assert_eq!(get(&launch_from(&home, false, no_env).unwrap(), JEV_KEY), Some(&OsString::from("s")));
        let home = home_with("jevprocess", &[entry]);
        let from_process = launch_from(&home, false, |k| (k == JEV_KEY).then(|| "e".to_owned())).unwrap();
        assert_eq!(from_process.env.iter().filter(|(k, _)| k == JEV_KEY).count(), 1);
        assert_eq!(get(&from_process, JEV_KEY), Some(&OsString::from("e")));
        let missing = launch_from(&home, false, no_env).err().unwrap();
        assert!(missing.starts_with("Falta a chave TYPESAFE_API_KEY (Jev)"), "{missing}");
    }

    #[test]
    fn mcp_requests_have_the_hcc_shape() {
        let init = initialize_params();
        assert_eq!(init["protocolVersion"], json!("2025-06-18"));
        assert!(init["capabilities"].is_object() && init["clientInfo"]["name"].is_string());
        assert_eq!(objective_params("abrir o Bloco de Notas"), json!({"name": "objetivo", "arguments": {"texto": "abrir o Bloco de Notas"}}));
    }

    #[test]
    fn call_result_text_and_error() {
        assert_eq!(call_text(&json!({"content": [{"type": "text", "text": "concluído: aberto"}]})), Ok("concluído: aberto".into()));
        assert_eq!(call_text(&json!({"content": [{"type": "text", "text": "alvo desconhecido"}], "isError": true})), Err("alvo desconhecido".into()));
    }

    #[test]
    fn empty_call_result_is_a_failure() {
        let empty = Err("O controle do computador respondeu sem resultado".into());
        assert_eq!(call_text(&json!({"content": []})), empty);
        assert_eq!(call_text(&json!({"content": [{"type": "text", "text": "  \n"}]})), empty);
    }

    #[test]
    fn home_must_resolve_to_an_absolute_dir() {
        assert!(resolve_home(None).is_err());
        assert!(resolve_home(Some(PathBuf::new())).is_err(), "vazio viraria caminho relativo");
        let absolute = std::env::temp_dir();
        assert_eq!(resolve_home(Some(absolute.clone())), Ok(absolute));
    }

    /// Só leitura: `initialize` + `tools/list` no HCC registrado; nunca roda `objetivo`.
    #[tokio::test]
    #[ignore]
    async fn hcc_dry_run_lists_tools() {
        let mut launch = launch().unwrap_or_else(|e| panic!("{e}"));
        launch.env.retain(|(k, _)| !k.starts_with("LLM_") && k != JEV_KEY);
        let tools = list_tools(launch).await.unwrap();
        assert!(tools.iter().any(|t| t == "objetivo"), "{tools:?}");
        println!("hcc tools: {tools:?}");
    }
}

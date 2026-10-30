//! Comando e ambiente do `codex app-server --stdio` que o Rust sobe no cano, sem perguntar ao Python:
//! port de `sem_terminal.argv`/`_ambiente` e do ramo Codex de `runtime_policy.launch_env`.
//! O ambiente é segredo: sai daqui só para o `process::spawn`, nunca para log nem disco.
//! Bloqueante (lê o arquivo da sessão e a configuração): chamar fora do runtime (`spawn_blocking`).
use super::{codex_sidecar,protocol::RuntimeError};
use crate::accounts::{AccountService,Provider,environment::AccountEnvironment};
use serde_json::{Value,json};
use std::collections::BTreeMap;
use std::path::Path;

/// `PERMISSION_POLICIES` do `lancador.py`: (nome na tela, approval_policy, sandbox_mode).
static MODES:[(&str,&str,&str);3] = [
    ("Ask for approval","on-request","read-only"),
    ("Approve for me","on-request","workspace-write"),
    ("Full Access","never","danger-full-access"),
];
/// `MODO_PADRAO`: o que a abertura grava (`null`) quando ninguém escolheu modo.
const DEFAULT_MODE:usize = 2;

const JEV_MARK:&str = "HANGAR_JEV";
/// `_JEV_TEXTO` do `runtime_config.py`.
const JEV_TEXT:[(&str,&str);4] = [
    ("jev_texto_base_url","JEV_TEXTO_BASE_URL"),
    ("jev_texto_api_key","JEV_TEXTO_API_KEY"),
    ("jev_texto_modelo","JEV_TEXTO_MODELO"),
    ("jev_texto_cmd","JEV_TEXTO_CMD"),
];

/// O comando e o ambiente da subida do Codex sem terminal.
#[derive(Debug)]
pub struct Launch { pub program:Vec<String>, pub env:BTreeMap<String,String> }

fn refused(code:&str,message:&str) -> RuntimeError { RuntimeError::new(code,message) }

/// Modo gravado no sidecar sem terminal. Ausente é o padrão que a abertura escolheu; texto fora da
/// lista é sidecar estragado e recusa: adivinhar aqui poderia abrir a sessão em Full Access.
pub fn stored_mode(meta:&Value) -> Result<&'static str,RuntimeError> { stored(meta).map(|&(name,..)|name) }

fn stored(meta:&Value) -> Result<&'static (&'static str,&'static str,&'static str),RuntimeError> {
    let wanted = match &meta["permission_mode"] {
        Value::Null=>return Ok(&MODES[DEFAULT_MODE]),
        Value::String(mode)=>mode.trim(),
        _=>return Err(refused("launch_permission_mode","modo de permissão do sidecar não é texto")),
    };
    MODES.iter().find(|(name,..)|name.eq_ignore_ascii_case(wanted)).ok_or_else(||refused("launch_permission_mode",
        &format!("modo de permissão desconhecido no sidecar: {wanted:?}; a sessão não foi aberta")))
}

/// (aprovação, sandbox) do modo do app pelo nome (sem caixa nem espaço nas pontas); fora da lista, nenhum.
pub fn policy_of(mode:&str) -> Option<(&'static str,&'static str)> {
    MODES.iter().find(|(name,..)|name.eq_ignore_ascii_case(mode.trim())).map(|&(_,approval,sandbox)|(approval,sandbox))
}

/// `politica` sobre o modo gravado.
fn policy(meta:&Value) -> Result<(&'static str,&'static str),RuntimeError> {
    let &(_,approval,sandbox) = stored(meta)?;
    Ok((approval,sandbox))
}

/// `sem_terminal.argv`, com `tool_output_override` e `service_tier_override`.
pub fn argv(meta:&Value) -> Result<Vec<String>,RuntimeError> {
    let (approval,sandbox) = policy(meta)?;
    let mut argv:Vec<String> = vec!["codex".into(),"app-server".into(),"--stdio".into(),
        "-c".into(),format!("sandbox_mode=\"{sandbox}\""),"-c".into(),format!("approval_policy=\"{approval}\"")];
    match &meta["tool_output_token_limit"] {
        Value::Null=>{},
        Value::Number(limit) if limit.as_u64().is_some_and(|limit|limit > 0)=>argv.extend(["-c".into(),format!("tool_output_token_limit={limit}")]),
        _=>return Err(refused("launch_tool_output_limit","limite de resultado de ferramenta inválido")),
    }
    match &meta["service_tier"] {
        Value::Null=>{},
        // ponytail: `json.dumps` escapa não-ASCII e o serde não; os valores de Fast são ASCII.
        tier=>argv.extend(["-c".into(),format!("service_tier={tier}")]),
    }
    Ok(argv)
}

/// Regra do destino do Jev; muda junto com as outras cópias: `destino_jev` (`backend/app/runtime_config.py`),
/// `voice::jev::destination` (`desktop-native/src/voice/jev.rs`) e `jev_config` (`skills/orquestrar/scripts/orq.py`).
fn jev_destination(key:&str,endpoint:&str,model:&str) -> (String,String) {
    let (endpoint,model) = (endpoint.trim(),model.trim());
    let or = |value:&str,fallback:&str|if value.is_empty() { fallback.to_owned() } else { value.to_owned() };
    if endpoint.contains("openrouter.ai") || (endpoint.is_empty() && key.trim().starts_with("sk-or-")) {
        let mut model = or(model,"~typesafe/jev-latest");
        if model.starts_with("typesafe/") && model.ends_with("-latest") { model.insert(0,'~'); }
        return (or(endpoint,"https://openrouter.ai/api/alpha/decisions"),model);
    }
    (or(endpoint,"https://api.typesafe.ai/v1/systemone"),or(model,"jev-latest"))
}

/// `runtime_config.env_jev`: o marcador vai sempre; chave e destino só com o recurso ligado nesta
/// sessão, relidos da configuração agora, nunca copiados do processo que morre.
pub fn jev_env(on:bool,config:&Value) -> BTreeMap<String,String> {
    let mut env = BTreeMap::from([(JEV_MARK.to_owned(),(if on { "on" } else { "off" }).to_owned())]);
    if !on { return env; }
    let text = |field:&str|config[field].as_str().map(str::trim).filter(|value|!value.is_empty()).map(str::to_owned);
    if let Some(key) = text("jev_api_key") {
        let (endpoint,model) = jev_destination(&key,&text("jev_endpoint").unwrap_or_default(),&text("jev_model").unwrap_or_default());
        env.extend([("TYPESAFE_API_KEY".to_owned(),key),("JEV_ENDPOINT".to_owned(),endpoint),("JEV_MODEL".to_owned(),model)]);
    }
    for (field,var) in JEV_TEXT { if let Some(value) = text(field) { env.insert(var.to_owned(),value); } }
    env
}

/// `runtime_config._carregar`: `<CLAUDE_CONFIG_DIR ou ~/.claude>/runtime-config.json`; ausente,
/// quebrado ou não-objeto vale `{}`, como no Python.
pub fn runtime_config(base:&AccountEnvironment) -> Value {
    std::fs::read(base.claude_base.join("runtime-config.json")).ok()
        .and_then(|raw|serde_json::from_slice::<Value>(&raw).ok()).filter(Value::is_object).unwrap_or_else(||json!({}))
}

/// `sem_terminal._ambiente`: conta resolvida pelo id (apagada depois da criação falha aqui, com o
/// código), sem o pane do operador, com a chave da sessão e o Jev da escolha de abertura.
pub fn environment(base:&AccountEnvironment,meta:&Value,config:&Value) -> Result<BTreeMap<String,String>,RuntimeError> {
    let field = move |name:&str|meta[name].as_str().filter(|value|!value.is_empty()).ok_or_else(||refused("launch_meta",&format!("sidecar sem `{name}`")));
    let (name,key) = (field("name")?,field("key")?);
    let id = match &meta["codex_account"] {
        Value::Null=>"default",
        Value::String(id) if id.is_empty()=>"default",
        Value::String(id)=>id.as_str(),
        _=>return Err(refused("codex_account_invalid_name","conta Codex do sidecar não é texto")),
    };
    let account = AccountService::new(base.clone()).resolve(Provider::Codex,id)
        .map_err(|error|refused(error.code,&format!("conta Codex indisponível: {}",error.code)))?;
    let mut env = base.codex(&account);
    // Backend subido de dentro de um tmux passaria o pane do operador para o processo.
    env.remove("TMUX");
    env.remove("TMUX_PANE");
    env.insert("CP_SESSION_NAME".into(),name.into());
    env.insert("CP_SESSION_KEY".into(),key.into());
    env.insert("HANGAR_CANO_KEY".into(),key.into());
    env.insert("HANGAR_CANO_OWNER".into(),base.home.to_string_lossy().into_owned());
    // ponytail: `jev` é gravado como booleano; o `bool()` do Python aceitaria qualquer verdadeiro.
    env.extend(jev_env(meta["jev"].as_bool() == Some(true),config));
    Ok(env)
}

/// `runtime_policy.launch_env` do Codex. O sidecar da mesma chave vence o `meta` da abertura: o modo
/// trocado depois de abrir só está nele. O `codex` sai sem resolver: o `process::spawn` o acha pelo
/// PATH deste ambiente, com a mesma regra do `shutil.which`.
pub fn launch_env(base:&AccountEnvironment,sidecar_dir:&Path,meta:&Value) -> Result<Launch,RuntimeError> {
    let mut meta = meta.clone();
    let current = meta["name"].as_str().and_then(|name|codex_sidecar::load(&codex_sidecar::path(sidecar_dir,name)));
    if let (Some(current),Value::Object(fields)) = (current,&mut meta) && current.get("key").is_some_and(|key|Some(key) == fields.get("key")) {
        fields.extend(current);
    }
    let config = runtime_config(base);
    let env = environment(base,&meta,&config)?;
    let mut program = argv(&meta)?;
    program.extend(identity(&env));
    program.extend(context_overrides(Path::new(&env["CODEX_HOME"]),&config).into_iter().flat_map(|value|["-c".to_owned(),value]));
    Ok(Launch { program,env })
}

/// Contexto de 1M e compactação de 900 mil, os valores que `codex_opcoes.py` grava ao ligar.
const CONTEXT:u64 = 1_000_000;
const COMPACTION:u64 = 900_000;

/// O 1M é o padrão do Hangar: sem `model_context_window` no `config.toml` da conta (nem no perfil
/// ativo), a sessão sobe com ele, a menos que a pessoa o tenha desligado (`codex_extended_context`).
/// O Codex limita ao teto de cada modelo. Arquivo ilegível fica com o Codex, que o recusa sozinho.
pub fn context_overrides(codex_home:&Path,runtime_config:&Value) -> Vec<String> {
    if runtime_config["codex_extended_context"] == Value::Bool(false) { return Vec::new(); }
    let config = match std::fs::read_to_string(codex_home.join("config.toml")) {
        Ok(raw)=>match toml::from_str::<toml::Table>(&raw) { Ok(config)=>config, Err(_)=>return Vec::new() },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound=>toml::Table::new(),
        Err(_)=>return Vec::new(),
    };
    let profile = config.get("profile").and_then(toml::Value::as_str)
        .and_then(|name|config.get("profiles")?.get(name)?.as_table());
    let set = |key:&str|config.contains_key(key) || profile.is_some_and(|p|p.contains_key(key));
    if set("model_context_window") { return Vec::new(); }
    let mut overrides = vec![format!("model_context_window={CONTEXT}")];
    if !set("model_auto_compact_token_limit") { overrides.push(format!("model_auto_compact_token_limit={COMPACTION}")); }
    overrides
}

/// `identity_override`: a identidade da sessão no shell dos comandos do Codex, por `set` (vale com
/// qualquer `inherit` e soma ao `set` do usuário). Só as variáveis presentes no ambiente da subida.
fn identity(env:&BTreeMap<String,String>) -> Vec<String> {
    ["CP_SESSION_NAME","CP_SESSION_KEY","HANGAR_CANO_KEY","TMUX","TMUX_PANE"].into_iter()
        .filter_map(|name|env.get(name).filter(|v|!v.is_empty()).map(|value|(name,value)))
        .flat_map(|(name,value)|["-c".to_owned(),format!("shell_environment_policy.set.{name}={}",Value::from(value.as_str()))]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(home:&Path) -> AccountEnvironment {
        let home = home.to_string_lossy();
        let map:BTreeMap<String,String> = [("HOME",&*home),("USERPROFILE",&*home),("PATH","/bin"),("TMUX","/tmp/tmux-1/default,1,0"),("TMUX_PANE","%3"),
            ("OPENAI_API_KEY","sk-main"),("HANGAR_INTERNAL_SECRET","s")].into_iter().map(|(k,v)|(k.into(),v.into())).collect();
        AccountEnvironment::from_map(map)
    }

    /// O argv de hoje (`sem_terminal.argv`) para o mesmo sidecar.
    #[test]
    fn argv_matches_the_python_launcher_command() {
        let meta = json!({"permission_mode":"approve for me ","tool_output_token_limit":9,"service_tier":"fast"});
        assert_eq!(argv(&meta).unwrap(),["codex","app-server","--stdio","-c","sandbox_mode=\"workspace-write\"","-c","approval_policy=\"on-request\"",
            "-c","tool_output_token_limit=9","-c","service_tier=\"fast\""]);
        // Sem modo: o padrão que a abertura gravou.
        for meta in [json!({}),json!({"permission_mode":null,"service_tier":null})] {
            assert_eq!(argv(&meta).unwrap(),["codex","app-server","--stdio","-c","sandbox_mode=\"danger-full-access\"","-c","approval_policy=\"never\""]);
        }
        for bad in [json!({"tool_output_token_limit":0}),json!({"tool_output_token_limit":-1}),json!({"tool_output_token_limit":1.5}),
            json!({"tool_output_token_limit":true}),json!({"permission_mode":3})] {
            assert!(argv(&bad).is_err(),"{bad}");
        }
    }

    /// A identidade da sessão vai ao shell dos comandos do Codex (`identity_override` do Python);
    /// sem ela, com `inherit` diferente de `all`, o `hangar-send` de dentro assinava `cli`.
    #[test]
    fn launch_carries_the_session_identity_to_the_command_shell() {
        let home = tempfile::tempdir().unwrap();
        let sidecars = home.path().join("sidecars");
        let launch = launch_env(&base(home.path()),&sidecars,&json!({"name":"cx \"a\"","key":"k1","permission_mode":"Full Access"})).unwrap();
        let tail:Vec<&str> = launch.program.iter().map(String::as_str).skip(7).take(6).collect();
        assert_eq!(tail,["-c","shell_environment_policy.set.CP_SESSION_NAME=\"cx \\\"a\\\"\"","-c","shell_environment_policy.set.CP_SESSION_KEY=\"k1\"",
            "-c","shell_environment_policy.set.HANGAR_CANO_KEY=\"k1\""],"TMUX e TMUX_PANE saem do ambiente do cano: nada a repassar");
    }

    /// Sem nada salvo, 1M; o desligado gravado no Hangar e o valor escrito no `config.toml` mandam.
    #[test]
    fn context_defaults_to_one_million_when_nothing_is_saved() {
        let home = tempfile::tempdir().unwrap();
        let one_million = ["model_context_window=1000000","model_auto_compact_token_limit=900000"];
        assert_eq!(context_overrides(home.path(),&json!({})),one_million,"instalação nova, sem config.toml");
        std::fs::write(home.path().join("config.toml"),"model = \"gpt-6.1-sol\"\n").unwrap();
        assert_eq!(context_overrides(home.path(),&json!({"codex_extended_context":true})),one_million);
        assert!(context_overrides(home.path(),&json!({"codex_extended_context":false})).is_empty(),"desligado pela pessoa");
        std::fs::write(home.path().join("config.toml"),"model_auto_compact_token_limit = 250000\n").unwrap();
        assert_eq!(context_overrides(home.path(),&json!({})),one_million[..1],"compactação própria continua valendo");
        for explicit in ["model_context_window = 300000\n","model_context_window = 1000000\n",
            "profile = \"p\"\n[profiles.p]\nmodel_context_window = 300000\n"] {
            std::fs::write(home.path().join("config.toml"),explicit).unwrap();
            assert!(context_overrides(home.path(),&json!({})).is_empty(),"{explicit}");
        }
        std::fs::write(home.path().join("config.toml"),"não é toml [").unwrap();
        assert!(context_overrides(home.path(),&json!({})).is_empty(),"o próprio Codex recusa o arquivo");
    }

    #[test]
    fn headless_launch_opens_with_one_million_by_default() {
        let home = tempfile::tempdir().unwrap();
        let launch = launch_env(&base(home.path()),&home.path().join("sidecars"),&json!({"name":"cx","key":"k1"})).unwrap();
        assert!(launch.program.windows(2).any(|w|w == ["-c","model_context_window=1000000"]),"{:?}",launch.program);
    }

    #[test]
    fn unknown_stored_mode_refuses_instead_of_full_access() {
        for mode in ["turbo",""," ","Full","full-access","danger-full-access"] {
            let refused = argv(&json!({"permission_mode":mode})).unwrap_err();
            assert_eq!(refused.code,"launch_permission_mode","{mode:?}");
        }
        for (mode,sandbox,approval) in [("Ask for approval","read-only","on-request"),(" APPROVE FOR ME","workspace-write","on-request"),
            ("full access","danger-full-access","never")] {
            assert_eq!(argv(&json!({"permission_mode":mode})).unwrap()[3..7],
                ["-c".to_owned(),format!("sandbox_mode=\"{sandbox}\""),"-c".to_owned(),format!("approval_policy=\"{approval}\"")],"{mode:?}");
        }
    }

    #[test]
    fn jev_follows_the_session_choice_and_the_config_rule() {
        assert_eq!(jev_env(false,&json!({"jev_api_key":"k"})),BTreeMap::from([("HANGAR_JEV".into(),"off".into())]),"desligado: só o marcador");
        let on = jev_env(true,&json!({"jev_api_key":" sk-or-x ","jev_model":"typesafe/jev-latest","jev_texto_cmd":"c","jev_texto_modelo":" "}));
        assert_eq!(on,BTreeMap::from([("HANGAR_JEV".into(),"on".into()),("TYPESAFE_API_KEY".into(),"sk-or-x".into()),
            ("JEV_ENDPOINT".into(),"https://openrouter.ai/api/alpha/decisions".into()),("JEV_MODEL".into(),"~typesafe/jev-latest".into()),
            ("JEV_TEXTO_CMD".into(),"c".into())]));
        let typesafe = jev_env(true,&json!({"jev_api_key":"k"}));
        assert_eq!((typesafe["JEV_ENDPOINT"].as_str(),typesafe["JEV_MODEL"].as_str()),("https://api.typesafe.ai/v1/systemone","jev-latest"));
        assert_eq!(jev_env(true,&json!({})),BTreeMap::from([("HANGAR_JEV".into(),"on".into())]),"sem chave não inventa destino");
    }

    /// O ambiente de hoje (`sem_terminal._ambiente`) para a conta padrão e para uma adicional.
    #[test]
    fn environment_matches_the_python_session_environment() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::write(claude.join("runtime-config.json"),json!({"jev_api_key":"k"}).to_string()).unwrap();
        let base = base(home.path());
        let meta = json!({"name":"cx","key":"k1","jev":true});
        let env = environment(&base,&meta,&runtime_config(&base)).unwrap();
        assert_eq!(env["CODEX_HOME"],home.path().join(".codex").to_string_lossy(),"padrão: CODEX_HOME do servidor ou ~/.codex");
        assert_eq!((env["CP_SESSION_NAME"].as_str(),env["CP_SESSION_KEY"].as_str(),env["HANGAR_CANO_KEY"].as_str()),("cx","k1","k1"));
        assert_eq!(env["HANGAR_CANO_OWNER"],home.path().to_string_lossy());
        assert_eq!((env["HANGAR_JEV"].as_str(),env["TYPESAFE_API_KEY"].as_str()),("on","k"));
        assert_eq!(env["OPENAI_API_KEY"],"sk-main","a padrão herda a identidade do servidor");
        assert!(!env.contains_key("TMUX") && !env.contains_key("TMUX_PANE"),"o pane do operador não vai junto");
        assert!(!env.contains_key("HANGAR_INTERNAL_SECRET"));
        // Adicional: sem a identidade da principal; apagada ou sem marcador falha com o código da conta.
        let other = home.path().join(".codex-work");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(crate::accounts::storage::CODEX_MARKER),json!({"version":1,"id":"work"}).to_string()).unwrap();
        let env = environment(&base,&json!({"name":"cx","key":"k1","codex_account":"work"}),&json!({})).unwrap();
        assert_eq!(env["CODEX_HOME"],crate::accounts::catalog::resolved(&other).to_string_lossy());
        assert!(!env.contains_key("OPENAI_API_KEY"));
        assert_eq!(env["HANGAR_JEV"],"off");
        assert_eq!(environment(&base,&json!({"name":"cx","key":"k1","codex_account":"gone"}),&json!({})).unwrap_err().code,"codex_account_not_found");
        assert_eq!(environment(&base,&json!({"name":"cx"}),&json!({})).unwrap_err().code,"launch_meta");
    }

    #[test]
    fn the_sidecar_of_the_same_key_wins_over_the_open_meta() {
        let home = tempfile::tempdir().unwrap();
        let dir = codex_sidecar::dir(home.path());
        std::fs::create_dir_all(&dir).unwrap();
        let base = base(home.path());
        let meta = json!({"name":"cx","key":"k1","permission_mode":"Full Access"});
        std::fs::write(codex_sidecar::path(&dir,"cx"),json!({"name":"cx","key":"k1","permission_mode":"Ask for approval"}).to_string()).unwrap();
        assert!(launch_env(&base,&dir,&meta).unwrap().program.contains(&"sandbox_mode=\"read-only\"".to_owned()),"modo trocado depois de abrir");
        std::fs::write(codex_sidecar::path(&dir,"cx"),json!({"name":"cx","key":"k2","permission_mode":"Ask for approval"}).to_string()).unwrap();
        assert!(launch_env(&base,&dir,&meta).unwrap().program.contains(&"sandbox_mode=\"danger-full-access\"".to_owned()),"outra vida não conta");
    }
}

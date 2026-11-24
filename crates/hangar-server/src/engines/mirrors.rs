//! Espelhos de credencial: preservam configuração pessoal e isolam falhas por destino.
use std::{collections::{BTreeMap,HashSet},fs,io::Write,path::Path,sync::{LazyLock,Mutex}};
use serde_json::{Value,json};
use crate::accounts::environment::AccountEnvironment;
use crate::orchestration::truthy;

static LOCK: Mutex<()> = Mutex::new(());
static TABLE: LazyLock<regex::Regex> = LazyLock::new(||regex::Regex::new(r"^\s*\[\[?([^\]]+)\]\]?\s*$").unwrap());
static ENGINE_BLOCK: LazyLock<regex::Regex> = LazyLock::new(||regex::Regex::new(r"(?m)^# >>> hangar: provedor (\S+) \(app/agentes_sync\)").unwrap());

pub use crate::harness_setup::files::{directory, write_preserving};

pub fn builtin(base: &str) -> Option<&'static str> {
    let base = base.trim().trim_end_matches('/'); let base = base.split_once("://").map_or(base,|(_,b)|b).strip_prefix("www.").unwrap_or(base.split_once("://").map_or(base,|(_,b)|b));
    let base = base.strip_suffix("/v1").unwrap_or(base).trim_end_matches('/');
    [("opencode.ai/zen/go","opencode-go"),("opencode.ai/zen","opencode"),("api.kimi.com/coding","kimi-coding"),("api.anthropic.com","anthropic"),("openrouter.ai/api","openrouter"),("api.groq.com/openai","groq"),("api.deepseek.com","deepseek"),("api.x.ai","xai"),("api.minimax.io","minimax"),("api.mistral.ai","mistral"),("api.cerebras.ai","cerebras"),("api.together.xyz","together")]
        .into_iter().find(|(prefix,_)|base == *prefix || base.starts_with(&format!("{prefix}/"))).map(|(_,name)|name)
}

fn base(base: &str) -> String { let b = base.trim().trim_end_matches('/'); if b.ends_with("/v1") { b.into() } else { format!("{b}/v1") } }
fn markers(name: &str) -> (String,String) { (format!("# >>> hangar: provedor {name} (app/agentes_sync) — gerado, não editar à mão"),format!("# <<< hangar: fim do provedor {name}")) }
fn quoted(s: &str) -> String { let mut out = String::from("\""); for c in s.chars() { match c { '\\' => out.push_str("\\\\"), '"' => out.push_str("\\\""), c if c < ' ' || c == '\x7f' => out.push_str(&format!("\\u{:04X}",c as u32)), c => out.push(c) } } out.push('"'); out }


fn backup(path: &Path) -> Result<(),String> {
    if !path.exists() { return Ok(()); }
    let backup = path.with_file_name(format!("{}.bak-hangar",path.file_name().unwrap().to_string_lossy()));
    let mut options = fs::OpenOptions::new(); options.write(true).create_new(true);
    #[cfg(unix)] { use std::os::unix::fs::{OpenOptionsExt,PermissionsExt}; options.mode(fs::metadata(path).map_err(|e|e.to_string())?.permissions().mode() & 0o777); }
    match options.open(backup) {
        Ok(mut file) => file.write_all(&fs::read(path).map_err(|e|e.to_string())?).map_err(|e|e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()), Err(e) => Err(e.to_string()),
    }
}

fn tail<'a>(middle: &'a str, tables: &BTreeMap<String,Vec<String>>) -> &'a str {
    let ours: HashSet<String> = tables.iter().flat_map(|(section,keys)| std::iter::once(section.clone()).chain(keys.iter().map(move |k|format!("{section}.{k}")))).collect();
    let mut offset = 0;
    for line in middle.split_inclusive('\n') {
        if let Some(c) = TABLE.captures(line) { if !ours.contains(c[1].replace('"',"").trim()) { return &middle[offset..]; } }
        offset += line.len();
    }
    ""
}

fn text(path: &Path) -> Result<String,String> {
    match fs::read_to_string(path) { Ok(s) => Ok(s), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()), Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Err("config-invalido".into()), Err(e) => Err(format!("erro: {e}")) }
}

fn toml_block(path: &Path, name: &str, block: Option<&str>, mut tables: BTreeMap<String,Vec<String>>) -> Result<String,String> {
    let raw = text(path)?; let (begin,end) = markers(name);
    let (before, after, rest) = if let Some(i) = raw.find(&begin) {
        let marker = raw[i..].find(&end).map(|j|i+j).ok_or("bloco-incompleto")?;
        let j = raw[marker..].find('\n').map(|j|marker+j+1).unwrap_or(raw.len());
        let middle = &raw[i..marker];
        if block.is_none() && tables.contains_key("models") {
            tables.insert("models".into(),middle.lines().filter_map(|l|TABLE.captures(l)).map(|c|c[1].replace('"',"").trim().to_owned()).filter_map(|s|s.strip_prefix(&format!("models.{name}/")).map(|_|s[7..].to_owned())).collect());
        }
        (raw[..i].to_owned(),format!("{}{}",tail(middle,&tables),&raw[j..]),format!("{}{}",&raw[..i],&raw[j..]))
    } else {
        if block.is_none() { return Err("nao-gerenciado".into()); }
        (format!("{raw}{}",if !raw.is_empty() && !raw.ends_with('\n') { "\n" } else { "" }),String::new(),raw.clone())
    };
    if block.is_some() && !rest.trim().is_empty() {
        let parsed: Value = toml::from_str(&rest).map_err(|_|"config-invalido")?;
        if tables.iter().any(|(section,keys)| parsed[section].as_object().is_some_and(|m|keys.iter().any(|k|m.contains_key(k)))) { return Err("ja-existe-fora-do-bloco".into()); }
    }
    backup(path)?; write_preserving(path,&format!("{before}{}{after}",block.unwrap_or("")))?;
    Ok(path.to_string_lossy().into_owned())
}

fn json_file(path: &Path, invalid: &str) -> Result<Value,String> {
    if !path.exists() { return Ok(json!({})); }
    let v: Value = serde_json::from_str(&text(path).map_err(|_|invalid.to_owned())?).map_err(|_|invalid.to_owned())?;
    if v.is_object() { Ok(v) } else { Err(invalid.into()) }
}
fn write_json(path: &Path, value: &Value) -> Result<(),String> { backup(path)?; write_preserving(path,&format!("{}\n",crate::transcript::pyjson::dumps_indent(value,2,false))) }

fn pi_model(model: &Value, catalog: &BTreeMap<String,Value>) -> Value {
    let absent = Value::Null; let cat = model["id"].as_str().and_then(|id|catalog.get(id)).unwrap_or(&absent);
    let vision = if model["vision"].is_null() { cat["input"].as_array().is_some_and(|a|a.contains(&json!("image"))) } else { model["vision"] == true };
    let mut out = json!({"id":model["id"],"name":cat["name"].as_str().map_or_else(||model["id"].clone(),|s|json!(s)),"input":if vision {vec!["text","image"]} else {vec!["text"]}});
    let integer = |v: &Value|v.is_boolean() || v.as_number().is_some_and(|n| !n.is_f64());
    let window = if integer(&model["context_length"]) { &model["context_length"] } else { &cat["contextWindow"] };
    if integer(window) { out["contextWindow"] = window.clone(); }
    if cat["reasoning"].is_boolean() { out["reasoning"] = cat["reasoning"].clone(); }
    out
}

fn apply_one(env: &AccountEnvironment,target: &str,name: &str,url: &str,key: &str,models: &[Value]) -> Result<String,String> {
    if !crate::accounts::catalog::valid_name(name) { return Err("nome-invalido".into()); }
    if url.is_empty() || key.is_empty() { return Err("faltando-base-url-ou-chave".into()); }
    let dir = directory(env,target); if !dir.is_dir() { return Err("nao-instalado".into()); }
    if target == "pi" {
        if let Some(provider) = builtin(url) {
            let path = dir.join("auth.json"); let mut data = json_file(&path,"auth-invalido")?;
            if data[provider].is_object() && data[provider]["type"] != "api_key" { return Err(format!("credencial-nao-substituivel ({provider} já está logado de outro jeito no Pi)")); }
            data[provider] = json!({"type":"api_key","key":key}); write_json(&path,&data)?;
            return Ok(format!("{} (provedor {provider}; o Pi resolve os modelos)",path.display()));
        }
        let path = dir.join("models.json"); let mut data = json_file(&path,"config-invalido")?;
        if data.get("providers").is_some_and(|p|!p.is_object()) { return Err("config-invalido".into()); }
        if data.get("providers").is_none() { data["providers"] = json!({}); }
        let stored = json_file(&dir.join("models-store.json"),"config-invalido").unwrap_or(json!({})); let mut catalog = BTreeMap::new();
        for entry in stored.as_object().into_iter().flat_map(|m|m.values()) { for m in entry["models"].as_array().into_iter().flatten() { if let Some(id) = m["id"].as_str() { catalog.entry(id.to_owned()).or_insert_with(||m.clone()); } } }
        data["providers"][name] = json!({"baseUrl":base(url),"api":"openai-completions","apiKey":key,"models":models.iter().filter(|m|m.is_object() && truthy(&m["id"])).map(|m|pi_model(m,&catalog)).collect::<Vec<_>>()});
        write_json(&path,&data)?; return Ok(path.to_string_lossy().into_owned());
    }
    let (begin,end) = markers(name); let path = dir.join("config.toml");
    let var = format!("HANGAR_{}_API_KEY",name.to_uppercase().replace('-',"_"));
    let block = format!("{begin}\n[model_providers.{}]\nname = {}\nbase_url = {}\nenv_key = {}\nwire_api = \"responses\"\n{end}\n",quoted(name),quoted(name),quoted(&base(url)),quoted(&var));
    toml_block(&path,name,Some(&block),BTreeMap::from([("model_providers".into(),vec![name.into()])])).map(|s|format!("{s} (exporte {var})"))
}

fn remove_one(env: &AccountEnvironment,target: &str,name: &str) -> Result<String,String> {
    let dir = directory(env,target); if !dir.is_dir() { return Err("nao-instalado".into()); }
    if target == "pi" {
        let path = dir.join("models.json"); let mut data = json_file(&path,"config-invalido")?;
        if !data["providers"].as_object_mut().is_some_and(|p|p.remove(name).is_some()) { return Err("nao-gerenciado".into()); }
        write_json(&path,&data)?; return Ok(path.to_string_lossy().into_owned());
    }
    let tables = if target == "codex" { BTreeMap::from([("model_providers".into(),vec![name.into()])]) } else { BTreeMap::from([("providers".into(),vec![name.into()]),("models".into(),vec![])]) };
    toml_block(&dir.join("config.toml"),name,None,tables)
}

/// Destinos que recebem o motor: o Kimi fica só com os provedores dele (harnesses.md).
pub const SYNC_TARGETS: [&str; 2] = ["pi","codex"];

pub fn sync(env: &AccountEnvironment,name: &str,url: &str,key: &str,models: &[Value],targets: &[String]) -> Value {
    run(targets,&SYNC_TARGETS,|target|apply_one(env,target,name,url,key,models))
}
pub fn remove(env: &AccountEnvironment,name: &str,targets: &[String]) -> Value { run(targets,&["pi","kimi","codex"],|target|remove_one(env,target,name)) }

/// Tira do `config.toml` do Kimi todo bloco de motor gravado por versão antiga; devolve os nomes tirados.
pub fn purge_kimi(env: &AccountEnvironment) -> Result<Vec<String>,String> {
    let _lock = LOCK.lock().unwrap_or_else(|e|e.into_inner());
    let path = directory(env,"kimi").join("config.toml");
    let names: Vec<String> = ENGINE_BLOCK.captures_iter(&text(&path)?).map(|c|c[1].to_owned()).collect();
    let (mut removed, mut failed) = (Vec::new(), Vec::new());
    for name in names {
        match remove_one(env,"kimi",&name) { Ok(_) => removed.push(name), Err(e) => failed.push(format!("{name}: {e}")) }
    }
    match remove_unmarked(env,&path) { Ok(names) => removed.extend(names), Err(e) => failed.push(e) }
    if failed.is_empty() { Ok(removed) } else { Err(format!("{}; tirados: [{}]",failed.join("; "),removed.join(", "))) }
}

/// Motor cujo marcador o Kimi tirou ao regravar o arquivo: sai o provedor com o nome, a chave e o
/// endereço do motor, e os modelos dele. Provedor pessoal com o mesmo nome e outra chave fica.
fn remove_unmarked(env: &AccountEnvironment, path: &Path) -> Result<Vec<String>,String> {
    let raw = text(path)?;
    if raw.trim().is_empty() { return Ok(vec![]); }
    let parsed: Value = toml::from_str(&raw).map_err(|_|"config-invalido".to_owned())?;
    let engines = crate::engines::store::state(env).0;
    let ours: Vec<String> = engines.iter().filter(|(name,data)| {
        let provider = &parsed["providers"][name.as_str()];
        truthy(&data["api_key"]) && provider["api_key"] == data["api_key"]
            && provider["base_url"].as_str() == data["base_url"].as_str().map(base).as_deref()
    }).map(|(name,_)|name.clone()).collect();
    if ours.is_empty() { return Ok(ours); }
    let owned = |table: &str| ours.iter().any(|n| table == format!("providers.{n}") || table.starts_with(&format!("providers.{n}.")) || table.starts_with(&format!("models.{n}/")));
    // Comentário e linha em branco antes de uma tabela são de quem vem depois: só saem se ela também sair.
    let (mut out, mut pending, mut skip) = (String::new(), String::new(), false);
    for line in raw.split_inclusive('\n') {
        if let Some(c) = TABLE.captures(line) {
            skip = owned(c[1].replace('"',"").trim());
            if !skip { out.push_str(&pending); }
            pending.clear();
        } else if skip {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') { pending.push_str(line); } else { pending.clear(); }
            continue;
        }
        if !skip { out.push_str(line); }
    }
    // A leitura por linha não vê tabela inline nem cabeçalho fora do formato: confere antes de gravar.
    let fixed: Value = toml::from_str(&out).map_err(|_|"config-invalido-apos-limpeza".to_owned())?;
    let left: Vec<&str> = ours.iter().filter(|n| !fixed["providers"][n.as_str()].is_null()).map(String::as_str).collect();
    if !left.is_empty() { return Err(format!("nao-removido: {}",left.join(", "))); }
    backup(path)?; write_preserving(path,&out)?;
    Ok(ours)
}
fn run(targets: &[String], allowed: &[&str], mut operation: impl FnMut(&str)->Result<String,String>) -> Value {
    let _lock = LOCK.lock().unwrap_or_else(|e|e.into_inner()); let mut result = json!({});
    for target in targets {
        let answer = if allowed.contains(&target.as_str()) { operation(target) } else { Err("alvo-desconhecido".into()) };
        result[target] = match answer { Ok(reason) => json!({"ok":true,"motivo":reason}),Err(reason) => json!({"ok":false,"motivo":reason}) };
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_at(home: &Path) -> AccountEnvironment {
        let home = home.to_string_lossy().into_owned();
        AccountEnvironment::from_pairs([("HOME", home.clone()), ("USERPROFILE", home)])
    }

    fn engine_block(name: &str) -> String {
        let (begin,end) = markers(name);
        format!("{begin}\n[providers.\"{name}\"]\ntype = \"kimi\"\napi_key = \"k\"\nbase_url = \"http://127.0.0.1:8317/v1\"\n\n[models.\"{name}/gpt-x\"]\nprovider = \"{name}\"\nmodel = \"gpt-x\"\ndisplay_name = \"gpt-x\"\n\n{end}\n")
    }

    const OWN: &str = "default_model = \"kimi-code/k3\"\n\n[providers.\"managed:kimi-code\"]\ntype = \"kimi\"\napi_key = \"\"\n\n";
    const HOOKS: &str = "\n# hangar: integração da sessão.\n[[hooks]]\nevent = \"Stop\"\ncommand = \"x\"\ntimeout = 5\n";

    /// O Kimi recusa gravar a própria configuração com modelo de motor sem `max_context_size`: o
    /// Hangar não leva motor nenhum para lá.
    #[test]
    fn engine_sync_never_writes_into_the_kimi_config() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".kimi-code/config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, OWN).unwrap();
        let result = sync(&env_at(home.path()), "astra", "http://127.0.0.1:8317/v1", "k", &[json!({"id": "gpt-x"})], &["kimi".into()]);
        assert_eq!(result["kimi"]["ok"], false);
        assert_eq!(fs::read_to_string(&config).unwrap(), OWN);
    }

    /// Os blocos que versões antigas gravaram saem; o que é do Kimi e os hooks ficam byte a byte.
    #[test]
    fn purge_removes_every_engine_block_and_keeps_the_rest() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".kimi-code/config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, format!("{OWN}{}{}{HOOKS}", engine_block("astra"), engine_block("sol"))).unwrap();
        let mut removed = purge_kimi(&env_at(home.path())).unwrap();
        removed.sort();
        assert_eq!(removed, ["astra", "sol"]);
        assert_eq!(fs::read_to_string(&config).unwrap(), format!("{OWN}{HOOKS}"));
    }

    fn write_engine(home: &Path, name: &str, key: &str) {
        let path = home.join(".claude/engines.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, json!({name: {"base_url": "http://127.0.0.1:8317", "api_key": key, "model": "gpt-x"}}).to_string()).unwrap();
    }

    /// O Kimi regrava o arquivo sem comentários: o motor que ficou sem marcador sai pelo nome e pela
    /// chave dele; provedor pessoal com o mesmo nome e outra chave fica.
    #[test]
    fn purge_removes_an_engine_whose_markers_the_kimi_rewrite_dropped() {
        let home = tempfile::tempdir().unwrap();
        write_engine(home.path(), "astra", "k");
        let config = home.path().join(".kimi-code/config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        let unmarked = "[providers.astra]\ntype = \"kimi\"\napi_key = \"k\"\nbase_url = \"http://127.0.0.1:8317/v1\"\n\n[models.\"astra/gpt-x\"]\nprovider = \"astra\"\nmodel = \"gpt-x\"\nmax_context_size = 128000\ndisplay_name = \"gpt-x\"\n\n";
        fs::write(&config, format!("{OWN}{unmarked}{HOOKS}")).unwrap();
        assert_eq!(purge_kimi(&env_at(home.path())).unwrap(), ["astra"]);
        let fixed = fs::read_to_string(&config).unwrap();
        assert!(fixed.starts_with(OWN) && !fixed.contains("astra") && fixed.ends_with(HOOKS.trim_start()), "{fixed}");
        assert!(toml::from_str::<Value>(&fixed).is_ok());

        write_engine(home.path(), "astra", "outra");
        fs::write(&config, format!("{OWN}{unmarked}{HOOKS}")).unwrap();
        assert!(purge_kimi(&env_at(home.path())).unwrap().is_empty());
        assert_eq!(fs::read_to_string(&config).unwrap(), format!("{OWN}{unmarked}{HOOKS}"));
    }

    /// Provedor que a leitura por linha não consegue tirar (tabela inline) volta como erro, sem
    /// regravar o arquivo nem dizer que saiu.
    #[test]
    fn purge_reports_an_engine_it_could_not_take_out() {
        let home = tempfile::tempdir().unwrap();
        write_engine(home.path(), "astra", "k");
        let config = home.path().join(".kimi-code/config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        let inline = format!("{OWN}[providers]\nastra = {{ type = \"kimi\", api_key = \"k\", base_url = \"http://127.0.0.1:8317/v1\" }}\n");
        fs::write(&config, &inline).unwrap();
        let error = purge_kimi(&env_at(home.path())).unwrap_err();
        assert!(error.contains("astra"), "{error}");
        assert_eq!(fs::read_to_string(&config).unwrap(), inline);
    }

    /// Um bloco quebrado não impede tirar os outros; o erro volta com o que foi tirado.
    #[test]
    fn purge_goes_on_past_a_broken_block() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".kimi-code/config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        let broken = engine_block("astra").replace(&markers("astra").1, "");
        fs::write(&config, format!("{OWN}{}{broken}", engine_block("sol"))).unwrap();
        let error = purge_kimi(&env_at(home.path())).unwrap_err();
        assert!(error.contains("astra") && error.contains("bloco-incompleto") && error.contains("sol"), "{error}");
        assert!(!fs::read_to_string(&config).unwrap().contains("provedor sol"));
    }

    /// Sem Kimi instalado, ou sem bloco do Hangar, não há o que tirar nem arquivo a criar.
    #[test]
    fn purge_without_kimi_or_without_blocks_touches_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert!(purge_kimi(&env_at(home.path())).unwrap().is_empty());
        let config = home.path().join(".kimi-code/config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, OWN).unwrap();
        assert!(purge_kimi(&env_at(home.path())).unwrap().is_empty());
        assert_eq!(fs::read_to_string(&config).unwrap(), OWN);
        assert!(!home.path().join(".kimi-code/config.toml.bak-hangar").exists());
    }
}

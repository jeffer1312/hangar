use crate::accounts::{AccountService, environment::AccountEnvironment};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::LazyLock};
use hangar_cliproxy_setup::login::{note_accepted, note_rejected, rejected};
use super::store;
use crate::orchestration::truthy;

static PREFIX: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\A[A-Za-z0-9][A-Za-z0-9_.-]{0,63}\z").unwrap());
static LEGACY: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\Ahangar-[0-9a-f]{24}\z").unwrap());

#[derive(Clone)]
pub struct Local { pub base_url: String, pub api_key: String }

pub fn local(env: &AccountEnvironment) -> Result<Option<Local>, String> {
    use hangar_cliproxy_setup::config::{self, ReadError};
    let path = config::path(&env.home);
    let config = match config::read(&env.home) {
        Ok(Some(config)) => config, Ok(None) => return Ok(None),
        Err(ReadError::Io(e)) => return Err(format!("{}: {e}", path.display())),
        Err(ReadError::InvalidYaml) => return Err(format!("{}: YAML inválido", path.display())),
    };
    let api_key = config.client_keys.into_iter().next().ok_or_else(|| format!("{}: sem api-keys", path.display()))?;
    let base_url = config.base_url.map_err(|_| format!("{}: port inválida", path.display()))?;
    Ok(Some(Local { base_url, api_key }))
}

pub fn is_local(env: &AccountEnvironment, base: &str) -> Result<bool, String> { Ok(serves(local(env)?.as_ref(), base)) }
pub fn serves(local: Option<&Local>, base: &str) -> bool { local.is_some_and(|l| store::normalize_base(base.trim()) == l.base_url) }

pub fn credentials(env: &AccountEnvironment) -> Result<Vec<(PathBuf, Value)>, String> {
    let error = || "CLIProxyAPI: não consegui ler as credenciais locais".to_string();
    // As contas ficam onde o `auth-dir` do config manda; config ilegível já aparece em `local` com a causa.
    use hangar_cliproxy_setup::config;
    let dir = config::accounts_dir(&env.home, config::read(&env.home).ok().flatten().as_ref());
    let mut paths: Vec<_> = match fs::read_dir(&dir) {
        Ok(entries) => entries.map(|e| e.map(|e| e.path())).collect::<Result<Vec<_>, _>>().map_err(|_| error())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()), Err(_) => return Err(error()),
    };
    paths.retain(|p| p.extension().is_some_and(|e| e == "json")); paths.sort();
    paths.into_iter().map(|p| {
        if p.is_symlink() { return Err("CLIProxyAPI: credencial por link não é permitida".into()); }
        let data: Value = serde_json::from_slice(&fs::read(&p).map_err(|_| error())?).map_err(|_| error())?;
        if !data.is_object() { return Err("CLIProxyAPI: arquivo de credencial inválido".into()); }
        if data.get("disabled").is_some_and(|v| !v.is_boolean()) { return Err("CLIProxyAPI: campo disabled inválido".into()); }
        Ok((p, data))
    }).collect()
}

pub fn associated(accounts: &AccountService, records: &[(PathBuf, Value)]) -> Result<Vec<(Value, usize)>, String> {
    let visible = accounts.visible_codex_accounts().map_err(|e| e.message)?.into_iter().map(|a| (a.id,a.home)).collect();
    entries(visible, records)
}

/// Conta ChatGPT conectada só no proxy, sem conta Codex no Hangar.
pub const PROXY_ONLY: &str = "cpa-";
/// Só a forma que `proxy_only_id` gera: uma conta Codex chamada `cpa-x` continua sendo Codex.
pub fn is_proxy_only(account: &str) -> bool {
    static ID: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"\Acpa-(?:[a-z0-9]{1,12}-)?[0-9a-f]{12}\z").unwrap());
    ID.is_match(account)
}

/// Sai de dentro da credencial, não do nome do arquivo: refazer o login troca o arquivo e não a conta.
/// O começo do e-mail vai junto porque o id aparece cru no rótulo da sessão.
fn proxy_only_id(record: &Value) -> Option<String> {
    let id = record["account_id"].as_str().filter(|s| !s.is_empty())?;
    let email = record["email"].as_str().unwrap_or("");
    let digest = ring::digest::digest(&ring::digest::SHA256, format!("{id}\n{email}").as_bytes());
    let hash: String = digest.as_ref()[..6].iter().map(|b| format!("{b:02x}")).collect();
    let local: String = email.split('@').next().unwrap_or("").to_lowercase().chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).take(12).collect();
    Some(if local.is_empty() { format!("{PROXY_ONLY}{hash}") } else { format!("{PROXY_ONLY}{local}-{hash}") })
}

/// As contas Codex do Hangar que casam com uma credencial, e as credenciais que sobram como contas só do proxy.
pub fn entries(visible: Vec<(String,PathBuf)>, records: &[(PathBuf,Value)]) -> Result<Vec<(Value,usize)>,String> {
    let mut result = associate(visible, records)?;
    let taken: std::collections::HashSet<usize> = result.iter().map(|(_, i)| *i).collect();
    let loose: Vec<(String, usize)> = records.iter().enumerate()
        .filter(|(i, (_, r))| !taken.contains(i) && r["type"] == "codex" && r["disabled"] != true)
        .filter_map(|(i, (_, r))| proxy_only_id(r).map(|id| (id, i))).collect();
    // Credencial solta com defeito fica de fora sozinha: nunca derruba as contas que estão boas. A duplicada vai
    // ao log e o `resolve` diz o motivo, em vez de a conta só sumir.
    let mut warned = std::collections::HashSet::new();
    for (id, i) in &loose {
        if loose.iter().filter(|(other, _)| other == id).count() > 1 {
            if warned.insert(id) {
                tracing::warn!(account = %id, "CLIProxyAPI: mais de uma credencial ativa para a mesma conta só do proxy; ela fica fora do seletor");
            }
            continue;
        }
        let record = &records[*i].1;
        let email = record["email"].as_str().unwrap_or("");
        let prefix = record["prefix"].as_str().filter(|p| PREFIX.is_match(p)
            && !records.iter().enumerate().any(|(j, (_, r))| j != *i && r["prefix"] == *p));
        result.push((json!({"account":id,"credential_id":format!("cliproxy:{id}"),"email":email,"label":if email.is_empty() { id.as_str() } else { email },"prefix":prefix,"plan":record["plan_type"]}), *i));
    }
    Ok(result)
}

fn checked_prefixes(entries: &[(Value, usize)], records: &[(PathBuf, Value)]) -> Result<(), String> {
    for (entry, index) in entries {
        if entry["prefix"].is_null() || entry["credential_id"].as_str().is_some_and(|c| c.starts_with("cliproxy:")) { continue; }
        let prefix = entry["prefix"].as_str().filter(|s| PREFIX.is_match(s)).ok_or("CLIProxyAPI: conta sem prefixo válido; prepare as contas primeiro")?;
        if records.iter().enumerate().any(|(i, (_, r))| i != *index && r["prefix"] == prefix) { return Err("CLIProxyAPI: prefixo compartilhado entre credenciais; prepare as contas primeiro".into()); }
    }
    Ok(())
}

fn associate(visible: Vec<(String,PathBuf)>, records: &[(PathBuf,Value)]) -> Result<Vec<(Value,usize)>,String> {
    let mut result = Vec::new();
    for (account_id, account_home) in visible {
        let bad = || format!("CLIProxyAPI: login Codex ilegível na conta {}", account_id);
        let data: Value = match fs::read(account_home.join("auth.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| bad())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue, Err(_) => return Err(bad()),
        };
        let Some(id) = data["tokens"]["account_id"].as_str().filter(|s| !s.is_empty()) else { continue };
        let matches: Vec<_> = records.iter().enumerate().filter(|(_, (_, r))| r["type"] == "codex" && r["account_id"] == id && r["disabled"] != true).collect();
        if matches.len() > 1 { return Err(format!("CLIProxyAPI: mais de uma credencial ativa para a conta {}", account_id)); }
        let Some(&(i, (_, record))) = matches.first() else { continue };
        if result.iter().any(|(_, previous)| *previous == i) { return Err("CLIProxyAPI: a mesma credencial corresponde a mais de uma conta Hangar".into()); }
        let email = record["email"].as_str().unwrap_or("");
        let prefix = if truthy(&record["prefix"]) { record["prefix"].clone() } else { Value::Null };
        let home = crate::accounts::catalog::resolved(&account_home);
        result.push((json!({"account":account_id,"credential_id":format!("codex:{}",home.display()),"email":email,"label":if email.is_empty() { &account_id } else { email },"prefix":prefix,"plan":record["plan_type"]}), i));
    }
    Ok(result)
}

pub fn resolve(accounts: &AccountService, account: &str, home: Option<&str>) -> Result<Value,String> {
    if !crate::accounts::catalog::valid_name(account) { return Err("CLIProxyAPI: identificador de conta inválido".into()); }
    let entries = if let Some(home) = home {
        let path = PathBuf::from(home);
        if home.contains(['\n','\r','\0']) || !path.is_absolute() { return Err("CLIProxyAPI: raiz Codex inválida".into()); }
        let records = credentials(&accounts.env)?;
        let entries = associate(vec![(account.into(),path)],&records)?;
        for (entry,index) in &entries {
            let prefix = entry["prefix"].as_str().filter(|p| PREFIX.is_match(p)).ok_or("CLIProxyAPI: conta sem prefixo válido; prepare as contas primeiro")?;
            if records.iter().enumerate().any(|(i,(_,r))| i != *index && r["prefix"] == prefix) { return Err("CLIProxyAPI: prefixo compartilhado entre credenciais; prepare as contas primeiro".into()); }
        }
        entries.into_iter().map(|(e,_)|e).collect()
    } else if is_proxy_only(account) {
        // Sem contas Codex na conta: a credencial continua achável depois que alguém cadastra a conta Codex dela.
        let records = credentials(&accounts.env)?;
        let duplicated = records.iter()
            .filter(|(_, r)| r["type"] == "codex" && r["disabled"] != true && proxy_only_id(r).as_deref() == Some(account))
            .count() > 1;
        if duplicated { return Err(format!("CLIProxyAPI: mais de uma credencial ativa para a conta {account}")); }
        entries(Vec::new(), &records)?.into_iter().map(|(e, _)| e).collect()
    } else { list_accounts(accounts)? };
    let entry = entries.into_iter().find(|e|e["account"] == account).ok_or_else(||format!("CLIProxyAPI: conta {account} desconhecida, desconectada ou desativada"))?;
    if !truthy(&entry["prefix"]) { return Err("CLIProxyAPI: conta sem prefixo; prepare as contas primeiro".into()); }
    Ok(entry)
}

/// Planos que o ChatGPT atende em Ultra Fast (os mesmos que o proxy aceita para `ultrafast`).
const ULTRAFAST_PLANS: [&str; 2] = ["pro", "promax"];

/// A conta fixa pode pedir Ultra Fast: plano dela e a regra que repõe o `ultrafast` no config do proxy.
pub fn ultrafast(accounts: &AccountService, account: &str) -> bool {
    let rule = match hangar_cliproxy_setup::config::read(&accounts.env.home) {
        Ok(config) => config.is_some_and(|c| c.ultrafast_rule),
        Err(error) => { tracing::warn!(?error, "cliproxy: config ilegível; Ultra Fast fica de fora"); false }
    };
    rule && resolve(accounts, account, None).is_ok_and(|e| e["plan"].as_str().is_some_and(|p| ULTRAFAST_PLANS.contains(&p)))
}

pub fn prefix_model(model: &str, prefix: &str) -> Result<String,String> {
    static MODEL: LazyLock<regex::Regex> = LazyLock::new(||regex::Regex::new(r"\A[A-Za-z0-9][A-Za-z0-9_.:\[\]-]*\z").unwrap());
    if !PREFIX.is_match(prefix) { return Err("CLIProxyAPI: prefixo inválido".into()); }
    let model = model.strip_prefix(&format!("{prefix}/")).unwrap_or(model);
    if !MODEL.is_match(model) { return Err("CLIProxyAPI: modelo inválido ou prefixo de outra conta".into()); }
    Ok(format!("{prefix}/{model}"))
}

pub fn list_accounts(accounts: &AccountService) -> Result<Vec<Value>, String> {
    let records = credentials(&accounts.env)?;
    let associated = associated(accounts, &records)?;
    checked_prefixes(&associated, &records)?;
    Ok(associated.into_iter().map(|(e, _)| e).collect())
}

fn management_path(env: &AccountEnvironment) -> PathBuf { env.home.join(".claude").join("cliproxy-management.key") }
pub fn management_key(env: &AccountEnvironment) -> Option<String> { fs::read_to_string(management_path(env)).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) }
pub fn set_management_key(env: &AccountEnvironment, key: &str) -> Result<(), String> {
    let key = key.trim(); let path = management_path(env);
    if key.is_empty() { return match fs::remove_file(path) { Ok(()) => Ok(()), Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()), Err(e) => Err(e.to_string()) }; }
    if !key.is_ascii() || key.contains(['\n','\r','\0']) { return Err("senha de gerenciamento só com caracteres ASCII, sem quebra de linha".into()); }
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    crate::runtime::queue::atomic_write(&path,key.as_bytes()).map_err(|e| e.to_string())
}

fn unnamed(accounts: &AccountService) -> Result<Vec<(Value, PathBuf)>, String> {
    let records = credentials(&accounts.env)?;
    Ok(associated(accounts, &records)?.into_iter().filter(|(e,_)| !truthy(&e["prefix"]) || e["prefix"].as_str().is_some_and(|s| LEGACY.is_match(s)))
        .map(|(e,i)| (e,records[i].0.clone())).collect())
}

pub fn prefix_from_email(email: &str, taken: &std::collections::HashSet<String>) -> Option<String> {
    static CLEAN: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new("[^a-z0-9_.-]+").unwrap());
    let email = email.trim().to_lowercase(); let (local, domain) = email.split_once('@').unwrap_or((&email,""));
    for raw in [local.to_owned(),format!("{local}-{}",domain.split('.').next().unwrap_or(""))] {
        let cleaned: String = CLEAN.replace_all(&raw,"-").trim_matches(['-','.','_']).chars().take(64).collect();
        if PREFIX.is_match(&cleaned) && !taken.contains(&cleaned) { return Some(cleaned); }
    }
    None
}

async fn name_accounts(accounts: &AccountService) -> Result<(), String> {
    let pending = unnamed(accounts)?;
    let Some(key) = management_key(&accounts.env) else { return Ok(()) };
    let Some(local) = local(&accounts.env)? else { return Ok(()) };
    if pending.is_empty() { return Ok(()); }
    if rejected(&key) { return Err("CLIProxyAPI recusou a senha de gerenciamento".into()); }
    let mut taken = credentials(&accounts.env)?.iter().filter_map(|(_,v)| v["prefix"].as_str().map(str::to_owned)).collect();
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
    let mut errors = Vec::new(); let mut named = 0;
    for (entry, path) in &pending {
        let Some(prefix) = prefix_from_email(entry["email"].as_str().unwrap_or(""), &taken) else { errors.push(format!("{}: sem e-mail que sirva de nome",entry["label"].as_str().unwrap_or(""))); continue };
        let response = client.patch(format!("{}/v0/management/auth-files/fields",local.base_url)).header("X-Management-Key", &key)
            .json(&json!({"name":path.file_name().unwrap().to_string_lossy(),"prefix":prefix})).send().await
            .map_err(|_| "CLIProxyAPI não respondeu ao nomear as contas".to_owned())?;
        match response.status().as_u16() {
            401 => { note_rejected(&key); return Err("CLIProxyAPI recusou a senha de gerenciamento".into()); }
            403 => return Err("CLIProxyAPI bloqueou o gerenciamento (403): confira o remote-management no config".into()),
            200..=299 => { note_accepted(&key); taken.insert(prefix); named += 1; }
            code => errors.push(format!("{}: HTTP {code}",entry["label"].as_str().unwrap_or(""))),
        }
    }
    let mut stubborn: Vec<_> = unnamed(accounts)?.iter().filter_map(|(e,_)| e["email"].as_str().map(str::to_owned))
        .filter(|email| pending.iter().any(|(e,_)| e["email"] == *email) && prefix_from_email(email,&Default::default()).is_some()).collect();
    stubborn.sort(); stubborn.dedup();
    if named > 0 && !stubborn.is_empty() { errors.push(format!("o proxy aceitou o nome mas não o gravou em {}",stubborn.join(", "))); }
    if !errors.is_empty() { return Err(format!("{named} de {} contas nomeadas — {}",pending.len(),errors.join("; "))); }
    Ok(())
}

pub async fn naming(accounts: &AccountService) -> Value {
    let mut error = name_accounts(accounts).await.err();
    let count = match unnamed(accounts) { Ok(entries) => Some(entries.len()), Err(e) => { if error.is_none() { error = Some(e); } None } };
    json!({"unnamed_accounts":count,"management_key_set":management_key(&accounts.env).is_some(),"naming_error":error})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_only_account_with_two_active_credentials_says_why_instead_of_vanishing() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".cli-proxy-api");
        fs::create_dir_all(&dir).unwrap();
        let record = json!({"type":"codex","account_id":"acc-1","email":"ana@example.com","prefix":"ana"});
        fs::write(dir.join("a.json"), record.to_string()).unwrap();
        let accounts = AccountService::new(AccountEnvironment::from_map([("HOME".into(), home.path().to_string_lossy().into_owned())].into()));
        let id = proxy_only_id(&record).unwrap();
        assert_eq!(resolve(&accounts, &id, None).unwrap()["account"], id.as_str());
        fs::write(dir.join("b.json"), json!({"type":"codex","account_id":"acc-1","email":"ana@example.com","prefix":"ana2"}).to_string()).unwrap();
        let error = resolve(&accounts, &id, None).unwrap_err();
        assert!(error.contains("mais de uma credencial ativa"), "{error}");
    }
}

//! `~/.cli-proxy-api/config.yaml`: leitura nos formatos legado e v8, e preparo das chaves que o
//! Hangar precisa sem tocar no que já existe.
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub const DEFAULT_PORT: u64 = 8317;

pub fn path(home: &Path) -> PathBuf {
    home.join(".cli-proxy-api").join("config.yaml")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Legacy,
    V8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    pub format: Format,
    /// Porta inválida não esconde as chaves: quem só precisa delas continua lendo.
    pub base_url: Result<String, InvalidPort>,
    pub client_keys: Vec<String>,
    pub management_key_set: bool,
    /// A regra que troca o pedido marcado com [`SPEED_HEADER`] por `service_tier: ultrafast` está no config.
    pub ultrafast_rule: bool,
}

/// O tradutor Claude → Codex do proxy descarta `ultrafast`; a regra de `payload` do config o põe de volta só
/// nos pedidos com este cabeçalho, que a sessão em Ultra Fast manda.
pub const SPEED_HEADER: &str = "X-Hangar-Speed";
pub const ULTRAFAST: &str = "ultrafast";

/// O valor de `payload`: igual no formato legado (na raiz) e no v8 (em `requests`).
const ULTRAFAST_PAYLOAD: &str = concat!(
    "override:\n",
    "  - models:\n",
    "      - name: \"*\"\n",
    "        protocol: \"codex\"\n",
    "        headers:\n",
    "          X-Hangar-Speed: \"ultrafast\"\n",
    "    params:\n",
    "      service_tier: \"ultrafast\"\n",
);

fn has_ultrafast_rule(payload: &Value) -> bool {
    payload["override"].as_array().into_iter().flatten().any(|rule| {
        rule["params"]["service_tier"] == ULTRAFAST
            && rule["models"].as_array().into_iter().flatten().any(|m| m["headers"][SPEED_HEADER] == ULTRAFAST)
    })
}

/// Onde o proxy lê o `payload`: o v8 presente vence o legado.
fn payload(data: &Value) -> &Value {
    field(data, "requests", "payload", &data["payload"])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPort;

#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    InvalidYaml,
}

pub fn read(home: &Path) -> Result<Option<ProxyConfig>, ReadError> {
    match fs::read_to_string(path(home)) {
        Ok(text) => Ok(parse(&text)?.map(|(config, _)| config)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ReadError::Io(e)),
    }
}

fn parse(text: &str) -> Result<Option<(ProxyConfig, Value)>, ReadError> {
    let data: Value = serde_saphyr::from_str(text).map_err(|_| ReadError::InvalidYaml)?;
    if !data.is_object() {
        return Ok(None);
    }
    Ok(Some((describe(&data), data)))
}

fn format(data: &Value) -> Format {
    let version = &data["config-version"];
    let version = version.as_u64().or_else(|| version.as_str().and_then(|s| s.trim().parse().ok()));
    if version.is_some_and(|v| v >= 8) { Format::V8 } else { Format::Legacy }
}

/// O campo v8 presente vence o legado, mesmo vazio ou falso (regra do próprio proxy).
fn field<'a>(data: &'a Value, parent: &str, key: &str, legacy: &'a Value) -> &'a Value {
    match data[parent].as_object().and_then(|o| o.get(key)) {
        Some(value) => value,
        None => legacy,
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn describe(data: &Value) -> ProxyConfig {
    let format = format(data);
    let host = field(data, "server", "host", &data["host"]);
    let port = field(data, "server", "port", &data["port"]);
    let tls = match data["server"].as_object().and_then(|o| o.get("tls")) {
        Some(tls) => &tls["enable"],
        None => &data["tls"]["enable"],
    };
    // Na v8 a raiz `api-keys` é grupo de provedor, nunca chave de cliente.
    let keys = match data["access"].as_object().and_then(|o| o.get("api-keys")) {
        Some(keys) => keys,
        None if format == Format::Legacy => &data["api-keys"],
        None => &Value::Null,
    };
    let secret = field(data, "management", "secret-key", &data["remote-management"]["secret-key"]);
    ProxyConfig {
        format,
        base_url: base_url(host, port, tls),
        client_keys: key_list(keys),
        management_key_set: secret.as_str().is_some_and(|s| !s.trim().is_empty()),
        ultrafast_rule: has_ultrafast_rule(payload(data)),
    }
}

fn key_list(list: &Value) -> Vec<String> {
    // Chave sem aspas só de dígitos vira número no YAML e continua sendo chave.
    let as_key = |v: &Value| v.as_str().map(str::to_owned).or_else(|| v.is_number().then(|| v.to_string()));
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(as_key)
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
        .collect()
}

fn base_url(host: &Value, port: &Value, tls: &Value) -> Result<String, InvalidPort> {
    let mut host = host.as_str().unwrap_or("").trim().to_owned();
    if matches!(host.as_str(), "" | "0.0.0.0" | "::") {
        host = "127.0.0.1".into();
    }
    if host.contains(':') && !host.starts_with('[') {
        host = format!("[{host}]");
    }
    let port = if !truthy(port) {
        DEFAULT_PORT
    } else if *port == Value::Bool(true) {
        1
    } else {
        port.as_u64()
            .or_else(|| port.as_f64().filter(|f| f.fract() == 0.0 && *f > 0.0).map(|f| f as u64))
            .or_else(|| port.as_str().and_then(|s| s.trim().parse().ok()))
            .unwrap_or(0)
    };
    if !(1..65536).contains(&port) {
        return Err(InvalidPort);
    }
    let scheme = if *tls == Value::Bool(true) { "https" } else { "http" };
    Ok(format!("{scheme}://{host}:{port}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ensured {
    pub client_key: String,
    /// Só existe quando o Hangar acabou de gerar: a que já estava no arquivo o proxy guarda em hash.
    pub management_key: Option<String>,
    pub changed: bool,
    pub backup: Option<PathBuf>,
}

#[derive(Debug)]
pub enum EnsureError {
    Io(io::Error),
    InvalidYaml,
    /// O arquivo usa uma forma que a edição por linhas não altera com segurança.
    Unsupported,
}

impl From<io::Error> for EnsureError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Gera o config que falta ou completa o existente; `now_unix` dá nome ao backup. A senha nova vai a
/// `persist_management` antes do config: depois de gravada o proxy só guarda o hash dela.
/// `ultrafast_rule`: o proxy entende `headers` nas regras (v8 em diante); num mais velho a regra valeria para todo pedido.
pub fn ensure(home: &Path, now_unix: u64, persist_management: impl FnOnce(&str) -> io::Result<()>, ultrafast_rule: bool) -> Result<Ensured, EnsureError> {
    let file = path(home);
    let text = match fs::read_to_string(&file) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let parsed = match &text {
        Some(text) => parse(text).map_err(|_| EnsureError::InvalidYaml)?,
        None => None,
    };
    let Some((config, data)) = parsed else {
        if text.as_deref().is_some_and(|t| !t.trim().is_empty() && !is_blank_yaml(t)) {
            return Err(EnsureError::Unsupported);
        }
        let (client_key, management_key) = (secret(), secret());
        let backup = match text { Some(_) => Some(backup(&file, now_unix)?), None => None };
        persist_management(&management_key)?;
        write(&file, &template(&client_key, &management_key, &home.join(".cli-proxy-api"), ultrafast_rule))?;
        return Ok(Ensured { client_key, management_key: Some(management_key), changed: true, backup });
    };
    let text = text.unwrap_or_default();
    let new_client = config.client_keys.is_empty().then(secret);
    let new_management = (!config.management_key_set).then(secret);
    // `payload` com regras da pessoa fica como está: o Ultra Fast só não aparece para esse proxy.
    let v8 = config.format == Format::V8;
    let mut edited = text.clone();
    // `payload` com regras da pessoa fica como está: o Ultra Fast só não aparece para esse proxy.
    if ultrafast_rule && payload(&data).is_null() {
        let with_rule = match v8 || data.get("requests").is_some() {
            true => set_child(&edited, Some("requests"), "payload", Child::Block(ULTRAFAST_PAYLOAD)),
            false => set_child(&edited, None, "payload", Child::Block(ULTRAFAST_PAYLOAD)),
        };
        // Regra que não se confirma na releitura fica de fora; as chaves seguem.
        if let Some(with_rule) = with_rule.ok().filter(|t| parse(t).ok().flatten().is_some_and(|(c, _)| c.ultrafast_rule)) {
            edited = with_rule;
        }
    }
    if edited == text && new_client.is_none() && new_management.is_none() {
        return Ok(Ensured { client_key: config.client_keys[0].clone(), management_key: None, changed: false, backup: None });
    }
    if let Some(key) = &new_client {
        // Raiz com grupo de provedor não é lista de chaves vazia: substituí-la apagaria os provedores.
        let root_is_keys = match data.get("api-keys") {
            None | Some(Value::Null) => true,
            Some(Value::Array(items)) => items.iter().all(|v| v.is_string() || v.is_number()),
            Some(_) => false,
        };
        let parent = (v8 || data.get("access").is_some() || !root_is_keys).then_some("access");
        edited = set_child(&edited, parent, "api-keys", Child::List(key))?;
    }
    if let Some(key) = &new_management {
        let parent = if v8 || data.get("management").is_some() { "management" } else { "remote-management" };
        edited = set_child(&edited, Some(parent), "secret-key", Child::Scalar(key))?;
    }
    let confirmed = parse(&edited).ok().flatten().map(|(c, _)| c).is_some_and(|c| {
        new_client.as_ref().is_none_or(|k| c.client_keys.first() == Some(k)) && c.management_key_set
    });
    if !confirmed {
        return Err(EnsureError::Unsupported);
    }
    let backup = backup(&file, now_unix)?;
    if let Some(key) = &new_management {
        persist_management(key)?;
    }
    write(&file, &edited)?;
    let client_key = new_client.unwrap_or_else(|| config.client_keys[0].clone());
    Ok(Ensured { client_key, management_key: new_management, changed: true, backup: Some(backup) })
}

/// Troca a senha de gerenciamento por uma nova: o proxy guarda só o hash, então a antiga não volta.
/// Mesma ordem do `ensure`: cópia, senha entregue ao Hangar, config gravado. Devolve a cópia.
pub fn reset_management(home: &Path, now_unix: u64, persist_management: impl FnOnce(&str) -> io::Result<()>) -> Result<PathBuf, EnsureError> {
    let file = path(home);
    let text = fs::read_to_string(&file)?;
    let Some((_, data)) = parse(&text).map_err(|_| EnsureError::InvalidYaml)? else { return Err(EnsureError::Unsupported) };
    let key = secret();
    let parent = if format(&data) == Format::V8 || data.get("management").is_some() { "management" } else { "remote-management" };
    let edited = set_child(&text, Some(parent), "secret-key", Child::Scalar(&key))?;
    if !parse(&edited).ok().flatten().is_some_and(|(_, d)| field(&d, "management", "secret-key", &d["remote-management"]["secret-key"]).as_str() == Some(key.as_str())) {
        return Err(EnsureError::Unsupported);
    }
    let backup = backup(&file, now_unix)?;
    persist_management(&key)?;
    write(&file, &edited)?;
    Ok(backup)
}

fn is_blank_yaml(text: &str) -> bool {
    text.lines().all(|l| matches!(l.trim(), "" | "---" | "~" | "null") || l.trim_start().starts_with('#'))
}

fn secret() -> String {
    hangar_core::ids::random_hex(32)
}

/// A pasta das contas vai explícita: o `~` padrão é o da pasta pessoal do processo do proxy, e o Hangar
/// lê as contas da pasta dele. Aspas simples porque o YAML trata `\` como escape entre aspas duplas.
fn template(client_key: &str, management_key: &str, auth_dir: &Path, ultrafast_rule: bool) -> String {
    let auth_dir = auth_dir.to_string_lossy().replace('\'', "''");
    let rule = if ultrafast_rule { render(0, "requests", &Child::Block(&render(0, "payload", &Child::Block(ULTRAFAST_PAYLOAD)))) } else { String::new() };
    format!(
        "# Gerado pelo Hangar. Formato v8 do CLIProxyAPI (config.example.yaml do projeto).\n\
         config-version: 8\n\
         server:\n  host: \"127.0.0.1\"\n  port: {DEFAULT_PORT}\n\
         management:\n  allow-remote: false\n  secret-key: \"{management_key}\"\n\
         access:\n  api-keys:\n    - \"{client_key}\"\n\
         oauth:\n  auth-dir: '{auth_dir}'\n\
         {rule}"
    )
}

const BACKUP_PREFIX: &str = "config.yaml.hangar-bak-";
const BACKUPS_KEPT: usize = 3;

/// O backup carrega as mesmas chaves do config: fica só do dono, e só os mais novos ficam.
fn backup(file: &Path, now_unix: u64) -> io::Result<PathBuf> {
    let target = file.with_file_name(format!("{BACKUP_PREFIX}{now_unix}"));
    fs::copy(file, &target)?;
    private(&target)?;
    let dir = file.parent().expect("config tem pasta");
    let mut old: Vec<(u64, PathBuf)> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| p.file_name()?.to_str()?.strip_prefix(BACKUP_PREFIX)?.parse().ok().map(|n| (n, p.clone())))
        .collect();
    old.sort();
    for (_, path) in old.iter().rev().skip(BACKUPS_KEPT) {
        fs::remove_file(path)?;
    }
    Ok(target)
}

fn write(file: &Path, text: &str) -> io::Result<()> {
    fs::create_dir_all(file.parent().expect("config tem pasta"))?;
    hangar_core::fs::atomic_write(file, text.as_bytes())?;
    private(file)
}

/// Chave de cliente e senha de gerenciamento ficam em texto no arquivo.
fn private(file: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o600))
    }
    #[cfg(windows)]
    {
        hangar_core::private_acl::restrict_path(file, "")
    }
}

enum Child<'a> {
    Scalar(&'a str),
    List(&'a str),
    /// Mapa já escrito em YAML, recuado a partir da coluna zero.
    Block(&'a str),
}

fn render(indent: usize, key: &str, child: &Child) -> String {
    let pad = " ".repeat(indent);
    match child {
        Child::Scalar(v) => format!("{pad}{key}: \"{v}\"\n"),
        Child::List(v) => format!("{pad}{key}:\n{pad}  - \"{v}\"\n"),
        Child::Block(v) => format!("{pad}{key}:\n{}", v.lines().map(|l| format!("{pad}  {l}\n")).collect::<String>()),
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn is_content(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && !t.starts_with('#')
}

/// Chave de mapa em bloco: `nome:` ou `nome: valor`; item de lista e comentário não têm chave.
fn key_of(line: &str) -> Option<(&str, &str)> {
    let t = line.trim_start();
    if t.starts_with(['-', '#']) {
        return None;
    }
    let (key, rest) = t.split_once(':')?;
    if !(rest.is_empty() || rest.starts_with([' ', '\n', '\r'])) {
        return None;
    }
    let rest = rest.split(" #").next().unwrap_or("").trim();
    Some((key.trim().trim_matches(['"', '\'']), rest))
}

fn lines_of(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
    if let Some(last) = lines.last_mut().filter(|l| !l.ends_with('\n')) {
        last.push('\n');
    }
    lines
}

/// Último índice de conteúdo que pertence ao bloco aberto em `start` (mais indentado, ou item de
/// lista no mesmo recuo).
fn block_last(lines: &[String], start: usize, indent: usize, end: usize) -> usize {
    let mut last = start;
    for (j, line) in lines.iter().enumerate().take(end).skip(start + 1) {
        if !is_content(line) {
            continue;
        }
        let i = indent_of(line);
        if i > indent || (i == indent && line.trim_start().starts_with('-')) {
            last = j;
        } else {
            break;
        }
    }
    last
}

fn set_child(text: &str, parent: Option<&str>, key: &str, child: Child) -> Result<String, EnsureError> {
    let mut lines = lines_of(text);
    let at_root = |l: &String, name: &str| indent_of(l) == 0 && key_of(l).is_some_and(|(k, _)| k == name);
    let Some(parent) = parent else {
        match lines.iter().position(|l| at_root(l, key)) {
            Some(i) => {
                let last = block_last(&lines, i, 0, lines.len());
                lines.splice(i..=last, [render(0, key, &child)]);
            }
            None => lines.push(format!("\n{}", render(0, key, &child))),
        }
        return Ok(lines.concat());
    };
    let Some(p) = lines.iter().position(|l| at_root(l, parent)) else {
        lines.push(format!("\n{parent}:\n{}", render(2, key, &child)));
        return Ok(lines.concat());
    };
    let (_, inline) = key_of(&lines[p]).expect("linha do pai tem chave");
    if !inline.is_empty() {
        if !matches!(inline, "{}" | "null" | "~") {
            return Err(EnsureError::Unsupported);
        }
        lines[p] = format!("{parent}:\n");
    }
    let end = (p + 1..lines.len()).find(|&j| is_content(&lines[j]) && indent_of(&lines[j]) == 0).unwrap_or(lines.len());
    let indent = (p + 1..end).find(|&j| is_content(&lines[j])).map(|j| indent_of(&lines[j])).unwrap_or(2);
    match (p + 1..end).find(|&j| indent_of(&lines[j]) == indent && key_of(&lines[j]).is_some_and(|(k, _)| k == key)) {
        Some(c) => {
            let last = block_last(&lines, c, indent, end);
            lines.splice(c..=last, [render(indent, key, &child)]);
        }
        None => lines.insert(p + 1, render(indent, key, &child)),
    }
    Ok(lines.concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(config: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        if let Some(text) = config {
            fs::create_dir_all(dir.path().join(".cli-proxy-api")).unwrap();
            fs::write(path(dir.path()), text).unwrap();
        }
        dir
    }

    fn keep(_: &str) -> io::Result<()> {
        Ok(())
    }

    #[test]
    fn the_new_password_is_handed_over_before_the_config_holds_it() {
        let dir = home(None);
        let file = path(dir.path());
        let mut seen = None;
        let ensured = ensure(dir.path(), 1, |key| {
            seen = Some((key.to_owned(), file.exists()));
            Ok(())
        }, true)
        .unwrap();
        assert_eq!(seen, Some((ensured.management_key.unwrap(), false)));
        let dir = home(Some("api-keys:\n  - k\n"));
        let refused = ensure(dir.path(), 1, |_| Err(io::Error::other("sem disco")), true);
        assert!(matches!(refused, Err(EnsureError::Io(_))));
        assert_eq!(fs::read_to_string(path(dir.path())).unwrap(), "api-keys:\n  - k\n", "sem a senha guardada, o config não muda");
    }

    #[test]
    fn only_the_newest_backups_stay() {
        let dir = home(Some("api-keys:\n  - k\n"));
        for now in 1..=5 {
            fs::write(path(dir.path()), "api-keys:\n  - k\n").unwrap();
            ensure(dir.path(), now, keep, true).unwrap();
        }
        let mut left: Vec<_> = fs::read_dir(dir.path().join(".cli-proxy-api")).unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap()).filter(|n| n.starts_with(BACKUP_PREFIX)).collect();
        left.sort();
        assert_eq!(left, ["config.yaml.hangar-bak-3", "config.yaml.hangar-bak-4", "config.yaml.hangar-bak-5"]);
    }

    const LEGACY: &str = "host: \"127.0.0.1\"\nport: 8317\nauth-dir: \"~/.cli-proxy-api\"\n# comentário do usuário\nremote-management:\n  allow-remote: false\n  secret-key: \"$2a$10$hash\"\napi-keys:\n  - \"k-legado\"\ndebug: true\n";

    #[test]
    fn reads_the_legacy_layout() {
        let dir = home(Some(LEGACY));
        let config = read(dir.path()).unwrap().unwrap();
        assert_eq!(config.format, Format::Legacy);
        assert_eq!(config.base_url.as_deref(), Ok("http://127.0.0.1:8317"));
        assert_eq!(config.client_keys, ["k-legado"]);
        assert!(config.management_key_set);
    }

    #[test]
    fn reads_the_v8_layout_and_ignores_provider_groups() {
        let dir = home(Some("config-version: 8\nserver:\n  host: \"\"\n  port: 9000\n  tls:\n    enable: true\nmanagement:\n  secret-key: \"\"\naccess:\n  api-keys:\n    - \"k8\"\n    - 12345678\napi-keys:\n  codex:\n    - api-key: upstream\n"));
        let config = read(dir.path()).unwrap().unwrap();
        assert_eq!(config.format, Format::V8);
        assert_eq!(config.base_url.as_deref(), Ok("https://127.0.0.1:9000"));
        assert_eq!(config.client_keys, ["k8", "12345678"]);
        assert!(!config.management_key_set, "secret-key vazio desliga o gerenciamento");
        let dir = home(Some("config-version: 8\napi-keys:\n  - parece-chave\n"));
        assert!(read(dir.path()).unwrap().unwrap().client_keys.is_empty());
    }

    #[test]
    fn present_v8_field_wins_over_the_legacy_one() {
        let dir = home(Some("port: 1111\nserver:\n  port: 2222\naccess:\n  api-keys: []\napi-keys:\n  - antiga\n"));
        let config = read(dir.path()).unwrap().unwrap();
        assert_eq!(config.base_url.as_deref(), Ok("http://127.0.0.1:2222"));
        assert!(config.client_keys.is_empty());
    }

    #[test]
    fn missing_invalid_and_bad_port() {
        assert!(read(home(None).path()).unwrap().is_none());
        assert!(matches!(read(home(Some("a: [")).path()), Err(ReadError::InvalidYaml)));
        let config = read(home(Some("port: 70000\napi-keys:\n  - k\n")).path()).unwrap().unwrap();
        assert_eq!(config.base_url, Err(InvalidPort));
        assert_eq!(config.client_keys, ["k"], "porta inválida não esconde a chave");
    }

    #[test]
    fn ensure_writes_a_v8_config_when_absent() {
        let dir = home(None);
        let ensured = ensure(dir.path(), 1, keep, true).unwrap();
        assert!(ensured.changed && ensured.backup.is_none());
        let management = ensured.management_key.clone().unwrap();
        assert_eq!((ensured.client_key.len(), management.len()), (64, 64));
        let config = read(dir.path()).unwrap().unwrap();
        assert_eq!(config.format, Format::V8);
        assert_eq!(config.base_url.as_deref(), Ok("http://127.0.0.1:8317"));
        assert_eq!(config.client_keys, [ensured.client_key]);
        assert!(config.management_key_set);
        let (_, data) = parse(&fs::read_to_string(path(dir.path())).unwrap()).unwrap().unwrap();
        assert_eq!(data["oauth"]["auth-dir"].as_str(), Some(dir.path().join(".cli-proxy-api").to_str().unwrap()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(path(dir.path())).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn the_accounts_folder_survives_windows_paths_and_quotes() {
        for folder in [r"C:\Users\O'Neil\.cli-proxy-api", r"C:\hp-srv\.cli-proxy-api"] {
            let (_, data) = parse(&template("c", "m", Path::new(folder), false)).unwrap().unwrap();
            assert_eq!(data["oauth"]["auth-dir"].as_str(), Some(folder));
        }
    }

    #[test]
    fn ensure_completes_a_legacy_config_in_its_own_layout() {
        let text = LEGACY.replace("  secret-key: \"$2a$10$hash\"\n", "");
        let dir = home(Some(&text));
        let ensured = ensure(dir.path(), 42, keep, true).unwrap();
        assert_eq!(ensured.client_key, "k-legado");
        let management = ensured.management_key.unwrap();
        let written = fs::read_to_string(path(dir.path())).unwrap();
        assert!(written.contains(&format!("remote-management:\n  secret-key: \"{management}\"\n  allow-remote: false\n")), "{written}");
        assert!(written.contains("# comentário do usuário") && written.contains("debug: true"));
        assert!(!written.lines().any(|l| l == "management:" || l == "access:"), "arquivo legado não ganha campo v8: {written}");
        let backup = ensured.backup.unwrap();
        assert!(backup.ends_with("config.yaml.hangar-bak-42"));
        assert_eq!(fs::read_to_string(backup).unwrap(), text);
    }

    #[test]
    fn ensure_fills_empty_v8_values_without_touching_the_rest() {
        let text = "config-version: 8\nserver:\n  port: 8317\nmanagement:\n  allow-remote: false\n  secret-key: \"\"\naccess:\n  api-keys: []\nrouting:\n  strategy: \"round-robin\"\n";
        let dir = home(Some(text));
        let ensured = ensure(dir.path(), 7, keep, true).unwrap();
        let config = read(dir.path()).unwrap().unwrap();
        assert_eq!(config.client_keys, [ensured.client_key.clone()]);
        assert!(config.management_key_set);
        let written = fs::read_to_string(path(dir.path())).unwrap();
        assert!(written.contains("routing:\n  strategy: \"round-robin\"\n"), "{written}");
        assert!(written.contains("  allow-remote: false\n"));
    }

    #[test]
    fn ensure_accepts_an_empty_inline_parent_and_appends_a_missing_one() {
        let dir = home(Some("config-version: 8\naccess: {}\nmanagement:\n  secret-key: \"x\"\n"));
        let ensured = ensure(dir.path(), 1, keep, true).unwrap();
        assert_eq!(read(dir.path()).unwrap().unwrap().client_keys, [ensured.client_key]);
        let dir = home(Some("config-version: 8\naccess:\n  api-keys:\n    - k\n"));
        let ensured = ensure(dir.path(), 1, keep, true).unwrap();
        assert!(ensured.management_key.is_some());
        assert!(read(dir.path()).unwrap().unwrap().management_key_set);
    }

    #[test]
    fn ensure_keeps_provider_groups_at_the_legacy_root() {
        let text = "port: 8317\napi-keys:\n  - name: provedor\n    base-url: x\nremote-management:\n  secret-key: \"h\"\n";
        let dir = home(Some(text));
        let ensured = ensure(dir.path(), 1, keep, true).unwrap();
        let written = fs::read_to_string(path(dir.path())).unwrap();
        assert!(written.starts_with("port: 8317\napi-keys:\n  - name: provedor\n    base-url: x\n"), "{written}");
        assert_eq!(read(dir.path()).unwrap().unwrap().client_keys, [ensured.client_key]);
    }

    #[test]
    fn ensure_edits_a_crlf_file() {
        let text = "config-version: 8\r\nmanagement:\r\n  secret-key: \"\"\r\naccess:\r\n  api-keys:\r\n    - k\r\n";
        let dir = home(Some(text));
        assert!(ensure(dir.path(), 1, keep, true).unwrap().management_key.is_some());
        let config = read(dir.path()).unwrap().unwrap();
        assert!(config.management_key_set);
        assert_eq!(config.client_keys, ["k"]);
    }

    #[test]
    fn ensure_leaves_a_complete_config_alone() {
        // `payload` da pessoa conta como completo: o Hangar não mistura regra nas dela.
        let text = format!("{LEGACY}payload:\n  default:\n    - models:\n        - name: \"gpt-*\"\n      params:\n        \"reasoning.effort\": \"high\"\n");
        let dir = home(Some(&text));
        let ensured = ensure(dir.path(), 1, keep, true).unwrap();
        assert_eq!(ensured, Ensured { client_key: "k-legado".into(), management_key: None, changed: false, backup: None });
        assert_eq!(fs::read_to_string(path(dir.path())).unwrap(), text);
        assert_eq!(fs::read_dir(dir.path().join(".cli-proxy-api")).unwrap().count(), 1, "sem backup");
        assert!(!read(dir.path()).unwrap().unwrap().ultrafast_rule);
    }

    #[test]
    fn the_ultrafast_rule_goes_where_each_layout_reads_it() {
        let dir = home(None);
        ensure(dir.path(), 1, keep, true).unwrap();
        let (config, data) = parse(&fs::read_to_string(path(dir.path())).unwrap()).unwrap().unwrap();
        assert!(config.ultrafast_rule && data["requests"]["payload"]["override"].is_array());

        let dir = home(Some(LEGACY));
        let ensured = ensure(dir.path(), 2, keep, true).unwrap();
        assert!(ensured.changed && ensured.backup.is_some() && ensured.management_key.is_none());
        let (config, data) = parse(&fs::read_to_string(path(dir.path())).unwrap()).unwrap().unwrap();
        assert!(config.ultrafast_rule && data["payload"]["override"].is_array() && data.get("requests").is_none());
        assert_eq!(config.client_keys, ["k-legado"]);

        let v8 = "config-version: 8\nmanagement:\n  secret-key: \"h\"\naccess:\n  api-keys:\n    - \"k\"\nrequests:\n  proxy-url: \"\"\n";
        let dir = home(Some(v8));
        ensure(dir.path(), 3, keep, true).unwrap();
        let (config, data) = parse(&fs::read_to_string(path(dir.path())).unwrap()).unwrap().unwrap();
        assert!(config.ultrafast_rule && data["requests"]["proxy-url"] == "");

        // Proxy sem versão conhecida (ou anterior ao v8): nenhuma regra, e o config fica como estava.
        let dir = home(Some(LEGACY));
        let ensured = ensure(dir.path(), 4, keep, false).unwrap();
        assert!(!ensured.changed);
        assert_eq!(fs::read_to_string(path(dir.path())).unwrap(), LEGACY);
    }

    #[test]
    fn ensure_never_rewrites_what_it_cannot_read() {
        let dir = home(Some("a: ["));
        assert!(matches!(ensure(dir.path(), 1, keep, true), Err(EnsureError::InvalidYaml)));
        assert_eq!(fs::read_to_string(path(dir.path())).unwrap(), "a: [");
        let dir = home(Some("config-version: 8\naccess: {api-keys: []}\n"));
        assert!(matches!(ensure(dir.path(), 1, keep, true), Err(EnsureError::Unsupported)));
        let dir = home(Some("- lista\n"));
        assert!(matches!(ensure(dir.path(), 1, keep, true), Err(EnsureError::Unsupported)));
    }

    #[test]
    fn reset_writes_a_new_password_where_the_proxy_reads_it() {
        let legacy = "host: \"127.0.0.1\"\nport: 8317\n# senha\nremote-management:\n  allow-remote: false\n  secret-key: \"$2a$10$hash\"\napi-keys:\n  - \"k\"\n";
        let dir = home(Some(legacy));
        let mut given = String::new();
        let backup = reset_management(dir.path(), 7, |key| { given = key.to_owned(); Ok(()) }).unwrap();
        let text = fs::read_to_string(path(dir.path())).unwrap();
        assert!(text.contains(&format!("  secret-key: \"{given}\"\n")), "{text}");
        assert!(text.contains("# senha\n") && text.contains("  allow-remote: false\n") && !text.contains("$2a$"));
        assert_eq!(fs::read_to_string(backup).unwrap(), legacy);
        assert_eq!(read(dir.path()).unwrap().unwrap().client_keys, vec!["k".to_owned()]);

        let v8 = "config-version: 8\nremote-management:\n  secret-key: \"velha\"\nmanagement:\n  secret-key: \"$2a$x\"\n";
        let dir = home(Some(v8));
        reset_management(dir.path(), 8, |key| { given = key.to_owned(); Ok(()) }).unwrap();
        let text = fs::read_to_string(path(dir.path())).unwrap();
        assert!(text.contains(&format!("management:\n  secret-key: \"{given}\"\n")) && text.contains("\"velha\""), "{text}");
    }

    #[test]
    fn reset_without_a_config_changes_nothing() {
        let dir = home(None);
        assert!(reset_management(dir.path(), 1, |_| panic!("senha entregue sem config")).is_err());
        assert!(!path(dir.path()).exists());
    }
}

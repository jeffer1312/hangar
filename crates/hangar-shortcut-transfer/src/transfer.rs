//! Exportar e importar a fileira: o export troca segredo por marcador e caminho desta máquina por
//! marcador de caminho; o import junta por `id` (mesmo id substitui no lugar, novo vai ao fim) e
//! preenche os segredos dados. Marcador deixado em branco continua no atalho e o bloqueia de rodar.
use std::collections::HashSet;

use serde_json::{Map, Value, json};

use hangar_core::path_markers::{canonicalize, resolve};
use hangar_core::pycompat::{pyjson, truthy};

use crate::host::{Error, Host, Machine, Snapshot};
use crate::scripts::{self, FileError, Script};
use crate::scrub::{placeholder_names, scrub};

const VERSION: u64 = 2;
const MAX_IMPORT_CHARS: usize = 1_000_000;
const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
const CONFIRM_SCRIPTS: &str =
    "Confira os scripts: importar instala os arquivos; executar um atalho pode executar esse código.";
const CONFIRM_VERIFY: &str =
    "Importar roda o comando de verificação de cada atalho logo depois: confira os comandos acima.";
const RESERVED: &str = "prefixo script: reservado aos arquivos do pacote";

/// O corpo de `POST /api/shortcuts/import`, já conferido na forma.
#[derive(Clone, Debug)]
pub struct ImportRequest {
    /// `{"version", "shortcuts", "scripts", "warnings"}` ou a lista crua (versão 1).
    pub data: Value,
    pub apply: bool,
    /// `{id do atalho: {nome do marcador: valor}}`. Nunca vai ao log.
    pub secrets: Map<String, Value>,
}

/// O pacote da seleção; `ids = None` exporta todos. A configuração salva não muda.
///
/// # Errors
/// `Invalid` para seleção recusada; `Io` para falha de leitura.
pub fn export(
    host: &dyn Host,
    ids: Option<&[String]>,
    include_scripts: bool,
    login_path: impl FnOnce() -> String,
) -> Result<Value, Error> {
    let machine = host.machine();
    let mut current = current(host)?;
    if let Some(ids) = ids {
        let known: HashSet<&str> = current.iter().filter_map(id_of).collect();
        if ids.iter().any(|id| !known.contains(id.as_str())) {
            return Err(Error::Invalid("seleção contém atalho desconhecido".into()));
        }
        current.retain(|item| id_of(item).is_some_and(|id| ids.iter().any(|wanted| wanted == id)));
    }
    if current.iter().any(|item| id_of(item).is_some_and(|id| id.starts_with("script:"))) {
        return Err(Error::Invalid(RESERVED.into()));
    }
    let roots = &machine.roots;
    let mut removed = 0;
    let mut out = Vec::with_capacity(current.len());
    for item in &current {
        let mut item = item.clone();
        if let Some(fields) = item.as_object_mut() {
            if let Some(field) = field_of(fields) {
                if let Some(text) = fields.get(field).and_then(Value::as_str) {
                    let fish = field == "command" && scripts::is_fish(text, None, &machine);
                    let (text, names) = scrub(text, fish, true);
                    removed += names.len();
                    fields.insert(field.into(), json!(canonicalize(&text, roots)));
                }
            }
            if let Some(pasta) = fields.get("pasta").and_then(Value::as_str) {
                let pasta = canonicalize(pasta, roots);
                fields.insert("pasta".into(), json!(pasta));
            }
            if let Some(verify) = fields.get("verify").and_then(Value::as_str) {
                let (verify, names) = scrub(verify, scripts::is_fish(verify, None, &machine), true);
                removed += names.len();
                fields.insert("verify".into(), json!(canonicalize(&verify, roots)));
            }
        }
        out.push(item);
    }
    let (mut bundled, warnings) = if include_scripts && !current.is_empty() {
        let commands: Vec<&str> = current
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("shell"))
            .filter_map(|item| item.get("command").and_then(Value::as_str))
            .collect();
        scripts::collect(&commands, &machine, login_path)?
    } else {
        (Vec::new(), Vec::new())
    };
    for script in &mut bundled {
        let path = script["path"].as_str().unwrap_or_default().to_owned();
        let content = script["content"].as_str().unwrap_or_default().to_owned();
        let fish = scripts::is_fish(&content, Some(&path), &machine);
        let (content, names) = scrub(&content, fish, scripts::is_shell_script(&content, Some(&path)));
        removed += names.len();
        script["content"] = json!(canonicalize(&content, roots));
    }
    let warnings: Vec<String> = warnings.iter().map(|w| canonicalize(&scrub(w, false, true).0, roots)).collect();
    Ok(json!({"version": VERSION, "shortcuts": out, "scripts": bundled, "warnings": warnings, "removed": removed}))
}

/// Prévia ou aplicação de um pacote; o pacote inteiro é conferido antes da primeira escrita e a
/// prévia nunca devolve segredo preenchido.
///
/// # Errors
/// `Invalid` com o motivo da recusa (nada muda); `Io` para falha inesperada do disco.
pub fn import(host: &dyn Host, request: ImportRequest) -> Result<Value, Error> {
    let ImportRequest { data, apply, secrets } = request;
    if pyjson::dumps_unicode(&data, false).len() > MAX_PAYLOAD_BYTES {
        return Err(invalid("pacote de atalhos grande demais"));
    }
    let machine = host.machine();
    let incoming = parse(host, &data)?;
    let object = data.as_object();
    let bundled = object.is_some_and(|data| data.get("version").and_then(Value::as_i64) == Some(2));
    if object.is_some_and(|data| !bundled && data.get("scripts").is_some_and(truthy)) {
        return Err(invalid("scripts exigem pacote versão 2"));
    }
    let empty = json!([]);
    let (warnings, mut scripts) = match object.filter(|_| bundled) {
        Some(data) => (
            package_warnings(data.get("warnings").unwrap_or(&empty))?,
            scripts::validate(data.get("scripts").unwrap_or(&empty), &machine).map_err(Error::Invalid)?,
        ),
        None => (Vec::new(), Vec::new()),
    };
    let roots = bundled.then_some(&machine.roots);
    let mut incoming: Vec<Value> = incoming
        .into_iter()
        .map(|mut item| {
            clean_item(&mut item, &machine, roots);
            item
        })
        .collect();
    let mut script_items: Vec<Value> = scripts
        .iter()
        .map(|script| {
            let fish = scripts::is_fish(&script.content, Some(&script.path), &machine);
            let shell = scripts::is_shell_script(&script.content, Some(&script.path));
            let resolved = roots.map_or_else(|| script.content.clone(), |roots| resolve(&script.content, roots));
            let (content, _) = scrub(&resolved, fish, shell);
            json!({"id": format!("script:{}", script.path), "label": script.path, "type": "shell", "command": content})
        })
        .collect();
    let display_scripts = script_items.clone();
    // Antes do preenchimento: a prévia nunca mostra segredo.
    let verify_commands: Vec<Value> = incoming
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("shell") && item.get("verify").is_some_and(truthy))
        .map(|item| {
            let (id, command) = (item["id"].clone(), item["verify"].clone());
            json!({"id": id, "label": label(item), "command": command})
        })
        .collect();
    if apply {
        incoming = fill(incoming, &secrets);
        script_items = fill(script_items, &secrets);
        if !placeholders(&script_items).is_empty() {
            return Err(invalid("preencha todos os segredos dos scripts antes de importar"));
        }
    }
    let (merged, added, replaced) = merge(current(host)?, &incoming);
    host.validate(&pyjson::dumps_unicode(&Value::Array(merged.clone()), false)).map_err(Error::Invalid)?;
    let mut found = placeholders(&incoming);
    found.extend(placeholders(&script_items));
    let verify: Vec<Value> = incoming.iter().filter(|item| item.get("verify").is_some_and(truthy)).map(|item| item["id"].clone()).collect();
    let mut result = Map::new();
    result.insert("added".into(), json!(added));
    result.insert("replaced".into(), json!(replaced));
    result.insert("placeholders".into(), Value::Array(found));
    result.insert("verify".into(), Value::Array(verify));
    result.insert("verify_commands".into(), Value::Array(verify_commands.clone()));
    if let Some(roots) = roots {
        let mut files = Vec::with_capacity(scripts.len());
        for ((script, filled), display) in scripts.iter_mut().zip(&script_items).zip(&display_scripts) {
            script.content = filled["command"].as_str().unwrap_or_default().to_owned();
            let (path, status) = (script.target.to_string_lossy().into_owned(), file_status(script)?);
            let content = display["command"].clone();
            files.push(json!({"path": path, "status": status, "content": content}));
        }
        // Os segredos preenchidos podem ter levado um script além do limite de tamanho.
        let filled: Vec<Value> = scripts
            .iter()
            .map(|s| json!({"path": s.path, "content": s.content, "executable": s.executable}))
            .collect();
        scripts::validate(&Value::Array(filled), &machine).map_err(Error::Invalid)?;
        result.insert("files".into(), Value::Array(files));
        let mut cleaned: Vec<String> = Vec::new();
        for warning in &warnings {
            let warning = scrub(&resolve(warning, roots), false, true).0;
            if !cleaned.contains(&warning) {
                cleaned.push(warning);
            }
        }
        if !scripts.is_empty() {
            cleaned.push(CONFIRM_SCRIPTS.into());
        }
        result.insert("warnings".into(), json!(cleaned));
    }
    if !verify_commands.is_empty() {
        let warnings = result.entry("warnings").or_insert_with(|| json!([]));
        if let Some(warnings) = warnings.as_array_mut() {
            warnings.push(json!(CONFIRM_VERIFY));
        }
    }
    if apply {
        save(host, &machine, &scripts, &pyjson::dumps_unicode(&Value::Array(merged), false))?;
    }
    Ok(Value::Object(result))
}

fn save(host: &dyn Host, machine: &Machine, scripts: &[Script], serialized: &str) -> Result<(), Error> {
    if scripts.is_empty() {
        return host.save(serialized);
    }
    let mut before: Option<Snapshot> = None;
    let written = scripts::apply_files(&machine.home, scripts, || {
        before = Some(host.snapshot()?);
        host.save(serialized)
    });
    let Err(message) = written else { return Ok(()) };
    // Só desfaz a gravação desta importação; outro campo ou gravação posterior permanece.
    if let Some(before) = &before {
        if host.undo(serialized, before).is_err() {
            return Err(Error::Invalid(format!("{message}; restauração dos atalhos na configuração falhou")));
        }
    }
    Err(Error::Invalid(message))
}

fn parse(host: &dyn Host, data: &Value) -> Result<Vec<Value>, Error> {
    if let Some(object) = data.as_object() {
        let version = object.get("version").map_or(Some(1), Value::as_i64);
        if !version.is_some_and(|v| v == 1 || v == 2) {
            return Err(invalid("versão do pacote de atalhos não suportada"));
        }
    }
    let items = match data {
        Value::Object(object) => object.get("shortcuts"),
        other => Some(other),
    };
    let Some(Value::Array(items)) = items else {
        return Err(invalid("arquivo sem lista de atalhos"));
    };
    let raw = pyjson::dumps(&Value::Array(items.clone()), false);
    // Vai inteiro para a configuração e para a prévia; um arquivo errado de MBs travaria as duas.
    if raw.chars().count() > MAX_IMPORT_CHARS {
        return Err(invalid("arquivo grande demais para uma lista de atalhos"));
    }
    host.validate(&raw).map_err(Error::Invalid)?;
    let ids: Vec<&str> = items.iter().filter_map(id_of).collect();
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err(invalid("id de atalho duplicado"));
    }
    if ids.iter().any(|id| id.starts_with("script:")) {
        return Err(invalid(RESERVED));
    }
    Ok(items.clone())
}

fn package_warnings(warnings: &Value) -> Result<Vec<String>, Error> {
    let list: Option<Vec<String>> = warnings
        .as_array()
        .filter(|list| list.len() <= scripts::MAX_WARNINGS)
        .and_then(|list| list.iter().map(|w| w.as_str().filter(|w| w.len() <= 1024).map(str::to_owned)).collect());
    let list = list.ok_or_else(|| invalid("avisos do pacote inválidos ou grandes demais"))?;
    if list.iter().map(String::len).sum::<usize>() > scripts::MAX_WARNING_BYTES {
        return Err(invalid("avisos do pacote grandes demais"));
    }
    Ok(list)
}

/// O que chega passa de novo pela varredura: o arquivo nunca grava a senha que veio nele.
fn clean_item(item: &mut Value, machine: &Machine, roots: Option<&hangar_core::path_markers::Roots>) {
    let Some(fields) = item.as_object_mut() else { return };
    if let Some(field) = field_of(fields) {
        if let Some(text) = fields.get(field).and_then(Value::as_str) {
            let fish = field == "command" && scripts::is_fish(text, None, machine);
            let (text, _) = scrub(text, fish, true);
            let text = roots.map_or(text.clone(), |roots| resolve(&text, roots));
            fields.insert(field.into(), json!(text));
        }
    }
    if let (Some(roots), Some(pasta)) = (roots, fields.get("pasta").and_then(Value::as_str)) {
        let pasta = resolve(pasta, roots);
        fields.insert("pasta".into(), json!(pasta));
    }
    if let Some(verify) = fields.get("verify").and_then(Value::as_str) {
        let (verify, _) = scrub(verify, scripts::is_fish(verify, None, machine), true);
        let verify = roots.map_or(verify.clone(), |roots| resolve(&verify, roots));
        fields.insert("verify".into(), json!(verify));
    }
}

fn file_status(script: &Script) -> Result<&'static str, Error> {
    if !script.target.exists() {
        return Ok("create");
    }
    let same = match scripts::read_file(&script.target) {
        Ok(data) => data == script.content.as_bytes(),
        Err(FileError::Invalid(message)) => return Err(invalid(message)),
        Err(FileError::Io(error)) => return Err(Error::Io(error.to_string())),
    };
    if !same {
        return Ok("replace");
    }
    let wanted = if script.executable { 0o700 } else { 0o600 };
    let mode = scripts::mode(&script.target).map_err(|error| Error::Io(error.to_string()))?;
    Ok(if mode == wanted { "same" } else { "replace" })
}

fn placeholders(items: &[Value]) -> Vec<Value> {
    let mut found = Vec::new();
    for item in items {
        let mut names = match item.as_object().and_then(field_of) {
            Some(field) => placeholder_names(item.get(field).and_then(Value::as_str).unwrap_or("")),
            None => Vec::new(),
        };
        if let Some(verify) = item.get("verify").and_then(Value::as_str) {
            names.extend(placeholder_names(verify));
        }
        let mut unique: Vec<String> = Vec::new();
        for name in names {
            if !unique.contains(&name) {
                unique.push(name);
            }
        }
        if !unique.is_empty() {
            let id = item["id"].clone();
            found.push(json!({"id": id, "label": label(item), "names": unique}));
        }
    }
    found
}

fn fill(items: Vec<Value>, secrets: &Map<String, Value>) -> Vec<Value> {
    items
        .into_iter()
        .map(|mut item| {
            let values = id_of(&item).and_then(|id| secrets.get(id)).and_then(Value::as_object).cloned();
            if let (Some(values), Some(fields)) = (values, item.as_object_mut())
                && let Some(field) = field_of(fields)
            {
                let value = |name: &str| values.get(name).and_then(Value::as_str).map(str::to_owned);
                for key in [field, "verify"] {
                    if let Some(text) = fields.get(key).and_then(Value::as_str) {
                        let filled = crate::scrub::fill(text, value);
                        fields.insert(key.into(), json!(filled));
                    }
                }
            }
            item
        })
        .collect()
}

fn merge(current: Vec<Value>, incoming: &[Value]) -> (Vec<Value>, usize, usize) {
    let by_id = |id: Option<&str>| id.and_then(|id| incoming.iter().rev().find(|item| id_of(item) == Some(id)));
    let replaced = current.iter().filter(|item| by_id(id_of(item)).is_some()).count();
    let known: Vec<Option<&str>> = current.iter().map(id_of).collect();
    let added: Vec<Value> = incoming.iter().filter(|item| !known.contains(&id_of(item))).cloned().collect();
    let count = added.len();
    let mut merged: Vec<Value> = current.iter().map(|item| by_id(id_of(item)).unwrap_or(item).clone()).collect();
    merged.extend(added);
    (merged, count, replaced)
}

/// A lista salva; vazia, JSON inválido ou não-lista contam como nenhum atalho.
pub(crate) fn current(host: &dyn Host) -> Result<Vec<Value>, Error> {
    Ok(match host.shortcuts()? {
        Value::String(raw) if !raw.is_empty() => match serde_json::from_str(&raw) {
            Ok(Value::Array(items)) => items,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    })
}

pub(crate) fn id_of(item: &Value) -> Option<&str> {
    item.get("id").and_then(Value::as_str)
}

pub(crate) fn label(item: &Value) -> Value {
    match item.get("label") {
        Some(label) if truthy(label) => label.clone(),
        _ => item.get("id").cloned().unwrap_or(Value::Null),
    }
}

fn field_of(item: &Map<String, Value>) -> Option<&'static str> {
    match item.get("type").and_then(Value::as_str) {
        Some("shell") => Some("command"),
        Some("send_text") => Some("text"),
        _ => None,
    }
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.to_owned())
}

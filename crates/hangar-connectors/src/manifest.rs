//! Manifesto de um conector: identidade, capacidades e o comando, sem segredo nem shell.
use crate::error::{AdminError, AdminKind};
use crate::protocol::{CAPABILITIES, MANIFEST_VERSION, PROTOCOL_VERSION};
use crate::work_tracking::TicketSummary;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_MANIFEST_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    manifest_version: u32,
    id: String,
    name: String,
    version: String,
    protocol_version: u32,
    capabilities: BTreeMap<String, u32>,
    command: Vec<String>,
    #[serde(default)]
    start_prompt: Option<String>,
}

/// Manifesto conferido, com o executável relativo já resolvido contra a pasta do manifesto.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol_version: u32,
    pub capabilities: BTreeMap<String, u32>,
    /// Vetor executado sem shell; o primeiro item é o programa.
    pub command: Vec<String>,
    /// Pasta de trabalho do processo: a do manifesto.
    pub cwd: PathBuf,
    /// Primeira mensagem da sessão aberta a partir de um ticket, com `{key}`, `{id}`, `{title}` e
    /// `{url}` trocados pelos campos dele. Sem ela, o cliente propõe um texto próprio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_prompt: Option<String>,
}

/// `id`: de 1 a 64 caracteres, minúsculas, dígitos e hífen, começando por letra.
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some('a'..='z'))
        && id.len() <= 64
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
}

fn text(field: &str, value: &str, max: usize) -> Result<(), AdminError> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        return Err(AdminError::invalid(format!("{field} vazio, com NUL ou acima de {max} bytes")));
    }
    Ok(())
}

/// Executável com separador de caminho e relativo parte da pasta do manifesto; nome simples fica
/// para o PATH do servidor.
fn resolve_program(program: &str, dir: &Path) -> String {
    let has_separator = program.contains('/') || (cfg!(windows) && program.contains('\\'));
    if has_separator && Path::new(program).is_relative() {
        dir.join(program).to_string_lossy().into_owned()
    } else {
        program.to_owned()
    }
}

impl Manifest {
    /// Lê o arquivo do manifesto; a pasta dele vira a pasta de trabalho.
    pub fn load(path: &Path) -> Result<Self, AdminError> {
        let path = std::path::absolute(path).map_err(|error| AdminError::io("caminho do manifesto", &error))?;
        let bytes = std::fs::read(&path).map_err(|error| AdminError::new(AdminKind::ManifestUnreadable, format!("manifesto ilegível: {:?}", error.kind())))?;
        let dir = path.parent().ok_or_else(|| AdminError::invalid("manifesto sem pasta"))?;
        Self::parse(&bytes, dir)
    }

    pub fn parse(bytes: &[u8], dir: &Path) -> Result<Self, AdminError> {
        if bytes.len() > MAX_MANIFEST_BYTES { return Err(AdminError::invalid("manifesto acima de 64 KiB")); }
        let raw: Raw = serde_json::from_slice(bytes).map_err(|error| AdminError::invalid(format!("manifesto inválido: {error}")))?;
        if raw.manifest_version != MANIFEST_VERSION {
            return Err(AdminError::new(AdminKind::UnsupportedVersion, format!("manifest_version {} não suportado", raw.manifest_version)));
        }
        if raw.protocol_version != PROTOCOL_VERSION {
            return Err(AdminError::new(AdminKind::UnsupportedVersion, format!("protocol_version {} não suportado", raw.protocol_version)));
        }
        if !valid_id(&raw.id) { return Err(AdminError::invalid("id fora do formato [a-z][a-z0-9-]{0,63}")); }
        text("name", &raw.name, 200)?;
        text("version", &raw.version, 64)?;
        if raw.capabilities.is_empty() { return Err(AdminError::invalid("nenhuma capacidade declarada")); }
        for (name, version) in &raw.capabilities {
            match CAPABILITIES.iter().find(|(known, _)| known == name) {
                None => return Err(AdminError::new(AdminKind::UnsupportedCapability, format!("capacidade desconhecida: {name}"))),
                Some((_, supported)) if supported != version => {
                    return Err(AdminError::new(AdminKind::UnsupportedVersion, format!("{name} versão {version} não suportada")));
                }
                Some(_) => {}
            }
        }
        if raw.command.is_empty() { return Err(AdminError::invalid("command vazio")); }
        if raw.command.iter().any(|part| part.is_empty() || part.contains('\0') || part.len() > 4096) {
            return Err(AdminError::invalid("item de command vazio, com NUL ou longo demais"));
        }
        if let Some(prompt) = &raw.start_prompt { text("start_prompt", prompt, 4096)?; }
        let mut command = raw.command;
        command[0] = resolve_program(&command[0], dir);
        Ok(Self {
            id: raw.id, name: raw.name, version: raw.version, protocol_version: raw.protocol_version,
            capabilities: raw.capabilities, command, cwd: dir.to_path_buf(), start_prompt: raw.start_prompt,
        })
    }

    /// `start_prompt` preenchido com os campos do ticket, numa passada só: o texto de um campo
    /// nunca é relido como marcador.
    pub fn start_prompt_for(&self, ticket: &TicketSummary) -> Option<String> {
        let template = self.start_prompt.as_deref()?;
        let mut out = String::with_capacity(template.len() + 64);
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let tail = &rest[open..];
            let field = tail.find('}').and_then(|close| {
                let value = match &tail[1..close] {
                    "key" => &ticket.key,
                    "id" => &ticket.id,
                    "title" => &ticket.title,
                    "url" => &ticket.url,
                    _ => return None,
                };
                Some((value, close + 1))
            });
            match field {
                Some((value, used)) => { out.push_str(value); rest = &tail[used..]; }
                None => { out.push('{'); rest = &tail[1..]; }
            }
        }
        out.push_str(rest);
        Some(out)
    }
}

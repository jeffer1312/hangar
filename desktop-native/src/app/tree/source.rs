//! De onde a árvore de arquivos lê: o disco desta máquina quando o servidor é ela mesma, as rotas do backend quando
//! não é. No disco valem as travas do `filetree.py` do backend: nada fora da raiz da sessão (caminho já resolvido, sem
//! fuga por atalho) e nada que passe por uma pasta `.git`.
use std::{collections::HashMap, path::{Path, PathBuf}};
use serde_json::Value;
use crate::api::{Api, Failure};

/// Os mesmos tetos do backend: itens por pasta, resultados de busca e bytes lidos de um arquivo.
const MAX_HITS: usize = 200;

#[derive(Clone)]
pub(crate) enum FileSource {
    Local { root: PathBuf },
    Remote { api: Api, name: String },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry { pub name: String, pub path: String, pub dir: bool, pub mark: Option<char> }

#[derive(Clone, Debug, Default)]
pub(crate) struct Listing { pub entries: Vec<Entry>, pub truncated: bool }

pub(crate) struct Read { pub text: String, pub truncated: bool, pub digest: Option<String> }

fn refuse(code: &str) -> Failure { Failure::local(code) }

fn workspace_failure(error: hangar_workspace::WorkspaceError) -> Failure {
    Failure { status: Some(error.status), ..Failure::local(error.code.unwrap_or_else(|| "invalid_response".into())) }
}

impl FileSource {
    /// Disco direto só com o servidor em loopback, a pasta da sessão existindo aqui e a raiz dela igual à que o servidor
    /// lista: um túnel para outra máquina também responde em 127.0.0.1. Qualquer dúvida fica com o servidor.
    pub async fn pick(api: Api, name: String, cwd: Option<String>) -> Self {
        let remote = FileSource::Remote { api: api.clone(), name: name.clone() };
        let Some(root) = cwd.filter(|_| api.is_loopback()).and_then(|cwd| std::fs::canonicalize(cwd).ok()) else { return remote };
        let local = FileSource::Local { root };
        match (local.list(vec![String::new()]).await.pop(), remote.list(vec![String::new()]).await.pop()) {
            (Some((_, Ok(mine))), Some((_, Ok(theirs)))) if names(&mine) == names(&theirs) => local,
            _ => remote,
        }
    }

    pub fn is_local(&self) -> bool { matches!(self, FileSource::Local { .. }) }

    pub fn root(&self) -> Option<&Path> { match self { FileSource::Local { root } => Some(root), _ => None } }

    /// Lê várias pastas de uma vez; no disco, o `git status` roda uma vez só para todas.
    pub async fn list(&self, dirs: Vec<String>) -> Vec<(String, Result<Listing, Failure>)> {
        match self {
            FileSource::Local { root } => {
                let root = root.clone();
                tokio::task::spawn_blocking(move || {
                    let marks = git_marks(&root);
                    let state = hangar_workspace::files::directory_state(&root);
                    dirs.into_iter().map(|dir| {
                        let listing = match &state {
                            Ok(state) => hangar_workspace::files::list_with_state(&root, Some(&dir), false, state).map_err(workspace_failure).map(|value| {
                                let mut listing = remote_listing(&value);
                                for entry in &mut listing.entries { entry.mark = marks.get(&entry.path).copied(); }
                                listing
                            }),
                            Err(error) => Err(workspace_failure(error.clone())),
                        };
                        (dir, listing)
                    }).collect()
                }).await.unwrap_or_default()
            }
            FileSource::Remote { api, name } => {
                let mut out = Vec::new();
                for dir in dirs {
                    let mut query = vec![("so_modificados", "false")];
                    if !dir.is_empty() { query.push(("path", dir.as_str())); }
                    let listing = api.read(name, &["files", "list"], &query, 30).await.map(|value| remote_listing(&value));
                    out.push((dir, listing));
                }
                out
            }
        }
    }

    /// Arquivos cujo caminho contém `query`, sem diferença de maiúsculas; os que o `.gitignore` esconde ficam de fora.
    pub async fn search(&self, query: String) -> Result<(Vec<String>, bool), Failure> {
        match self {
            FileSource::Local { root } => {
                let root = root.clone();
                tokio::task::spawn_blocking(move || {
                    let needle = query.to_lowercase();
                    let mut hits: Vec<String> = repo_files(&root)?.into_iter()
                        .filter(|path| path.to_lowercase().contains(&needle) && root.join(path).symlink_metadata().is_ok()).collect();
                    let truncated = hits.len() > MAX_HITS;
                    hits.truncate(MAX_HITS);
                    Ok((hits, truncated))
                }).await.unwrap_or_else(|_| Err(refuse("erro_arq_busca_falhou")))
            }
            FileSource::Remote { api, name } => {
                let value = api.read(name, &["files", "search"], &[("q", query.as_str()), ("mode", "names")], 30).await?;
                let hits = value.get("hits").and_then(Value::as_array).map(|hits| hits.iter()
                    .filter_map(|hit| hit.get("path").and_then(Value::as_str).map(str::to_owned)).collect()).unwrap_or_default();
                Ok((hits, value.get("truncated").and_then(Value::as_bool).unwrap_or(false)))
            }
        }
    }

    /// Pastas que o vigia do disco acompanha: a raiz, as que o git conhece (sem as ignoradas, que o agente enche de
    /// build) e a pasta do git, onde `index` e `HEAD` mudam a cada commit ou `add`.
    pub fn watch_dirs(root: &Path) -> Vec<PathBuf> {
        let mut dirs = vec![root.to_path_buf()];
        if let Ok(files) = repo_files(root) {
            let mut seen = std::collections::HashSet::new();
            for file in files {
                let mut parent = Path::new(&file).parent();
                while let Some(dir) = parent.filter(|dir| !dir.as_os_str().is_empty()) {
                    if !seen.insert(dir.to_path_buf()) { break; }
                    parent = dir.parent();
                }
            }
            dirs.extend(seen.into_iter().map(|dir| root.join(dir)).filter(|dir| resolve(root, &dir.strip_prefix(root).unwrap_or(dir).to_string_lossy()).is_ok()));
        }
        if let Some(git_dir) = git(root, &["rev-parse", "--absolute-git-dir"]).map(|out| PathBuf::from(out.trim())) { dirs.push(git_dir); }
        dirs
    }
}

fn names(listing: &Listing) -> Vec<&str> {
    let mut names: Vec<&str> = listing.entries.iter().map(|entry| entry.name.as_str()).collect();
    names.sort_unstable();
    names
}

fn remote_listing(value: &Value) -> Listing {
    let entries = value.get("entries").and_then(Value::as_array).map(|entries| entries.iter().filter_map(|entry| Some(Entry {
        name: entry.get("name")?.as_str()?.to_owned(),
        path: entry.get("path")?.as_str()?.to_owned(),
        dir: entry.get("is_dir").and_then(Value::as_bool).unwrap_or(false),
        mark: entry.get("changed").and_then(Value::as_str).and_then(|mark| mark.chars().next()),
    })).collect()).unwrap_or_default();
    Listing { entries, truncated: value.get("truncated").and_then(Value::as_bool).unwrap_or(false) }
}

/// Caminho relativo à raiz, provado dentro dela depois de resolvido e sem nenhum componente `.git`.
pub(crate) fn resolve(root: &Path, rel: &str) -> Result<PathBuf, Failure> {
    hangar_workspace::files::resolve(root, rel).map_err(workspace_failure)
}

/// Lê um arquivo da raiz como o `read_at` do backend: binário recusado, corte em 512 KB e a impressão SHA-256 do que
/// foi lido, que a gravação pelo servidor confere.
pub(crate) fn read_local(root: &Path, rel: &str) -> Result<Read, Failure> {
    let result = hangar_workspace::files::read(&hangar_workspace::files::resolve(root, rel).map_err(workspace_failure)?, rel).map_err(workspace_failure)?;
    let read: hangar_api::workspace::FileContent = serde_json::from_value(result).map_err(|_| refuse("invalid_response"))?;
    Ok(Read { text: read.text, truncated: read.truncated, digest: read.digest })
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let mut command = vec!["-c", "core.quotePath=false"];
    command.extend_from_slice(args);
    hangar_workspace::git::command(root, &command).ok().filter(|out| out.code == 0).map(|out| out.stdout)
}

/// Arquivos do repositório relativos à raiz (rastreados e novos, sem os ignorados). Fora de repositório, o mesmo erro
/// da busca do backend.
fn repo_files(root: &Path) -> Result<Vec<String>, Failure> {
    hangar_workspace::files::repo_files(root).map_err(workspace_failure)
}

/// Letra de cada arquivo mudado e, em cada pasta acima dele, a mais grave entre as de dentro. Fora de repositório,
/// nenhuma marca.
fn git_marks(root: &Path) -> HashMap<String, char> {
    let (Some(prefix), Some(status)) = (git(root, &["rev-parse", "--show-prefix"]), git(root, &["status", "--porcelain=v1", "-z", "-uall"]))
        else { return HashMap::new() };
    marks_from_porcelain(prefix.trim(), &status)
}

fn marks_from_porcelain(prefix: &str, status: &str) -> HashMap<String, char> {
    let mut marks = HashMap::new();
    let mut records = status.split('\0');
    while let Some(record) = records.next() {
        if record.len() < 4 { continue; }
        let (code, path) = record.split_at(3);
        let code = code.as_bytes();
        // Renomeado traz o caminho antigo no registro seguinte.
        if matches!(code[0], b'R' | b'C') { records.next(); }
        let Some(path) = path.strip_prefix(prefix) else { continue };
        let mark = letter(code[0], code[1]);
        marks.insert(path.trim_end_matches('/').to_owned(), mark);
        let mut parent = Path::new(path).parent();
        while let Some(dir) = parent.filter(|dir| !dir.as_os_str().is_empty()) {
            let slot = marks.entry(dir.to_string_lossy().into_owned()).or_insert(mark);
            if severity(mark) > severity(*slot) { *slot = mark; }
            parent = dir.parent();
        }
    }
    marks
}

/// As duas colunas do porcelain numa letra só, com a gravidade do Zeron: conflito, apagado, renomeado, mudado, novo.
fn letter(index: u8, tree: u8) -> char {
    let both = [index, tree];
    if both.contains(&b'U') || both == *b"AA" || both == *b"DD" { 'U' }
    else if both == *b"??" { '?' }
    else if both.contains(&b'D') { 'D' }
    else if both.iter().any(|c| matches!(c, b'R' | b'C')) { 'R' }
    else if both.iter().any(|c| matches!(c, b'M' | b'T')) { 'M' }
    else { 'A' }
}

pub(crate) fn severity(mark: char) -> u8 {
    match mark { '?' => 0, 'A' => 1, 'M' => 2, 'R' => 3, 'D' => 4, 'U' => 5, _ => 2 }
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[cfg(unix)]
    #[test]
    fn local_paths_stay_inside_the_root_and_out_of_git() {
        let base = std::env::temp_dir().join(format!("hangar-tree-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, outside) = (base.join("root"), base.join("outside"));
        for dir in [root.join("src"), root.join(".git"), outside.clone()] { std::fs::create_dir_all(dir).unwrap(); }
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join(".git/config"), "[core]").unwrap();
        std::fs::write(outside.join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        std::os::unix::fs::symlink(root.join(".git"), root.join("gitlink")).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();

        assert!(resolve(&root, "src/main.rs").is_ok());
        assert_eq!(resolve(&root, "").unwrap(), root);
        let code = |rel: &str| resolve(&root, rel).err().map(|failure| failure.detail);
        assert_eq!(code("../outside/secret").as_deref(), Some("erro_arq_fora_da_raiz"));
        assert_eq!(code("escape/secret").as_deref(), Some("erro_arq_fora_da_raiz"));
        assert_eq!(code(".git/config").as_deref(), Some("erro_arq_area_do_git"));
        assert_eq!(code("gitlink/config").as_deref(), Some("erro_arq_area_do_git"));
        assert_eq!(code("/etc/passwd").as_deref(), Some("erro_arq_caminho_invalido"));
        assert_eq!(code("-x").as_deref(), Some("erro_arq_caminho_invalido"));
        assert!(read_local(&root, "escape/secret").is_err());

        // A listagem não mostra `.git`, nem atalho para fora ou para dentro dele.
        let listing = remote_listing(&hangar_workspace::files::list(&root, Some(""), false).unwrap());
        let names: Vec<&str> = listing.entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["src"]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn porcelain_marks_files_and_the_worst_mark_up_the_tree() {
        let status = " M app/src/a.rs\0?? app/new.txt\0R  app/src/b.rs\0app/src/old.rs\0UU app/c.rs\0 M other/x\0";
        let marks = marks_from_porcelain("app/", status);
        assert_eq!(marks["src/a.rs"], 'M');
        assert_eq!(marks["new.txt"], '?');
        assert_eq!(marks["src/b.rs"], 'R');
        assert_eq!(marks["src"], 'R');
        assert_eq!(marks["c.rs"], 'U');
        assert!(!marks.contains_key("src/old.rs") && !marks.keys().any(|path| path.contains("other")));
    }
}

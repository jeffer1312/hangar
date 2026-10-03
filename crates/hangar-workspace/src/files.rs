//! Políticas de raiz e mecânica de leitura/escrita são separadas para os arquivos citados.
use crate::{Result, WorkspaceError, error, file_error, git, real};
use hangar_api::workspace::{FileContent, FileEntry};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub const MAX_BYTES: usize = 512 * 1024;
pub const MAX_ENTRIES: usize = 1000;
pub const MAX_HITS: usize = 200;

pub fn resolve(cwd: &Path, path: &str) -> Result<PathBuf> {
    if path.starts_with('-') || path.contains('\0') || Path::new(path).is_absolute() {
        return Err(file_error(
            400,
            "erro_arq_caminho_invalido",
            "caminho inválido",
        ));
    }
    let root = real(cwd);
    let target = real(&cwd.join(path));
    if !target.starts_with(&root) {
        return Err(file_error(
            400,
            "erro_arq_fora_da_raiz",
            "caminho sai da raiz da sessão",
        ));
    }
    if target != root
        && target
            .strip_prefix(&root)
            .unwrap()
            .components()
            .any(|c| c.as_os_str() == ".git")
    {
        return Err(file_error(
            403,
            "erro_arq_area_do_git",
            "área interna do git",
        ));
    }
    if !target.exists() {
        return Err(file_error(
            404,
            "erro_arq_inexistente",
            "caminho não existe",
        ));
    }
    Ok(target)
}
pub fn read(target: &Path, path: &str) -> Result<Value> {
    if target.is_dir() {
        return Err(file_error(400, "erro_arq_e_pasta", "isso é uma pasta"));
    }
    if !target.is_file() {
        return Err(file_error(
            400,
            "erro_arq_nao_e_arquivo",
            "não é um arquivo comum",
        ));
    }
    let file = std::fs::File::open(target).map_err(|e| io_error(e, "erro_arq_lista_falhou"))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(e, "erro_arq_lista_falhou"))?;
    if bytes.contains(&0) {
        return Err(file_error(415, "erro_arq_binario", "arquivo binário"));
    }
    let truncated = bytes.len() > MAX_BYTES;
    let digest = (!truncated).then(|| digest(&bytes));
    bytes.truncate(MAX_BYTES);
    let result = FileContent {
        path: path.into(),
        text: String::from_utf8_lossy(&bytes).into_owned(),
        size: target
            .metadata()
            .map_err(|e| io_error(e, "erro_arq_lista_falhou"))?
            .len(),
        truncated,
        digest,
    };
    Ok(serde_json::to_value(result).unwrap())
}
pub fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn write(target: &Path, path: &str, text: &str, expected: Option<&str>) -> Result<Value> {
    if target.is_dir() {
        return Err(file_error(400, "erro_arq_e_pasta", "isso é uma pasta"));
    }
    if !target.is_file() {
        return Err(file_error(
            400,
            "erro_arq_nao_e_arquivo",
            "não é um arquivo comum",
        ));
    }
    if text.contains('\0') {
        return Err(file_error(415, "erro_arq_binario", "arquivo binário"));
    }
    if text.len() > MAX_BYTES {
        return Err(file_error(
            413,
            "erro_arq_grande_demais",
            "arquivo grande demais",
        ));
    }
    let expected = expected
        .filter(|s| !s.is_empty())
        .ok_or_else(|| file_error(409, "erro_arq_sem_digest", "sem a impressão da leitura"))?;
    let metadata = target
        .metadata()
        .map_err(|_| file_error(409, "erro_arq_sumiu", "o arquivo sumiu do disco"))?;
    if metadata.len() > MAX_BYTES as u64 {
        return Err(file_error(
            413,
            "erro_arq_grande_demais",
            "arquivo grande demais",
        ));
    }
    let check = || -> Result<()> {
        let file = std::fs::File::open(target).map_err(|e| io_error(e, "erro_arq_sumiu"))?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| io_error(e, "erro_arq_sumiu"))?;
        if bytes.len() > MAX_BYTES || digest(&bytes) != expected {
            return Err(file_error(
                409,
                "erro_arq_mudou_no_disco",
                "o arquivo mudou no disco",
            ));
        }
        Ok(())
    };
    check()?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".hangar-escrita-")
        .tempfile_in(target.parent().unwrap_or(Path::new(".")))
        .map_err(|e| io_error(e, "erro_arq_escrita_falhou"))?;
    tmp.write_all(text.as_bytes())
        .and_then(|_| tmp.flush())
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| io_error(e, "erro_arq_escrita_falhou"))?;
    std::fs::set_permissions(tmp.path(), metadata.permissions())
        .map_err(|e| io_error(e, "erro_arq_escrita_falhou"))?;
    check()?;
    let waits: &[u64] = if cfg!(windows) {
        &[20, 40, 60, 80, 100, 120]
    } else {
        &[]
    };
    let mut attempts = waits.iter();
    loop {
        match tmp.persist(target) {
            Ok(_) => break,
            Err(failure) => {
                if failure.error.kind() == std::io::ErrorKind::PermissionDenied && cfg!(windows) {
                    if let Some(ms) = attempts.next() {
                        tmp = failure.file;
                        std::thread::sleep(std::time::Duration::from_millis(*ms));
                        check()?;
                        continue;
                    }
                    return Err(file_error(
                        409,
                        "erro_arq_em_uso",
                        "arquivo aberto por outro programa",
                    ));
                }
                return Err(io_error(failure.error, "erro_arq_escrita_falhou"));
            }
        }
    }
    Ok(json!({"path":path,"size":text.len(),"digest":digest(text.as_bytes())}))
}
fn io_error(e: std::io::Error, code: &str) -> WorkspaceError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => file_error(
            403,
            "erro_arq_sem_permissao",
            "sem permissão de leitura ou escrita",
        ),
        std::io::ErrorKind::NotFound => {
            file_error(404, "erro_arq_inexistente", "caminho não existe")
        }
        _ => file_error(
            if code == "erro_arq_escrita_falhou" {
                409
            } else {
                500
            },
            code,
            "não deu para acessar esse arquivo ou pasta",
        ),
    }
}
fn git_error(e: WorkspaceError, code: &str) -> WorkspaceError {
    WorkspaceError {
        code: Some(code.into()),
        ..e
    }
}
fn repository(cwd: &Path) -> Result<bool> {
    let p = git::command(cwd, &["rev-parse", "--is-inside-work-tree"])
        .map_err(|e| git_error(e, "erro_arq_lista_falhou"))?;
    Ok(p.code == 0 && p.stdout.trim() == "true")
}

pub struct DirectoryState {
    marks: serde_json::Map<String, Value>,
    nums: HashMap<String, (u64, u64)>,
    in_repo: bool,
}

pub fn directory_state(cwd: &Path) -> Result<DirectoryState> {
    let mut marks = serde_json::Map::new();
    let mut nums = HashMap::new();
    let in_repo = repository(cwd)?;
    if in_repo {
        let prefix = git::command(cwd, &["rev-parse", "--show-prefix"])
            .map_err(|e| git_error(e, "erro_arq_lista_falhou"))?
            .stdout
            .trim()
            .to_owned();
        for item in git::changed(cwd)
            .map_err(|e| git_error(e, "erro_arq_lista_falhou"))?
            .as_array()
            .unwrap()
        {
            let p = item["path"].as_str().unwrap_or("");
            if let Some(p) = p.strip_prefix(&prefix) {
                marks.insert(
                    p.trim_end_matches('/').into(),
                    json!(
                        item["code"]
                            .as_str()
                            .unwrap_or("?")
                            .trim()
                            .chars()
                            .next()
                            .unwrap_or('?')
                            .to_string()
                    ),
                );
            }
        }
        let head = git::command(cwd, &["rev-parse", "--verify", "-q", "HEAD"])
            .map_err(|e| git_error(e, "erro_arq_lista_falhou"))?;
        if head.code != 1 || !head.stderr.trim().is_empty() {
            if head.code != 0 {
                return Err(file_error(
                    500,
                    "erro_arq_lista_falhou",
                    "não deu para consultar HEAD",
                ));
            }
            let out = git::checked(
                cwd,
                &["-c", "core.quotePath=false", "diff", "--numstat", "HEAD"],
                500,
            )
            .map_err(|e| git_error(e, "erro_arq_lista_falhou"))?;
            for (p, n) in git::numstat(&out.stdout) {
                if let Some(p) = p.strip_prefix(&prefix) {
                    nums.insert(p.to_owned(), n);
                }
            }
        }
    }
    Ok(DirectoryState {
        marks,
        nums,
        in_repo,
    })
}

pub fn list(cwd: &Path, path: Option<&str>, modified: bool) -> Result<Value> {
    // Valida antes de consultar o Git: caminho recusado não causa trabalho no repositório.
    resolve(cwd, path.unwrap_or(""))?;
    list_with_state(cwd, path, modified, &directory_state(cwd)?)
}

pub fn list_with_state(
    cwd: &Path,
    path: Option<&str>,
    modified: bool,
    state: &DirectoryState,
) -> Result<Value> {
    let root = real(cwd);
    let target = resolve(cwd, path.unwrap_or(""))?;
    if !target.is_dir() {
        return Err(file_error(400, "erro_arq_nao_e_pasta", "não é uma pasta"));
    }
    let marks = &state.marks;
    let nums = &state.nums;
    let modified = modified && state.in_repo;
    let mut raw = std::fs::read_dir(&target)
        .map_err(|e| io_error(e, "erro_arq_lista_falhou"))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| io_error(e, "erro_arq_lista_falhou"))?;
    raw.sort_by_key(|e| {
        (
            !e.path().is_dir(),
            e.file_name().to_string_lossy().to_lowercase(),
        )
    });
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in raw {
        let physical = real(&entry.path());
        if !physical.starts_with(&root)
            || physical
                .strip_prefix(&root)
                .unwrap()
                .components()
                .any(|p| p.as_os_str() == ".git")
        {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&root)
            .unwrap_or(&entry.path())
            .to_string_lossy()
            .into_owned();
        let git_relative = if cfg!(windows) {
            relative.replace('\\', "/")
        } else {
            relative.clone()
        };
        let dir = entry.path().is_dir();
        let descendants = marks
            .keys()
            .filter(|p| p.as_str() == git_relative || p.starts_with(&format!("{git_relative}/")))
            .collect::<Vec<_>>();
        let mark = marks.get(&git_relative).cloned().or_else(|| {
            if dir {
                descendants.first().and_then(|p| marks.get(*p).cloned())
            } else {
                None
            }
        });
        if modified && mark.is_none() {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: relative,
            is_dir: dir,
            size: if dir {
                0
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            },
            changed: mark.and_then(|v| v.as_str().map(str::to_owned)),
            add: descendants
                .iter()
                .map(|p| nums.get(*p).map(|v| v.0).unwrap_or(0))
                .sum(),
            del: descendants
                .iter()
                .map(|p| nums.get(*p).map(|v| v.1).unwrap_or(0))
                .sum(),
        });
    }
    Ok(json!({"entries":entries,"truncated":truncated}))
}
pub fn repo_files(cwd: &Path) -> Result<Vec<String>> {
    let p = git::command(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )
    .map_err(|e| git_error(e, "erro_arq_busca_falhou"))?;
    if p.code != 0 {
        return Err(file_error(
            409,
            "erro_arq_busca_falhou",
            p.reason("git ls-files falhou"),
        ));
    }
    let mut seen = HashSet::new();
    Ok(p.stdout
        .split('\0')
        .filter(|p| !p.is_empty() && seen.insert((*p).to_owned()))
        .map(str::to_owned)
        .collect())
}
pub fn search(cwd: &Path, q: &str, mode: &str) -> Result<Value> {
    if q.trim().is_empty() {
        return Err(file_error(
            400,
            "erro_arq_busca_vazia",
            "digite algo para buscar",
        ));
    }
    if q.contains('\0') {
        return Err(file_error(
            400,
            "erro_arq_busca_falhou",
            "termo de busca inválido",
        ));
    }
    if !["names", "contents"].contains(&mode) {
        return Err(file_error(
            400,
            "erro_arq_modo_invalido",
            "modo de busca inválido",
        ));
    }
    if !repository(cwd).map_err(|e| git_error(e, "erro_arq_busca_falhou"))? {
        return Err(file_error(
            409,
            "erro_arq_nao_e_repo_git",
            "a busca precisa de um repositório git",
        ));
    }
    let mut hits = Vec::new();
    if mode == "names" {
        let needle = q.to_lowercase();
        for p in repo_files(cwd)? {
            if p.to_lowercase().contains(&needle) && cwd.join(&p).symlink_metadata().is_ok() {
                hits.push(json!({"path":p,"line":null,"text":null}));
            }
        }
    } else {
        let p = git::command(
            cwd,
            &[
                "-c",
                "core.quotePath=false",
                "grep",
                "-z",
                "-n",
                "-I",
                "--untracked",
                "-F",
                "-e",
                q,
            ],
        )
        .map_err(|e| git_error(e, "erro_arq_busca_falhou"))?;
        if ![0, 1].contains(&p.code) {
            return Err(file_error(
                409,
                "erro_arq_busca_falhou",
                p.reason("git grep falhou"),
            ));
        }
        if p.code == 0 {
            let mut rest = p.stdout.as_str();
            while let Some((path, r)) = rest.split_once('\0') {
                let Some((line, r)) = r.split_once('\0') else {
                    break;
                };
                let (text, r) = r.split_once('\n').unwrap_or((r, ""));
                hits.push(json!({"path":path,"line":line.parse::<u64>().unwrap_or(0),"text":text}));
                rest = r;
            }
        }
    }
    let truncated = hits.len() > MAX_HITS;
    hits.truncate(MAX_HITS);
    Ok(json!({"hits":hits,"truncated":truncated,"mode":mode}))
}
pub fn resolver(cwd: &Path, paths: &[String], suffix: bool) -> Result<Value> {
    let root = real(cwd);
    let mut ok = serde_json::Map::new();
    let mut missing = Vec::new();
    let mut repo = None;
    for raw in paths.iter().take(500) {
        if raw.is_empty() || raw.contains('\0') {
            missing.push(raw.clone());
            continue;
        }
        let expanded = if raw.starts_with('~') {
            real(Path::new(raw))
        } else {
            PathBuf::from(raw)
        };
        let mut found = None;
        let mut logical = None;
        if expanded.is_absolute() {
            if expanded.symlink_metadata().is_ok() {
                found = Some(real(&expanded));
            }
        } else {
            let normalized = raw.replace('\\', "/");
            let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
            let direct = real(&cwd.join(normalized));
            if !normalized.split('/').any(|p| p == "..")
                && cwd.join(normalized).symlink_metadata().is_ok()
            {
                found = Some(direct);
                logical = Some(normalized.to_owned());
            } else if suffix {
                if repo.is_none() {
                    repo = Some(if repository(cwd)? {
                        repo_files(cwd).unwrap_or_default()
                    } else {
                        Vec::new()
                    });
                }
                let segments = raw.trim_start_matches("./").replace('\\', "/");
                let mut tail = segments.as_str();
                loop {
                    let matches = repo
                        .as_ref()
                        .unwrap()
                        .iter()
                        .filter(|p| p.as_str() == tail || p.ends_with(&format!("/{tail}")))
                        .filter(|p| cwd.join(p).symlink_metadata().is_ok())
                        .collect::<Vec<_>>();
                    if let Some(first) = matches.first() {
                        found = Some(real(&cwd.join(first)));
                        logical = Some((*first).clone());
                        break;
                    }
                    if !matches.is_empty() {
                        break;
                    }
                    let Some((_, r)) = tail.split_once('/') else {
                        break;
                    };
                    tail = r;
                }
            }
        }
        if let Some(found) = found {
            let relative = logical.or_else(|| {
                found
                    .strip_prefix(&root)
                    .ok()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
            });
            ok.insert(
                raw.clone(),
                json!({"relativo":relative,"real":found.to_string_lossy()}),
            );
        } else {
            missing.push(raw.clone());
        }
    }
    Ok(json!({"ok":ok,"faltam":missing}))
}
pub fn roots(roots: &[String]) -> Result<Value> {
    Ok(json!(roots.iter().map(|r|{let p=real(Path::new(r));json!({"name":p.file_name().map(|s|s.to_string_lossy().into_owned()).filter(|s|!s.is_empty()).unwrap_or_else(||p.to_string_lossy().into_owned()),"path":p.to_string_lossy()})}).collect::<Vec<_>>()))
}
fn folder(root: &str, path: Option<&str>, roots: &[String]) -> Result<PathBuf> {
    if root.contains('\0') || path.is_some_and(|p| p.contains('\0')) {
        return Err(error(400, "invalid path"));
    }
    let root = real(Path::new(root));
    if !roots.iter().any(|r| real(Path::new(r)) == root) {
        return Err(error(403, "root not allowed"));
    }
    let target = path.map(|p| real(Path::new(p))).unwrap_or(root.clone());
    if !target.starts_with(&root) {
        return Err(error(400, "path escapes its root"));
    }
    if !target.exists() {
        return Err(error(404, "path not found"));
    }
    if !target.is_dir() {
        return Err(error(400, "not a directory"));
    }
    Ok(target)
}
pub fn scan(root: &str, path: Option<&str>, roots: &[String]) -> Result<Value> {
    let target = folder(root, path, roots)?;
    let allowed = real(Path::new(root));
    let raw = match std::fs::read_dir(&target) {
        Ok(r) => r,
        Err(e) => {
            return Ok(
                json!({"entries":[],"error":if e.kind()==std::io::ErrorKind::PermissionDenied{"permission_denied"}else{"unreadable"}}),
            );
        }
    };
    let mut list = Vec::new();
    for entry in raw.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let p = entry.path();
        if name.starts_with('.') || !p.is_dir() || !real(&p).starts_with(&allowed) {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64());
            list.push(json!({"name":name,"path":p.to_string_lossy(),"is_git":p.join(".git").exists(),"has_claude_md":p.join("CLAUDE.md").is_file(),"mtime":mtime}));
        }
    }
    list.sort_by(|a, b| {
        b["mtime"]
            .as_f64()
            .unwrap_or(0.)
            .total_cmp(&a["mtime"].as_f64().unwrap_or(0.))
    });
    Ok(json!({"entries":list,"error":null}))
}
pub fn mkdir(root: &str, path: Option<&str>, name: &str, roots: &[String]) -> Result<Value> {
    let parent = folder(root, path, roots)?;
    let name = name.trim();
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', '\0']) {
        return Err(error(400, "invalid folder name"));
    }
    let child = parent.join(name);
    std::fs::create_dir(&child).map_err(|e| {
        error(
            match e.kind() {
                std::io::ErrorKind::AlreadyExists => 409,
                std::io::ErrorKind::PermissionDenied => 403,
                _ => 400,
            },
            "could not create folder",
        )
    })?;
    let mtime = child
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64());
    Ok(
        json!({"name":name,"path":child.to_string_lossy(),"is_git":false,"has_claude_md":false,"mtime":mtime}),
    )
}

//! Políticas de raiz e mecânica de leitura/escrita são separadas para os arquivos citados.
mod open_root;
pub use open_root::{OpenedRootFile, open_root_file};
use crate::{Result, WorkspaceError, error, file_error, git, real};
use hangar_api::workspace::{FileContent, FileEntry};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub const MAX_BYTES: usize = 512 * 1024;
/// Teto da leitura pedida com `large`: o leitor de arquivo grande abre sem edição até aqui.
pub const LARGE_MAX_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 1000;
pub const MAX_HITS: usize = 200;

pub fn resolve(cwd: &Path, path: &str) -> Result<PathBuf> {
    path_syntax(path)?;
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
/// `large` sobe o teto para LARGE_MAX_BYTES; a impressão (e com ela a edição) continua só até MAX_BYTES.
pub fn read(target: &Path, path: &str, large: bool) -> Result<Value> {
    let limit = if large { LARGE_MAX_BYTES } else { MAX_BYTES };
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
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(e, "erro_arq_lista_falhou"))?;
    if bytes.contains(&0) {
        return Err(file_error(415, "erro_arq_binario", "arquivo binário"));
    }
    let truncated = bytes.len() > limit;
    let digest = (bytes.len() <= MAX_BYTES).then(|| digest(&bytes));
    bytes.truncate(limit);
    let result = FileContent {
        path: path.into(),
        text: String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
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
        let file = std::fs::File::open(target).map_err(vanished)?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(vanished)?;
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
/// Como o filetree.py: o arquivo lido sumiu entre a leitura e a gravação é sempre 409, inclusive
/// quando foi apagado (o agente da sessão mexe nos mesmos arquivos).
fn vanished(e: std::io::Error) -> WorkspaceError {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        return io_error(e, "erro_arq_sumiu");
    }
    file_error(409, "erro_arq_sumiu", "o arquivo sumiu do disco")
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
                // Segue o atalho, como o `e.stat()` do Python; quebrado fica sem tamanho.
                std::fs::metadata(entry.path()).map(|m| m.len()).unwrap_or(0)
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
        let expanded = crate::expand_home(raw);
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
/// Tamanho (bytes) e data (segundos Unix) de um achado do `resolver`: o cliente mostra o tamanho antes de baixar.
/// Fica fora do `resolver` para a operação manter o corpo do contrato; pasta e arquivo que sumiu ficam sem os campos.
pub fn with_stat(mut entry: Value) -> Value {
    let meta = entry["real"].as_str().and_then(|path| std::fs::metadata(path).ok()).filter(|meta| meta.is_file());
    if let Some(meta) = meta {
        entry["size"] = json!(meta.len());
        if let Some(at) = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {
            entry["mtime"] = json!(at.as_secs());
        }
    }
    entry
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
    // Vazio é a própria raiz, como o `if not path` do Python.
    let target = path
        .filter(|p| !p.is_empty())
        .map(|p| real(Path::new(p)))
        .unwrap_or(root.clone());
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
/// Caminho que o cliente manda, antes de juntar à pasta da sessão.
fn path_syntax(path: &str) -> Result<()> {
    if path.starts_with('-') || path.contains('\0') || Path::new(path).is_absolute() {
        return Err(file_error(400, "erro_arq_caminho_invalido", "caminho inválido"));
    }
    Ok(())
}

/// Nome de um item criado ou renomeado: uma parte só do caminho, nunca `.git`. No Windows `C:nome` também tem
/// prefixo de unidade, e o `join` trocaria a pasta da sessão pela pasta corrente dessa unidade.
fn entry_name(name: &str) -> Result<&str> {
    let name = name.trim();
    let mut parts = Path::new(name).components();
    let single = matches!((parts.next(), parts.next()), (Some(std::path::Component::Normal(part)), None) if part == name);
    if !single || is_git_name(std::ffi::OsStr::new(name)) || name.contains(['/', '\\', '\0']) || windows_reserved(name) {
        return Err(file_error(400, "erro_arq_nome_invalido", "nome inválido"));
    }
    Ok(name)
}

/// No Windows `:` abre um fluxo alternativo (`.git:x` grava no próprio `.git`) e `NUL`, `CON` e afins são
/// dispositivos, com ou sem extensão.
fn windows_reserved(name: &str) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ').to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    name.contains(':') || device
}

/// `.git` como o disco o entende: Windows e macOS não distinguem caixa, e o Windows tira ponto e espaço do fim do nome.
fn is_git_name(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|name| name.trim_end_matches(['.', ' ']).eq_ignore_ascii_case(".git"))
}

/// O item em si, não o destino de um atalho: renomear ou apagar um link mexe no link, mesmo que ele aponte para
/// fora. A pasta de cima é a que passa pelas guardas da leitura; a raiz da sessão nunca é alvo.
fn own_entry(cwd: &Path, path: &str) -> Result<PathBuf> {
    path_syntax(path)?;
    // `.git::$INDEX_ALLOCATION` abre o próprio `.git`, e `NUL` um dispositivo.
    if cfg!(windows) && Path::new(path).components().any(|c| c.as_os_str().to_str().is_none_or(windows_reserved)) {
        return Err(file_error(400, "erro_arq_caminho_invalido", "caminho inválido"));
    }
    let joined = cwd.join(path);
    let name = joined.file_name().filter(|n| *n != "." && *n != "..")
        .ok_or_else(|| file_error(400, "erro_arq_caminho_invalido", "caminho inválido"))?;
    let root = real(cwd);
    let link = joined.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink());
    if !link && real(&joined) == root {
        return Err(file_error(400, "erro_arq_raiz_da_sessao", "a pasta da sessão não muda por aqui"));
    }
    let parent = real(joined.parent().unwrap_or(cwd));
    if !parent.starts_with(&root) {
        return Err(file_error(400, "erro_arq_fora_da_raiz", "caminho sai da raiz da sessão"));
    }
    let entry = parent.join(name);
    if entry == root {
        return Err(file_error(400, "erro_arq_raiz_da_sessao", "a pasta da sessão não muda por aqui"));
    }
    // O último nome não passa pelo `real`: `.GIT`, `.git.` ou o nome curto `GIT~1` chegariam ao `.git` de verdade.
    let git_area = || file_error(403, "erro_arq_area_do_git", "área interna do git");
    if entry.strip_prefix(&root).is_ok_and(|rest| rest.components().any(|c| is_git_name(c.as_os_str()))) {
        return Err(git_area());
    }
    if entry.symlink_metadata().is_err() {
        return Err(file_error(404, "erro_arq_inexistente", "caminho não existe"));
    }
    // A pasta de cima já veio com nomes longos; falta o do item. Sem conseguir lê-lo, recusa.
    if cfg!(windows) {
        let long = long_name(&entry).map_err(|e| {
            tracing::warn!(tipo = ?e.kind(), os = ?e.raw_os_error(), "nome longo do item ilegível");
            io_error(e, "erro_arq_escrita_falhou")
        })?;
        if long.file_name().is_none_or(is_git_name) {
            return Err(git_area());
        }
    }
    Ok(entry)
}

/// O nome que o disco guarda para o caminho, sem seguir o link do fim.
#[cfg(windows)]
fn long_name(path: &Path) -> std::io::Result<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut buffer = vec![0u16; 1024];
    loop {
        // SAFETY: `wide` termina em NUL e o tamanho passado é o de `buffer`.
        let n = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetLongPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32)
        } as usize;
        if n == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n < buffer.len() {
            return Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..n])));
        }
        buffer.resize(n, 0);
    }
}

#[cfg(not(windows))]
fn long_name(path: &Path) -> std::io::Result<PathBuf> {
    Ok(path.to_path_buf())
}

fn exists_error(e: std::io::Error) -> WorkspaceError {
    if e.kind() == std::io::ErrorKind::AlreadyExists {
        file_error(409, "erro_arq_ja_existe", "já existe um item com esse nome")
    } else {
        io_error(e, "erro_arq_escrita_falhou")
    }
}

/// Novo nome na mesma pasta. Nome que já existe recusa; só a troca de maiúsculas do próprio item passa.
pub fn rename(cwd: &Path, path: &str, name: &str) -> Result<Value> {
    let source = own_entry(cwd, path)?;
    let name = entry_name(name)?;
    let target = source.with_file_name(name);
    if target == source {
        return Ok(json!({"path": requested(path, name)}));
    }
    // Ocupado é outra entrada da pasta com esse nome em qualquer caixa, comparada sem seguir link: só sobra o
    // próprio item, na troca de maiúsculas de um sistema que não as distingue.
    let own_case = target.symlink_metadata().is_ok();
    if own_case {
        let lower = name.to_lowercase();
        let own = source.file_name();
        let parent = source.parent().unwrap_or(cwd);
        let other = std::fs::read_dir(parent).map_err(|e| io_error(e, "erro_arq_escrita_falhou"))?.flatten()
            .any(|e| Some(e.file_name().as_os_str()) != own && e.file_name().to_string_lossy().to_lowercase() == lower);
        if other || own.is_none_or(|own| own.to_string_lossy().to_lowercase() != lower) {
            return Err(file_error(409, "erro_arq_ja_existe", "já existe um item com esse nome"));
        }
    }
    // O item que surgir no destino depois da conferência não é substituído.
    let moved = if own_case { std::fs::rename(&source, &target) } else { rename_no_replace(&source, &target) };
    moved.map_err(exists_error)?;
    Ok(json!({"path": requested(path, name)}))
}

/// O caminho como a árvore o mostra, com o atalho da pasta de cima, e não o resolvido.
fn requested(path: &str, name: &str) -> String {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/{name}"),
        None => name.to_owned(),
    }
}

#[cfg(target_os = "linux")]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| {
        std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    };
    let (c_from, c_to) = (c(from)?, c(to)?);
    // SAFETY: os dois caminhos são C strings válidas durante a chamada.
    let done = unsafe {
        libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, c_from.as_ptr(), libc::AT_FDCWD, c_to.as_ptr(), libc::RENAME_NOREPLACE)
    };
    if done == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    // Sistema de arquivos sem a troca exclusiva.
    if matches!(error.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS)) {
        tracing::warn!(os = ?error.raw_os_error(), "renameat2 sem RENAME_NOREPLACE: troca conferida antes, sem garantia contra corrida");
        return rename_checked(from, to);
    }
    Err(error)
}

#[cfg(target_os = "macos")]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| {
        std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
    };
    let (c_from, c_to) = (c(from)?, c(to)?);
    // SAFETY: os dois caminhos são C strings válidas durante a chamada.
    if unsafe { libc::renamex_np(c_from.as_ptr(), c_to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let (from, to) = (wide(from), wide(to));
    // SAFETY: os dois caminhos terminam em NUL; sem `MOVEFILE_REPLACE_EXISTING` o destino ocupado recusa.
    if unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    rename_checked(from, to)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn rename_checked(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.symlink_metadata().is_ok() {
        return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
    }
    std::fs::rename(from, to)
}

/// O tipo do erro da lixeira, sem o caminho que a mensagem dele carrega.
fn trash_kind(error: &trash::Error) -> String {
    let kind: String = format!("{error:?}").chars().take_while(char::is_ascii_alphanumeric).collect();
    match error {
        trash::Error::Os { code, .. } => format!("{kind} os={code}"),
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        trash::Error::FileSystem { source, .. } => format!("{kind} {:?} os={:?}", source.kind(), source.raw_os_error()),
        _ => kind,
    }
}

/// Manda para a lixeira do sistema da máquina da sessão; nunca apaga de vez.
pub fn trash(cwd: &Path, path: &str) -> Result<Value> {
    let entry = own_entry(cwd, path)?;
    trash::delete(&entry).map_err(|e| {
        tracing::warn!(erro = %trash_kind(&e), "lixeira recusou o item");
        file_error(409, "erro_arq_lixeira_falhou", "não deu para mandar para a lixeira")
    })?;
    Ok(json!({"path": path.trim_end_matches('/')}))
}

/// Arquivo vazio ou pasta dentro de `path` (vazio é a raiz da sessão).
pub fn create(cwd: &Path, path: Option<&str>, name: &str, dir: bool) -> Result<Value> {
    let folder = resolve(cwd, path.unwrap_or(""))?;
    if !folder.is_dir() {
        return Err(file_error(400, "erro_arq_nao_e_pasta", "não é uma pasta"));
    }
    let name = entry_name(name)?;
    let target = folder.join(name);
    if dir {
        std::fs::create_dir(&target).map_err(exists_error)?;
    } else {
        std::fs::OpenOptions::new().write(true).create_new(true).open(&target).map_err(exists_error)?;
    }
    let path = match path.map(|p| p.trim_end_matches('/')).filter(|p| !p.is_empty()) {
        Some(folder) => format!("{folder}/{name}"),
        None => name.to_owned(),
    };
    Ok(json!({"path": path, "dir": dir}))
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

#[cfg(test)]
mod tests {
    #[test]
    fn create_rename_and_guards_stay_inside_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        std::fs::create_dir(cwd.join(".git")).unwrap();
        assert_eq!(super::create(cwd, None, "docs", true).unwrap()["path"], "docs");
        assert_eq!(super::create(cwd, Some("docs"), "a.md", false).unwrap()["path"], "docs/a.md");
        assert_eq!(super::create(cwd, Some("docs"), "a.md", false).unwrap_err().code.as_deref(), Some("erro_arq_ja_existe"));
        assert_eq!(super::rename(cwd, "docs/a.md", "b.md").unwrap()["path"], "docs/b.md");
        assert!(cwd.join("docs/b.md").is_file() && !cwd.join("docs/a.md").exists());
        super::create(cwd, Some("docs"), "c.md", false).unwrap();
        assert_eq!(super::rename(cwd, "docs/c.md", "b.md").unwrap_err().code.as_deref(), Some("erro_arq_ja_existe"));
        for bad in ["", "..", ".git", "x/y"] {
            assert_eq!(super::rename(cwd, "docs/b.md", bad).unwrap_err().code.as_deref(), Some("erro_arq_nome_invalido"), "{bad}");
        }
        assert_eq!(super::rename(cwd, ".git", "x").unwrap_err().code.as_deref(), Some("erro_arq_area_do_git"));
        // Em disco que não distingue caixa, `.GIT` é o mesmo `.git`; em qualquer sistema a guarda recusa antes.
        assert_eq!(super::trash(cwd, ".GIT").unwrap_err().code.as_deref(), Some("erro_arq_area_do_git"));
        assert_eq!(super::rename(cwd, ".git.", "x").unwrap_err().code.as_deref(), Some("erro_arq_area_do_git"));
        assert_eq!(super::rename(cwd, "../fora", "x").unwrap_err().status, 400);
        assert_eq!(super::trash(cwd, "").unwrap_err().code.as_deref(), Some("erro_arq_raiz_da_sessao"));
    }

    #[cfg(unix)]
    #[test]
    fn rename_moves_the_link_not_what_it_points_to() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path();
        std::fs::write(cwd.join("alvo.txt"), "x").unwrap();
        std::os::unix::fs::symlink(cwd.join("alvo.txt"), cwd.join("atalho.txt")).unwrap();
        assert_eq!(super::rename(cwd, "atalho.txt", "outro.txt").unwrap()["path"], "outro.txt");
        assert!(cwd.join("alvo.txt").is_file());
        assert!(cwd.join("outro.txt").symlink_metadata().unwrap().file_type().is_symlink());
        // Link para fora da sessão: renomear mexe só no link, sem passar pela guarda do destino.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("segredo.txt"), "s").unwrap();
        std::os::unix::fs::symlink(outside.path().join("segredo.txt"), cwd.join("externo")).unwrap();
        assert_eq!(super::rename(cwd, "externo", "externo2").unwrap()["path"], "externo2");
        assert!(outside.path().join("segredo.txt").is_file());
        // Link `a -> A` não é o próprio `A` em outra caixa: a troca apagaria o arquivo.
        std::fs::write(cwd.join("A"), "conteúdo").unwrap();
        std::os::unix::fs::symlink(cwd.join("A"), cwd.join("a")).unwrap();
        assert_eq!(super::rename(cwd, "a", "A").unwrap_err().code.as_deref(), Some("erro_arq_ja_existe"));
        assert_eq!(std::fs::read_to_string(cwd.join("A")).unwrap(), "conteúdo");
        // Pasta que é atalho: a resposta traz o caminho que a árvore mostra, não o resolvido.
        std::fs::create_dir_all(cwd.join("vendor/lib/sub")).unwrap();
        std::os::unix::fs::symlink(cwd.join("vendor/lib"), cwd.join("lib")).unwrap();
        assert_eq!(super::rename(cwd, "lib/sub", "sub2").unwrap()["path"], "lib/sub2");
        assert_eq!(super::create(cwd, Some("lib/"), "novo.md", false).unwrap()["path"], "lib/novo.md");
        assert_eq!(super::trash(cwd, "lib/novo.md").unwrap()["path"], "lib/novo.md");
    }

    /// Destino que surge depois da conferência do nome livre não é substituído.
    #[test]
    fn rename_never_replaces_an_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("a.md"), dir.path().join("b.md"));
        std::fs::write(&from, "a").unwrap();
        std::fs::write(&to, "escrito pelo agente").unwrap();
        assert_eq!(super::rename_no_replace(&from, &to).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "escrito pelo agente");
        assert!(from.is_file());
        std::fs::remove_file(&to).unwrap();
        super::rename_no_replace(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "a");
    }

    /// O nome curto 8.3 do `.git` é o próprio `.git`.
    #[cfg(windows)]
    #[test]
    fn the_short_name_of_git_is_the_git_area() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let dir = tempfile::tempdir().unwrap();
        let cwd = crate::real(dir.path());
        std::fs::create_dir(cwd.join(".git")).unwrap();
        let wide: Vec<u16> = cwd.join(".git").as_os_str().encode_wide().chain([0]).collect();
        let mut buffer = vec![0u16; 1024];
        let n = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32)
        } as usize;
        assert!(n > 0 && n < buffer.len(), "GetShortPathNameW falhou");
        let short = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..n]));
        let short = short.file_name().unwrap().to_string_lossy().into_owned();
        assert_ne!(short, ".git", "o volume da pasta temporária não gera nomes 8.3: o teste não prova nada aqui");
        assert_eq!(super::trash(&cwd, &short).unwrap_err().code.as_deref(), Some("erro_arq_area_do_git"));
        assert_eq!(super::rename(&cwd, &short, "x").unwrap_err().code.as_deref(), Some("erro_arq_area_do_git"));
        assert_eq!(super::trash(&cwd, ".git::$INDEX_ALLOCATION").unwrap_err().code.as_deref(), Some("erro_arq_caminho_invalido"));
        assert!(cwd.join(".git").is_dir());
    }

    /// Fluxo alternativo e nome de dispositivo só são recusados onde o Windows os interpreta.
    #[test]
    fn windows_device_and_stream_names_are_refused_there() {
        for bad in ["nul", "CON.txt", "com1", "Lpt9.log", ".git:x", "a:b"] {
            assert_eq!(super::entry_name(bad).is_err(), cfg!(windows), "{bad}");
        }
        for ok in ["console.txt", "com10", "nulo.md", "lpt"] {
            assert!(super::entry_name(ok).is_ok(), "{ok}");
        }
    }

    /// Nome com prefixo de unidade sairia da pasta no Windows; nos outros sistemas é um nome comum.
    #[test]
    fn entry_name_is_one_plain_component() {
        assert_eq!(super::entry_name(" arquivo.txt ").unwrap(), "arquivo.txt");
        for bad in ["", ".", "..", ".git", ".GIT", ".git.", ".Git ", "x/y", "x\\y"] {
            assert!(super::entry_name(bad).is_err(), "{bad}");
        }
        assert_eq!(super::entry_name("C:escape.txt").is_err(), cfg!(windows));
    }

    #[test]
    fn file_gone_during_save_is_a_conflict_like_python() {
        let gone = super::vanished(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert_eq!((gone.status, gone.code.as_deref()), (409, Some("erro_arq_sumiu")));
        let denied = super::vanished(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(denied.status, 403);
    }

    #[test]
    fn resolved_file_carries_size_and_mtime_and_folders_do_not() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/a.pdf"), b"12345").unwrap();
        let found = super::resolver(dir.path(), &["sub/a.pdf".into(), "sub".into()], false).unwrap();
        let file = super::with_stat(found["ok"]["sub/a.pdf"].clone());
        assert_eq!(file["size"], 5);
        let mtime = std::fs::metadata(dir.path().join("sub/a.pdf")).unwrap().modified().unwrap()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(file["mtime"], mtime);
        assert_eq!(file["relativo"], "sub/a.pdf", "os campos de antes ficam");
        let folder = super::with_stat(found["ok"]["sub"].clone());
        assert!(folder.get("size").is_none() && folder.get("mtime").is_none());
        let gone = super::with_stat(serde_json::json!({"relativo": null, "real": dir.path().join("sumiu.txt")}));
        assert!(gone.get("size").is_none());
    }
}

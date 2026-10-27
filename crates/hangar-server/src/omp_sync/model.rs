use super::*;
use std::{
    fs,
    sync::{Mutex, OnceLock},
    time::UNIX_EPOCH,
};
pub fn matches_re(pattern: &'static str, value: &str) -> bool {
    static CACHE: OnceLock<Mutex<BTreeMap<&'static str, regex::Regex>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    cache
        .entry(pattern)
        .or_insert_with(|| regex::Regex::new(pattern).expect("expressão constante válida"))
        .is_match(value)
}
pub fn name(v: &str) -> bool {
    matches_re(r"\A(?:@[a-z0-9._-]+/)?[a-z0-9][a-z0-9._-]*\z", v)
}
pub fn sha(v: &str) -> bool {
    matches_re(r"\A[0-9a-f]{40}\z", v)
}
pub fn features(v: &Value) -> bool {
    v.is_null()
        || v.as_array().is_some_and(|a| {
            a.iter().all(|s| {
                s.as_str()
                    .is_some_and(|s| matches_re(r"\A[A-Za-z0-9._-]+\z", s))
            })
        })
}
pub fn identity(raw: &str) -> Result<Value, String> {
    if raw.chars().any(char::is_whitespace) {
        return Err(err("Origem Git inválida"));
    }
    let mut raw = raw.strip_prefix("git+").unwrap_or(raw).to_owned();
    if matches_re(r"\A[^/@:]+@[^/:]+:.+\z", &raw) {
        let (a, b) = raw.split_once(':').unwrap();
        raw = format!("ssh://{a}/{b}")
    }
    let (scheme, rest) = raw
        .split_once("://")
        .ok_or_else(|| err("Origem Git não verificável"))?;
    let scheme_owned = scheme.to_ascii_lowercase();
    let scheme = scheme_owned.as_str();
    if !["https", "http", "ssh", "git"].contains(&scheme) {
        return Err(err("Origem Git não verificável"));
    }
    let (authority, path) = rest
        .split_once('/')
        .map(|(a, p)| (a, format!("/{p}")))
        .unwrap_or((rest, String::new()));
    if raw.contains('?') || raw.contains('#') {
        return Err(err("Origem com credenciais ou parâmetros recusada"));
    }
    let (user, hostport) = authority.rsplit_once('@').unwrap_or(("", authority));
    if user.contains(':') || (!user.is_empty() && scheme != "ssh") {
        return Err(err("Origem com credenciais ou parâmetros recusada"));
    }
    if path.is_empty() || path.split('/').any(|p| p == "." || p == "..") {
        return Err(err("Caminho da origem inválido"));
    }
    let default = match scheme {
        "https" => 443,
        "http" => 80,
        "ssh" => 22,
        _ => 9418,
    };
    let (host, port) = if hostport.starts_with('[') {
        let end = hostport.find(']').ok_or("Origem Git não verificável")?;
        let host = &hostport[1..end];
        let tail = &hostport[end + 1..];
        let port = if tail.is_empty() {
            default
        } else {
            tail.strip_prefix(':')
                .ok_or("Porta da origem inválida")?
                .parse::<u16>()
                .map_err(|_| err("Porta da origem inválida"))? as i32
        };
        (host, port)
    } else if let Some((h, p)) = hostport.rsplit_once(':') {
        (
            h,
            p.parse::<u16>()
                .map_err(|_| err("Porta da origem inválida"))? as i32,
        )
    } else {
        (hostport, default)
    };
    if host.is_empty() {
        return Err(err("Origem Git não verificável"));
    }
    Ok(json!([
        scheme,
        host.to_lowercase(),
        if port == 0 { default } else { port },
        user,
        path.strip_suffix(".git").unwrap_or(&path)
    ]))
}
pub fn spec(raw: &str) -> Result<(Value, String), String> {
    let (url, rev) = raw
        .rsplit_once('#')
        .ok_or("Instalação sem pin Git completo")?;
    if !sha(rev) {
        return Err(err("Revisão Git não verificável"));
    }
    Ok((identity(url)?, rev.into()))
}
pub fn inside(p: &Path, root: &Path) -> Result<PathBuf, String> {
    let p = p.canonicalize().map_err(io::os_error)?;
    let root = root.canonicalize().map_err(io::os_error)?;
    if !p.starts_with(root) {
        return Err(err("Caminho escapa da raiz do pacote"));
    }
    Ok(p)
}
pub fn manifest(root: &Path) -> Result<Value, String> {
    if io::symlink(root) {
        return Err(err("Raiz simbólica não pode ser gerenciada"));
    }
    let p = root.join("package.json");
    inside(&p, root)?;
    let package = io::read(&p, None)?;
    if !package["name"].as_str().is_some_and(name) {
        return Err(err("Nome de pacote inválido"));
    }
    let m = package
        .get("omp")
        .or_else(|| package.get("pi"))
        .ok_or("Manifesto OMP/Pi ausente")?;
    if !m.is_object() {
        return Err(err("Manifesto OMP/Pi ausente"));
    }
    let empty = json!({});
    let f = m
        .get("features")
        .unwrap_or(&empty)
        .as_object()
        .ok_or("Features inválidas")?;
    if f.values().any(|v| !v.is_object()) {
        return Err(err("Features inválidas"));
    }
    for group in std::iter::once(m).chain(f.values()) {
        for key in [
            "extensions",
            "skills",
            "prompts",
            "commands",
            "agents",
            "tools",
        ] {
            if let Some(paths) = group.get(key) {
                let paths = paths.as_array().ok_or("Caminhos de recursos inválidos")?;
                for entry in paths {
                    let entry = entry.as_str().ok_or("Caminhos de recursos inválidos")?;
                    let p = Path::new(entry);
                    if p.is_absolute()
                        || p.components().any(|p| p == std::path::Component::ParentDir)
                    {
                        return Err(err("Recurso fora da raiz instalável"));
                    }
                    inside(&root.join(p), root)?;
                }
            }
        }
    }
    Ok(package)
}
pub fn git_identity(root: &Path) -> Result<Option<(Value, String)>, String> {
    let d = root.join(".git");
    if !d.is_dir() || io::symlink(&d) {
        return Ok(None);
    }
    let config = fs::read_to_string(d.join("config")).map_err(io::os_error)?;
    let mut section = String::new();
    let mut keys = BTreeSet::new();
    let mut origin = String::new();
    for raw in config.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].into();
            continue;
        }
        let (key, val) = line.split_once('=').ok_or("MissingSectionHeaderError")?;
        if section.is_empty() {
            return Err(err("MissingSectionHeaderError"));
        }
        if !keys.insert((section.clone(), key.trim().to_lowercase())) {
            return Err(err("DuplicateOptionError"));
        }
        if section == "remote \"origin\"" && key.trim().eq_ignore_ascii_case("url") {
            origin = val.trim().into()
        }
    }
    let mut head = fs::read_to_string(d.join("HEAD"))
        .map_err(io::os_error)?
        .trim()
        .to_owned();
    if let Some(reference) = head.strip_prefix("ref: ") {
        if !reference.starts_with("refs/")
            || Path::new(reference)
                .components()
                .any(|c| c == std::path::Component::ParentDir)
        {
            return Err(err("Referência Git inválida"));
        }
        let p = d.join(reference);
        head = if p.is_file() {
            fs::read_to_string(inside(&p, &d)?)
                .map_err(io::os_error)?
                .trim()
                .into()
        } else {
            fs::read_to_string(d.join("packed-refs"))
                .unwrap_or_default()
                .lines()
                .find_map(|l| l.strip_suffix(&format!(" {reference}")).map(String::from))
                .unwrap_or_default()
        };
    }
    if !sha(&head) {
        return Err(err("HEAD Git inválido"));
    }
    Ok(Some((identity(&origin)?, head)))
}
pub fn bun_revision(root: &Path, name: &str) -> Result<Option<(Value, String)>, String> {
    let p = root.join("bun.lock");
    if !p.is_file() || io::symlink(&p) {
        return Ok(None);
    }
    let raw = fs::read_to_string(p).map_err(io::os_error)?;
    let re = regex::Regex::new(r#"("(?:[^"\\]|\\.)*")|//[^\n]*|/\*[\s\S]*?\*/"#).unwrap();
    let raw = re.replace_all(&raw, |c: &regex::Captures| {
        c.get(1).map_or("", |m| m.as_str()).to_owned()
    });
    let re = regex::Regex::new(r#"("(?:[^"\\]|\\.)*")|,(\s*[}\]])"#).unwrap();
    let raw = re.replace_all(&raw, |c: &regex::Captures| {
        c.get(1)
            .or_else(|| c.get(2))
            .map_or("", |m| m.as_str())
            .to_owned()
    });
    let v = io::parse(&raw)?;
    let Some(value) = v["packages"][name]
        .as_array()
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .and_then(|s| s.strip_prefix(&format!("{name}@")))
    else {
        return Ok(None);
    };
    spec(value).map(Some)
}
type Signature = Vec<(String, u128, i64)>;
#[cfg(test)]
thread_local! { pub(crate) static DIGEST_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub fn digest(root: &Path) -> Result<String, String> {
    fn walk(root: &Path, d: &Path, v: &mut Signature) -> Result<(), String> {
        let mut dirs = vec![];
        let mut files = vec![];
        for e in fs::read_dir(d).map_err(io::os_error)? {
            let p = e.map_err(io::os_error)?.path();
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            if io::symlink(&p) {
                return Err(err("Conteúdo simbólico não pode ser gerenciado"));
            }
            if p.is_dir() {
                dirs.push(p)
            } else {
                files.push(p)
            }
        }
        dirs.sort();
        files.sort();
        for p in dirs.iter().chain(&files) {
            let m = fs::metadata(p).map_err(io::os_error)?;
            v.push((
                p.strip_prefix(root)
                    .map_err(|_| err("OSError"))?
                    .to_string_lossy()
                    .into_owned(),
                m.modified()
                    .map_err(io::os_error)?
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| err("OSError"))?
                    .as_nanos(),
                if p.is_file() { m.len() as i64 } else { -1 },
            ));
        }
        for p in dirs {
            walk(root, &p, v)?
        }
        Ok(())
    }
    static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, (Signature, String)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut signature = vec![];
    walk(root, root, &mut signature)?;
    if let Some((old, d)) = cache.lock().unwrap().get(root) {
        if old == &signature {
            return Ok(d.clone());
        }
    }
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    for (rel, _, size) in &signature {
        hash.update(rel.as_bytes());
        hash.update(&[0]);
        if *size >= 0 {
            #[cfg(test)]
            DIGEST_READS.with(|reads| reads.set(reads.get() + 1));
            hash.update(&fs::read(root.join(rel)).map_err(io::os_error)?);
        }
    }
    let result = hash
        .finish()
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    cache
        .lock()
        .unwrap()
        .insert(root.to_owned(), (signature, result.clone()));
    Ok(result)
}

//! Servidor nesta máquina: anexos e o editor vão direto ao disco e ao processo, com as regras de
//! `backend/app/uploads.py` e das rotas `upload` e `open-editor` de `backend/app/api.py`. Atalho shell vai sempre ao
//! backend: é ele quem cria o terminal escondido que vira aba do painel. Servidor
//! remoto, pasta que não existe aqui ou vídeo (quadros e fala saem do backend) seguem pelo backend.
use super::*;
use crate::api::MAX_BYTES;
use std::{path::Path, process::{Command, Stdio}};

/// Os `VIDEO_EXTS` de `backend/app/video.py`.
const VIDEO_EXTS: [&str; 6] = ["mp4", "mov", "webm", "mkv", "m4v", "avi"];

fn refusal(status: u16, detail: impl Into<String>) -> Failure {
    Failure { status: Some(status), detail: detail.into(), retry_after: None, uncertain: false, code: None }
}

/// `_slug`: nome de pasta seguro. O backend normaliza acento (NFKD) antes; aqui só nome ASCII, o resto vai ao backend.
// ponytail: sem NFKD no nativo; nome de pasta ou sessão com acento cai no backend. Trazer a normalização se isso pesar.
fn slug(text: &str) -> Option<String> {
    if !text.is_ascii() { return None; }
    let mapped: String = text.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect();
    let trimmed = mapped.trim_matches('-');
    let slug = if trimmed.is_empty() || trimmed == "." || trimmed == ".." { "_" } else { trimmed };
    Some(slug.chars().take(64).collect())
}

/// `session_key` de `backend/app/models.py`: id durável do transcript (Kimi: pasta da sessão; Codex: uuid do rollout).
fn session_id(jsonl: &str) -> Option<String> {
    let path = Path::new(jsonl);
    let name = |p: Option<&Path>| p.and_then(Path::file_name).and_then(|n| n.to_str()).map(str::to_owned);
    if path.file_name().is_some_and(|n| n == "wire.jsonl") {
        let agents = path.parent().and_then(Path::parent);
        if name(agents).as_deref() == Some("agents") { return name(agents.and_then(Path::parent)); }
    }
    let stem = path.file_stem()?.to_str()?;
    let uuid = |id: &str| id.len() == 36 && id.char_indices().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() });
    if stem.is_ascii() && stem.starts_with("rollout-") && stem.len() >= 45 && &stem[stem.len() - 37..stem.len() - 36] == "-" {
        let id = &stem[stem.len() - 36..];
        if uuid(id) { return Some(id.to_owned()); }
    }
    Some(stem.to_owned())
}

/// `_projeto`: nome da pasta do projeto + 6 hex do sha256 do caminho real (caminho fora de UTF-8 vai ao backend).
fn project(cwd: &Path) -> Option<String> {
    let digest = ring::digest::digest(&ring::digest::SHA256, cwd.to_str()?.as_bytes());
    let hex: String = digest.as_ref().iter().take(3).map(|b| format!("{b:02x}")).collect();
    Some(format!("{}-{hex}", slug(cwd.file_name()?.to_str()?)?))
}

/// `_base`: `~/.hangar/uploads/<projeto>/<id da sessão>/`; sem transcript, o id é o nome da sessão.
fn uploads_dir(home: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
    Some(home.join(".hangar").join("uploads").join(project(cwd)?).join(slug(id)?))
}

/// `list_uploads`: arquivos da pasta, mais recente primeiro; um item que falha no stat é pulado.
fn list(dir: &Path) -> Vec<UploadFile> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<UploadFile> = entries.flatten().filter_map(|entry| {
        let meta = std::fs::metadata(entry.path()).ok().filter(|m| m.is_file())?;
        let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
        Some(UploadFile { filename: entry.file_name().into_string().ok()?, size: meta.len(), mtime })
    }).collect();
    files.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
    files
}

/// `resolve_upload`: só um nome solto, e o caminho real tem de ser `<pasta real>/<nome>` (symlink para fora é recusado).
fn resolve(dir: &Path, filename: &str) -> Result<PathBuf, Failure> {
    if filename.is_empty() || filename.contains(['/', '\\']) || filename.contains("..") { return Err(refusal(400, "filename invalido")); }
    let missing = || refusal(404, "arquivo nao encontrado");
    let base = std::fs::canonicalize(dir).map_err(|_| missing())?;
    let real = std::fs::canonicalize(dir.join(filename)).map_err(|_| missing())?;
    if real != base.join(filename) { return Err(refusal(400, "caminho invalido")); }
    if !real.is_file() { return Err(missing()); }
    Ok(real)
}

fn read(dir: &Path, filename: &str) -> Result<Vec<u8>, Failure> {
    read_file(&resolve(dir, filename)?)
}

fn read_file(path: &Path) -> Result<Vec<u8>, Failure> {
    if std::fs::metadata(path).map_err(|_| Failure::local("invalid_response"))?.len() > MAX_BYTES { return Err(Failure::local("attach_too_big")); }
    std::fs::read(path).map_err(|_| Failure::local("invalid_response"))
}

/// `_resolver_citado` sem a varredura do transcript: o caminho saiu dos eventos da própria conversa, que são o transcript
/// (varrê-lo custa segundos numa conversa longa). `cwd` é o caminho real da pasta da sessão. Relativo resolve dentro dela
/// e não sai; `.git` fica de fora pelo caminho real. `None` manda ao backend, que tenta também as pastas das linhas que
/// citaram e dá o erro certo.
fn cited(cwd: &Path, path: &str) -> Option<PathBuf> {
    let expanded = match path.strip_prefix('~') {
        Some("") => std::env::home_dir()?,
        Some(rest) => std::env::home_dir()?.join(rest.strip_prefix('/')?),
        None => PathBuf::from(path),
    };
    let real = if expanded.is_absolute() { std::fs::canonicalize(&expanded).ok()? } else {
        if path.split(['/', '\\']).any(|part| part == "..") { return None; }
        let real = std::fs::canonicalize(cwd.join(&expanded)).ok()?;
        if real == cwd || !real.starts_with(cwd) { return None; }
        real
    };
    (real.is_file() && !real.components().any(|part| part.as_os_str() == ".git")).then_some(real)
}

/// `canonicalize` no Windows devolve `\\?\C:\...`, que o Explorer e outros programas não abrem.
pub(super) fn plain_path(text: &str) -> String {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") { return format!(r"\\{rest}"); }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_owned(),
        _ => text.to_owned(),
    }
}

/// `get_transcript_image`: a `index`-ésima imagem base64 da linha do evento `id`. Só a linha que contém o id vira JSON.
fn transcript_image(jsonl: &Path, id: &str, index: usize) -> Option<Vec<u8>> {
    use std::io::BufRead;
    use base64::Engine as _;
    let mut reader = std::io::BufReader::new(std::fs::File::open(jsonl).ok()?);
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line).ok()? > 0 {
        if std::str::from_utf8(&line).is_ok_and(|text| text.contains(id))
            && let Ok(event) = serde_json::from_slice::<Value>(&line)
            && event.get("uuid").and_then(Value::as_str) == Some(id) {
            let image = event.pointer("/message/content")?.as_array()?.iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("image")).nth(index)?;
            return base64::engine::general_purpose::STANDARD.decode(image.pointer("/source/data")?.as_str()?).ok();
        }
        line.clear();
    }
    None
}

/// `_safe_ext` sobre o nome como ele iria no `X-Filename` (percent-encoded): [a-z0-9] até 8, ou `bin`.
fn safe_ext(filename: &str) -> String {
    let encoded = composer::encode_component(if filename.is_empty() { "arquivo" } else { filename });
    let suffix = encoded.rfind('.').filter(|&i| i > 0).map_or("", |i| &encoded[i + 1..]);
    let ext: String = suffix.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).take(8).collect();
    if ext.is_empty() { "bin".into() } else { ext }
}

/// `save_upload`: nome gerado aqui (segundos + 6 hex), só a extensão vem do arquivo; nunca sobrescreve.
fn save(dir: &Path, filename: &str, bytes: &[u8]) -> Result<Uploaded, Failure> {
    if bytes.is_empty() { return Err(refusal(400, "arquivo vazio")); }
    if bytes.len() as u64 > MAX_BYTES { return Err(refusal(413, "arquivo maior que 100 MiB")); }
    let mut token = [0u8; 3];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut token).map_err(|_| Failure::local("invalid_response"))?;
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let name = format!("{seconds}-{}.{}", token.iter().map(|b| format!("{b:02x}")).collect::<String>(), safe_ext(filename));
    std::fs::create_dir_all(dir).map_err(|error| refusal(500, error.to_string()))?;
    let path = std::fs::canonicalize(dir).map_err(|error| refusal(500, error.to_string()))?.join(&name);
    use std::io::Write;
    std::fs::OpenOptions::new().write(true).create_new(true).open(&path).and_then(|mut file| file.write_all(bytes))
        .map_err(|error| refusal(500, error.to_string()))?;
    Ok(Uploaded { path: path.to_string_lossy().into_owned(), frames: Vec::new(), transcript: Some(String::new()) })
}

/// `prune_old`: apaga do projeto inteiro (todas as sessões) o que passou de `days` dias; erro de um arquivo não para a
/// varredura. Pasta ligada por symlink não é percorrida, como o `rglob` do backend.
fn prune(project: &Path, days: i64) {
    if days <= 0 { return; }
    let Some(cut) = std::time::SystemTime::now().checked_sub(std::time::Duration::from_secs(days as u64 * 86_400)) else { return };
    let Ok(entries) = std::fs::read_dir(project) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) { prune(&path, days); continue; }
        if std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.modified().is_ok_and(|t| t < cut)) { let _ = std::fs::remove_file(&path); }
    }
}

/// `open-editor`: o binário da configuração do servidor com a pasta como único argumento, sem shell.
fn editor(binary: &str, cwd: &str) -> Result<Value, Failure> {
    launch(Command::new(binary).arg(cwd)).map_err(|error| refusal(500, format!("editor '{binary}' falhou: {error}")))
}

fn launch(command: &mut Command) -> std::io::Result<Value> {
    // Grupo próprio: fechar o app ou um Ctrl-C no terminal que o abriu não leva o programa junto.
    // ponytail: o backend usa setsid; grupo próprio basta sem terminal de controle. setsid via libc se precisar.
    #[cfg(unix)] { use std::os::unix::process::CommandExt; command.process_group(0); }
    // Editor lançado por .cmd (o `code` do VS Code) abriria um console junto.
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x0800_0000); }
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    // Quem espera é uma thread, senão o filho que termina fica zumbi até o app fechar.
    std::thread::spawn(move || { let _ = child.wait(); });
    Ok(json!({"ok": true}))
}

async fn blocking<T: Send + 'static>(job: impl FnOnce() -> Result<T, Failure> + Send + 'static) -> Result<T, Failure> {
    tokio::task::spawn_blocking(job).await.map_err(|_| Failure::local("invalid_response"))?
}

/// Onde os anexos de uma sessão são lidos e gravados. Local, também as imagens citadas e as do transcript vêm do disco.
#[derive(Clone)]
pub(super) enum Uploads { Local { dir: PathBuf, cwd: PathBuf, jsonl: PathBuf }, Remote }

impl Uploads {
    pub(super) async fn list(&self, api: &Api, name: &str) -> Result<Vec<UploadFile>, Failure> {
        match self {
            Uploads::Local { dir, .. } => { let dir = dir.clone(); blocking(move || Ok(list(&dir))).await }
            Uploads::Remote => api.uploads(name).await,
        }
    }

    pub(super) async fn fetch(&self, api: &Api, name: &str, source: &Source) -> Result<Vec<u8>, Failure> {
        let Uploads::Local { dir, cwd, jsonl } = self else { return api.fetch(name, source).await };
        let (dir, cwd, jsonl, local) = (dir.clone(), cwd.clone(), jsonl.clone(), source.clone());
        // `None`: não deu para ler daqui; o backend responde, com o erro dele se for o caso.
        let read = blocking(move || Ok(match local {
            Source::Upload(file) => Some(read(&dir, &file)),
            Source::Cited(path) => cited(&cwd, &path).map(|path| read_file(&path)),
            Source::Transcript(id, index) => transcript_image(&jsonl, &id, index).map(Ok),
            Source::Remote(_) => None,
            Source::Memory(_, bytes) => Some(Ok(bytes.0.to_vec())),
        })).await?;
        match read { Some(result) => result, None => api.fetch(name, source).await }
    }

    /// Vídeo sobe pelo backend, que extrai quadros e transcreve a fala; o resto grava aqui e varre os vencidos do projeto.
    pub(super) async fn upload(&self, api: &Api, name: &str, filename: &str, bytes: Vec<u8>, retention: Option<i64>) -> Result<Uploaded, Failure> {
        match self {
            Uploads::Local { dir, .. } if !VIDEO_EXTS.contains(&safe_ext(filename).as_str()) => {
                let (dir, filename) = (dir.clone(), filename.to_owned());
                blocking(move || {
                    let saved = save(&dir, &filename, &bytes)?;
                    if let (Some(days), Some(project)) = (retention, dir.parent()) { prune(project, days); }
                    Ok(saved)
                }).await
            }
            _ => api.upload(name, filename, composer::mime_for(filename), bytes).await,
        }
    }
}

/// Retenção dos anexos (`upload_retention_days`); sem a configuração, a varredura fica para o próximo envio.
pub(super) async fn retention(api: &Api) -> Option<i64> {
    api.config().await.ok()?.pointer("/campos/upload_retention_days/valor")?.as_i64()
}

impl Hangar {
    /// A sessão roda nesta máquina: servidor em loopback e a pasta dela existe aqui (o mesmo critério do painel de git).
    /// Fora do Unix tudo segue pelo backend: shell, pasta pessoal e separador de caminho mudam.
    /// `api` é a máquina dona da sessão: a aberta pode ser de outra que não a ativa.
    fn local_cwd(&self, api: Option<Api>, name: &str) -> Option<(String, PathBuf)> {
        let api = api?;
        if !cfg!(unix) || !api.is_loopback() { return None; }
        // A aberta vem antes: a lista ativa pode ter outra de mesmo nome quando ela é de outra máquina.
        let open = self.selected.as_ref().filter(|s| s.name == name && self.session_server().as_deref() == Some(api.identity().as_str()));
        let cwd = open.or_else(|| self.sessions_of(&api.identity()).iter().find(|s| s.name == name))?.cwd.clone()?;
        let real = self.local_dirs.get(&cwd).cloned().flatten()?;
        Some((cwd, real))
    }

    /// A sessão aberta é desta máquina e a pasta dela já foi resolvida. Sem tocar no disco: roda a cada quadro.
    pub(super) fn session_on_disk(&self) -> bool {
        self.session_api().is_some_and(|api| api.is_loopback())
            && self.selected.as_ref().and_then(|s| s.cwd.as_ref()).is_some_and(|cwd| matches!(self.local_dirs.get(cwd), Some(Some(_))))
    }

    /// O arquivo do visor neste disco, para o gerenciador de arquivos do sistema. Sessão de outra máquina: `None`.
    /// Sem o corte do Unix do `local_cwd`: aqui só se entrega o caminho ao sistema, sem shell nem separador.
    /// Toca no disco: só no clique, nunca no render.
    pub(super) fn file_on_disk(&self, path: &str) -> Option<PathBuf> {
        self.session_api().filter(Api::is_loopback)?;
        let cwd = self.selected.as_ref()?.cwd.as_ref()?;
        let real = self.local_dirs.get(cwd).cloned().flatten()?;
        let real = cited(&real, path)?;
        Some(real.to_str().map_or_else(|| real.clone(), |text| PathBuf::from(plain_path(text))))
    }

    /// Resolve em segundo plano a pasta real das sessões ainda não vistas; até chegar, elas seguem pelo backend.
    pub(super) fn resolve_local_dirs(&mut self, cx: &mut Context<Self>) {
        if !self.api.as_ref().is_some_and(|api| api.is_loopback()) { return; }
        // A worktree onde o agente trabalha também: é nela que o painel de git roda.
        let pending: Vec<String> = self.sessions.iter().flat_map(|s| [s.cwd.clone(), s.git_cwd.clone()]).flatten()
            .filter(|cwd| !self.local_dirs.contains_key(cwd)).collect();
        if pending.is_empty() { return; }
        for cwd in &pending { self.local_dirs.insert(cwd.clone(), None); }
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move {
                pending.into_iter().map(|cwd| { let real = std::fs::canonicalize(&cwd).ok().filter(|p| p.is_dir()); (cwd, real) })
                    .collect::<Vec<_>>()
            }).await;
            let _ = this.update(cx, |this, cx| { this.local_dirs.extend(found); cx.notify(); });
        }).detach();
    }

    /// Anexos da sessão aberta: da pasta desta máquina quando dá, senão pelo backend.
    pub(super) fn uploads_for(&self, key: &SessionKey) -> Uploads {
        let local = self.local_cwd(self.api_for(&key.server), &key.name).and_then(|(_, real)| {
            let id = if key.jsonl.is_empty() { Some(key.name.clone()) } else { session_id(&key.jsonl) }?;
            let dir = uploads_dir(&std::env::home_dir()?, &real, &id)?;
            Some(Uploads::Local { dir, cwd: real, jsonl: PathBuf::from(&key.jsonl) })
        });
        local.unwrap_or(Uploads::Remote)
    }

    /// `open-editor` local quando a sessão é desta máquina: o editor vem da configuração do servidor dela (`api`).
    pub(super) fn local_editor(&self, api: Option<Api>, name: &str) -> Option<impl Future<Output = Result<Value, Failure>> + use<>> {
        let (cwd, _) = self.local_cwd(api.clone(), name)?;
        let api = api?;
        Some(async move {
            let config = api.config().await?;
            let binary = config.pointer("/campos/editor/valor").and_then(Value::as_str).filter(|b| !b.is_empty()).map(str::to_owned)
                .ok_or_else(|| Failure::local("invalid_response"))?;
            blocking(move || editor(&binary, &cwd)).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{cited, plain_path, resolve, safe_ext, session_id, slug, transcript_image, uploads_dir};
    use std::path::Path;

    #[test]
    fn verbatim_windows_prefix_is_dropped_for_the_system() {
        assert_eq!(plain_path(r"\\?\C:\Users\a\x.wav"), r"C:\Users\a\x.wav");
        assert_eq!(plain_path(r"\\?\UNC\server\share\x.wav"), r"\\server\share\x.wav");
        // Volume sem letra só abre no formato verbatim; caminho do Unix passa igual.
        assert_eq!(plain_path(r"\\?\Volume{abc}\x.wav"), r"\\?\Volume{abc}\x.wav");
        assert_eq!(plain_path("/home/u/x.wav"), "/home/u/x.wav");
    }

    #[cfg(unix)]
    #[test]
    fn cited_path_stays_in_the_session_folder_and_out_of_git() {
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("hangar-cited-{}", std::process::id()));
        let cwd = root.join("repo");
        std::fs::create_dir_all(cwd.join(".git")).unwrap();
        std::fs::create_dir_all(cwd.join("sub")).unwrap();
        for file in ["sub/a.png", ".git/config"] { std::fs::write(cwd.join(file), b"x").unwrap(); }
        std::fs::write(root.join("fora.png"), b"x").unwrap();
        std::os::unix::fs::symlink(root.join("fora.png"), cwd.join("link.png")).unwrap();
        std::os::unix::fs::symlink(cwd.join(".git"), cwd.join("atalho")).unwrap();
        assert_eq!(cited(&cwd, "sub/a.png"), Some(cwd.join("sub/a.png")));
        let absolute = root.join("fora.png");
        assert_eq!(cited(&cwd, absolute.to_str().unwrap()), Some(absolute));
        // Relativo que sai da pasta (por `..` ou symlink), área do git, pasta e o que não existe vão ao backend.
        for path in ["../fora.png", "link.png", ".git/config", "atalho/config", "sub", "sumiu.png", "~outro/a.png"] {
            assert_eq!(cited(&cwd, path), None, "{path}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn transcript_image_is_read_from_the_event_line() {
        use base64::Engine as _;
        let jsonl = std::env::temp_dir().join(format!("hangar-transcript-{}.jsonl", std::process::id()));
        let data = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let lines = [
            serde_json::json!({"uuid": "b", "parentUuid": "a", "message": {"content": [{"type": "image", "source": {"data": data(b"filho")}}]}}),
            serde_json::json!({"uuid": "a", "message": {"content": [
                {"type": "text", "text": "oi"},
                {"type": "image", "source": {"data": data(b"um")}},
                {"type": "image", "source": {"data": data(b"dois")}}]}}),
        ];
        std::fs::write(&jsonl, lines.iter().map(|line| format!("{line}\n")).collect::<String>()).unwrap();
        // A linha que só cita o id como pai não conta; o índice conta só imagens.
        assert_eq!(transcript_image(&jsonl, "a", 1).as_deref(), Some(&b"dois"[..]));
        assert_eq!(transcript_image(&jsonl, "a", 0).as_deref(), Some(&b"um"[..]));
        assert_eq!(transcript_image(&jsonl, "a", 2), None);
        assert_eq!(transcript_image(&jsonl, "c", 0), None);
        let _ = std::fs::remove_file(&jsonl);
    }

    #[test]
    fn upload_folder_follows_the_backend_rule() {
        // hashlib.sha256(b"/home/u/meu projeto").hexdigest()[:6] == "1c77cd", o `_projeto` do backend.
        let dir = uploads_dir(Path::new("/home/u"), Path::new("/home/u/meu projeto"), "abc/../x").unwrap();
        assert_eq!(dir, Path::new("/home/u/.hangar/uploads/meu-projeto-1c77cd/abc-..-x"));
        assert_eq!(slug(".."), Some("_".into()));
        assert_eq!(slug("Área"), None);
        assert_eq!(session_id("/p/0f1e.jsonl").as_deref(), Some("0f1e"));
        assert_eq!(session_id("/s/wd/sid-1/agents/main/wire.jsonl").as_deref(), Some("sid-1"));
        assert_eq!(session_id("/c/rollout-2026-09-27T10-00-00-0199a1b2-c3d4-e5f6-a7b8-c9d0e1f2a3b4.jsonl").as_deref(),
            Some("0199a1b2-c3d4-e5f6-a7b8-c9d0e1f2a3b4"));
        assert_eq!((safe_ext("foto.PNG").as_str(), safe_ext("x").as_str(), safe_ext("a.tar gz").as_str()), ("png", "bin", "tar20gz"));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_outside_the_folder_is_refused() {
        let root = std::env::temp_dir().join(format!("hangar-uploads-{}", std::process::id()));
        let dir = root.join("sessao");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(root.join("fora.txt"), b"x").unwrap();
        std::fs::write(dir.join("dentro.txt"), b"x").unwrap();
        std::os::unix::fs::symlink(root.join("fora.txt"), dir.join("link.txt")).unwrap();
        assert!(resolve(&dir, "dentro.txt").is_ok());
        for name in ["../fora.txt", "link.txt", "a/b", ""] { assert!(resolve(&dir, name).is_err(), "{name}"); }
        let _ = std::fs::remove_dir_all(&root);
    }
}

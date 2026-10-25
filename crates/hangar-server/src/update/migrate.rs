//! Migração única de uma máquina do motor `git` para o pacote por versão, com volta.
//!
//!   hangar-server migrate-package [--dry-run] [--undo] [--checkout <pasta>]
//!
//! Modo desenvolvedor (o `current` aponta para o próprio checkout, nada é baixado) quando o checkout
//! tem edição local, commit à frente do upstream, está fora da `main` ou a máquina serve a tela pelo
//! `vite` (`SERVE=preview`); senão pacote (baixa o primeiro). Para o backend, copia os dados do
//! usuário para `~/.hangar/config/`, cria o `current`, regrava o serviço, os hooks do Kimi e do
//! Codex, os atalhos e os apontadores, sobe e prova. Falha antes da prova desfaz tudo e sobe o
//! backend de antes. `--undo` reaponta tudo para o checkout, que nunca é apagado.
//!
//! Cada efeito vai para o registro (`~/.hangar/migration.partial.json`, depois `migration.json`)
//! com o que a volta precisa, e é a mesma volta na falha e no `--undo`.
//! Saída: 0 feito ou nada a fazer, 1 falhou (máquina como estava), 2 uso errado.
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::ExitCode;
use std::time::Duration;

use base64::Engine;
use hangar_package::Manifest;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{HealthProbe, Layout, Reading, Release, Settings, UpdateError, UpdateLock};

pub const MARKER: &str = "migration.json";
const PARTIAL: &str = "migration.partial.json";
/// Dados do usuário, em `backend/` antes e em `~/.hangar/config/` depois.
const DATA: [&str; 4] = [".env", "peers.json", "projects.json", "launchers"];
/// Pastas com links para o checkout que passam a apontar para o `current` (Linux).
const LINK_DIRS: [&str; 7] = [".local/bin", ".claude/skills", ".claude/agents", ".pi/agent/extensions", ".omp/agent/extensions",
    ".pi/agent/skills-bridge", ".kimi-code/skills-bridge"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode { Package, Developer, NotMigrated }

/// O que decide o modo.
#[derive(Debug, Clone, Default)]
pub struct Facts { pub dirty: usize, pub untracked: usize, pub ahead: u64, pub branch: String, pub upstream: bool, pub preview: bool }

/// Pacote só com o checkout limpo, na `main`, sem nada à frente do upstream e sem o `vite`; senão
/// desenvolvedor, com o motivo.
pub fn decide(f: &Facts) -> (Mode, Option<String>) {
    let why = if f.preview { Some("a máquina serve a tela pelo vite (SERVE=preview)".to_owned()) }
        else if f.dirty > 0 { Some(format!("o checkout tem {} arquivo(s) com edição", f.dirty)) }
        else if f.untracked > 0 { Some(format!("o checkout tem {} arquivo(s) novo(s) fora do git", f.untracked)) }
        else if f.branch != "main" { Some(format!("o checkout está na branch {}, fora da main", f.branch)) }
        else if !f.upstream { Some("a main do checkout não tem upstream para comparar".to_owned()) }
        else if f.ahead > 0 { Some(format!("a main tem {} commit(s) à frente do upstream", f.ahead)) }
        else { None };
    (if why.is_some() { Mode::Developer } else { Mode::Package }, why)
}

/// Para e sobe o backend desta máquina (unit do systemd ou tarefa agendada).
pub trait Service: Sync {
    fn stop(&self) -> Result<(), String>;
    fn start(&self) -> Result<(), String>;
}

pub enum Fetched {
    Version { dir: PathBuf, manifest: Manifest, notes: Vec<String> },
    /// O índice não tem pacote para este sistema: a máquina fica no motor `git`.
    NoPlatform(String),
}

/// Baixa e extrai o primeiro pacote; o real é o [`ReleaseFetcher`].
pub trait Fetcher: Sync {
    fn fetch<'a>(&'a self, layout: &'a Layout) -> Pin<Box<dyn Future<Output = Result<Fetched, String>> + Send + 'a>>;
}

/// A máquina como a migração a vê; os caminhos vêm de fora para o teste.
pub struct Machine<'a> {
    pub home: PathBuf,
    pub checkout: PathBuf,
    pub windows: bool,
    /// `~/.config/systemd/user`.
    pub systemd_dir: PathBuf,
    /// `%LOCALAPPDATA%\hangar`, onde moram os `.vbs` das tarefas; `None` fora do Windows.
    pub tasks_dir: Option<PathBuf>,
    pub kimi_home: PathBuf,
    pub codex_homes: Vec<PathBuf>,
    /// O binário copiado para `~/.hangar/bin/hangar-launcher`.
    pub launcher_source: PathBuf,
    /// Este processo morre junto com o backend (cgroup da unit; Job da tarefa que mata ao fechar).
    pub in_service_group: bool,
    pub service: &'a dyn Service,
    pub fetcher: &'a dyn Fetcher,
    pub settings: Settings,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome { Migrated(Mode), AlreadyMigrated(Mode), NotMigrated(String), DryRun(Vec<String>) }

/// Um efeito feito, com o que a volta precisa.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Done {
    /// Pasta criada pela migração; a volta a tira só vazia.
    Dir { path: PathBuf },
    /// `backend/<name>` copiado para `~/.hangar/config/<name>` (`copied` = a cópia é nossa) e, no
    /// Linux, trocado por um link (`linked`).
    Data { name: String, copied: bool, linked: bool },
    Launcher { existed: bool },
    Current,
    /// Arquivo gerado reescrito (unit, `.vbs`, `.cmd`, apontadores); `original` em base64, `None` = não existia.
    File { path: PathBuf, original: Option<String> },
    /// Blocos `[[hooks]]` do Kimi tirados, como texto.
    Kimi { path: PathBuf, removed: Vec<String> },
    /// Grupos do `hooks.json` do Codex tirados, por evento.
    Codex { path: PathBuf, removed: Vec<(String, Value)> },
    /// Link que apontava para `target` dentro do checkout.
    Link { path: PathBuf, target: PathBuf },
}

/// O registro da migração; com `date` vazio enquanto ela corre.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub mode: Mode,
    pub date: String,
    pub checkout: PathBuf,
    pub version: Option<String>,
    pub reason: Option<String>,
    pub notes: Vec<String>,
    pub done: Vec<Done>,
}

impl Machine<'_> {
    fn hangar(&self) -> PathBuf { self.home.join(".hangar") }
    fn layout(&self) -> Layout { Layout::new(&self.home) }
    fn config(&self) -> PathBuf { self.hangar().join("config") }
    fn backend(&self) -> PathBuf { self.checkout.join("backend") }
    fn launcher(&self) -> PathBuf { self.hangar().join("bin").join(if self.windows { "hangar-launcher.exe" } else { "hangar-launcher" }) }
    fn marker(&self) -> PathBuf { self.hangar().join(MARKER) }
    fn partial(&self) -> PathBuf { self.hangar().join(PARTIAL) }
    /// O arquivo que sobe o backend: a unit (Linux) ou o `.vbs` da tarefa (Windows).
    fn service_file(&self) -> Option<PathBuf> {
        let file = if self.windows { self.tasks_dir.as_ref()?.join("hangar-backend.vbs") } else { self.systemd_dir.join("hangar-backend.service") };
        file.is_file().then_some(file)
    }
    /// Uma porta do `.env` em uso: o novo quando existe, senão o do checkout.
    fn env_port(&self, key: &str) -> Result<Option<u16>, String> { crate::user_data::env_port(&self.home, Some(&self.backend()), key) }
    /// `SERVE=preview`: a unit do front existe ou o `.env` aponta o front para outra porta.
    fn preview(&self) -> Result<bool, String> {
        Ok(self.systemd_dir.join("hangar-frontend.service").exists() || self.env_port("CP_FRONT_PORT")?.is_some_and(|p| p > 0))
    }
    fn sep(&self) -> char { if self.windows { '\\' } else { '/' } }
    /// `<raiz>/backend/hooks/`, como os instaladores gravam no comando do hook.
    fn hooks_needle(&self, root: &Path) -> String { format!("{}{}", root.join("backend").join("hooks").display(), self.sep()) }
}

fn facts(m: &Machine<'_>) -> Result<Facts, String> {
    let checkout = super::dev::read(&m.checkout, false).map_err(|e| format!("não consegui ler o checkout {}: {e}", m.checkout.display()))?;
    Ok(Facts { dirty: checkout.dirty, untracked: checkout.untracked,ahead: checkout.ahead, branch: checkout.branch, upstream: checkout.upstream.is_some(), preview: m.preview()? })
}

fn read_record(path: &Path) -> Option<Record> { serde_json::from_slice(&std::fs::read(path).ok()?).ok() }

fn save(path: &Path, record: &Record) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
    crate::runtime::queue::atomic_write(path, &bytes).map_err(|e| format!("não consegui gravar {}: {e}", path.display()))
}

fn now() -> String { chrono::Local::now().to_rfc3339() }

fn b64(bytes: &[u8]) -> String { base64::engine::general_purpose::STANDARD.encode(bytes) }

/// Comparação de caminho em texto: o Windows não diferencia caixa.
fn find(hay: &str, needle: &str, windows: bool) -> Option<usize> {
    if windows { hay.to_lowercase().find(&needle.to_lowercase()) } else { hay.find(needle) }
}

/// Troca toda ocorrência de `from` por `to`; `None` quando não há nenhuma.
pub fn retarget(text: &str, from: &str, to: &str, windows: bool) -> Option<String> {
    find(text, from, windows)?;
    let (mut out, mut rest) = (String::new(), text);
    // Minúsculas de ASCII não mudam o tamanho; caminho com outra letra minúscula de tamanho diferente não casa.
    while let Some(at) = find(rest, from, windows).filter(|at| rest.is_char_boundary(at + from.len())) {
        out.push_str(&rest[..at]);
        out.push_str(to);
        rest = &rest[at + from.len()..];
    }
    out.push_str(rest);
    Some(out)
}

/// A unit chamando o lançador estável; o resto dela fica.
pub fn unit_with_launcher(unit: &str, launcher: &Path, workdir: &Path) -> Result<String, String> {
    if !unit.lines().any(|l| l.starts_with("ExecStart=")) { return Err("a unit hangar-backend.service não tem ExecStart=".into()); }
    let lines: Vec<String> = unit.lines().map(|line| {
        if line.starts_with("ExecStart=") { format!("ExecStart=\"{}\" launch backend", launcher.display()) }
        else if line.starts_with("WorkingDirectory=") { format!("WorkingDirectory={}", workdir.display()) }
        else { line.to_owned() }
    }).collect();
    Ok(lines.join("\n") + if unit.ends_with('\n') { "\n" } else { "" })
}

/// O `.vbs` da tarefa chamando o lançador, pelo caminho já resolvido e entre aspas, com o log de antes.
pub fn vbs_with_launcher(old: &str, launcher: &Path, default_log: &Path) -> String {
    let log = old.split("--log \"\"").nth(1).and_then(|rest| rest.split("\"\"").next()).filter(|l| !l.is_empty())
        .map_or_else(|| default_log.display().to_string(), str::to_owned);
    let line = format!("\"{}\" launch backend --log \"{log}\"", launcher.display());
    format!("WScript.Quit CreateObject(\"WScript.Shell\").Run(\"{}\", 0, True)\r\n", line.replace('"', "\"\""))
}

/// UTF-16LE com BOM, o que o `Escrever-Lancador` do instalador grava para o WSH.
pub fn encode_vbs(text: &str) -> Vec<u8> {
    [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect()
}

pub fn decode_vbs(bytes: &[u8]) -> String {
    match bytes.strip_prefix(&[0xFF_u8, 0xFE][..]) {
        Some(rest) => String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()),
        None => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Tira os blocos `[[hooks]]` cujo comando aponta para `needle`, com o cabeçalho de comentário que o
/// instalador põe antes deles. Devolve o texto novo e os blocos tirados.
pub fn kimi_drop(text: &str, needle: &str, windows: bool) -> (String, Vec<String>) {
    let mut segments: Vec<Vec<&str>> = vec![Vec::new()];
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with('[') { segments.push(Vec::new()); }
        segments.last_mut().expect("um segmento").push(line);
    }
    let ours = |seg: &[&str]| seg.first().is_some_and(|h| h.trim() == "[[hooks]]")
        && seg.iter().any(|l| l.trim_start().starts_with("command") && find(l, needle, windows).is_some());
    let (mut out, mut removed): (Vec<&str>, Vec<String>) = (Vec::new(), Vec::new());
    for seg in &segments {
        if !ours(seg) { out.extend(seg); continue; }
        while out.last().is_some_and(|l| l.trim().is_empty() || l.starts_with("# hangar:") || l.starts_with("# Remova este bloco")) { out.pop(); }
        if !out.is_empty() { out.push("\n"); }
        removed.push(seg.concat());
    }
    (out.concat(), removed)
}

/// Devolve os blocos tirados ao fim do arquivo.
fn kimi_restore(text: &str, removed: &[String]) -> String {
    if removed.is_empty() { return text.to_owned(); }
    let mut out = text.to_owned();
    if !out.is_empty() && !out.ends_with('\n') { out.push('\n'); }
    for block in removed {
        out.push('\n');
        out.push_str(block);
        if !block.ends_with('\n') { out.push('\n'); }
    }
    out
}

/// Tira do `hooks.json` do Codex os hooks cujo comando aponta para `needle`; grupo que fica vazio sai.
pub fn codex_drop(doc: &mut Value, needle: &str, windows: bool) -> Vec<(String, Value)> {
    let mut removed = Vec::new();
    let Some(events) = doc.get_mut("hooks").and_then(Value::as_object_mut) else { return removed };
    for (event, groups) in events.iter_mut() {
        let Some(groups) = groups.as_array_mut() else { continue };
        groups.retain_mut(|group| {
            let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else { return true };
            let (ours, theirs): (Vec<Value>, Vec<Value>) = hooks.drain(..)
                .partition(|h| h.get("command").and_then(Value::as_str).is_some_and(|c| find(c, needle, windows).is_some()));
            *hooks = theirs;
            if !ours.is_empty() {
                let mut taken = group.clone();
                taken["hooks"] = Value::Array(ours);
                removed.push((event.clone(), taken));
            }
            !group["hooks"].as_array().is_some_and(Vec::is_empty)
        });
    }
    removed
}

fn codex_restore(doc: &mut Value, removed: &[(String, Value)]) {
    if !doc.is_object() { *doc = Value::Object(Default::default()); }
    if !doc.get("hooks").is_some_and(Value::is_object) { doc["hooks"] = Value::Object(Default::default()); }
    for (event, group) in removed {
        let list = &mut doc["hooks"][event.as_str()];
        if !list.is_array() { *list = Value::Array(Vec::new()); }
        if let Some(list) = list.as_array_mut() { list.push(group.clone()); }
    }
}

fn write_json(path: &Path, doc: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())? + "\n";
    crate::runtime::queue::atomic_write(path, text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

fn remove_tree(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> { std::os::unix::fs::symlink(target, link) }

#[cfg(windows)]
fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> { Err(std::io::Error::other("sem link de arquivo no Windows")) }

/// Troca `path` por um link para `target` sem instante sem nome: link novo ao lado e `rename`.
fn relink(path: &Path, target: &Path) -> std::io::Result<()> {
    let fresh = path.with_file_name(format!(".{}.pacote-link", path.file_name().and_then(|n| n.to_str()).unwrap_or("x")));
    let _ = std::fs::remove_file(&fresh);
    symlink(target, &fresh)?;
    std::fs::rename(&fresh, path).inspect_err(|_| { let _ = std::fs::remove_file(&fresh); })
}

/// O que impede a migração, conferido antes de parar qualquer coisa.
fn preflight(m: &Machine<'_>, service: &Path) -> Result<(), String> {
    for name in DATA {
        let (old, new) = (m.backend().join(name), m.config().join(name));
        let Ok(meta) = std::fs::symlink_metadata(&old) else { continue };
        if meta.is_symlink() { return Err(format!("{} já é um link; confira à mão antes de migrar", old.display())); }
        if std::fs::symlink_metadata(&new).is_ok() && (meta.is_dir() || std::fs::read(&old).ok() != std::fs::read(&new).ok()) {
            return Err(format!("{} já existe e é diferente de {}; confira à mão antes de migrar", new.display(), old.display()));
        }
    }
    if !m.windows {
        let unit = std::fs::read_to_string(service).map_err(|e| format!("{}: {e}", service.display()))?;
        unit_with_launcher(&unit, &m.launcher(), &m.hangar())?;
    }
    Ok(())
}

/// As frases do plano, para o `--dry-run` e o registro.
fn plan(m: &Machine<'_>, mode: Mode, why: Option<&str>, service: &Path) -> Vec<String> {
    let target = if mode == Mode::Package { "a versão baixada".to_owned() } else { m.checkout.display().to_string() };
    let mut lines = vec![match mode {
        Mode::Package => "modo: pacote; baixar o pacote mais novo do canal latest".to_owned(),
        _ => format!("modo: desenvolvedor ({}); nada é baixado", why.unwrap_or("")),
    }];
    lines.push(format!("parar o backend ({})", service.display()));
    for name in DATA.iter().filter(|n| m.backend().join(n).exists()) {
        lines.push(format!("copiar {} para {}{}", m.backend().join(name).display(), m.config().join(name).display(),
            if m.windows { " (o antigo fica como cópia)" } else { " e deixar um link no lugar antigo" }));
    }
    lines.push(format!("copiar o lançador para {}", m.launcher().display()));
    lines.push(format!("criar {} -> {target}", m.layout().current().display()));
    lines.push(format!("regravar {} para chamar \"{}\" launch backend", service.display(), m.launcher().display()));
    lines.push(format!("tirar do Kimi e do Codex os hooks de {}", m.hooks_needle(&m.checkout)));
    lines.push(if m.windows { "apontar os atalhos .cmd para %USERPROFILE%\\.hangar\\current".to_owned() }
        else { format!("apontar para {} os links de {} que vão ao checkout", m.layout().current().display(), LINK_DIRS.join(", ")) });
    lines.push("regravar ~/.hangar/backend-dir e ~/.hangar/plugin-dir".to_owned());
    lines.push("subir o backend e provar o servidor em modo rust".to_owned());
    lines.push(format!("gravar {}", m.marker().display()));
    lines
}

/// Grava um passo feito no registro parcial.
fn push(m: &Machine<'_>, record: &mut Record, done: Done) -> Result<(), String> {
    record.done.push(done);
    save(&m.partial(), record)
}

fn create_dir(m: &Machine<'_>, record: &mut Record, dir: &Path) -> Result<(), String> {
    if dir.exists() { return Ok(()); }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    if dir == m.config() {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    push(m, record, Done::Dir { path: dir.to_path_buf() })
}

/// Reescreve um arquivo gerado guardando o original.
fn rewrite(m: &Machine<'_>, record: &mut Record, path: &Path, new: &[u8]) -> Result<(), String> {
    let original = std::fs::read(path).ok();
    if original.as_deref() == Some(new) { return Ok(()); }
    // Registrado antes: se a escrita cair no meio, a volta repõe o original.
    push(m, record, Done::File { path: path.to_path_buf(), original: original.as_deref().map(b64) })?;
    crate::runtime::queue::atomic_write(path, new).map_err(|e| format!("{}: {e}", path.display()))
}

/// Os efeitos, na ordem; cada um entra no registro assim que feito.
fn apply(m: &Machine<'_>, record: &mut Record, target: &Path, service: &Path) -> Result<(), String> {
    let current = m.layout().current();
    // Dados do usuário.
    create_dir(m, record, &m.config())?;
    for name in DATA {
        let (old, new) = (m.backend().join(name), m.config().join(name));
        if std::fs::symlink_metadata(&old).is_err() { continue; }
        let copied = std::fs::symlink_metadata(&new).is_err();
        if copied {
            // Cópia com nome temporário e `rename`: uma cópia pela metade nunca tem o nome final.
            let tmp = m.config().join(format!(".{name}.copiando"));
            let copy = remove_tree(&tmp).and_then(|()| copy_tree(&old, &tmp)).and_then(|()| std::fs::rename(&tmp, &new));
            if let Err(e) = copy {
                let _ = remove_tree(&tmp);
                return Err(format!("não consegui copiar {} para {}: {e}", old.display(), new.display()));
            }
        }
        push(m, record, Done::Data { name: name.to_owned(), copied, linked: false })?;
        if !m.windows {
            // Pasta não se troca por `rename` por cima: a original sai do lugar antes.
            let aside = old.with_file_name(format!(".{name}.pre-pacote"));
            if old.is_dir() { std::fs::rename(&old, &aside).map_err(|e| format!("{}: {e}", old.display()))?; }
            if let Err(e) = relink(&old, &new) {
                if aside.exists() { let _ = std::fs::rename(&aside, &old); }
                return Err(format!("não consegui deixar o link em {}: {e}", old.display()));
            }
            if let Some(Done::Data { linked, .. }) = record.done.last_mut() { *linked = true; }
            save(&m.partial(), record)?;
            let _ = remove_tree(&aside);
        }
    }
    // Lançador estável, fora das versões: a unit e as tarefas o chamam.
    let bytes = std::fs::read(&m.launcher_source).map_err(|e| format!("não consegui ler o lançador {}: {e}", m.launcher_source.display()))?;
    let launcher = m.launcher();
    create_dir(m, record, launcher.parent().expect("bin"))?;
    push(m, record, Done::Launcher { existed: launcher.exists() })?;
    let tmp = launcher.with_extension("new");
    std::fs::write(&tmp, &bytes).and_then(|()| {
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?; }
        std::fs::rename(&tmp, &launcher)
    }).map_err(|e| { let _ = std::fs::remove_file(&tmp); format!("não consegui gravar o lançador {}: {e}", launcher.display()) })?;
    // O apontador.
    push(m, record, Done::Current)?;
    super::swap(&m.layout(), target).map_err(|e| e.to_string())?;
    // O serviço chama o lançador.
    let old = std::fs::read(service).map_err(|e| format!("{}: {e}", service.display()))?;
    let new = if m.windows {
        let log = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default().join(r"hangar\logs\privado\hangar-backend.log");
        encode_vbs(&vbs_with_launcher(&decode_vbs(&old), &launcher, &log))
    } else {
        unit_with_launcher(&String::from_utf8_lossy(&old), &launcher, &m.hangar())?.into_bytes()
    };
    rewrite(m, record, service, &new)?;
    // Hooks que apontam para o checkout: o Kimi só acrescenta e o Codex não troca hook de outro checkout.
    let needle = m.hooks_needle(&m.checkout);
    // Arquivo que não se lê fica como está, com nota: o hook velho dele segue apontando para o checkout.
    let unread = |record: &mut Record, path: &Path, e: &dyn std::fmt::Display| {
        record.notes.push(format!("não li {} ({e}); os hooks de lá que apontam para o checkout ficaram", path.display()));
        save(&m.partial(), record)
    };
    let kimi = m.kimi_home.join("config.toml");
    match std::fs::read_to_string(&kimi) {
        Ok(text) => {
            let (new, removed) = kimi_drop(&text, &needle, m.windows);
            if !removed.is_empty() {
                push(m, record, Done::Kimi { path: kimi.clone(), removed })?;
                crate::runtime::queue::atomic_write(&kimi, new.as_bytes()).map_err(|e| format!("{}: {e}", kimi.display()))?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => unread(record, &kimi, &e)?,
    }
    for home in &m.codex_homes {
        let path = home.join("hooks.json");
        let read = match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => Err(e.to_string()),
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string()),
        };
        let mut doc = match read {
            Ok(doc) => doc,
            Err(e) => { unread(record, &path, &e)?; continue; }
        };
        let removed = codex_drop(&mut doc, &needle, m.windows);
        if !removed.is_empty() {
            push(m, record, Done::Codex { path: path.clone(), removed })?;
            write_json(&path, &doc)?;
        }
    }
    // Atalhos e links pelo `current`.
    if m.windows {
        let from = format!("{}\\", m.checkout.join("scripts").display());
        let bin = m.home.join(".local").join("bin");
        for entry in std::fs::read_dir(&bin).into_iter().flatten().flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd")) { continue; }
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) => {
                    record.notes.push(format!("não li o atalho {} ({e}); ele segue apontando para o checkout", path.display()));
                    save(&m.partial(), record)?;
                    continue;
                }
            };
            if let Some(new) = retarget(&text, &from, r"%USERPROFILE%\.hangar\current\scripts\", true) {
                rewrite(m, record, &path, new.as_bytes())?;
            }
        }
    } else {
        for dir in LINK_DIRS.map(|d| m.home.join(d)) {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                let Ok(raw) = std::fs::read_link(&path) else { continue };
                let Ok(rest) = dir.join(&raw).strip_prefix(&m.checkout).map(Path::to_path_buf) else { continue };
                push(m, record, Done::Link { path: path.clone(), target: raw })?;
                relink(&path, &current.join(rest)).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
    }
    // Apontadores que os scripts e o wrapper leem.
    let (from, to) = (format!("{}{}", m.checkout.display(), m.sep()), format!("{}{}", current.display(), m.sep()));
    for name in ["backend-dir", "plugin-dir"] {
        let path = m.hangar().join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) => if let Some(new) = retarget(&text, &from, &to, m.windows) { rewrite(m, record, &path, new.as_bytes())?; },
            Err(_) if name == "backend-dir" => rewrite(m, record, &path, format!("{}\n", current.join("backend").display()).as_bytes())?,
            Err(_) => {}
        }
    }
    Ok(())
}

/// Arquivo apagado depois da migração volta só com o que ela tirou.
fn absent_is_empty(read: std::io::Result<String>) -> std::io::Result<String> {
    match read {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        other => other,
    }
}

/// Desfaz os efeitos do fim para o começo; devolve o que não voltou.
fn revert(m: &Machine<'_>, done: &[Done]) -> Vec<String> {
    let mut failures = Vec::new();
    let mut fail = |what: String| { tracing::warn!("migrate: {what}"); failures.push(what) };
    let current = m.layout().current();
    for item in done.iter().rev() {
        match item {
            Done::Dir { path } => match std::fs::remove_dir(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => fail(format!("{} não saiu ({e}); o que sobrou dentro dela não é da migração", path.display())),
                Ok(()) => {}
            },
            Done::Data { name, copied, linked } => {
                let (old, new) = (m.backend().join(name), m.config().join(name));
                if *linked && std::fs::symlink_metadata(&old).is_ok_and(|meta| meta.is_symlink()) { let _ = std::fs::remove_file(&old); }
                // O dado mais novo é o do lugar novo: ele volta inteiro.
                if std::fs::symlink_metadata(&new).is_ok() {
                    let restored = remove_tree(&old).and_then(|()| copy_tree(&new, &old));
                    if let Err(e) = restored { fail(format!("{} não voltou de {}: {e}", old.display(), new.display())); continue; }
                    if *copied && let Err(e) = remove_tree(&new) { fail(format!("{} não saiu: {e}", new.display())); }
                }
            }
            Done::Launcher { existed } => if !existed && let Err(e) = remove_tree(&m.launcher()) {
                fail(format!("{} não saiu: {e}", m.launcher().display()));
            },
            Done::Current => if let Err(e) = super::remove_link(&current) { fail(format!("{} não saiu: {e}", current.display())); },
            Done::File { path, original } => {
                let result = match original.as_deref().map(|o| base64::engine::general_purpose::STANDARD.decode(o)) {
                    Some(Ok(bytes)) => crate::runtime::queue::atomic_write(path, &bytes).map_err(|e| e.to_string()),
                    Some(Err(e)) => Err(e.to_string()),
                    None => remove_tree(path).map_err(|e| e.to_string()),
                };
                if let Err(e) = result { fail(format!("{} não voltou: {e}", path.display())); }
            }
            // Ilegível fica como está: regravar só com os blocos devolvidos apagaria o resto do arquivo.
            Done::Kimi { path, removed } => {
                let text = match absent_is_empty(std::fs::read_to_string(path)) {
                    Ok(text) => text,
                    Err(e) => { fail(format!("{} não voltou: ilegível ({e}), ficou como estava", path.display())); continue; }
                };
                let (text, _) = kimi_drop(&text, &m.hooks_needle(&current), m.windows);
                if let Err(e) = crate::runtime::queue::atomic_write(path, kimi_restore(&text, removed).as_bytes()) {
                    fail(format!("{} não voltou: {e}", path.display()));
                }
            }
            Done::Codex { path, removed } => {
                let read = match std::fs::read(path) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
                    Err(e) => Err(e.to_string()),
                    Ok(bytes) => serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string()),
                };
                let mut doc = match read {
                    Ok(doc) => doc,
                    Err(e) => { fail(format!("{} não voltou: ilegível ({e}), ficou como estava", path.display())); continue; }
                };
                codex_drop(&mut doc, &m.hooks_needle(&current), m.windows);
                codex_restore(&mut doc, removed);
                if let Err(e) = write_json(path, &doc) { fail(format!("{} não voltou: {e}", path.display())); }
            }
            Done::Link { path, target } => if let Err(e) = relink(path, target) { fail(format!("{} não voltou para {}: {e}", path.display(), target.display())); },
        }
    }
    failures
}

/// Volta a máquina como estava e sobe o backend de antes. `running`: o backend novo está de pé.
fn roll_back(m: &Machine<'_>, record: &Record, running: bool, why: String) -> String {
    // Desfazer com o backend novo de pé trocaria os dados e o `current` debaixo dele.
    if running && let Err(e) = m.service.stop() {
        return format!("{why}; e o backend novo não parou ({e}), então a volta não mexeu em nada: a máquina segue migrada \
            pela metade, com o registro em {}; pare o backend e rode `hangar-server migrate-package --undo`", m.partial().display());
    }
    let failures = revert(m, &record.done);
    let started = m.service.start();
    if failures.is_empty() { let _ = std::fs::remove_file(m.partial()); }
    let mut message = format!("{why}; a migração foi desfeita");
    if !failures.is_empty() {
        message += &format!(", menos: {} (o registro ficou em {})", failures.join("; "), m.partial().display());
    }
    if let Err(e) = started { message += &format!("; e o backend de antes não subiu: {e}"); }
    message
}

/// Parar o backend mataria quem morre junto com ele: recusa antes de qualquer efeito.
fn refuse_inside_service(m: &Machine<'_>) -> Result<(), String> {
    if !m.in_service_group { return Ok(()); }
    Err("a migração está rodando dentro do serviço do backend, e parar o backend a mataria no meio; \
         rode-a fora dele (o Atualizar do motor git já roda num escopo próprio)".into())
}

fn lock(m: &Machine<'_>) -> Result<UpdateLock, String> {
    UpdateLock::try_acquire(&m.layout()).map_err(|e| match e {
        UpdateError::Locked => "uma atualização está rodando nesta máquina; tente de novo depois".to_owned(),
        e => e.to_string(),
    })
}

/// Desfaz a tentativa registrada em `migration.partial.json` e sobe o backend de antes (o do
/// checkout, que nunca saiu do lugar). `Ok(false)`: não havia tentativa.
fn undo_interrupted(m: &Machine<'_>) -> Result<bool, String> {
    if !m.partial().exists() { return Ok(false); }
    let record = read_record(&m.partial()).ok_or_else(|| format!("o registro {} está ilegível; confira à mão", m.partial().display()))?;
    m.service.stop().map_err(|e| format!("o backend não parou para desfazer a migração interrompida: {e}"))?;
    let failures = revert(m, &record.done);
    let started = m.service.start();
    if !failures.is_empty() {
        return Err(format!("a migração interrompida não voltou inteira: {} (o registro ficou em {})", failures.join("; "), m.partial().display()));
    }
    let _ = std::fs::remove_file(m.partial());
    started.map_err(|e| format!("a migração interrompida foi desfeita, mas o backend não subiu: {e}"))?;
    Ok(true)
}

/// Migra até o fim ou até `interrupt` (SIGTERM, SIGINT, Ctrl-Break): interrompida, desfaz o que
/// estiver registrado antes de devolver, inclusive durante a espera da prova.
pub async fn migrate_until<H: HealthProbe>(m: &Machine<'_>, health: &H, dry_run: bool, interrupt: impl Future<Output = ()>) -> Result<Outcome, String> {
    tokio::select! {
        biased;
        result = migrate(m, health, dry_run) => result,
        () = interrupt => Err(match undo_interrupted(m) {
            Ok(true) => "a migração foi interrompida e desfeita; o backend voltou ao checkout".to_owned(),
            Ok(false) => "a migração foi interrompida antes de mudar a máquina".to_owned(),
            Err(e) => format!("a migração foi interrompida e a volta falhou: {e}"),
        }),
    }
}

/// Migra. `dry_run` só devolve o plano, sem mudar nada.
pub async fn migrate<H: HealthProbe>(m: &Machine<'_>, health: &H, dry_run: bool) -> Result<Outcome, String> {
    if let Some(record) = read_record(&m.marker()) { return Ok(Outcome::AlreadyMigrated(record.mode)); }
    // Uma tentativa morta no meio (SIGKILL, queda de energia) é desfeita antes de começar de novo.
    if m.partial().exists() {
        if dry_run {
            return Ok(Outcome::DryRun(vec![format!("desfazer a migração interrompida registrada em {}", m.partial().display()),
                "depois, migrar do começo (o plano dela sai num --dry-run seguinte)".to_owned()]));
        }
        refuse_inside_service(m)?;
        let _lock = lock(m)?;
        undo_interrupted(m)?;
    }
    if std::fs::symlink_metadata(m.layout().current()).is_ok() {
        return Err(format!("{} já existe sem a marca da migração; confira à mão", m.layout().current().display()));
    }
    let not_migrated = |reason: String| -> Result<Outcome, String> {
        if !dry_run {
            save(&m.marker(), &Record { mode: Mode::NotMigrated, date: now(), checkout: m.checkout.clone(), version: None,
                reason: Some(reason.clone()), notes: Vec::new(), done: Vec::new() })?;
        }
        Ok(Outcome::NotMigrated(reason))
    };
    let Some(service) = m.service_file() else {
        return not_migrated("sem a unit nem a tarefa do backend, não há como parar e subir o servidor; a máquina segue no motor git".into());
    };
    let (mode, why) = decide(&facts(m)?);
    preflight(m, &service)?;
    if dry_run { return Ok(Outcome::DryRun(plan(m, mode, why.as_deref(), &service))); }
    refuse_inside_service(m)?;
    let _lock = lock(m)?;
    let (target, manifest, notes) = match mode {
        Mode::Package => match m.fetcher.fetch(&m.layout()).await? {
            Fetched::Version { dir, manifest, notes } => (dir, Some(manifest), notes),
            Fetched::NoPlatform(reason) => return not_migrated(reason),
        },
        _ => (m.checkout.clone(), None, Vec::new()),
    };
    let before = match health.read().await { Reading::Rust { pid, .. } => pid, _ => None };
    let mut record = Record { mode, date: String::new(), checkout: m.checkout.clone(), version: manifest.as_ref().map(|x| x.version.clone()),
        reason: why, notes, done: Vec::new() };
    // O registro nasce antes de parar: morte daqui em diante sempre deixa o que desfazer.
    save(&m.partial(), &record)?;
    if let Err(e) = m.service.stop() {
        let _ = m.service.start();
        let _ = std::fs::remove_file(m.partial());
        return Err(format!("o backend não parou: {e}; nada foi mudado"));
    }
    if let Err(why) = apply(m, &mut record, &target, &service) {
        return Err(roll_back(m, &record, false, why));
    }
    if let Err(e) = m.service.start() {
        return Err(roll_back(m, &record, true, format!("o backend não subiu: {e}")));
    }
    let proved = match &manifest {
        Some(manifest) => super::wait_rust(health, manifest, before, &m.settings).await,
        None => super::wait_new(health, before, &m.settings).await,
    };
    if let Err(why) = proved { return Err(roll_back(m, &record, true, why)); }
    record.date = now();
    save(&m.marker(), &record)?;
    let _ = std::fs::remove_file(m.partial());
    Ok(Outcome::Migrated(mode))
}

/// Reaponta tudo para o checkout e devolve os dados ao lugar antigo; o checkout nunca saiu dali.
pub async fn undo<H: HealthProbe>(m: &Machine<'_>, health: &H) -> Result<(), String> {
    let (path, record) = [m.marker(), m.partial()].into_iter().find_map(|p| Some((p.clone(), read_record(&p)?)))
        .ok_or("esta máquina não tem migração para desfazer")?;
    if record.mode == Mode::NotMigrated {
        return std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()));
    }
    let _lock = lock(m)?;
    let before = match health.read().await { Reading::Rust { pid, .. } => pid, _ => None };
    m.service.stop().map_err(|e| format!("o backend não parou: {e}; nada foi desfeito"))?;
    let failures = revert(m, &record.done);
    if failures.is_empty() {
        let _ = std::fs::remove_file(&path);
    }
    m.service.start().map_err(|e| format!("a migração foi desfeita, mas o backend não subiu: {e}"))?;
    if !failures.is_empty() { return Err(format!("não voltaram: {}", failures.join("; "))); }
    super::wait_new(health, before, &m.settings).await
}

/// A unit do systemd ou as tarefas agendadas, pelo shell do sistema.
pub struct SystemService {
    pub runner: super::ShellRunner,
    pub windows: bool,
    pub checkout: PathBuf,
    pub port: u16,
}

impl SystemService {
    fn run(&self, command: &str) -> Result<(), String> {
        let run = super::StepRunner::run(&self.runner, command, &self.checkout);
        match run.code {
            Some(0) => Ok(()),
            code => Err(format!("{command} saiu com {code:?}: {}", run.output.trim())),
        }
    }
}

/// Aspa simples do PowerShell, inclusive as tipográficas que ele também aceita.
fn ps_quote(path: &Path) -> String {
    path.display().to_string().chars().flat_map(|c| if matches!(c, '\'' | '‘' | '’' | '‚' | '‛') { vec![c, c] } else { vec![c] }).collect()
}

impl Service for SystemService {
    fn stop(&self) -> Result<(), String> {
        if !self.windows { return self.run("systemctl --user stop hangar-backend.service"); }
        let helper = self.checkout.join("scripts").join("windows-tasks.ps1");
        self.run(&format!("powershell -NoProfile -ExecutionPolicy Bypass -Command \". '{}'; Restart-HangarTask 'hangar-backend' {} '{}' -NoStart\"",
            ps_quote(&helper), self.port, ps_quote(&self.checkout.join("backend"))))
    }

    fn start(&self) -> Result<(), String> {
        if self.windows { return self.run("powershell -NoProfile -Command \"Start-ScheduledTask -TaskName hangar-backend\""); }
        self.run("systemctl --user daemon-reload && systemctl --user start hangar-backend.service")
    }
}

/// O primeiro pacote, da release, e a tela web da chave dele quando ela está ligada.
pub struct ReleaseFetcher {
    pub client: reqwest::Client,
    pub release: Release,
    pub platform: String,
    pub home: PathBuf,
    pub deadline: Duration,
}

impl Fetcher for ReleaseFetcher {
    fn fetch<'a>(&'a self, layout: &'a Layout) -> Pin<Box<dyn Future<Output = Result<Fetched, String>> + Send + 'a>> {
        Box::pin(async move {
            let entry = match super::check(&self.client, &self.release, &self.platform, None).await {
                Ok(Some(entry)) => entry,
                Ok(None) => return Err("o índice não trouxe pacote".into()),
                Err(UpdateError::NoPlatform(p)) => return Ok(Fetched::NoPlatform(format!("o índice não tem pacote para {p}; a máquina segue no motor git"))),
                Err(e) => return Err(e.to_string()),
            };
            let dir = super::prepare(&self.client, &self.release, &entry, layout, self.deadline).await.map_err(|e| e.to_string())?;
            let manifest = Manifest::parse(&std::fs::read(dir.join(hangar_package::MANIFEST_FILE)).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let mut notes = Vec::new();
            // Escolha ilegível: a tela fica ligada (o padrão), e a leitura que falhou fica na nota.
            let web_ui = crate::ui::enabled(&self.home).unwrap_or_else(|e| {
                tracing::warn!(code = "update_web_ui_unreadable", "migrate: escolha da tela web ilegível, segue ligada: {e}");
                notes.push(format!("a escolha da tela web (~/.hangar/web-ui.json) está ilegível ({e}); a tela segue ligada"));
                true
            });
            if let Some(key) = manifest.dist_key.as_deref() && web_ui
                && let Err(e) = crate::ui::fetch(key, &layout.hangar.join("ui")).await {
                notes.push(format!("a tela web não foi baixada: {e}"));
            }
            Ok(Fetched::Version { dir, manifest, notes })
        })
    }
}

fn label(mode: Mode) -> &'static str {
    match mode { Mode::Package => "pacote", Mode::Developer => "modo desenvolvedor", Mode::NotMigrated => "motor git, sem migrar" }
}

/// Linux: este processo está no cgroup da unit do backend (o `systemctl stop` o mataria).
#[cfg(target_os = "linux")]
pub(crate) fn in_service_group() -> bool {
    std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|c| c.contains("hangar-backend.service"))
}

/// Windows: está num Job que mata tudo ao fechar (o do lançador da tarefa); parar a tarefa o levaria.
/// Job ilegível não recusa: a migração da máquina sem lançador (motor `git`) não pode travar nisso.
#[cfg(windows)]
pub(crate) fn in_service_group() -> bool {
    use windows_sys::Win32::System::JobObjects::{
        IsProcessInJob, JobObjectExtendedLimitInformation, QueryInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut inside = 0;
        if IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut inside) == 0 || inside == 0 { return false; }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        // Handle nulo: o Job do próprio processo.
        let read = QueryInformationJobObject(std::ptr::null_mut(), JobObjectExtendedLimitInformation,
            &mut info as *mut _ as *mut std::ffi::c_void, std::mem::size_of_val(&info) as u32, std::ptr::null_mut());
        read != 0 && info.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn in_service_group() -> bool { false }

/// SIGTERM ou SIGINT (Unix), Ctrl-Break ou Ctrl-C (Windows). Registrado já na chamada, antes de a
/// migração começar; sem registro, nunca dispara.
fn interrupted() -> Pin<Box<dyn Future<Output = ()> + Send>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) {
            (Ok(mut term), Ok(mut int)) => Box::pin(async move { tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} } }),
            _ => Box::pin(std::future::pending()),
        }
    }
    #[cfg(windows)]
    {
        match (tokio::signal::windows::ctrl_break(), tokio::signal::windows::ctrl_c()) {
            (Ok(mut brk), Ok(mut c)) => Box::pin(async move { tokio::select! { _ = brk.recv() => {}, _ = c.recv() => {} } }),
            _ => Box::pin(std::future::pending()),
        }
    }
}

pub(crate) fn is_checkout(dir: &Path) -> bool { dir.join(".git").exists() && dir.join("backend").join("app").is_dir() }

/// `hangar-server migrate-package …`: `args` sem o nome do programa nem o subcomando.
pub fn cli(args: &[String]) -> ExitCode {
    let usage = "uso: hangar-server migrate-package [--dry-run] [--undo] [--checkout <pasta>]";
    let (mut dry_run, mut undo_it, mut checkout) = (false, false, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--undo" => undo_it = true,
            "--checkout" => match it.next() { Some(dir) => checkout = Some(PathBuf::from(dir)), None => { eprintln!("{usage}"); return ExitCode::from(2); } },
            _ => { eprintln!("{usage}"); return ExitCode::from(2); }
        }
    }
    let env = crate::accounts::environment::AccountEnvironment::capture();
    let home = env.home.clone();
    let checkout = checkout.or_else(|| std::env::current_dir().ok().filter(|d| is_checkout(d)))
        .or_else(|| {
            let read = read_record(&home.join(".hangar").join(MARKER)).or_else(|| read_record(&home.join(".hangar").join(PARTIAL)));
            read.map(|r| r.checkout)
        })
        .or_else(|| Some(PathBuf::from(std::fs::read_to_string(home.join(".hangar").join("backend-dir")).ok()?.trim()).parent()?.to_path_buf()))
        .filter(|d| is_checkout(d));
    let Some(checkout) = checkout else {
        eprintln!("##HANGAR-FALHA## não achei o checkout do Hangar; rode de dentro dele ou passe --checkout");
        return ExitCode::from(2);
    };
    let windows = cfg!(windows);
    let var = |k: &str| env.base.get(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut codex_homes: Vec<PathBuf> = var("CODEX_HOME").into_iter().collect();
    codex_homes.extend(std::fs::read_dir(&home).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n == ".codex" || n.starts_with(".codex-")) && p.join("hooks.json").is_file()));
    codex_homes.dedup();
    let launcher_source = std::env::current_exe().and_then(|exe| super::executor::executor_path(&exe)).unwrap_or_default();
    let runner = super::ShellRunner { timeout: Duration::from_secs(180) };
    let probe = Machine {
        home: home.clone(), checkout: checkout.clone(), windows,
        systemd_dir: var("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("systemd").join("user"),
        tasks_dir: windows.then(|| var("LOCALAPPDATA").map(|d| d.join("hangar"))).flatten(),
        kimi_home: var("KIMI_CODE_HOME").unwrap_or_else(|| home.join(".kimi-code")),
        codex_homes, launcher_source, in_service_group: in_service_group(),
        service: &Unused, fetcher: &Unused,
        // A prova cabe no prazo do passo do motor `git` (600 s), com o download antes dela.
        settings: Settings { download_deadline: Duration::from_secs(300), ..Settings::default() },
    };
    let port = match probe.env_port("CP_PORT") {
        Ok(port) => port.unwrap_or(8765),
        Err(e) => { println!("##HANGAR-FALHA## {e}"); return ExitCode::from(1); }
    };
    let service = SystemService { runner, windows, checkout, port };
    let client = match crate::http_client().build() {
        Ok(client) => client,
        Err(e) => { eprintln!("##HANGAR-FALHA## cliente http: {e}"); return ExitCode::from(1); }
    };
    let fetcher = ReleaseFetcher { client, release: Release { base_url: super::RELEASE_BASE.to_owned(), channel: "latest".into() },
        platform: super::platform(), home: home.clone(), deadline: probe.settings.download_deadline };
    let machine = Machine { service: &service, fetcher: &fetcher, ..probe };
    let health = super::HttpHealth::new(&format!("http://127.0.0.1:{port}"));
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => { eprintln!("##HANGAR-FALHA## runtime: {e}"); return ExitCode::from(1); }
    };
    let result = if undo_it {
        runtime.block_on(undo(&machine, &health)).map(|()| vec!["migração desfeita: tudo aponta de novo para o checkout".to_owned()])
    } else {
        runtime.block_on(async { migrate_until(&machine, &health, dry_run, interrupted()).await }).map(|outcome| match outcome {
            Outcome::DryRun(lines) => lines,
            Outcome::Migrated(mode) => std::iter::once(format!("migrada para o {}; marca em {}", label(mode), machine.marker().display()))
                .chain(read_record(&machine.marker()).map(|r| r.notes).unwrap_or_default().into_iter().map(|note| format!("aviso: {note}")))
                .collect(),
            Outcome::AlreadyMigrated(mode) => vec![format!("já migrada ({}); nada a fazer", label(mode))],
            Outcome::NotMigrated(reason) => vec![format!("não migrada: {reason}")],
        })
    };
    match result {
        Ok(lines) => { for line in lines { println!("{line}"); } ExitCode::SUCCESS }
        // O motor `git` mostra a linha marcada como o motivo do passo.
        Err(e) => { println!("##HANGAR-FALHA## {e}"); ExitCode::from(1) }
    }
}

/// Lugar de serviço e download antes de o `cli` montar os reais (só lê caminhos).
struct Unused;

impl Service for Unused {
    fn stop(&self) -> Result<(), String> { Err("sem serviço".into()) }
    fn start(&self) -> Result<(), String> { Err("sem serviço".into()) }
}

impl Fetcher for Unused {
    fn fetch<'a>(&'a self, _layout: &'a Layout) -> Pin<Box<dyn Future<Output = Result<Fetched, String>> + Send + 'a>> {
        Box::pin(async { Err("sem download".into()) })
    }
}

// A caixa de teste monta a máquina Linux (unit do systemd, links em ~/.local/bin); o Windows se prova na VM.
#[cfg(all(test, unix))]
mod tests;

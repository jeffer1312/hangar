//! A conta Claude do Hangar (`~/.claude-<nome>`) alinhada ao `~/.claude`: atalho de cada arquivo
//! compartilhado, cópia espelhada do `settings.json`, espelho dos `mcpServers` do `~/.claude.json`,
//! atalho da memória de cada projeto, conferência dos plugins e a gaveta `.drift` para o que diverge.
//! Relê o disco a cada chamada; as travas são por chamada. O comando `hangar-server conta` é a borda
//! que os wrappers de shell (`hangar-conta`, `claude-conta`) chamam.
use super::environment::AccountEnvironment;
use super::storage::{self, CLAUDE_MARKER, PENDING};
use super::{AccountKey, AccountLocks, AccountService, GuardMode, LockError, Provider};
use crate::transcript::{py, pyjson};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Versões que a gaveta guarda por nome de origem; sem teto ela cresce para sempre.
const DRIFT_CAP: usize = 3;
/// Runtime que o CLI e o servidor regravam por conta com tmp+rename (e a trava que o CLI cria ao
/// gravar o `.claude.json`): ligado, cada passada acharia deriva de novo, mandaria para a gaveta ou
/// subiria a cópia da conta para o `~/.claude`.
const ACCOUNT_RUNTIME: [&str; 8] = ["sessions", "telemetry", "feedback", "image-cache", ".last-update-result.json", ".hangar-models.json",
    ".claude.json.lock", "mcp-needs-auth-cache.json"];
/// Nunca viram atalho: nossos (`.drift`, marcador), da conta (login, `projects/` pelo custo por
/// pasta), cópia (`settings.json`, que o CLI regravava atravessando o link) e apelidos (cópia velha
/// subia por cima dos novos).
const NEVER_LINK: [&str; 7] = [CLAUDE_MARKER, ".drift", ".claude.json", ".credentials.json", "projects", "settings.json", ".hangar-apelidos.json"];
/// Quanto cada trava de configuração espera outro processo.
pub const LOCK_WAIT: Duration = Duration::from_secs(120);
const NAME_REFUSED: &str = "nome: use minúsculas, números, '-' ou '_' (até 32 caracteres)";
const SYMLINK_HINT: &str = "No Windows isso exige o Modo Desenvolvedor ligado (Configurações → Sistema → Para \
    desenvolvedores). Sem ele a conta ficaria com uma CÓPIA, que passa a divergir do original sem ninguém perceber.";

#[derive(Debug)]
pub enum ConfigError {
    /// Recusa com a frase que o comando imprime e a abertura repassa.
    Refused(String),
    /// Falha do sistema de arquivos fora dos casos tratados, com o caminho envolvido (`a -> b` quando são
    /// dois): o comando sai com 126.
    Io(io::Error, String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(detail) => f.write_str(detail),
            Self::Io(error, path) => write!(f, "{path}: {error}"),
        }
    }
}

impl ConfigError {
    /// A frase que o comando imprime depois de `hangar-conta: ` e que a abertura repassa.
    pub fn cause(&self) -> String {
        match self {
            Self::Refused(detail) => detail.clone(),
            Self::Io(..) => format!("falha ao executar: {self}"),
        }
    }
}

type Result<T> = std::result::Result<T, ConfigError>;

/// O caminho junto do erro do sistema, como a mensagem do Python trazia.
trait At<T> {
    fn at(self, path: &Path) -> Result<T>;
    fn between(self, from: &Path, to: &Path) -> Result<T>;
}

impl<T> At<T> for io::Result<T> {
    fn at(self, path: &Path) -> Result<T> { self.map_err(|e| ConfigError::Io(e, path.display().to_string())) }
    fn between(self, from: &Path, to: &Path) -> Result<T> {
        self.map_err(|e| ConfigError::Io(e, format!("{} -> {}", from.display(), to.display())))
    }
}

pub(crate) fn refused(detail: impl Into<String>) -> ConfigError { ConfigError::Refused(detail.into()) }

/// O resultado e os eventos `conta.reconciliar.*` do diário, que quem chama grava no arquivo dele.
pub struct Outcome {
    pub result: Result<Vec<String>>,
    pub diary: Vec<Value>,
}

impl Outcome {
    pub(crate) fn failed(error: ConfigError) -> Self { Self { result: Err(error), diary: Vec::new() } }
}

// ── o sistema de arquivos como o Python o vê ──────────────────────────────────────────────────

fn is_symlink(path: &Path) -> bool { fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) }
fn exists(path: &Path) -> bool { fs::metadata(path).is_ok() }
fn is_dir(path: &Path) -> bool { fs::metadata(path).is_ok_and(|m| m.is_dir()) }
fn is_file(path: &Path) -> bool { fs::metadata(path).is_ok_and(|m| m.is_file()) }

fn unlink(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink_dir()) { return fs::remove_dir(path); }
    }
    fs::remove_file(path)
}

/// `mkdir(exist_ok=True)` sem criar a pasta-mãe.
fn mkdir(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && is_dir(path) => Ok(()),
        other => other,
    }
}

fn name_of(path: &Path) -> String { path.file_name().unwrap_or_default().to_string_lossy().into_owned() }

/// Irmão com pid e aleatório no nome: nome fixo apagaria um arquivo legítimo do usuário.
fn sibling(path: &Path) -> PathBuf {
    let mut name: OsString = path.file_name().unwrap_or_default().to_owned();
    name.push(format!(".hangar-novo.{}.{}", std::process::id(), crate::mods::state::random_hex(4)));
    path.with_file_name(name)
}

fn make_symlink(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    { std::os::unix::fs::symlink(target, link) }
    #[cfg(windows)]
    {
        if is_dir(target) { std::os::windows::fs::symlink_dir(target, link) } else { std::os::windows::fs::symlink_file(target, link) }
    }
}

/// `shutil.copyfile`: os bytes, sem o modo de origem; destino que é link é seguido.
fn copy_file(from: &Path, to: &Path) -> Result<()> {
    let mut source = fs::File::open(from).at(from)?;
    let mut target = fs::OpenOptions::new().write(true).create(true).truncate(true).open(to).at(to)?;
    io::copy(&mut source, &mut target).map(drop).between(from, to)
}

fn same_bytes(a: &Path, b: &Path) -> Result<bool> {
    if fs::metadata(a).at(a)?.len() != fs::metadata(b).at(b)?.len() { return Ok(false); }
    Ok(fs::read(a).at(a)? == fs::read(b).at(b)?)
}

/// `os.path.realpath(strict=False)`: link morto resolve até onde dá e laço devolve o próprio caminho
/// (o `canonicalize` falharia nos dois).
#[cfg(unix)]
fn realpath(path: &Path) -> PathBuf {
    use std::collections::HashMap;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    enum Part { Name(Vec<u8>), Resolved(Vec<u8>) }
    let parts = |text: &[u8]| text.split(|b| *b == b'/').rev().map(|p| Part::Name(p.to_vec())).collect::<Vec<_>>();
    let text = path.as_os_str().as_bytes();
    let mut rest = parts(text);
    let mut count = rest.len();
    let mut current = if text.starts_with(b"/") {
        b"/".to_vec()
    } else {
        std::env::current_dir().map(|d| d.into_os_string().into_vec()).unwrap_or_else(|_| b"/".to_vec())
    };
    let mut seen: HashMap<Vec<u8>, Option<Vec<u8>>> = HashMap::new();
    while count > 0 {
        let Some(part) = rest.pop() else { break };
        let name = match part {
            Part::Resolved(link) => { seen.insert(link, Some(current.clone())); continue; }
            Part::Name(name) => name,
        };
        count -= 1;
        if name.is_empty() || name == b"." { continue; }
        if name == b".." {
            let cut = current.iter().rposition(|b| *b == b'/').unwrap_or(0);
            current.truncate(cut);
            if current.is_empty() { current = b"/".to_vec(); }
            continue;
        }
        let next = if current == b"/" { [&b"/"[..], &name].concat() } else { [&current[..], b"/", &name].concat() };
        let link = std::ffi::OsStr::from_bytes(&next);
        match fs::symlink_metadata(link) {
            Ok(meta) if meta.file_type().is_symlink() => {
                if let Some(known) = seen.get(&next) {
                    current = known.clone().unwrap_or(next);
                    continue;
                }
                match fs::read_link(link) {
                    Ok(target) => {
                        let target = target.into_os_string().into_vec();
                        if target.starts_with(b"/") { current = b"/".to_vec(); }
                        seen.insert(next.clone(), None);
                        rest.push(Part::Resolved(next));
                        let more = parts(&target);
                        count += more.len();
                        rest.extend(more);
                    }
                    Err(_) => current = next,
                }
            }
            _ => current = next,
        }
    }
    PathBuf::from(OsString::from_vec(current))
}

/// No Windows: o que existe resolve pelo `canonicalize` (sem o prefixo `\\?\`), link morto segue o
/// texto dele, e o resto do caminho é anexado.
#[cfg(windows)]
fn realpath(path: &Path) -> PathBuf {
    let (mut current, mut tail, mut hops) = (path.to_owned(), Vec::new(), 0);
    loop {
        if let Ok(real) = fs::canonicalize(&current) {
            let mut real = super::catalog::resolved(&real);
            for name in tail.iter().rev() { real.push(name); }
            return real;
        }
        if hops < 40 && let Ok(target) = fs::read_link(&current) {
            hops += 1;
            current = if target.is_absolute() { target } else { current.parent().map_or(target.clone(), |p| p.join(&target)) };
            continue;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_owned());
                current = parent.to_owned();
            }
            _ => return path.to_owned(),
        }
    }
}

/// `link` é atalho que já aponta para o alvo? O comum é o texto dele ser o alvo e resolver; senão, pelo
/// caminho resolvido dos dois.
fn points_to(link: &Path, target: &Path) -> bool {
    let Ok(text) = fs::read_link(link) else { return false };
    (plain(&text) == target && exists(link)) || realpath(link) == realpath(target)
}

/// O texto do link sem o prefixo `\\?\` que o Windows devolve.
fn plain(text: &Path) -> &Path {
    #[cfg(windows)]
    if let Some(rest) = text.to_str().and_then(|t| t.strip_prefix(r"\\?\")) { return Path::new(rest); }
    text
}

/// Ordem do `sorted(Path)`: ponto de código no POSIX, caminho em minúsculas no Windows.
fn sort_names(names: &mut [OsString]) {
    if cfg!(windows) { names.sort_by_cached_key(|n| n.to_string_lossy().to_lowercase()) } else { names.sort() }
}

/// Ordem de ponto de código do Python, com o surrogate solto no lugar dele.
fn py_order(a: &str, b: &str) -> std::cmp::Ordering { a.chars().map(py::py_code).cmp(b.chars().map(py::py_code)) }

// ── JSON como o Python lê, compara e grava ────────────────────────────────────────────────────

/// `json.loads(path.read_text("utf-8"))`: ausente é `None`; surrogate solto é aceito.
fn read_json(path: &Path) -> std::result::Result<Option<Value>, String> {
    // A leitura que atravessa a troca tmp+rename do Windows (o CLI vivo regrava estes arquivos).
    let bytes = match crate::runtime::queue::read_replaced(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).or_else(|error| pyjson::loads_lossless(&text).ok_or(error)).map(Some).map_err(|e| e.to_string())
}

/// `==` do Python: `1 == 1.0 == True`, dict sem ordem. Troca só de tipo não é mudança.
fn py_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q)),
        (Value::Object(x), Value::Object(y)) => py_eq_map(x, y),
        _ => match ((py::int_of(a), py::number(a)), (py::int_of(b), py::number(b))) {
            ((Some(x), _), (Some(y), _)) => x == y,
            ((None, Some(x)), (None, Some(y))) => x == y,
            ((Some(i), _), (None, Some(f))) | ((None, Some(f)), (Some(i), _)) => f.fract() == 0.0 && f.abs() < 1e30 && f as i128 == i,
            _ => false,
        },
    }
}

fn py_eq_map(a: &Map<String, Value>, b: &Map<String, Value>) -> bool {
    a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| py_eq(v, w)))
}

/// Grava por temporário e troca: um CLI vivo lendo o arquivo no meio receberia JSON truncado.
fn write_replacing(path: &Path, text: &str) -> Result<()> {
    let temp = sibling(path);
    fs::write(&temp, text).at(&temp)?;
    crate::runtime::queue::replace(&temp, path).between(&temp, path)
}

// ── travas ────────────────────────────────────────────────────────────────────────────────────

/// Trava de arquivo presa até o fim da chamada.
struct Held(fs::File);

impl Drop for Held {
    fn drop(&mut self) { let _ = self.0.unlock(); }
}

/// Quanto as travas esperam: cada uma até `Each` (o comando e a criação), ou as duas dentro do prazo da
/// abertura (`Within`), que, passado, não deixa a reconciliação começar: a abertura já recusou.
#[derive(Clone, Copy, Debug)]
pub enum Wait { Each(Duration), Within { limit: Duration, deadline: Instant } }

impl Wait {
    pub fn within(limit: Duration) -> Self { Wait::Within { limit, deadline: Instant::now() + limit } }
}

/// A recusa no limite com o fim dela no diário: sem ele, a reconciliação que não terminou não deixava rastro.
pub fn late_outcome(limit: Duration) -> Outcome {
    let mut end = event("conta.reconciliar.fim", "erro", &crate::mods::state::random_hex(8));
    end["codigo"] = json!("limite");
    end["ms"] = json!(limit.as_millis() as u64);
    let detail = format!("a reconciliação da conta passou do limite de {} s", limit.as_secs_f64());
    Outcome { result: Err(refused(detail)), diary: vec![end] }
}

/// Exclusiva, tentando até `deadline`; `None` = o prazo passou com ela presa por outro processo. Esperar
/// sem fim prenderia quem chama com a trava largada por um processo travado.
fn lock(path: &Path, create: bool, deadline: Instant) -> Result<Option<Held>> {
    let mut options = fs::OpenOptions::new();
    if create { options.read(true).append(true).create(true) } else { options.read(true).write(true) };
    let file = options.open(path).at(path)?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(Held(file))),
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Err(fs::TryLockError::WouldBlock) => return Ok(None),
            Err(fs::TryLockError::Error(error)) => return Err(ConfigError::Io(error, path.display().to_string())),
        }
    }
}

// ── a reconciliação ───────────────────────────────────────────────────────────────────────────

/// `~/.claude-<nome>`, recusando o nome fora da regra (barra, `..` e quebra de linha incluídas).
pub fn account_dir(home: &Path, name: &str) -> Result<PathBuf> {
    if !super::catalog::valid_name(name) { return Err(refused(NAME_REFUSED)); }
    Ok(home.join(format!(".claude-{name}")))
}

/// Pasta de conta de verdade: diretório real com marcador real e sem pendência (nem link morto).
pub fn is_account(dir: &Path) -> bool {
    storage::real_dir(dir) && !storage::pending(dir) && storage::real_file(&dir.join(CLAUDE_MARKER))
}

/// `contas.reconciliar` sob a guarda que quem chama já segura: confere a conta e pega as travas.
pub fn reconcile_account(home: &Path, name: &str, project: Option<&str>, wait: Wait) -> Outcome {
    let dir = match account_dir(home, name) {
        Ok(dir) => dir,
        Err(error) => return Outcome::failed(error),
    };
    if !is_account(&dir) { return Outcome::failed(refused(format!("{} não é uma conta criada pelo hangar", dir.display()))); }
    reconcile_locked(home, &dir, project, CLAUDE_MARKER, wait)
}

/// Trava compartilhada (duas contas devolvendo o mesmo arquivo ao `~/.claude`) e depois a da conta
/// (no seed, a da pendência), sempre nessa ordem; soltas ao sair, também em erro.
pub fn reconcile_locked(home: &Path, dir: &Path, project: Option<&str>, lock_name: &str, wait: Wait) -> Outcome {
    let take = |path: &Path, create: bool| -> std::result::Result<Held, Outcome> {
        let deadline = match wait { Wait::Each(each) => Instant::now() + each, Wait::Within { deadline, .. } => deadline };
        match lock(path, create, deadline) {
            Ok(Some(held)) => Ok(held),
            Ok(None) => Err(match wait {
                Wait::Each(each) => Outcome::failed(refused(format!("a trava {} ficou presa por outro processo por mais de {} s",
                    path.display(), each.as_secs()))),
                Wait::Within { limit, .. } => late_outcome(limit),
            }),
            Err(error) => Err(Outcome::failed(error)),
        }
    };
    let _shared = match take(&home.join(".hangar-contas.lock"), true) { Ok(held) => held, Err(outcome) => return outcome };
    let _own = match take(&dir.join(lock_name), false) { Ok(held) => held, Err(outcome) => return outcome };
    if let Wait::Within { limit, deadline } = wait && Instant::now() >= deadline { return late_outcome(limit); }
    reconcile(home, dir, project)
}

/// O corpo, sem travas: quem chama já as segura.
pub fn reconcile(home: &Path, dir: &Path, project: Option<&str>) -> Outcome {
    let op = crate::mods::state::random_hex(8);
    let started = Instant::now();
    let mut diary = vec![event("conta.reconciliar.inicio", "ok", &op)];
    let result = steps(home, dir, project, &op, &mut diary);
    let mut end = event("conta.reconciliar.fim", if result.is_ok() { "ok" } else { "erro" }, &op);
    end["codigo"] = json!(if result.is_ok() { "retornou" } else { "excecao" });
    end["ms"] = json!(started.elapsed().as_millis() as u64);
    if let Err(error) = &result {
        end["erro_tipo"] = json!(match error { ConfigError::Refused(_) => "ContaError", ConfigError::Io(..) => "OSError" });
        if let ConfigError::Io(error, _) = error && let Some(code) = error.raw_os_error() { end["errno"] = json!(code); }
    }
    diary.push(end);
    Outcome { result, diary }
}

fn event(evento: &str, nivel: &str, op: &str) -> Value {
    json!({"evento": evento, "nivel": nivel, "provider": "claude", "operacao": op})
}

fn conta_id(dir: &Path) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, dir.to_string_lossy().as_bytes());
    digest.as_ref().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn steps(home: &Path, dir: &Path, project: Option<&str>, op: &str, diary: &mut Vec<Value>) -> Result<Vec<String>> {
    // O projeto entra em caminho: absoluto, `..` ou barra escapariam de `~/.claude/projects`.
    if let Some(project) = project
        && (project.is_empty() || !project.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')) {
        return Err(refused("projeto inválido"));
    }
    let mut stage = |etapa: &str, id: bool| {
        let mut e = event("conta.reconciliar.etapa", "ok", op);
        e["etapa"] = json!(etapa);
        if id { e["conta_id"] = json!(conta_id(dir)); }
        diary.push(e);
    };
    let shared = home.join(".claude");
    let mut warnings = Vec::new();
    stage("ligar_arquivos", true);
    let mut names = fs::read_dir(&shared).at(&shared)?.map(|entry| entry.map(|e| e.file_name())).collect::<io::Result<Vec<_>>>().at(&shared)?;
    sort_names(&mut names);
    for name in names {
        let (target, destination) = (shared.join(&name), dir.join(&name));
        let text = name.to_str().unwrap_or_default();
        if ACCOUNT_RUNTIME.contains(&text) {
            // Atalho de runtime de versão antiga: senão a conta segue gravando o runtime dela no compartilhado.
            if points_to(&destination, &target) { unlink(&destination).at(&destination)?; }
            continue;
        }
        if NEVER_LINK.contains(&text) { continue; }
        if points_to(&destination, &target) { continue; }
        if is_symlink(&destination) || exists(&destination) {
            warnings.extend(collide(dir, &destination, &target)?);
        }
        link(&destination, &target)?;
    }
    stage("espelhar_settings", false);
    warnings.extend(seed_settings(home, dir)?);
    stage("conferir_plugins", false);
    warnings.extend(check_plugins(home, dir)?);
    stage("espelhar_config", false);
    warnings.extend(mirror_claude_json(home, dir)?);
    // Atalho para o que sumiu do compartilhado, e qualquer outro link morto no topo.
    for entry in fs::read_dir(dir).at(dir)? {
        let entry = entry.at(dir)?;
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_symlink()) && !exists(&path) { unlink(&path).at(&path)?; }
    }
    stage("ligar_memoria", false);
    warnings.extend(link_memory(home, dir, project)?);
    let mut done = event("conta.reconciliar.concluiu", if warnings.is_empty() { "ok" } else { "aviso" }, op);
    done["quantidade"] = json!(warnings.len());
    done["codigo"] = json!(if warnings.is_empty() { "ok" } else { "com_avisos" });
    diary.push(done);
    Ok(warnings)
}

/// Troca atômica pelo atalho: `unlink` + `symlink` deixava um instante sem o caminho para o CLI vivo.
fn link(destination: &Path, target: &Path) -> Result<()> {
    let temp = sibling(destination);
    if is_symlink(&temp) || exists(&temp) {
        return Err(refused(format!("{} já existe — reconciliação anterior deixou lixo?", name_of(&temp))));
    }
    // Nunca cai para cópia: ela passaria a divergir do original calada.
    make_symlink(target, &temp).map_err(|e| refused(format!("não consegui criar o atalho {}: {} -> {}: {e}. {SYMLINK_HINT}",
        name_of(destination), target.display(), temp.display())))?;
    crate::runtime::queue::replace(&temp, destination).between(&temp, destination)
}

/// O caminho existe e não é o atalho esperado. Tipo incompatível vai para a gaveta; arquivo contra
/// arquivo funde (a mudança sobe); link inesperado nunca é seguido (vazaria o alvo externo).
fn collide(dir: &Path, destination: &Path, target: &Path) -> Result<Option<String>> {
    if is_symlink(destination) || is_dir(destination) || !is_file(destination) || !is_file(target) {
        return drawer(dir, destination).map(Some);
    }
    if !same_bytes(destination, target)? {
        copy_file(destination, target)?;
        unlink(destination).at(destination)?;
        return Ok(Some(format!("'{}' foi alterado dentro desta conta; a mudança subiu pro ~/.claude", name_of(destination))));
    }
    unlink(destination).at(destination)?;
    Ok(None)
}

/// `str.isdigit` do Python 3.14 (Unicode 16): decimais e os dígitos sobrescritos, circulados etc.
fn py_isdigit(c: char) -> bool {
    const DIGITS: [(u32, u32); 90] = [(0x30, 0x39), (0xB2, 0xB3), (0xB9, 0xB9), (0x660, 0x669), (0x6F0, 0x6F9), (0x7C0, 0x7C9),
        (0x966, 0x96F), (0x9E6, 0x9EF), (0xA66, 0xA6F), (0xAE6, 0xAEF), (0xB66, 0xB6F), (0xBE6, 0xBEF), (0xC66, 0xC6F),
        (0xCE6, 0xCEF), (0xD66, 0xD6F), (0xDE6, 0xDEF), (0xE50, 0xE59), (0xED0, 0xED9), (0xF20, 0xF29), (0x1040, 0x1049),
        (0x1090, 0x1099), (0x1369, 0x1371), (0x17E0, 0x17E9), (0x1810, 0x1819), (0x1946, 0x194F), (0x19D0, 0x19DA),
        (0x1A80, 0x1A89), (0x1A90, 0x1A99), (0x1B50, 0x1B59), (0x1BB0, 0x1BB9), (0x1C40, 0x1C49), (0x1C50, 0x1C59),
        (0x2070, 0x2070), (0x2074, 0x2079), (0x2080, 0x2089), (0x2460, 0x2468), (0x2474, 0x247C), (0x2488, 0x2490),
        (0x24EA, 0x24EA), (0x24F5, 0x24FD), (0x24FF, 0x24FF), (0x2776, 0x277E), (0x2780, 0x2788), (0x278A, 0x2792),
        (0xA620, 0xA629), (0xA8D0, 0xA8D9), (0xA900, 0xA909), (0xA9D0, 0xA9D9), (0xA9F0, 0xA9F9), (0xAA50, 0xAA59),
        (0xABF0, 0xABF9), (0xFF10, 0xFF19), (0x104A0, 0x104A9), (0x10A40, 0x10A43), (0x10D30, 0x10D39), (0x10D40, 0x10D49),
        (0x10E60, 0x10E68), (0x11052, 0x1105A), (0x11066, 0x1106F), (0x110F0, 0x110F9), (0x11136, 0x1113F), (0x111D0, 0x111D9),
        (0x112F0, 0x112F9), (0x11450, 0x11459), (0x114D0, 0x114D9), (0x11650, 0x11659), (0x116C0, 0x116C9), (0x116D0, 0x116E3),
        (0x11730, 0x11739), (0x118E0, 0x118E9), (0x11950, 0x11959), (0x11BF0, 0x11BF9), (0x11C50, 0x11C59), (0x11D50, 0x11D59),
        (0x11DA0, 0x11DA9), (0x11F50, 0x11F59), (0x16130, 0x16139), (0x16A60, 0x16A69), (0x16AC0, 0x16AC9), (0x16B50, 0x16B59),
        (0x16D70, 0x16D79), (0x1CCF0, 0x1CCF9), (0x1D7CE, 0x1D7FF), (0x1E140, 0x1E149), (0x1E2F0, 0x1E2F9), (0x1E4F0, 0x1E4F9),
        (0x1E5F1, 0x1E5FA), (0x1E950, 0x1E959), (0x1F100, 0x1F10A), (0x1FBF0, 0x1FBF9)];
    let c = c as u32;
    DIGITS.binary_search_by(|&(lo, hi)| if hi < c { std::cmp::Ordering::Less } else if lo > c { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Equal }).is_ok()
}

/// O nome de origem de uma entrada da gaveta: sem o sufixo `.N` que a gaveta põe.
fn origin(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((base, last)) if !base.is_empty() && !last.is_empty() && last.chars().all(py_isdigit) => base,
        _ => name,
    }
}

fn mtime(meta: &fs::Metadata) -> f64 {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs() as f64 + f64::from(d.subsec_nanos()) * 1e-9)
}

/// Move para `.drift/<nome>.N` (a menor vaga livre) e guarda só as `DRIFT_CAP` mais novas do mesmo nome.
fn drawer(dir: &Path, destination: &Path) -> Result<String> { drawer_with(dir, destination, &|from, to| fs::rename(from, to)) }

/// O link com o mesmo texto, sem segui-lo (no Windows, link de arquivo, como o `os.symlink` do `shutil`).
fn relink(text: &Path, at: &Path) -> io::Result<()> {
    #[cfg(unix)]
    { std::os::unix::fs::symlink(text, at) }
    #[cfg(windows)]
    { std::os::windows::fs::symlink_file(text, at) }
}

/// Acesso e modificação de `meta` em `path`, e depois o modo (o `copystat`: o modo por último, para um
/// modo sem escrita não barrar as datas).
fn copy_stat(path: &Path, meta: &fs::Metadata) -> Result<()> {
    let times = fs::FileTimes::new().set_accessed(meta.accessed().at(path)?).set_modified(meta.modified().at(path)?);
    #[cfg(unix)]
    let file = fs::File::open(path).at(path)?;
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES e FILE_FLAG_BACKUP_SEMANTICS: abre pasta também.
        fs::OpenOptions::new().access_mode(0x100).custom_flags(0x0200_0000).open(path).at(path)?
    };
    file.set_times(times).at(path)?;
    drop(file);
    fs::set_permissions(path, meta.permissions()).at(path)
}

/// `copy2`: os bytes, depois datas e modo. Pasta ou arquivo especial (FIFO) não é copiado: abrir um FIFO
/// para leitura esperaria um escritor para sempre.
fn copy2(from: &Path, to: &Path, meta: &fs::Metadata) -> Result<()> {
    if !meta.is_file() { return Err(ConfigError::Io(io::Error::other("não é um arquivo comum"), from.display().to_string())); }
    copy_file(from, to)?;
    copy_stat(to, meta)
}

/// `copytree(symlinks=True)`: links ficam links, sem seguir; a pasta recebe datas e modo por último.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir(to).at(to)?;
    for entry in fs::read_dir(from).at(from)? {
        let source = entry.at(from)?.path();
        let target = to.join(source.file_name().unwrap_or_default());
        let meta = fs::symlink_metadata(&source).at(&source)?;
        // ponytail: o link interno não guarda o próprio mtime (o `copystat` do Python guardava); a poda
        // só lê o das entradas do topo da gaveta.
        if meta.file_type().is_symlink() {
            relink(&fs::read_link(&source).at(&source)?, &target).between(&source, &target)?;
        } else if meta.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            copy2(&source, &target, &meta)?;
        }
    }
    copy_stat(to, &fs::metadata(from).at(from)?)
}

/// `shutil.move` para uma vaga livre: `rename`; se ele falha (outro volume, link morto na vaga), copia
/// sem seguir link, com datas e modo, e só apaga a origem depois da cópia inteira. Cópia que falha
/// deixa a origem intacta (o destino pode ficar pela metade, como no Python).
fn move_entry(from: &Path, to: &Path, rename: &dyn Fn(&Path, &Path) -> io::Result<()>) -> Result<()> {
    if rename(from, to).is_ok() { return Ok(()); }
    let meta = fs::symlink_metadata(from).at(from)?;
    if meta.file_type().is_symlink() {
        relink(&fs::read_link(from).at(from)?, to).between(from, to)?;
        return unlink(from).at(from);
    }
    if meta.is_dir() {
        copy_tree(from, to)?;
        return fs::remove_dir_all(from).at(from);
    }
    copy2(from, to, &meta)?;
    unlink(from).at(from)
}

/// `drawer` com o `rename` dado: os testes simulam o outro volume (EXDEV).
fn drawer_with(dir: &Path, destination: &Path, rename: &dyn Fn(&Path, &Path) -> io::Result<()>) -> Result<String> {
    let drift = dir.join(".drift");
    if is_symlink(&drift) || (exists(&drift) && !is_dir(&drift)) {
        // Gaveta apontando para fora levaria as entradas, e a poda, para o diretório externo.
        return Err(refused("a gaveta .drift desta conta é um symlink ou não é uma pasta"));
    }
    mkdir(&drift).at(&drift)?;
    let name = name_of(destination);
    let mut n = 1;
    // Link morto ocupa a vaga: dado como livre, a pasta não entrava nele e o arquivo o sobrescrevia.
    while fs::symlink_metadata(drift.join(format!("{name}.{n}"))).is_ok() { n += 1; }
    let slot = drift.join(format!("{name}.{n}"));
    move_entry(destination, &slot, rename)?;
    // Teto por nome de origem: pela gaveta inteira, três `skills` levavam o único `hooks.1`.
    // A recém-movida fica fora da poda: podada, o aviso "movido pra .drift/…" mentiria.
    let mut siblings = Vec::new();
    for entry in fs::read_dir(&drift).at(&drift)? {
        let path = entry.at(&drift)?.path();
        if path != slot && origin(&name_of(&path)) == name {
            // lstat: a entrada pode ser link, e o alvo quebrado não pode parar a poda.
            siblings.push((mtime(&fs::symlink_metadata(&path).at(&path)?), path));
        }
    }
    siblings.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, old) in siblings.into_iter().skip(DRIFT_CAP - 1) {
        if is_symlink(&old) || is_file(&old) { unlink(&old).at(&old)?; } else { let _ = fs::remove_dir_all(&old); }
    }
    Ok(format!("'{name}' era local nesta conta; movido pra .drift/{name}.{n}"))
}

/// `projects/` é real por conta (custo por pasta); só o `memory/` de cada projeto é atalho.
fn link_memory(home: &Path, dir: &Path, project: Option<&str>) -> Result<Vec<String>> {
    let root = home.join(".claude").join("projects");
    fs::create_dir_all(&root).at(&root)?;
    // Entrada que não é pasta sai pelo `is_dir` do alvo, logo abaixo.
    let mut names = fs::read_dir(&root).at(&root)?.map(|entry| entry.map(|e| e.file_name()))
        .collect::<io::Result<std::collections::BTreeSet<_>>>().at(&root)?;
    if let Some(project) = project {
        let memory = root.join(project).join("memory");
        fs::create_dir_all(&memory).at(&memory)?;
        names.insert(project.into());
    }
    let mut warnings = Vec::new();
    for name in names {
        let target = root.join(&name).join("memory");
        if !is_dir(&target) { continue; }
        let local = dir.join("projects").join(&name);
        let destination = local.join("memory");
        if points_to(&destination, &target) { continue; }
        fs::create_dir_all(&local).at(&local)?;
        if is_symlink(&destination) || exists(&destination) {
            warnings.extend(collide(dir, &destination, &target)?);
        }
        link(&destination, &target)?;
    }
    // A poda do topo não alcança `projects/`: sem esta passada o link morto ficava para sempre.
    let projects = dir.join("projects");
    if is_dir(&projects) {
        for entry in fs::read_dir(&projects).at(&projects)? {
            let memory = entry.at(&projects)?.path().join("memory");
            if is_symlink(&memory) && !exists(&memory) { unlink(&memory).at(&memory)?; }
        }
    }
    Ok(warnings)
}

/// `settings.json` é cópia, não atalho (o CLI regravava o compartilhado pelo link). O link do layout
/// antigo vira cópia; link para outro lugar vai para a gaveta; cópia existente recebe o espelho.
fn seed_settings(home: &Path, dir: &Path) -> Result<Option<String>> {
    let target = home.join(".claude").join("settings.json");
    let destination = dir.join("settings.json");
    let mut warning = None;
    if is_symlink(&destination) {
        if points_to(&destination, &target) { unlink(&destination).at(&destination)?; } else { warning = Some(drawer(dir, &destination)?); }
    } else if exists(&destination) {
        return mirror_settings(&target, &destination);
    }
    if is_file(&target) { copy_file(&target, &destination)?; }
    Ok(warning)
}

/// O principal manda em toda chave que tem; o que só a cópia tem fica, e ausência no principal não
/// apaga. `enabledPlugins` mescla por plugin (o `claude plugin install` da conta liga só na cópia).
/// Avisa quando desfaz algo que a cópia tinha com outro valor; nunca escreve no principal.
fn mirror_settings(target: &Path, destination: &Path) -> Result<Option<String>> {
    let failed = |e: String| refused(format!("não consegui espelhar o settings.json pra conta: {e}"));
    let Some(principal) = read_json(target).map_err(failed)? else { return Ok(None) };
    let Some(copy) = read_json(destination).map_err(failed)? else { return Ok(None) };
    let (Value::Object(principal), Value::Object(copy)) = (principal, copy) else {
        return Err(refused("settings.json não é um objeto JSON — não dá pra espelhar pra conta"));
    };
    // Plugin só acrescentado pelo principal não desfaz nada: o `enabledPlugins` conta como desfeito só
    // quando algum plugin mudou de valor.
    let mut theirs = principal;
    let mut plugins_undone = None;
    if let (Some(Value::Object(ours)), Some(Value::Object(plugins))) = (copy.get("enabledPlugins"), theirs.get("enabledPlugins")) {
        let (merged, undone) = overlay(ours, plugins);
        plugins_undone = Some(!undone.is_empty());
        theirs.insert("enabledPlugins".into(), Value::Object(merged));
    }
    let (merged, mut undone) = overlay(&copy, &theirs);
    if py_eq_map(&merged, &copy) { return Ok(None); }
    if plugins_undone == Some(false) { undone.retain(|k| k != "enabledPlugins"); }
    let text = pyjson::dumps_indent(&Value::Object(merged), 2, false) + "\n";
    if text.chars().any(py::is_marker) {
        return Err(refused("settings.json tem texto com surrogate solto, que não dá pra gravar em UTF-8 — não dá pra espelhar pra conta"));
    }
    write_replacing(destination, &text)?;
    Ok((!undone.is_empty()).then(|| format!("settings.json: o principal sobrescreveu {} na cópia desta conta", undone.join(", "))))
}

/// `ours` com as chaves de `theirs` por cima (a chave que já existia fica no lugar dela) e as que
/// `ours` tinha com outro valor, em ordem.
fn overlay(ours: &Map<String, Value>, theirs: &Map<String, Value>) -> (Map<String, Value>, Vec<String>) {
    let mut merged = ours.clone();
    let mut undone = Vec::new();
    for (key, value) in theirs {
        if ours.get(key).is_some_and(|was| !py_eq(was, value)) { undone.push(key.clone()); }
        merged.insert(key.clone(), value.clone());
    }
    undone.sort_by(|a, b| py_order(a, b));
    (merged, undone)
}

/// Os `mcpServers` da cópia com os do principal por cima, servidor a servidor, e os desfeitos; `None`
/// sem mudança.
fn mcp_plan(principal: &Map<String, Value>, copy: &Map<String, Value>) -> Option<(Map<String, Value>, Vec<String>)> {
    let Some(Value::Object(theirs)) = principal.get("mcpServers") else { return None };
    if theirs.is_empty() { return None; }
    let none = Map::new();
    let ours = match copy.get("mcpServers") { Some(Value::Object(m)) => m, _ => &none };
    let (merged, undone) = overlay(ours, theirs);
    (!py_eq_map(&merged, ours)).then_some((merged, undone))
}

/// Só os `mcpServers` do `~/.claude.json` chegam à conta: o resto do arquivo (oauthAccount, projetos)
/// é estado do CLI dela.
fn mirror_claude_json(home: &Path, dir: &Path) -> Result<Option<String>> {
    let failed = |e: String| refused(format!("não consegui espelhar o ~/.claude.json pra conta: {e}"));
    let not_object = || refused("~/.claude.json não é um objeto JSON — não dá pra espelhar pra conta");
    let destination = dir.join(".claude.json");
    let Some(principal) = read_json(&home.join(".claude.json")).map_err(failed)? else { return Ok(None) };
    let Some(copy) = read_json(&destination).map_err(failed)? else { return Ok(None) };
    let (Value::Object(principal), Value::Object(copy)) = (principal, copy) else { return Err(not_object()) };
    if mcp_plan(&principal, &copy).is_none() { return Ok(None); }
    // Regrava sob a trava do Claude Code, relendo dentro dela: uma sessão viva da conta grava o mesmo arquivo.
    // Sem a trava grava assim mesmo, como o Python e o próprio Claude Code.
    let _lock = crate::session_open::claude::lock_config(&destination).inspect_err(|error| {
        tracing::warn!(file = %destination.display(), error, "trava do .claude.json indisponível; o espelho dos MCP grava sem ela");
    }).ok();
    let mut copy = match read_json(&destination).map_err(failed)? {
        None => return Ok(None),
        Some(Value::Object(copy)) => copy,
        Some(_) => return Err(not_object()),
    };
    let Some((servers, undone)) = mcp_plan(&principal, &copy) else { return Ok(None) };
    copy.insert("mcpServers".into(), Value::Object(servers));
    write_replacing(&destination, &pyjson::dumps_indent(&Value::Object(copy), 2, true))?;
    Ok((!undone.is_empty()).then(|| format!(".claude.json: o principal sobrescreveu os MCP {} na cópia desta conta", undone.join(", "))))
}

/// Plugin ligado no `settings.json` da conta sem instalação no compartilhado: avisa, nunca instala
/// (instalar escreveria no compartilhado a partir de uma conta).
fn check_plugins(home: &Path, dir: &Path) -> Result<Vec<String>> {
    let Some(settings) = read_json(&dir.join("settings.json"))
        .map_err(|e| refused(format!("não consegui ler o settings.json da conta: {e}")))? else { return Ok(vec![]) };
    let Value::Object(settings) = settings else {
        return Err(refused("o settings.json da conta não é um objeto JSON — não dá pra conferir os plugins"));
    };
    let enabled = match settings.get("enabledPlugins").filter(|v| py::truthy(Some(v))) {
        None => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return Ok(vec![]),
    };
    let registry = home.join(".claude").join("plugins").join("installed_plugins.json");
    let malformed = || refused(format!("plugins: {} fora do formato esperado — não dá pra conferir os plugins", registry.display()));
    let installed = match read_json(&registry) {
        Err(e) => return Ok(vec![format!("plugins: não consegui ler {}: {e}", registry.display())]),
        Ok(None) => Value::Object(Map::new()),
        Ok(Some(Value::Object(data))) => data.get("plugins").filter(|v| py::truthy(Some(v))).cloned().unwrap_or_else(|| json!({})),
        Ok(Some(_)) => return Err(malformed()),
    };
    let mut names: Vec<(&String, &Value)> = enabled.iter().collect();
    names.sort_by(|a, b| py_order(a.0, b.0));
    let mut warnings = Vec::new();
    for (name, on) in names {
        if !py::truthy(Some(on)) { continue; }
        let Value::Object(installed) = &installed else { return Err(malformed()) };
        let Some(entries) = installed.get(name).filter(|v| py::truthy(Some(v))) else {
            warnings.push(format!("plugin {name} está ligado no settings.json mas não consta em installed_plugins.json — \
                reinstalar: claude plugin install {name}"));
            continue;
        };
        let Value::Array(entries) = entries else { return Err(malformed()) };
        let mut missing = Vec::new();
        for entry in entries {
            let Value::Object(entry) = entry else { return Err(malformed()) };
            match entry.get("installPath") {
                Some(Value::String(path)) if !path.is_empty() => if !is_dir(Path::new(path)) { missing.push(path.clone()) },
                path if !py::truthy(path) => missing.push(py::py_str(path.unwrap_or(&Value::Null))),
                // `Path(5)`: o Python levantava TypeError cru.
                _ => return Err(malformed()),
            }
        }
        if !missing.is_empty() {
            warnings.push(format!("plugin {name} consta instalado mas a pasta sumiu ({}) — reinstalar: claude plugin install {name}",
                missing.join(", ")));
        }
    }
    Ok(warnings)
}

/// O preparo da pasta pendente da criação: `~/.claude` garantido, o `.claude.json` do principal sem o
/// `oauthAccount` (as permissões e os MCP ficam, o login não), `projects/` e a reconciliação sob a
/// trava da pendência, o único arquivo que existe antes da publicação.
pub fn seed(home: &Path, dir: &Path) -> Outcome {
    match prepare_pending(home, dir) {
        Ok(()) => reconcile_locked(home, dir, None, PENDING, Wait::Each(LOCK_WAIT)),
        Err(error) => Outcome::failed(error),
    }
}

fn prepare_pending(home: &Path, dir: &Path) -> Result<()> {
    let shared = home.join(".claude");
    fs::create_dir_all(&shared).at(&shared)?;
    let mut data = match read_json(&home.join(".claude.json")) {
        Ok(None) => Map::new(),
        Ok(Some(Value::Object(data))) => data,
        Ok(Some(_)) => return Err(refused("o ~/.claude.json não é um objeto JSON — não dá pra semear a conta")),
        Err(e) => return Err(refused(format!("não consegui ler o ~/.claude.json pra semear a conta: {e}"))),
    };
    data.shift_remove("oauthAccount");
    let copy = dir.join(".claude.json");
    fs::write(&copy, pyjson::dumps_indent(&Value::Object(data), 2, true)).at(&copy)?;
    let projects = dir.join("projects");
    mkdir(&projects).at(&projects)
}

// ── o comando `hangar-server conta` ───────────────────────────────────────────────────────────

const DOC: &str = "CLI de contas Claude, para o wrapper de shell chamar.

A regra mora num lugar SÓ, o hangar-server: o shell chega a ela pelo hangar-conta, que chama
este `hangar-server conta`, e a abertura de sessão roda a mesma reconciliação no próprio processo.

  hangar-conta --list                    -> um nome de conta por linha
  hangar-conta --prep <nome> [<projeto>] -> refaz os atalhos e imprime o caminho do config dir;
                                            com o projeto (a pasta já saneada), liga a memória dele
  hangar-conta --new <nome>              -> cria a conta e imprime o caminho
";

/// Os eventos de um comando no diário exportável, num arquivo só dos comandos (`diario/hangar-conta`):
/// no Windows dois processos anexando no mesmo arquivo se sobrescrevem, então quem grava segura uma
/// trava só do diário. A das contas não serve: presa, ela é justamente o motivo do evento de limite.
/// Falha não chega a quem chamou.
pub fn write_cli_diary(env: &AccountEnvironment, events: &[Value]) {
    if events.is_empty() { return; }
    let Some(dir) = crate::runtime::local_policy::log_base_in(env).map(|base| base.join("diario").join("hangar-conta")) else { return };
    if fs::create_dir_all(&dir).is_err() { return; }
    let Ok(Some(_held)) = lock(&dir.with_extension("lock"), true, Instant::now() + Duration::from_secs(5)) else { return };
    let _ = crate::diag::write_diary(&dir, events, chrono::Local::now());
}

/// `contas.listar`: os nomes das contas do Hangar, em ordem.
fn list(home: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(home) else { return vec![] };
    let mut names: Vec<String> = entries.flatten().filter_map(|entry| {
        let name = entry.file_name().to_str()?.strip_prefix(".claude-")?.to_owned();
        is_account(&entry.path()).then_some(name)
    }).collect();
    names.sort_by(|a, b| py_order(a, b));
    names
}

/// A guarda compartilhada de existência, como a do `ciclo_conta`: 5 s, depois "ocupada".
fn guard(home: &Path, dir: &Path) -> Result<super::AccountGuard> {
    let locks = AccountLocks::new(home.join(".hangar").join("account-locks"));
    let key = AccountKey::new(Provider::Claude, dir).at(dir)?;
    let file = locks.path(&key).at(dir)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match locks.try_acquire(&key, GuardMode::Shared) {
            Ok(guard) => return Ok(guard),
            Err(LockError::Busy) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Err(LockError::Busy) => return Err(refused("a conta está ocupada por outra operação")),
            Err(LockError::Io(error)) => return Err(ConfigError::Io(error, file.display().to_string())),
        }
    }
}

fn refusal(err: &mut dyn Write, error: &ConfigError) -> u8 {
    let _ = writeln!(err, "hangar-conta: {}", error.cause());
    match error { ConfigError::Refused(_) => 1, ConfigError::Io(..) => 126 }
}

fn prep(env: &AccountEnvironment, name: &str, project: Option<&str>, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let dir = match account_dir(&env.home, name) {
        Ok(dir) => dir,
        Err(error) => return refusal(err, &error),
    };
    let outcome = match guard(&env.home, &dir) {
        Ok(_guard) => reconcile_account(&env.home, name, project, Wait::Each(LOCK_WAIT)),
        Err(error) => return refusal(err, &error),
    };
    write_cli_diary(env, &outcome.diary);
    match outcome.result {
        Ok(warnings) => {
            for warning in warnings { let _ = writeln!(err, "hangar-conta: {warning}"); }
            let _ = writeln!(out, "{}", dir.display());
            0
        }
        Err(error) => refusal(err, &error),
    }
}

fn new(env: &AccountEnvironment, name: &str, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let mut seeded: Option<Outcome> = None;
    let created = AccountService::new(env.clone()).create(Provider::Claude, name, |path| {
        let outcome = seed(&env.home, path);
        let failed = outcome.result.as_ref().err()
            .map(|e| super::catalog::AccountError::new(500, "account_prepare_failed", e.to_string(), json!({})));
        seeded = Some(outcome);
        failed.map_or(Ok(()), Err)
    });
    if let Some(outcome) = &seeded { write_cli_diary(env, &outcome.diary); }
    match (created, seeded.and_then(|o| o.result.err())) {
        // Os avisos da primeira reconciliação não saem: a conta acabou de nascer, como no `criar`.
        (Ok(_), _) => { let _ = writeln!(out, "{}", env.home.join(format!(".claude-{name}")).display()); 0 }
        (Err(_), Some(error)) => refusal(err, &error),
        (Err(error), None) => { let _ = writeln!(err, "hangar-conta: {}", error.message); 1 }
    }
}

/// O comando com o ambiente e as saídas dadas; devolve o código de saída.
pub fn run(args: &[String], env: &AccountEnvironment, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    match args {
        [flag] if flag == "--help" || flag == "-h" => { let _ = writeln!(out, "{DOC}"); 0 }
        [flag] if flag == "--list" => {
            for name in list(&env.home) { let _ = writeln!(out, "{name}"); }
            0
        }
        [flag, name] if flag == "--new" => new(env, name, out, err),
        [flag, name] if flag == "--prep" => prep(env, name, None, out, err),
        [flag, name, project] if flag == "--prep" => prep(env, name, Some(project), out, err),
        _ => { let _ = writeln!(err, "{DOC}"); 2 }
    }
}

/// `hangar-server conta …`: `args` sem o nome do subcomando.
pub fn cli(args: &[String]) -> std::process::ExitCode {
    // Os avisos internos (rollback que não concluiu, trava que não saiu) chegam ao stderr; sem eles, nada muda.
    let _ = tracing_subscriber::fmt().with_writer(io::stderr).with_max_level(tracing::Level::WARN)
        .without_time().with_target(false).with_ansi(false).try_init();
    let env = AccountEnvironment::capture();
    std::process::ExitCode::from(run(args, &env, &mut io::stdout().lock(), &mut io::stderr().lock()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    /// O `rename` entre volumes: a gaveta tem de copiar, como o `shutil.move`.
    fn cross_device(_: &Path, _: &Path) -> io::Result<()> { Err(io::ErrorKind::CrossesDevices.into()) }

    fn set_mtime(path: &Path, secs: u64) {
        let at = std::time::UNIX_EPOCH + Duration::from_secs(secs);
        let file = fs::File::open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_accessed(at).set_modified(at)).unwrap();
    }
    fn mtime_secs(path: &Path) -> u64 {
        fs::symlink_metadata(path).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }
    fn mode(path: &Path) -> u32 { fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777 }

    #[test]
    fn a_folder_crossing_devices_is_copied_with_links_mode_and_mtime() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let skills = dir.join("skills");
        fs::create_dir_all(skills.join("a/b")).unwrap();
        fs::write(skills.join("a/b/fundo.md"), "fundo").unwrap();
        fs::write(skills.join("topo.sh"), "topo").unwrap();
        fs::set_permissions(skills.join("topo.sh"), fs::Permissions::from_mode(0o750)).unwrap();
        symlink("../fora", skills.join("a/link")).unwrap();
        set_mtime(&skills.join("topo.sh"), 1_600_000_000);
        set_mtime(&skills, 1_700_000_000);
        let warning = drawer_with(dir, &skills, &cross_device);
        assert_eq!(warning.ok(), Some("'skills' era local nesta conta; movido pra .drift/skills.1".to_owned()));
        let slot = dir.join(".drift/skills.1");
        assert!(fs::symlink_metadata(&skills).is_err(), "a origem sai depois da cópia inteira");
        assert_eq!(fs::read_to_string(slot.join("a/b/fundo.md")).unwrap(), "fundo");
        assert_eq!(fs::read_to_string(slot.join("topo.sh")).unwrap(), "topo");
        assert_eq!(fs::read_link(slot.join("a/link")).unwrap(), Path::new("../fora"), "link fica link, sem ser seguido");
        assert_eq!((mode(&slot.join("topo.sh")), mtime_secs(&slot.join("topo.sh"))), (0o750, 1_600_000_000));
        assert_eq!(mtime_secs(&slot), 1_700_000_000, "a pasta guarda o mtime: a poda da gaveta lê ele");
    }

    #[test]
    fn a_link_crossing_devices_moves_as_a_link() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        symlink(dir.join("segredo.txt"), dir.join("CLAUDE.md")).unwrap();
        assert!(drawer_with(dir, &dir.join("CLAUDE.md"), &cross_device).is_ok());
        assert_eq!(fs::read_link(dir.join(".drift/CLAUDE.md.1")).unwrap(), dir.join("segredo.txt"));
        assert!(fs::symlink_metadata(dir.join("CLAUDE.md")).is_err());
    }

    #[test]
    fn a_file_crossing_devices_keeps_bytes_mode_and_mtime() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let hooks = dir.join("hooks");
        fs::write(&hooks, "velho").unwrap();
        fs::set_permissions(&hooks, fs::Permissions::from_mode(0o640)).unwrap();
        set_mtime(&hooks, 1_000);
        assert!(drawer_with(dir, &hooks, &cross_device).is_ok());
        let slot = dir.join(".drift/hooks.1");
        assert!(fs::symlink_metadata(&hooks).is_err());
        assert_eq!((fs::read_to_string(&slot).unwrap(), mode(&slot), mtime_secs(&slot)), ("velho".to_owned(), 0o640, 1_000));
    }

    /// A recém-movida fica mesmo mais velha que as irmãs; na entrada seguinte, a cópia com o mtime antigo
    /// preservado é a mais velha das irmãs e a poda a leva (nascida com o mtime de agora, levaria outra).
    #[test]
    fn the_pruning_reads_the_preserved_mtime_of_the_copy() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        fs::create_dir(dir.join(".drift")).unwrap();
        for n in 1..=3 {
            let sibling = dir.join(format!(".drift/hooks.{n}"));
            fs::write(&sibling, n.to_string()).unwrap();
            set_mtime(&sibling, 1_500_000_000 + n);
        }
        let hooks = dir.join("hooks");
        fs::write(&hooks, "velho").unwrap();
        set_mtime(&hooks, 1_000);
        assert!(drawer_with(dir, &hooks, &cross_device).is_ok());
        assert_eq!(fs::read_to_string(dir.join(".drift/hooks.4")).ok().as_deref(), Some("velho"), "a recém-movida nunca é podada");
        fs::write(&hooks, "novo").unwrap();
        set_mtime(&hooks, 2_000_000_000);
        assert!(drawer(dir, &hooks).is_ok());
        let mut left: Vec<String> = fs::read_dir(dir.join(".drift")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["hooks.1", "hooks.2", "hooks.3"], "a cópia com o mtime antigo é a podada na entrada seguinte");
        assert_eq!(fs::read_to_string(dir.join(".drift/hooks.1")).unwrap(), "novo");
    }

    #[test]
    fn a_copy_failing_midway_keeps_the_source() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let skills = dir.join("skills");
        fs::create_dir(&skills).unwrap();
        fs::write(skills.join("a.md"), "a").unwrap();
        fs::write(skills.join("trancado.md"), "t").unwrap();
        fs::set_permissions(skills.join("trancado.md"), fs::Permissions::from_mode(0o000)).unwrap();
        let error = drawer_with(dir, &skills, &cross_device).unwrap_err().to_string();
        fs::set_permissions(skills.join("trancado.md"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(error.contains(&skills.join("trancado.md").display().to_string()), "{error}");
        assert_eq!(fs::read_to_string(skills.join("a.md")).unwrap(), "a", "a origem fica inteira");
        assert_eq!(fs::read_to_string(skills.join("trancado.md")).unwrap(), "t");
    }
}

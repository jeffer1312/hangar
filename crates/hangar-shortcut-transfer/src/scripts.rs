//! Pacote limitado de scripts locais; nunca executa o conteúdo para descobrir dependências.
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde_json::{Value, json};

use hangar_core::pycompat::py;

use crate::host::{Error, Machine};
use crate::lexer::{LexError, Lexer};
use crate::scrub::scrub;

pub(crate) const MAX_FILES: usize = 32;
pub(crate) const MAX_FILE_BYTES: usize = 128 * 1024;
pub(crate) const MAX_TOTAL_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_WARNINGS: usize = 128;
pub(crate) const MAX_WARNING_BYTES: usize = 16 * 1024;
const ROOTS: [&str; 3] = [".local/bin/", "bin/", ".config/fish/functions/"];
const SUFFIXES: [&str; 10] = ["", ".sh", ".bash", ".zsh", ".fish", ".py", ".js", ".mjs", ".rb", ".pl"];
const NON_SHELL_SUFFIXES: [&str; 5] = [".py", ".js", ".mjs", ".rb", ".pl"];
const INTERPRETERS: [&str; 9] = ["sh", "bash", "zsh", "fish", "python", "python3", "node", "ruby", "perl"];
const CONTROL: [&str; 11] = ["if", "then", "else", "elif", "while", "do", "begin", "and", "or", "not", "!"];
const CREDENTIAL_PARTS: [&str; 7] = [".ssh", ".aws", ".gnupg", "credentials", "secrets", "id_rsa", "id_ed25519"];

const NOT_LINUX: &str = "Scripts não incluídos: o empacotamento de scripts locais só está disponível em Linux.";
const CREDENTIAL: &str = "Arquivo de credencial é dependência externa; não foi lido nem incluído.";
const DYNAMIC: &str = "Dependências dinâmicas precisam de conferência manual; nenhum comando foi executado.";
const RELATIVE: &str = "Caminho relativo de dependência depende da pasta de execução e não foi incluído.";
const OUTSIDE: &str = "Dependência fora das pastas locais permitidas não foi incluída.";
const EXCLUDED: &str = "Uma dependência local foi excluída: arquivo ausente, tipo, caminho ou tamanho não permitido.";
const INTERPRETER: &str = "Comando de interpretador com opções precisa de conferência manual das dependências.";
const COMPOSITE: &str = "Sintaxe composta precisa de conferência manual das dependências.";
const SYNTAX: &str = "Sintaxe de shell não reconhecida; dependências exigem conferência manual.";
const LIMIT: &str = "Limite de avisos atingido; confira as demais dependências manualmente.";

static FISH_SHEBANG: LazyLock<regex::Regex> = LazyLock::new(|| plain(r"(?:^|[/\s])fish(?:\s|$)"));
static OTHER_SHEBANG: LazyLock<regex::Regex> =
    LazyLock::new(|| plain(r"(?:^|[/\s])(?:python[\d.]*|node|ruby|perl)(?:\s|$)"));
static ASSIGNMENT: LazyLock<regex::Regex> = LazyLock::new(|| plain(r"^[A-Za-z_]\w*="));
static DYNAMIC_SYNTAX: LazyLock<regex::Regex> = LazyLock::new(|| plain(r"(?:\$\(|`|\(\s*cat\b)"));
static WORD: LazyLock<regex::Regex> = LazyLock::new(|| plain(r#"[^\s"'();]+"#));
static LITERAL_PATH: LazyLock<regex::Regex> = LazyLock::new(|| plain(r"\A(?:/|~/|\$HOME/|\$\{HOME\}/)?[\w./-]+\z"));

/// Script do pacote já conferido, com o destino nesta máquina.
#[derive(Clone, Debug)]
pub(crate) struct Script {
    pub(crate) path: String,
    pub(crate) content: String,
    pub(crate) executable: bool,
    pub(crate) target: PathBuf,
}

/// Recusa de `destination` e `read_file`: `Invalid` vira mensagem, `Io` é falha do disco.
#[derive(Debug)]
pub(crate) enum FileError {
    Invalid(&'static str),
    Io(io::Error),
}

struct Collector<'a> {
    machine: &'a Machine,
    home: String,
    search: Vec<String>,
    scripts: Vec<Value>,
    warnings: Vec<String>,
    visited: HashSet<String>,
    total: usize,
}

impl From<io::Error> for FileError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// O conteúdo é fish: shebang, extensão `.fish` ou, sem caminho, o `SHELL` desta máquina.
pub(crate) fn is_fish(content: &str, path: Option<&str>, machine: &Machine) -> bool {
    let first = content.split('\n').next().unwrap_or("");
    if let Some(shebang) = first.strip_prefix("#!") {
        return FISH_SHEBANG.is_match(py::strip(shebang));
    }
    if let Some(path) = path {
        return path.ends_with(".fish");
    }
    let shell = machine.env.get("SHELL").filter(|s| !s.is_empty()).map_or("/bin/sh", String::as_str);
    name(shell) == "fish"
}

pub(crate) fn is_shell_script(content: &str, path: Option<&str>) -> bool {
    let first = content.split('\n').next().unwrap_or("");
    if let Some(shebang) = first.strip_prefix("#!") {
        return !OTHER_SHEBANG.is_match(py::strip(shebang));
    }
    path.is_none_or(|path| !NON_SHELL_SUFFIXES.contains(&suffix(path)))
}

pub(crate) fn credential_path(value: &str) -> bool {
    value.to_lowercase().replace('\\', "/").split('/').any(|part| {
        CREDENTIAL_PARTS.contains(&part)
            || part == ".env"
            || part.starts_with(".env.")
            || part.ends_with(".pw")
            || part.ends_with(".pem")
            || part.ends_with(".key")
    })
}

/// Destino permitido de um script, relativo ao home, sem link no caminho até ele.
pub(crate) fn destination(home: &Path, relative: &str) -> Result<PathBuf, &'static str> {
    if relative.is_empty() || relative.contains('\\') || relative.chars().any(|c| (c as u32) < 32 || c as u32 == 127) {
        return Err("caminho de script inválido");
    }
    if relative.split('/').any(|part| matches!(part, "" | "." | ".."))
        || !ROOTS.iter().any(|root| relative.starts_with(root))
        || credential_path(relative)
        || !SUFFIXES.contains(&suffix(relative).to_lowercase().as_str())
    {
        return Err("script fora das pastas e tipos permitidos");
    }
    if relative.starts_with(".config/fish/functions/") && !relative.ends_with(".fish") {
        return Err("função fish precisa da extensão .fish");
    }
    let target = home.join(relative);
    let mut current = Some(target.as_path());
    while let Some(path) = current {
        if is_symlink(path) {
            return Err("script ou pasta de destino é um link simbólico");
        }
        if path == home {
            break;
        }
        current = path.parent();
    }
    if target.exists() && !target.is_file() {
        return Err("destino do script não é um arquivo regular");
    }
    Ok(target)
}

/// Lê sem seguir link: fecha a troca do arquivo por link entre a validação e a leitura.
pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>, FileError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(FileError::Invalid("script não é um arquivo regular"));
    }
    let mut data = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut data)?;
    if data.len() > MAX_FILE_BYTES || data.contains(&0) {
        return Err(FileError::Invalid("script binário ou maior que 128 KiB"));
    }
    Ok(data)
}

/// Confere os scripts do pacote inteiro antes de qualquer escrita.
pub(crate) fn validate(scripts: &Value, machine: &Machine) -> Result<Vec<Script>, String> {
    let list = scripts.as_array().filter(|list| list.len() <= MAX_FILES).ok_or("pacote deve conter até 32 scripts")?;
    if !list.is_empty() && !machine.linux {
        return Err("pacotes com scripts só podem ser importados em Linux".into());
    }
    let (mut out, mut seen, mut total) = (Vec::new(), HashSet::new(), 0);
    for script in list {
        let script = script.as_object().ok_or("script inválido")?;
        let relative = script.get("path").and_then(Value::as_str).ok_or("caminho de script inválido")?;
        let target = destination(&machine.home, relative)?;
        if !seen.insert(relative.to_owned()) {
            return Err("caminho de script duplicado".into());
        }
        let content = script.get("content").and_then(Value::as_str).filter(|c| !c.contains('\0'));
        let (Some(content), Some(executable)) = (content, script.get("executable").and_then(Value::as_bool)) else {
            return Err("conteúdo ou permissão de script inválido".into());
        };
        total += content.len();
        if content.len() > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
            return Err("pacote de scripts ultrapassa o limite de tamanho".into());
        }
        out.push(Script { path: relative.to_owned(), content: content.to_owned(), executable, target });
    }
    Ok(out)
}

/// Os scripts de que os comandos dependem, varridos, e os avisos do que ficou de fora.
///
/// # Errors
/// Falha de disco ao ler o modo de um script já incluído.
pub(crate) fn collect(
    commands: &[&str],
    machine: &Machine,
    login_path: impl FnOnce() -> String,
) -> Result<(Vec<Value>, Vec<String>), Error> {
    if commands.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    if !machine.linux {
        return Ok((Vec::new(), vec![NOT_LINUX.to_owned()]));
    }
    let home = machine.home.to_string_lossy().into_owned();
    let mut search: Vec<String> = login_path().split(':').map(str::to_owned).collect();
    for fallback in [join(&home, ".local/bin"), join(&home, "bin")] {
        if !search.contains(&fallback) {
            search.push(fallback);
        }
    }
    let mut collector = Collector {
        machine,
        home,
        search,
        scripts: Vec::new(),
        warnings: Vec::new(),
        visited: HashSet::new(),
        total: 0,
    };
    for command in commands {
        collector.scan(command, None)?;
    }
    Ok((collector.scripts, collector.warnings))
}

/// Grava cada script, guardando o anterior, e só então a configuração; falha desfaz tudo.
pub(crate) fn apply_files(home: &Path, scripts: &[Script], save: impl FnOnce() -> Result<(), Error>) -> Result<(), String> {
    let mut backups: Vec<(PathBuf, Option<Vec<u8>>, u32)> = Vec::new();
    let mut created = Vec::new();
    let outcome = write_all(home, scripts, &mut backups, &mut created).and_then(|()| save().map_err(|_| ()));
    if outcome.is_ok() {
        return Ok(());
    }
    let mut failed = false;
    for (target, data, mode) in backups.iter().rev() {
        let restored = (|| -> Result<(), ()> {
            let relative = target.strip_prefix(home).map_err(|_| ())?.to_string_lossy().replace('\\', "/");
            destination(home, &relative).map_err(|_| ())?;
            match data {
                None => match fs::remove_file(target) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => Err(()),
                    _ => Ok(()),
                },
                Some(data) => write_file(target, data, *mode, &mut Vec::new()).map_err(|_| ()),
            }
        })();
        failed |= restored.is_err();
    }
    // A pasta que esta importação criou e ficou vazia sai; `remove_dir` não apaga pasta com conteúdo.
    for dir in created.iter().rev() {
        let _ = fs::remove_dir(dir);
    }
    let suffix = if failed { "; restauração dos arquivos falhou, confira os destinos" } else { "; arquivos anteriores restaurados" };
    Err(format!("importação falhou{suffix}"))
}

impl Collector<'_> {
    fn scan(&mut self, content: &str, path: Option<&str>) -> Result<(), Error> {
        let content = scrub(content, is_fish(content, path, self.machine), is_shell_script(content, path)).0;
        if DYNAMIC_SYNTAX.is_match(&content) {
            self.warn(DYNAMIC, "");
        }
        for word in WORD.find_iter(&content) {
            if credential_path(word.as_str()) {
                self.warn(CREDENTIAL, word.as_str());
            }
        }
        // Análise estática por linha; expansão e sintaxe composta exigem conferência manual.
        let joined = content.replace("\\\n", "");
        for line in py::splitlines(&joined) {
            match self.scan_line(line) {
                Ok(()) => {}
                Err(Scan::Syntax) => self.warn(SYNTAX, ""),
                Err(Scan::Disk(error)) => return Err(error),
            }
        }
        Ok(())
    }

    fn scan_line(&mut self, line: &str) -> Result<(), Scan> {
        let mut lexer = Lexer::new(line);
        let mut words: Vec<String> = Vec::new();
        while let Some(word) = lexer.next_word()? {
            match word.as_str() {
                ";" | "&&" | "||" | "|" | "&" => {
                    self.segment(&words)?;
                    words.clear();
                }
                "<" | ">" | ">>" | "<>" | ">|" | ">&" | "<&" | "&>" | "&>>" => {
                    // O destino não é comando; a chamada depois de &&, || ou ; ainda precisa entrar.
                    if words.last().is_some_and(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit())) {
                        words.pop();
                    }
                    lexer.next_word()?;
                }
                "<<" | "<<<" | "(" | ")" => {
                    self.warn(COMPOSITE, "");
                    break;
                }
                _ => words.push(word),
            }
        }
        self.segment(&words)
    }

    fn segment(&mut self, words: &[String]) -> Result<(), Scan> {
        let mut words = words;
        while words.first().is_some_and(|w| CONTROL.contains(&w.as_str()) || ASSIGNMENT.is_match(w)) {
            words = &words[1..];
        }
        while words.first().is_some_and(|w| matches!(w.as_str(), "exec" | "command" | "env")) {
            words = &words[1..];
            while words.first().is_some_and(|w| w.starts_with('-') || ASSIGNMENT.is_match(w)) {
                words = &words[1..];
            }
        }
        let Some(first) = words.first() else { return Ok(()) };
        if matches!(first.as_str(), "function" | "end" | "fi" | "done" | "for" | "case" | "esac") {
            return Ok(());
        }
        let executable = name(first);
        if executable == "source" || executable == "." {
            if let Some(file) = words.get(1) {
                self.include(file)?;
            }
        } else if INTERPRETERS.contains(&executable) {
            match words.get(1) {
                Some(file) if !file.starts_with('-') => self.include(file)?,
                Some(flag) if !matches!(flag.as_str(), "--version" | "-V") => self.warn(INTERPRETER, ""),
                _ => {}
            }
        } else {
            self.include(first)?;
        }
        Ok(())
    }

    fn include(&mut self, token: &str) -> Result<(), Scan> {
        if credential_path(token) {
            self.warn(CREDENTIAL, token);
            return Ok(());
        }
        let token = self.expand_home(token);
        if token.chars().any(|c| "$`(){}*?[]".contains(c)) {
            self.warn(DYNAMIC, "");
            return Ok(());
        }
        let path = if token.contains('/') {
            if !token.starts_with('/') {
                // Caminho relativo depende do cwd de execução, não da pasta do script.
                self.warn(RELATIVE, &token);
                return Ok(());
            }
            normalize(&token)
        } else {
            let fish = join(&self.home, &format!(".config/fish/functions/{token}.fish"));
            if Path::new(&fish).exists() || is_symlink(Path::new(&fish)) {
                fish
            } else if let Some(found) = which(&token, &self.search) {
                normalize(&found)
            } else {
                return Ok(());
            }
        };
        let Some(relative) = relative_to(&path, &self.home) else {
            if !["/usr/", "/bin/", "/sbin/"].iter().any(|prefix| path.starts_with(prefix)) {
                self.warn(OUTSIDE, &path);
            }
            return Ok(());
        };
        if !self.visited.insert(relative.clone()) {
            return Ok(());
        }
        let Some((target, size, content)) = self.read(&relative) else {
            self.warn(EXCLUDED, &path);
            return Ok(());
        };
        self.total += size;
        let executable = executable(&target).map_err(|error| Scan::Disk(Error::Io(error.to_string())))?;
        self.scripts.push(json!({"path": relative.clone(), "content": content.clone(), "executable": executable}));
        self.scan(&content, Some(relative.as_str())).map_err(Scan::Disk)
    }

    fn read(&self, relative: &str) -> Option<(PathBuf, usize, String)> {
        let target = destination(&self.machine.home, relative).ok()?;
        if self.scripts.len() >= MAX_FILES {
            return None;
        }
        let raw = read_file(&target).ok()?;
        let size = raw.len();
        let content = String::from_utf8(raw).ok()?;
        (self.total + size <= MAX_TOTAL_BYTES).then_some((target, size, content))
    }

    /// Só caminho literal entra no aviso; argv e atribuição podem trazer uma senha.
    fn warn(&mut self, message: &str, token: &str) {
        let mut message = message.to_owned();
        if token.chars().count() <= 400 && LITERAL_PATH.is_match(token) && !token.split('/').any(|part| part == "..") {
            let detailed = format!("{message} Caminho: {}", self.expand_home(token));
            if detailed.len() <= 1024 {
                message = detailed;
            }
        }
        if self.warnings.iter().any(|w| *w == message || w == LIMIT) {
            return;
        }
        let used: usize = self.warnings.iter().map(String::len).sum();
        if self.warnings.len() >= MAX_WARNINGS - 1 || used + message.len() > MAX_WARNING_BYTES - LIMIT.len() {
            self.warnings.push(LIMIT.to_owned());
        } else {
            self.warnings.push(message);
        }
    }

    fn expand_home(&self, token: &str) -> String {
        let token = token.replace("${HOME}", &self.home).replace("$HOME", &self.home);
        match token.strip_prefix("~/") {
            Some(rest) => join(&self.home, rest),
            None => token,
        }
    }
}

enum Scan {
    Syntax,
    Disk(Error),
}

impl From<LexError> for Scan {
    fn from(_: LexError) -> Self {
        Self::Syntax
    }
}

fn write_all(
    home: &Path,
    scripts: &[Script],
    backups: &mut Vec<(PathBuf, Option<Vec<u8>>, u32)>,
    created: &mut Vec<PathBuf>,
) -> Result<(), ()> {
    for script in scripts {
        let target = destination(home, &script.path).map_err(|_| ())?;
        let previous = if target.exists() { Some(read_file(&target).map_err(|_| ())?) } else { None };
        let mode = if previous.is_some() { mode(&target).map_err(|_| ())? } else { 0 };
        backups.push((target.clone(), previous, mode));
        let mode = if script.executable { 0o700 } else { 0o600 };
        write_file(&target, script.content.as_bytes(), mode, created).map_err(|_| ())?;
    }
    Ok(())
}

/// Temporário na mesma pasta e troca atômica; as pastas que faltavam ficam anotadas em `created`.
pub(crate) fn write_file(target: &Path, data: &[u8], mode: u32, created: &mut Vec<PathBuf>) -> io::Result<()> {
    let parent = target.parent().ok_or_else(|| io::Error::other("script sem pasta"))?;
    let mut missing = Vec::new();
    let mut current = Some(parent);
    while let Some(dir) = current {
        if dir.exists() || is_symlink(dir) {
            break;
        }
        missing.push(dir.to_path_buf());
        current = dir.parent();
    }
    // Anotadas antes de criar: uma criação que para no meio também é desfeita.
    created.extend(missing.into_iter().rev());
    fs::create_dir_all(parent)?;
    let mut file = tempfile::Builder::new().prefix(".hangar-shortcut-").tempfile_in(parent)?;
    file.write_all(data)?;
    set_mode(file.path(), mode)?;
    file.persist(target).map_err(|error| error.error)?;
    Ok(())
}

pub(crate) fn mode(path: &Path) -> io::Result<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(fs::metadata(path)?.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        fs::metadata(path).map(|_| 0o600)
    }
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

fn executable(path: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(fs::metadata(path)?.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        fs::metadata(path).map(|_| false)
    }
}

/// `shutil.which`: o primeiro executável de `search` com esse nome, como o PATH o escreve.
fn which(name: &str, search: &[String]) -> Option<String> {
    let mut seen = HashSet::new();
    for dir in search {
        if !seen.insert(dir.as_str()) {
            continue;
        }
        let candidate = if dir.is_empty() {
            name.to_owned()
        } else if dir.ends_with('/') {
            format!("{dir}{name}")
        } else {
            format!("{dir}/{name}")
        };
        let path = Path::new(&candidate);
        if path.exists() && !path.is_dir() && can_execute(path) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn can_execute(path: &Path) -> bool {
    hangar_core::terminal_process::can_execute(path)
}

#[cfg(not(unix))]
fn can_execute(path: &Path) -> bool {
    path.is_file()
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Caminho como o `pathlib` o escreve: sem `.`, sem barra repetida, `//` inicial preservado.
fn normalize(path: &str) -> String {
    let root = if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else if path.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    if root.is_empty() && parts.is_empty() { ".".to_owned() } else { format!("{root}{}", parts.join("/")) }
}

/// `str(Path(base) / rest)`.
fn join(base: &str, rest: &str) -> String {
    if rest.starts_with('/') { normalize(rest) } else { normalize(&format!("{base}/{rest}")) }
}

/// `Path.name`: o último componente depois de normalizar.
pub(crate) fn name(path: &str) -> &str {
    path.split('/').rfind(|part| !part.is_empty() && *part != ".").unwrap_or("")
}

/// `PurePosixPath.suffix`: os pontos iniciais do nome não contam.
fn suffix(path: &str) -> &str {
    let name = name(path);
    match (name.find(|c: char| c != '.'), name.rfind('.')) {
        (Some(start), Some(dot)) if dot > start => &name[dot..],
        _ => "",
    }
}

/// `Path.relative_to`: `None` quando `path` não está dentro de `base`.
fn relative_to(path: &str, base: &str) -> Option<String> {
    let (path, base) = (normalize(path), normalize(base));
    let root = |p: &str| if p.starts_with("//") && !p.starts_with("///") { "//" } else if p.starts_with('/') { "/" } else { "" };
    if root(&path) != root(&base) {
        return None;
    }
    let parts = |p: &str| p.split('/').filter(|part| !part.is_empty() && *part != ".").map(str::to_owned).collect::<Vec<_>>();
    let (path_parts, base_parts) = (parts(&path), parts(&base));
    let rest = path_parts.strip_prefix(base_parts.as_slice())?;
    Some(if rest.is_empty() { ".".to_owned() } else { rest.join("/") })
}

fn plain(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("expressão constante")
}

//! Atualiza o Codex do npm ou o standalone oficial antes do app-server, com o andamento no pane. A
//! TUI sobe com a checagem dela desligada (o aviso travava a abertura). A versão publicada é consultada
//! no máximo uma vez por hora; falha nunca impede a abertura, e a saída crua do npm não vai à tela
//! (traz caminho, registro e token). A do instalador oficial vai: mostra versão, URL pública e pastas.
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::Screen;

const QUERY_EVERY: f64 = 3600.0;
const RETRY_IN: f64 = 300.0;
const LATEST: &str = "https://releases.openai.com/codex/channels/latest";

/// Prazos: consulta de versão e instalação.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits { pub query: Duration, pub install: Duration }

impl Default for Limits {
    fn default() -> Self { Self { query: Duration::from_secs(15), install: Duration::from_secs(600) } }
}

pub(crate) struct Update<'a> {
    /// A casa do usuário: o controle fica em `~/.hangar/codex-atualizacao.json`.
    home: &'a Path,
    /// O ambiente da sessão (PATH, `CODEX_HOME` da conta).
    env: &'a BTreeMap<String, String>,
    latest: String,
    limits: Limits,
}

/// A versão como o npm e a tela aceitam (`X.Y.Z` e um sufixo de pré-lançamento), comparável sem o sufixo.
pub(crate) fn version(text: &str) -> Option<[u64; 3]> {
    let text = text.trim();
    let (core, suffix) = text.split_once('-').map_or((text, None), |(core, suffix)| (core, Some(suffix)));
    if suffix.is_some_and(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.')) {
        return None;
    }
    let parts: Vec<&str> = core.split('.').collect();
    let [a, b, c] = parts.as_slice() else { return None };
    let number = |part: &str| (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())).then(|| part.parse::<u64>().ok()).flatten();
    Some([number(a)?, number(b)?, number(c)?])
}

/// A classe da falha do npm, sem o texto dele.
pub(crate) fn npm_failure(stderr: &str) -> &'static str {
    if stderr.contains("EACCES") || stderr.contains("EPERM") {
        "sem permissão para instalar"
    } else if ["ENOTFOUND", "ETIMEDOUT", "ECONNRESET", "EAI_AGAIN"].iter().any(|code| stderr.contains(code)) {
        "falha de rede"
    } else {
        "outro erro do npm"
    }
}

/// A casa do standalone oficial: o binário fica em `<casa>/packages/standalone/releases/`, e é o
/// trecho inteiro que conta (`packages/standalone` sozinho casa um monorepo do npm).
pub(crate) fn standalone_home(path: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = path.components().collect();
    let lower: Vec<String> = parts.iter().map(|part| part.as_os_str().to_string_lossy().to_lowercase()).collect();
    (0..lower.len().saturating_sub(2)).find(|&at| lower[at..at + 3] == ["packages", "standalone", "releases"])
        .map(|at| parts[..at].iter().collect())
}

/// O controle de frequência vence se ilegível, fora de forma, no passado ou absurdo (mais de 24 h à
/// frente desligaria a atualização para sempre).
pub(crate) fn due(raw: Option<&[u8]>, now: f64) -> bool {
    let next = raw.and_then(|raw| serde_json::from_slice::<Value>(raw).ok())
        .and_then(|value| value.get("proxima_em").and_then(Value::as_f64));
    !next.is_some_and(|next| next.is_finite() && now < next && next <= now + 86400.0)
}

fn now() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

/// A última palavra do `codex --version`.
/// A versão do `codex --version`: a primeira palavra com forma de versão. A última pode ser a de um
/// aviso de atualização impresso depois (`0.170.0 → 0.171.0`), que não é a instalada.
pub(crate) fn cli_version(text: &str) -> Option<String> {
    text.split_whitespace().find(|word| version(word).is_some()).map(str::to_owned)
}

enum Ran { Done { code: Option<i32>, stdout: String, stderr: String }, Timeout }

impl<'a> Update<'a> {
    pub fn new(home: &'a Path, env: &'a BTreeMap<String, String>) -> Self {
        Self { home, env, latest: LATEST.into(), limits: Limits::default() }
    }

    fn control(&self) -> PathBuf { self.home.join(".hangar").join("codex-atualizacao.json") }

    fn is_due(&self) -> bool { due(std::fs::read(self.control()).ok().as_deref(), now()) }

    /// Sem o controle gravado a consulta só se repete; a atualização segue.
    fn record(&self, seconds: f64, screen: &dyn Screen) {
        if std::fs::write(self.control(), json!({"proxima_em": now() + seconds}).to_string()).is_err() {
            screen.say("não deu para gravar o controle de atualização do Codex; a consulta se repete na próxima abertura");
        }
    }

    async fn run_captured(&self, program: &Path, args: &[&str], limit: Duration) -> io::Result<Ran> {
        let child = tokio::process::Command::new(program).args(args).env_clear().envs(self.env).stdin(Stdio::null())
            .stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;
        match tokio::time::timeout(limit, child.wait_with_output()).await {
            Ok(output) => {
                let output = output?;
                Ok(Ran::Done { code: hangar_core::terminal_process::exit_code(output.status),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(), stderr: String::from_utf8_lossy(&output.stderr).into_owned() })
            }
            Err(_) => Ok(Ran::Timeout),
        }
    }

    async fn installed_version(&self, codex: &Path) -> Option<String> {
        match self.run_captured(codex, &["--version"], self.limits.query).await {
            Ok(Ran::Done { code: Some(0), stdout, .. }) => cli_version(&stdout),
            _ => None,
        }
    }

    /// No Windows o atalho `codex.cmd` fica fora de `node_modules`: o pacote global é quem diz.
    async fn installed_by_npm(&self, codex: &Path, npm: &Path) -> io::Result<bool> {
        if real(codex).to_string_lossy().replace('\\', "/").contains("node_modules") {
            return Ok(true);
        }
        if !cfg!(windows) {
            return Ok(false);
        }
        match self.run_captured(npm, &["root", "-g"], self.limits.query).await? {
            Ran::Done { code: Some(0), stdout, .. } => Ok(Path::new(stdout.trim()).join("@openai").join("codex").is_dir()),
            Ran::Done { .. } => Ok(false),
            Ran::Timeout => Err(io::Error::from(io::ErrorKind::TimedOut)),
        }
    }

    /// O canal que o instalador oficial lê para "latest"; o User-Agent padrão leva 403.
    async fn latest_standalone(&self) -> String {
        let Ok(client) = crate::http_client().timeout(self.limits.query).build() else { return String::new() };
        let Ok(response) = client.get(&self.latest).header("User-Agent", "hangar-codex-tui").send().await else { return String::new() };
        if !response.status().is_success() { return String::new(); }
        let Ok(body) = response.bytes().await else { return String::new() };
        serde_json::from_slice::<Value>(&body).ok().and_then(|value| value.get("tag_name").and_then(Value::as_str).map(str::to_owned))
            .map(|tag| tag.strip_prefix("rust-v").unwrap_or(&tag).to_owned()).unwrap_or_default()
    }

    pub async fn run(&self, screen: &dyn Screen) {
        // Sem `codex` no PATH a abertura avisa logo depois.
        let Some(codex) = crate::runtime::process::resolve_program("codex", self.env) else { return };
        if !self.is_due() { return; }
        let lock_path = self.control().with_extension("lock");
        let lock = std::fs::create_dir_all(self.home.join(".hangar"))
            .and_then(|()| std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&lock_path));
        let lock = match lock {
            Ok(file) => Some(file),
            Err(_) => {
                // Sem trava o risco é dois npm juntos, que só falham; pular a atualização é pior.
                screen.say("não deu para criar a trava da atualização do Codex; ela segue sem trava");
                None
            }
        };
        if let Some(file) = &lock {
            match file.try_lock() {
                Ok(()) => {}
                // Esperar prenderia esta abertura durante o npm da outra.
                Err(std::fs::TryLockError::WouldBlock) => {
                    screen.say("outra sessão está atualizando o Codex; esta abre na versão instalada");
                    return;
                }
                Err(std::fs::TryLockError::Error(error)) => return self.unexpected(&error, screen),
            }
        }
        if !self.is_due() { return; } // a outra sessão acabou de conferir
        if let Err(error) = self.update(&codex, screen).await {
            self.unexpected(&error, screen);
        }
        drop(lock);
    }

    fn unexpected(&self, error: &io::Error, screen: &dyn Screen) {
        screen.say(&format!("não deu para preparar a atualização do Codex ({}); a sessão abre na versão instalada e tenta de novo em 5 min",
            super::app_server::error_kind(error)));
        self.record(RETRY_IN, screen);
    }

    async fn update(&self, codex: &Path, screen: &dyn Screen) -> io::Result<()> {
        let npm = crate::runtime::process::resolve_program("npm", self.env);
        let standalone = standalone_home(&real(codex));
        let by_npm = match (&standalone, &npm) {
            (None, Some(npm)) => match self.installed_by_npm(codex, npm).await {
                Ok(by_npm) => by_npm,
                Err(_) => {
                    screen.say("não deu para identificar como o Codex foi instalado; a sessão abre na versão instalada e tenta de novo em 5 min");
                    self.record(RETRY_IN, screen);
                    return Ok(());
                }
            },
            _ => false,
        };
        if standalone.is_none() && !by_npm {
            screen.say("atualização automática do Codex indisponível nesta instalação");
            self.record(QUERY_EVERY, screen);
            return Ok(());
        }
        screen.say("conferindo a versão do Codex…");
        let installed = self.installed_version(codex).await;
        let published = match (&standalone, &npm) {
            (Some(_), _) => self.latest_standalone().await,
            (None, Some(npm)) => match self.run_captured(npm, &["view", "@openai/codex", "version"], self.limits.query).await {
                Ok(Ran::Done { code: Some(0), stdout, .. }) => stdout.trim().to_owned(),
                _ => String::new(),
            },
            (None, None) => String::new(),
        };
        let (Some(current), Some(newer)) = (installed.as_deref().and_then(version), version(&published)) else {
            screen.say("não deu para conferir a versão do Codex; a sessão abre na versão instalada e tenta de novo em 5 min");
            self.record(RETRY_IN, screen);
            return Ok(());
        };
        self.record(QUERY_EVERY, screen);
        if newer <= current { return Ok(()); }
        let installed = installed.unwrap_or_default();
        screen.say(&format!("atualizando o Codex {installed} → {published}…"));
        let reinstall = if standalone.is_some() { "codex update" } else { "npm install -g @openai/codex" };
        let outcome = match (&standalone, &npm) {
            (Some(home), _) => self.official(codex, home, &published).await?,
            (None, Some(npm)) => match self.run_captured(npm, &["install", "-g", &format!("@openai/codex@{published}")], self.limits.install).await? {
                Ran::Done { code, stderr, .. } => Some((code, stderr)),
                Ran::Timeout => None,
            },
            (None, None) => return Ok(()),
        };
        let Some((code, stderr)) = outcome else {
            screen.say("a atualização do Codex passou de 10 min e foi interrompida");
            if self.installed_version(codex).await.as_deref().and_then(version).is_none() {
                screen.say(&format!("o Codex pode ter ficado incompleto; reinstale com {reinstall}"));
            }
            return Ok(());
        };
        if code == Some(0) && standalone.is_some() {
            // Código 0 não prova a troca: o que vale é a versão que o `codex` responde agora.
            let after = self.installed_version(codex).await.unwrap_or_default();
            if after != published {
                let read = if version(&after).is_some() { after.as_str() } else { "ilegível" };
                screen.say(&format!("o atualizador oficial terminou, mas o Codex responde {read} em vez de {published}; a sessão abre assim e tenta de novo em 1 h"));
                return Ok(());
            }
        }
        let code_text = code.map_or_else(|| "None".to_owned(), |code| code.to_string());
        if code == Some(0) {
            screen.say(&format!("Codex atualizado para {published}"));
        } else if standalone.is_some() {
            screen.say(&format!("não deu para atualizar o Codex (o atualizador oficial saiu com código {code_text}); a sessão abre na {installed} e tenta de novo em 1 h"));
        } else {
            screen.say(&format!("não deu para atualizar o Codex ({}); a sessão abre na {installed} e tenta de novo em 1 h", npm_failure(&stderr)));
        }
        Ok(())
    }

    /// `codex update` com a casa do standalone (nunca a da conta), a versão conferida e o link visível
    /// onde o PATH o achou. A saída vai ao pane. No prazo, morre o grupo inteiro: o `curl | sh` é neto e
    /// trocaria o binário com a sessão aberta. `None` = prazo.
    async fn official(&self, codex: &Path, home: &Path, release: &str) -> io::Result<Option<(Option<i32>, String)>> {
        let mut env = self.env.clone();
        env.insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
        env.insert("CODEX_RELEASE".into(), release.into());
        let visible = codex.parent().map(Path::to_path_buf).unwrap_or_default();
        if standalone_home(&real(&visible)).is_none() {
            env.insert("CODEX_INSTALL_DIR".into(), visible.to_string_lossy().into_owned());
        } else {
            env.remove("CODEX_INSTALL_DIR");
        }
        let mut command = tokio::process::Command::new(codex);
        command.arg("update").env_clear().envs(&env).stdin(Stdio::null()).stdout(Stdio::from(io::stderr())).stderr(Stdio::from(io::stderr()));
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        match tokio::time::timeout(self.limits.install, child.wait()).await {
            Ok(status) => Ok(Some((hangar_core::terminal_process::exit_code(status?), String::new()))),
            Err(_) => {
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    // SAFETY: o grupo é o deste filho, criado acima.
                    unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL); }
                }
                let _ = child.kill().await;
                Ok(None)
            }
        }
    }
}

/// O caminho real, sem o `\\?\` do Windows; sem resolver, o próprio caminho.
fn real(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).map(hangar_core::fs::without_verbatim).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Lines(Mutex<Vec<String>>);
    impl Screen for Lines {
        fn say(&self, line: &str) { self.0.lock().unwrap().push(line.to_owned()); }
    }
    impl Lines {
        fn new() -> Self { Self(Mutex::default()) }
        fn all(&self) -> Vec<String> { self.0.lock().unwrap().clone() }
    }

    #[test]
    fn only_the_published_format_is_a_version() {
        assert_eq!(version("0.161.0"), Some([0, 161, 0]));
        assert_eq!(version(" 0.161.0-alpha.2 \n"), Some([0, 161, 0]), "o sufixo não conta na comparação");
        for bad in ["", "0.161", "0.161.0.1", "v0.161.0", "0.161.0-", "0.161.0-a b", "0.161.0-\u{1b}[31m", "1.2.x", "ilegível", "-1.2.3"] {
            assert_eq!(version(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_installed_version_is_the_first_one_printed() {
        assert_eq!(cli_version("codex-cli 0.170.0\n").as_deref(), Some("0.170.0"));
        assert_eq!(cli_version("codex-cli 0.170.0\nUpdate available · 0.170.0 → 0.171.0\n").as_deref(), Some("0.170.0"),
            "o aviso de atualização vem depois e não é a instalada");
        assert_eq!(cli_version("codex-cli\n"), None);
        assert_eq!(cli_version(""), None);
    }

    #[test]
    fn the_npm_failure_shows_only_its_class() {
        assert_eq!(npm_failure("npm ERR! code EACCES /usr/lib/node_modules token=x"), "sem permissão para instalar");
        assert_eq!(npm_failure("EPERM"), "sem permissão para instalar");
        assert_eq!(npm_failure("getaddrinfo ENOTFOUND registry"), "falha de rede");
        assert_eq!(npm_failure("EAI_AGAIN"), "falha de rede");
        assert_eq!(npm_failure("qualquer outra"), "outro erro do npm");
    }

    #[test]
    fn the_standalone_home_needs_the_whole_releases_path() {
        assert_eq!(standalone_home(Path::new("/h/.codex/packages/standalone/releases/0.160.0/codex")), Some(PathBuf::from("/h/.codex")));
        assert_eq!(standalone_home(Path::new("/h/.codex/Packages/Standalone/Releases/x")), Some(PathBuf::from("/h/.codex")), "sem caixa");
        assert_eq!(standalone_home(Path::new("/repo/packages/standalone/node_modules/.bin/codex")), None, "monorepo do npm");
        assert_eq!(standalone_home(Path::new("/usr/bin/codex")), None);
    }

    /// `launcher__update-control-*`: ilegível, NaN, infinito, não objeto, texto, passado e mais de 24 h vencem.
    #[test]
    fn the_control_is_due_unless_it_holds_a_sane_future() {
        let now = 1_000_000.0;
        assert!(due(None, now), "sem arquivo");
        let stale: [&[u8]; 9] = [b"torto", b"[1]", b"{\"proxima_em\": NaN}", b"{\"proxima_em\": Infinity}", b"{\"proxima_em\": \"amanha\"}",
            b"{\"proxima_em\": 999999.0}", b"{\"proxima_em\": 1086401.0}", b"{}", b"{\"proxima_em\": true}"];
        for raw in stale {
            assert!(due(Some(raw), now), "{:?}", String::from_utf8_lossy(raw));
        }
        assert!(!due(Some(b"{\"proxima_em\": 1003600.5}"), now));
        assert!(!due(Some(b"{\"proxima_em\": 1086400}"), now), "24 h em ponto ainda vale");
    }

    proptest::proptest! {
        /// Qualquer texto vira versão ou nada; versão aceita nunca leva caractere de controle à tela nem ao npm.
        #[test]
        fn any_text_is_a_clean_version_or_none(text in "\\PC{0,24}") {
            if version(&text).is_some() {
                proptest::prop_assert!(text.trim().bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'), "{text:?}");
            }
        }

        #[test]
        fn any_control_file_decides_without_panicking(raw in proptest::collection::vec(proptest::num::u8::ANY, 0..64)) {
            let _ = due(Some(&raw), 1_000_000.0);
        }
    }

    /// Um `codex` e um `npm` falsos no PATH, como a captura fez.
    #[cfg(unix)]
    struct Fakes { dir: tempfile::TempDir, env: BTreeMap<String, String> }

    #[cfg(unix)]
    impl Fakes {
        fn new(codex_dir: &str, version: &str, npm: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join(codex_dir);
            std::fs::create_dir_all(&bin).unwrap();
            let state = dir.path().join("version");
            std::fs::write(&state, version).unwrap();
            let log = dir.path().join("spawn.log");
            write_exec(&bin.join("codex"), &format!("#!/bin/sh\necho \"codex $*|$CODEX_HOME|$CODEX_RELEASE|${{CODEX_INSTALL_DIR-unset}}\" >> '{}'\n\
                case \"$1\" in --version) echo \"codex-cli $(cat '{}')\";; update) echo 'fake official installer: installing'; echo \"$CODEX_RELEASE\" > '{}';; esac\n",
                log.display(), state.display(), state.display()));
            let npm_bin = dir.path().join("npm-bin");
            std::fs::create_dir_all(&npm_bin).unwrap();
            write_exec(&npm_bin.join("npm"), &format!("#!/bin/sh\necho \"npm $*\" >> '{}'\n{npm}\n", log.display()));
            let path = format!("{}:{}:/usr/bin:/bin", bin.display(), npm_bin.display());
            let env = BTreeMap::from([("PATH".to_owned(), path), ("HOME".to_owned(), dir.path().to_string_lossy().into_owned()),
                ("CODEX_HOME".to_owned(), dir.path().join(".codex-conta").to_string_lossy().into_owned())]);
            Self { dir, env }
        }
        fn log(&self) -> String { std::fs::read_to_string(self.dir.path().join("spawn.log")).unwrap_or_default() }
        fn control(&self) -> Value { serde_json::from_slice(&std::fs::read(self.dir.path().join(".hangar/codex-atualizacao.json")).unwrap()).unwrap() }
        fn update(&self) -> Update<'_> {
            Update { home: self.dir.path(), env: &self.env, latest: "http://127.0.0.1:9/nada".into(),
                limits: Limits { query: Duration::from_secs(5), install: Duration::from_secs(2) } }
        }
    }

    #[cfg(unix)]
    fn write_exec(path: &Path, text: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// `launcher__update-npm-newer-installs` e `-older-published`.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_outdated_npm_codex_is_updated_and_shown() {
        let fakes = Fakes::new("lib/node_modules/@openai/codex/bin", "0.160.0", "case \"$1\" in view) echo 0.161.0;; install) exit 0;; esac");
        let lines = Lines::new();
        fakes.update().run(&lines).await;
        assert_eq!(lines.all(), ["conferindo a versão do Codex…", "atualizando o Codex 0.160.0 → 0.161.0…", "Codex atualizado para 0.161.0"]);
        assert!(fakes.log().contains("npm install -g @openai/codex@0.161.0"), "{}", fakes.log());
        let next = fakes.control()["proxima_em"].as_f64().unwrap();
        assert!((next - now() - 3600.0).abs() < 60.0, "próxima consulta em 1 h");
        // Consultado agora: a próxima abertura não consulta.
        let lines = Lines::new();
        fakes.update().run(&lines).await;
        assert!(lines.all().is_empty());
    }

    /// `launcher__update-npm-install-eacces`, `-view-malformed` e `-other-install`.
    #[cfg(unix)]
    #[tokio::test]
    async fn npm_failures_show_the_class_and_retry() {
        let fakes = Fakes::new("node_modules/.bin", "0.160.0", "case \"$1\" in view) echo 0.161.0;; install) echo 'EACCES /usr/lib token' >&2; exit 243;; esac");
        let lines = Lines::new();
        fakes.update().run(&lines).await;
        assert_eq!(lines.all()[2], "não deu para atualizar o Codex (sem permissão para instalar); a sessão abre na 0.160.0 e tenta de novo em 1 h");
        let fakes = Fakes::new("node_modules/.bin", "0.160.0", "printf '0.161.0\\033[31m\\n'");
        let lines = Lines::new();
        fakes.update().run(&lines).await;
        assert_eq!(lines.all(), ["conferindo a versão do Codex…",
            "não deu para conferir a versão do Codex; a sessão abre na versão instalada e tenta de novo em 5 min"]);
        let next = fakes.control()["proxima_em"].as_f64().unwrap();
        assert!((next - now() - 300.0).abs() < 60.0, "nova tentativa em 5 min");
        assert!(!fakes.log().contains("install"));
        let fakes = Fakes::new("usr/local/bin", "0.160.0", "exit 0");
        let lines = Lines::new();
        fakes.update().run(&lines).await;
        assert_eq!(lines.all(), ["atualização automática do Codex indisponível nesta instalação"]);
    }

    /// `launcher__update-lock-busy`: outra sessão segura a trava; esta abre sem instalar.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_busy_lock_opens_on_the_installed_version() {
        let fakes = Fakes::new("node_modules/.bin", "0.160.0", "case \"$1\" in view) echo 0.161.0;; esac");
        std::fs::create_dir_all(fakes.dir.path().join(".hangar")).unwrap();
        let held = hangar_core::fs::lock_file(&fakes.dir.path().join(".hangar/codex-atualizacao.lock")).unwrap();
        let lines = Lines::new();
        fakes.update().run(&lines).await;
        assert_eq!(lines.all(), ["outra sessão está atualizando o Codex; esta abre na versão instalada"]);
        assert!(fakes.log().is_empty(), "nada consultado nem instalado");
        drop(held);
    }

    /// `launcher__update-standalone-newer` e `-update-rc0-no-change`: a casa do standalone, a versão
    /// conferida e o código 0 que só vale com a versão nova respondendo.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_official_updater_gets_the_standalone_home_and_must_change_the_version() {
        let fakes = Fakes::new(".codex/packages/standalone/releases/0.160.0", "0.160.0", "exit 1");
        let channel = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/latest", channel.local_addr().unwrap());
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            while let Ok((mut stream, _)) = channel.accept().await {
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).to_lowercase();
                assert!(request.contains("user-agent: hangar-codex-tui"), "{request}");
                let body = r#"{"tag_name":"rust-v0.161.0","assets":[]}"#;
                let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
            }
        });
        let mut update = fakes.update();
        update.latest = url;
        let lines = Lines::new();
        update.run(&lines).await;
        assert_eq!(lines.all(), ["conferindo a versão do Codex…", "atualizando o Codex 0.160.0 → 0.161.0…", "Codex atualizado para 0.161.0"]);
        let home = fakes.dir.path().join(".codex").to_string_lossy().into_owned();
        assert!(fakes.log().contains(&format!("codex update|{home}|0.161.0|unset")), "o link visível está dentro do standalone: {}", fakes.log());
        assert!(!fakes.log().contains("npm "), "standalone não passa pelo npm");
    }
}

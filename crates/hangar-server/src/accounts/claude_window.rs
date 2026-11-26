//! Janela escondida do multiplexador onde roda o `claude auth login` (login) ou o `claude` numa pasta
//! confiada (renovação) de uma conta Claude: criar, digitar, ler o link, colar o código e fechar.
//! A identidade é a operação (`term-login-<operação>`), nunca a conta: limpar a antiga não toca a nova.
use super::AccountKey;
use super::private_window::{PrivateWindows, ProcessWindowMux, WindowMux};
use crate::session_open::terminal::{TerminalEnv, exports_config_dir};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::LazyLock,
};

/// O que o login e a renovação pedem à janela; os testes de quem chama trocam por um falso.
pub trait WindowBackend: Send + Sync {
    /// `open`, `refresh`, `read` (devolve `url`), `code` e `close`. Erro = código para o log.
    fn call<'a>(&'a self, key: &'a AccountKey, operation: &'a str, action: &'a str, code: Option<&'a str>)
        -> BoxFuture<'a, Result<Value, &'static str>>;
    /// `open` com a janela nascendo com `BROWSER` apontando para o script da tentativa.
    fn open_with_browser<'a>(&'a self, key: &'a AccountKey, operation: &'a str, _script: &'a Path)
        -> BoxFuture<'a, Result<Value, &'static str>> {
        self.call(key, operation, "open", None)
    }
    /// Fecha as janelas de login deste servidor que nenhuma operação viva usa.
    fn sweep(&self) -> BoxFuture<'_, ()>;
}

const PREFIX: &str = "term-login-";
const PROMPT: &str = "paste code here if prompted";

#[derive(Default)]
struct Table {
    active: HashMap<String, AccountKey>,
    closed: HashSet<String>,
}

pub struct ClaudeWindows {
    win: PrivateWindows,
    env: TerminalEnv,
    /// `BROWSER` da janela: com ele a CLI não abre navegador na máquina do servidor.
    browser: Option<String>,
    /// Abrir, varrer e fechar não se cruzam.
    table: tokio::sync::Mutex<Table>,
}

pub fn valid_operation(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_code(code: &str) -> bool {
    !code.is_empty() && code.chars().count() <= 4096 && !code.contains(['\n', '\r', '\0'])
}

/// A CLI quebra o link na margem do pane, e o `capture-pane -J` do psmux não junta. Linha com a
/// largura exata do pane continua na seguinte, desde que esta não tenha espaço: continuação de link
/// nunca tem, e o texto seguinte da tela ("Paste code here…") tem.
pub fn join_wrapped(screen: &str, width: Option<usize>) -> String {
    let mut out = String::new();
    let mut lines = screen.lines().peekable();
    while let Some(line) = lines.next() {
        out.push_str(line);
        let full = width.is_some_and(|width| width > 0 && line.chars().count() == width);
        let continues = lines.peek().is_some_and(|next| !next.is_empty() && !next.contains(char::is_whitespace));
        if !(full && continues) {
            out.push('\n');
        }
    }
    out
}

/// O link de autorização da tela; sem ele, o último link, porque o que vem antes (saudação do shell,
/// aviso de versão) é anterior ao login. Para em espaço e no `ESC` do hiperlink OSC 8.
pub fn oauth_url(screen: &str) -> Option<String> {
    static URL: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"https?://[^\s\x1b]+").unwrap());
    let links: Vec<&str> = URL.find_iter(screen).map(|found| found.as_str()).collect();
    links.iter().find(|link| link.contains("/oauth/authorize")).or(links.last()).map(|link| (*link).to_owned())
}

/// Primeira pasta confiada e ainda existente da conta: abrir o `claude` noutra trava na pergunta de
/// confiança. O `true` é literal, como a CLI grava.
pub fn trusted_folder(config_file: &Path) -> Option<PathBuf> {
    let config: Value = serde_json::from_slice(&std::fs::read(config_file).ok()?).ok()?;
    config.get("projects")?.as_object()?.iter().find_map(|(path, entry)| {
        let path = PathBuf::from(path);
        (entry.get("hasTrustDialogAccepted") == Some(&Value::Bool(true)) && path.is_dir()).then_some(path)
    })
}

impl ClaudeWindows {
    pub fn new(mux: Box<dyn WindowMux>, env: TerminalEnv, browser: Option<String>, diary: Option<PathBuf>) -> Self {
        let owner = env.default_config_dir.to_string_lossy().into_owned();
        Self { win: PrivateWindows::new(mux, &env, PREFIX, owner, diary), env, browser, table: Default::default() }
    }

    pub async fn open(&self, key: &AccountKey, operation: &str) -> Result<(), &'static str> {
        self.start(key, operation, None, "claude auth login --claudeai", self.browser.as_deref()).await
    }

    pub async fn open_with_browser(&self, key: &AccountKey, operation: &str, script: &Path) -> Result<(), &'static str> {
        self.start(key, operation, None, "claude auth login --claudeai", Some(&script.to_string_lossy())).await
    }

    pub async fn refresh(&self, key: &AccountKey, operation: &str) -> Result<(), &'static str> {
        // A conta padrão no psmux nasce sem a variável, e aí a CLI lê o `.claude.json` da HOME.
        let home = key.canonical_home.to_string_lossy();
        let config_file = match self.env.default_config_dir.parent() {
            Some(user_home) if self.env.windows && exports_config_dir(&self.env, Some(&home)).is_none() => user_home.join(".claude.json"),
            _ => key.canonical_home.join(".claude.json"),
        };
        let folder = trusted_folder(&config_file).ok_or("window_no_trusted_folder")?;
        self.start(key, operation, Some(folder), "claude", self.browser.as_deref()).await
    }

    async fn start(&self, key: &AccountKey, operation: &str, folder: Option<PathBuf>, command: &str, browser: Option<&str>) -> Result<(), &'static str> {
        if !valid_operation(operation) { return Err("window_operation_invalid"); }
        let mut table = self.table.lock().await;
        if table.closed.contains(operation) { return Err("window_operation_closed"); }
        match table.active.get(operation) {
            Some(owner) if owner == key => return Ok(()),
            Some(_) => return Err("window_operation_diverged"),
            None => {}
        }
        self.sweep_locked(&table).await;
        let name = format!("{PREFIX}{operation}");
        let home = key.canonical_home.to_string_lossy().into_owned();
        let cwd = folder.map_or_else(|| home.clone(), |folder| folder.to_string_lossy().into_owned());
        self.spawn(&name, &cwd, &home, browser).await?;
        table.active.insert(operation.to_owned(), key.clone());
        if let Err(error) = self.win.type_line(&name, command).await {
            if self.win.kill(&name).await.is_err() { self.win.note("nao_fechou"); }
            table.active.remove(operation);
            return Err(error);
        }
        Ok(())
    }

    async fn spawn(&self, name: &str, cwd: &str, home: &str, browser: Option<&str>) -> Result<(), &'static str> {
        // O shell não tem comando onde pôr o `env -u`: no tmux a conta vai sempre no `-e`, senão a
        // credencial cairia na conta que o servidor tmux herdou. No psmux o pane herda de quem chama,
        // e a conta padrão é a ausência da variável.
        let config = exports_config_dir(&self.env, Some(home)).or_else(|| (!self.env.windows).then(|| home.to_owned()));
        let mut env = Vec::new();
        if let Some(dir) = config { env.push(("CLAUDE_CONFIG_DIR".to_owned(), dir)); }
        if let Some(browser) = browser { env.push(("BROWSER".to_owned(), browser.to_owned())); }
        // Larga o bastante para o link (~500 colunas) caber numa linha: juntar linhas quebradas não
        // distingue a última linha cheia do link da linha seguinte da tela.
        self.win.spawn(name, cwd, 1000, &env).await
    }

    /// A tela das últimas 200 linhas com o link juntado.
    async fn screen(&self, name: &str) -> Result<String, &'static str> {
        let width = self.win.run(&["display-message", "-p", "-t", &PrivateWindows::pane(name), "#{pane_width}"]).await;
        let width = (width.code == Some(0)).then(|| width.stdout.trim().parse().ok()).flatten();
        if width.is_none() {
            tracing::warn!(code = "window_width_unread", "janela de login: largura não lida; o link quebrado não é juntado");
        }
        let out = self.win.run(&["capture-pane", "-p", "-J", "-t", &PrivateWindows::pane(name), "-S", "-200"]).await;
        if out.stdout.is_empty() && out.code != Some(0) {
            tracing::warn!(code = "window_capture_failed", "janela de login: a tela não foi lida");
            return Err("window_capture_failed");
        }
        Ok(join_wrapped(&out.stdout, width))
    }

    fn require(table: &Table, key: &AccountKey, operation: &str) -> Result<(), &'static str> {
        if table.closed.contains(operation) { return Err("window_operation_closed"); }
        match table.active.get(operation) {
            Some(owner) if owner == key => Ok(()),
            _ => Err("window_operation_missing"),
        }
    }

    pub async fn read(&self, key: &AccountKey, operation: &str) -> Result<Option<String>, &'static str> {
        let table = self.table.lock().await;
        Self::require(&table, key, operation)?;
        let name = format!("{PREFIX}{operation}");
        match self.screen(&name).await {
            Ok(screen) => Ok(oauth_url(&screen)),
            // Janela que sumiu não terá link: o passo fecha a tentativa e solta a conta.
            Err(_) if self.win.alive(&name).await == Some(false) => Err("window_gone"),
            // Tela que não se lê com a janela de pé é "ainda sem link".
            Err(_) => Ok(None),
        }
    }

    /// O código entra por stdin num buffer nomeado, nunca no argv; o buffer sai mesmo na falha.
    pub async fn code(&self, key: &AccountKey, operation: &str, code: &str) -> Result<(), &'static str> {
        if !valid_code(code) { return Err("window_invalid_code"); }
        let table = self.table.lock().await;
        Self::require(&table, key, operation)?;
        let name = format!("{PREFIX}{operation}");
        if !self.screen(&name).await?.to_lowercase().contains(PROMPT) { return Err("window_cli_not_waiting"); }
        let buffer = format!("hangar-login-{}", crate::mods::state::random_hex(16));
        // O buffer do psmux é da sessão: sem `-t` ele iria para a última criada. O tmux recusa o `-t`.
        let of_session = |verb: &str, tail: &[&str]| -> Vec<String> {
            let session: &[&str] = if self.env.windows { &["-t", &name] } else { &[] };
            std::iter::once(verb).chain(session.iter().copied()).chain(tail.iter().copied()).map(str::to_owned).collect()
        };
        let loaded = self.win.run_input(of_session("load-buffer", &["-b", &buffer, "-"]), code.as_bytes().to_vec()).await;
        let result = if loaded.code != Some(0) {
            Err("window_buffer_failed")
        } else if self.win.run(&["paste-buffer", "-t", &PrivateWindows::pane(&name), "-b", &buffer, "-d"]).await.code != Some(0) || !self.win.enter(&name).await {
            Err("window_paste_failed")
        } else {
            Ok(())
        };
        let _ = self.win.run_input(of_session("delete-buffer", &["-b", &buffer]), Vec::new()).await;
        result
    }

    /// Encerra a operação antes de matar: nada mais lê nem cola nela. A janela que não morreu sai da
    /// tabela mesmo assim, e a varredura seguinte a derruba.
    pub async fn close(&self, key: &AccountKey, operation: &str) -> Result<(), &'static str> {
        if !valid_operation(operation) { return Err("window_operation_invalid"); }
        let mut table = self.table.lock().await;
        if table.active.get(operation).is_some_and(|owner| owner != key) { return Err("window_operation_diverged"); }
        table.closed.insert(operation.to_owned());
        let result = self.win.kill(&format!("{PREFIX}{operation}")).await;
        table.active.remove(operation);
        result
    }

    pub async fn sweep(&self) {
        let table = self.table.lock().await;
        self.sweep_locked(&table).await;
    }

    async fn sweep_locked(&self, table: &Table) {
        self.win.sweep(|operation| valid_operation(operation) && !table.active.contains_key(operation)).await;
    }
}

impl WindowBackend for ClaudeWindows {
    fn call<'a>(&'a self, key: &'a AccountKey, operation: &'a str, action: &'a str, code: Option<&'a str>)
        -> BoxFuture<'a, Result<Value, &'static str>> {
        Box::pin(async move {
            if (action == "code") != code.is_some() { return Err("window_unexpected_input"); }
            match action {
                "open" => self.open(key, operation).await.map(|()| json!({"ok": true})),
                "refresh" => self.refresh(key, operation).await.map(|()| json!({"ok": true})),
                "read" => self.read(key, operation).await.map(|url| json!({"ok": true, "url": url})),
                "code" => self.code(key, operation, code.unwrap_or_default()).await.map(|()| json!({"ok": true})),
                "close" => self.close(key, operation).await.map(|()| json!({"ok": true})),
                _ => Err("window_action_invalid"),
            }
        })
    }

    fn open_with_browser<'a>(&'a self, key: &'a AccountKey, operation: &'a str, script: &'a Path)
        -> BoxFuture<'a, Result<Value, &'static str>> {
        Box::pin(async move { ClaudeWindows::open_with_browser(self, key, operation, script).await.map(|()| json!({"ok": true})) })
    }

    fn sweep(&self) -> BoxFuture<'_, ()> { Box::pin(ClaudeWindows::sweep(self)) }
}

/// Programa que recebe o link e não abre nada: a tela do Hangar mostra o link e o botão. No Windows a
/// CLI roda o `BROWSER` no lugar do `rundll32`; o `hostname` recusa o argumento e sai sem janela.
const BROWSER: Option<&str> = Some(if cfg!(windows) { "hostname.exe" } else { "true" });

/// As janelas do multiplexador da máquina, no ambiente das contas. Sem as variáveis de tela do
/// `TerminalEnv::local`: o shell do login não as usa, e achá-las pode consultar o systemd.
pub fn local(accounts: &super::environment::AccountEnvironment) -> ClaudeWindows {
    let env = terminal_env(accounts);
    let diary = crate::diag::diary_dir_in(accounts);
    let mux = ProcessWindowMux::new(&env, diary.clone());
    ClaudeWindows::new(Box::new(mux), env, BROWSER.map(str::to_owned), diary)
}

/// O multiplexador da máquina no ambiente das contas, também o das janelas dos Termos do `agy`.
pub(super) fn terminal_env(accounts: &super::environment::AccountEnvironment) -> TerminalEnv {
    // Na forma da chave da conta: a pasta padrão escrita de outro jeito ganharia o `-e` no psmux.
    let default = accounts.home.join(".claude");
    let default_config_dir = AccountKey::new(super::Provider::Claude, &default).map_or(default, |key| key.canonical_home);
    TerminalEnv { windows: cfg!(windows), program: "tmux".into(), display: Vec::new(),
        server_config_dir: accounts.base.get("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()).cloned(),
        default_config_dir, birth: std::time::Duration::ZERO }
}

/// Janela que recusa tudo: teste de quem chama que não pode chegar ao multiplexador da máquina.
#[cfg(test)]
pub(crate) struct Refused;

#[cfg(test)]
impl WindowBackend for Refused {
    fn call<'a>(&'a self, _: &'a AccountKey, _: &'a str, _: &'a str, _: Option<&'a str>) -> BoxFuture<'a, Result<Value, &'static str>> {
        Box::pin(async { Err("window_test_refused") })
    }
    fn sweep(&self) -> BoxFuture<'_, ()> { Box::pin(async {}) }
}

#[cfg(test)]
mod tests;

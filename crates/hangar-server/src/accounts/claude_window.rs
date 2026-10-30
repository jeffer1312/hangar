//! Janela escondida do multiplexador onde roda o `claude auth login` (login) ou o `claude` numa pasta
//! confiada (renovação) de uma conta Claude: criar, digitar, ler o link, colar o código e fechar.
//! A identidade é a operação (`term-login-<operação>`), nunca a conta: limpar a antiga não toca a nova.
use super::AccountKey;
use crate::session_open::terminal::{Mux, MuxOutput, ProcessMux, TerminalEnv, exports_config_dir};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::LazyLock,
};

/// Quem roda o multiplexador; os testes trocam por um falso.
pub trait WindowMux: Send + Sync {
    /// Prefixo do escopo do systemd (`systemd-run … --`); vazio sem ele.
    fn scope(&self) -> BoxFuture<'_, Vec<String>>;
    /// `scope` vem antes do programa (só no `new-session`); `args` sem o programa; `stdin` vazio = sem entrada.
    fn run(&self, scope: Vec<String>, args: Vec<String>, stdin: Vec<u8>) -> BoxFuture<'_, MuxOutput>;
}

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
    mux: Box<dyn WindowMux>,
    env: TerminalEnv,
    /// Marca as janelas deste servidor: a varredura nunca derruba a de outro processo (um teste com
    /// outra HOME, outra instalação) que use o mesmo multiplexador.
    owner: String,
    /// `BROWSER` da janela: com ele a CLI não abre navegador na máquina do servidor.
    browser: Option<String>,
    diary: Option<PathBuf>,
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

/// O primeiro link `http(s)` da tela; para em espaço e no `ESC` do hiperlink OSC 8.
pub fn oauth_url(screen: &str) -> Option<String> {
    static URL: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"https?://[^\s\x1b]+").unwrap());
    URL.find(screen).map(|found| found.as_str().to_owned())
}

/// Primeira pasta confiada e ainda existente da conta: abrir o `claude` noutra trava na pergunta de
/// confiança. O `true` é literal, como a CLI grava.
pub fn trusted_folder(home: &Path) -> Option<PathBuf> {
    let config: Value = serde_json::from_slice(&std::fs::read(home.join(".claude.json")).ok()?).ok()?;
    config.get("projects")?.as_object()?.iter().find_map(|(path, entry)| {
        let path = PathBuf::from(path);
        (entry.get("hasTrustDialogAccepted") == Some(&Value::Bool(true)) && path.is_dir()).then_some(path)
    })
}

impl ClaudeWindows {
    pub fn new(mux: Box<dyn WindowMux>, env: TerminalEnv, browser: Option<String>, diary: Option<PathBuf>) -> Self {
        let owner = env.default_config_dir.to_string_lossy().into_owned();
        Self { mux, env, owner, browser, diary, table: Default::default() }
    }

    fn pane(name: &str) -> String { format!("={name}:") }

    async fn run(&self, args: &[&str]) -> MuxOutput {
        self.mux.run(Vec::new(), args.iter().map(|arg| (*arg).to_owned()).collect(), Vec::new()).await
    }

    fn note(&self, code: &'static str) {
        tracing::warn!(code, "janela de login: limpeza de janela esquecida falhou");
        if self.diary.is_some() {
            crate::diag::record(self.diary.clone(), vec![json!({"evento": "conta.janela_orfa", "nivel": "aviso", "codigo": code})], "contas");
        }
    }

    pub async fn open(&self, key: &AccountKey, operation: &str) -> Result<(), &'static str> {
        self.start(key, operation, None, "claude auth login --claudeai", self.browser.as_deref()).await
    }

    pub async fn open_with_browser(&self, key: &AccountKey, operation: &str, script: &Path) -> Result<(), &'static str> {
        self.start(key, operation, None, "claude auth login --claudeai", Some(&script.to_string_lossy())).await
    }

    pub async fn refresh(&self, key: &AccountKey, operation: &str) -> Result<(), &'static str> {
        let folder = trusted_folder(&key.canonical_home).ok_or("window_no_trusted_folder")?;
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
        if let Err(error) = self.type_line(&name, command).await {
            if self.kill(&name).await.is_err() { self.note("nao_fechou"); }
            table.active.remove(operation);
            return Err(error);
        }
        Ok(())
    }

    async fn spawn(&self, name: &str, cwd: &str, home: &str, browser: Option<&str>) -> Result<(), &'static str> {
        // Larga o bastante para o link (~500 colunas) caber numa linha: juntar linhas quebradas não
        // distingue a última linha cheia do link da linha seguinte da tela.
        let mut args: Vec<String> = ["new-session", "-d", "-s", name, "-c", cwd, "-x", "1000", "-y", "50"].map(str::to_owned).into();
        // O shell não tem comando onde pôr o `env -u`: no tmux a conta vai sempre no `-e`, senão a
        // credencial cairia na conta que o servidor tmux herdou. No psmux o pane herda de quem chama,
        // e a conta padrão é a ausência da variável.
        let config = exports_config_dir(&self.env, Some(home)).or_else(|| (!self.env.windows).then(|| home.to_owned()));
        if let Some(dir) = config { args.extend(["-e".into(), format!("CLAUDE_CONFIG_DIR={dir}")]); }
        if let Some(browser) = browser { args.extend(["-e".into(), format!("BROWSER={browser}")]); }
        let scope = if self.env.windows { Vec::new() } else { self.mux.scope().await };
        let scoped = !scope.is_empty();
        let mut created = self.mux.run(scope, args.clone(), Vec::new()).await;
        // `systemd-run` que não cria mais escopo: sobe sem ele, como a sessão.
        if scoped && created.code != Some(0) && created.stderr.trim_start().starts_with("Failed to ") {
            created = self.mux.run(Vec::new(), args, Vec::new()).await;
        }
        if created.code != Some(0) && self.run(&["has-session", "-t", &format!("={name}")]).await.code != Some(0) {
            let cause: String = created.stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("").chars().take(200).collect();
            tracing::warn!(code = "window_mux_refused", retorno = ?created.code, %cause, "janela de login não foi criada");
            return Err("window_mux_refused");
        }
        // Sem a marca a janela vira card de sessão nas listas.
        let hidden = self.run(&["set-option", "-t", &Self::pane(name), "@cp_hidden", "1"]).await;
        let owned = self.run(&["set-option", "-t", &Self::pane(name), "@cp_login_owner", &self.owner]).await;
        if hidden.code != Some(0) || owned.code != Some(0) {
            if self.kill(name).await.is_err() { self.note("nao_fechou"); }
            return Err("window_hide_failed");
        }
        Ok(())
    }

    async fn keys(&self, args: Vec<String>) -> MuxOutput {
        let out = self.mux.run(Vec::new(), args.clone(), Vec::new()).await;
        // Observador tmux somente de leitura herdado: o mesmo pane com cliente vazio passa.
        if self.env.windows || out.code == Some(0) || out.stderr.trim() != "client is read-only" { return out; }
        let mut retry = args;
        retry.splice(1..1, ["-c".to_owned(), String::new()]);
        self.mux.run(Vec::new(), retry, Vec::new()).await
    }

    async fn enter(&self, name: &str) -> bool {
        // Fora do Windows, CR cru: com `extended-keys` o nome `Enter` sai codificado. No psmux o CR
        // dentro de `-l` é engolido, e lá vale a tecla nomeada.
        let tail: &[&str] = if self.env.windows { &["Enter"] } else { &["-l", "--", "\r"] };
        let args = ["send-keys", "-t", &Self::pane(name)].iter().chain(tail).map(|arg| (*arg).to_owned()).collect();
        self.keys(args).await.code == Some(0)
    }

    async fn type_line(&self, name: &str, text: &str) -> Result<(), &'static str> {
        let typed = self.keys(["send-keys", "-t", &Self::pane(name), "-l", "--", text].map(str::to_owned).into()).await;
        if typed.code != Some(0) || !self.enter(name).await { return Err("window_type_failed"); }
        Ok(())
    }

    /// A tela das últimas 200 linhas com o link juntado.
    async fn screen(&self, name: &str) -> Result<String, &'static str> {
        let width = self.run(&["display-message", "-p", "-t", &Self::pane(name), "#{pane_width}"]).await;
        let width = (width.code == Some(0)).then(|| width.stdout.trim().parse().ok()).flatten();
        if width.is_none() {
            tracing::warn!(code = "window_width_unread", "janela de login: largura não lida; o link quebrado não é juntado");
        }
        let out = self.run(&["capture-pane", "-p", "-J", "-t", &Self::pane(name), "-S", "-200"]).await;
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
        // Tela que não se lê é "ainda sem link", como o passo sempre tratou.
        Ok(self.screen(&format!("{PREFIX}{operation}")).await.ok().and_then(|screen| oauth_url(&screen)))
    }

    /// O código entra por stdin num buffer nomeado, nunca no argv; o buffer sai mesmo na falha.
    pub async fn code(&self, key: &AccountKey, operation: &str, code: &str) -> Result<(), &'static str> {
        if !valid_code(code) { return Err("window_invalid_code"); }
        let table = self.table.lock().await;
        Self::require(&table, key, operation)?;
        let name = format!("{PREFIX}{operation}");
        if !self.screen(&name).await?.to_lowercase().contains(PROMPT) { return Err("window_cli_not_waiting"); }
        let buffer = format!("hangar-login-{}", crate::mods::state::random_hex(16));
        let loaded = self.mux.run(Vec::new(), ["load-buffer", "-b", &buffer, "-"].map(str::to_owned).into(), code.as_bytes().to_vec()).await;
        let result = if loaded.code != Some(0) {
            Err("window_buffer_failed")
        } else if self.run(&["paste-buffer", "-t", &Self::pane(&name), "-b", &buffer, "-d"]).await.code != Some(0) || !self.enter(&name).await {
            Err("window_paste_failed")
        } else {
            Ok(())
        };
        let _ = self.run(&["delete-buffer", "-b", &buffer]).await;
        result
    }

    /// Encerra a operação antes de matar: nada mais lê nem cola nela. A janela que não morreu sai da
    /// tabela mesmo assim, e a varredura seguinte a derruba.
    pub async fn close(&self, key: &AccountKey, operation: &str) -> Result<(), &'static str> {
        if !valid_operation(operation) { return Err("window_operation_invalid"); }
        let mut table = self.table.lock().await;
        if table.active.get(operation).is_some_and(|owner| owner != key) { return Err("window_operation_diverged"); }
        table.closed.insert(operation.to_owned());
        let result = self.kill(&format!("{PREFIX}{operation}")).await;
        table.active.remove(operation);
        result
    }

    /// Só a releitura prova: o psmux responde 0 ao `kill-session` com a sessão viva. No psmux o `=`
    /// não vale no `kill-session` e o nome cru já é exato.
    async fn kill(&self, name: &str) -> Result<(), &'static str> {
        let target = if self.env.windows { name.to_owned() } else { format!("={name}") };
        let _ = self.run(&["kill-session", "-t", &target]).await;
        match self.run(&["has-session", "-t", &format!("={name}")]).await.code {
            Some(0) => Err("window_still_open"),
            Some(1) => Ok(()),
            _ => Err("window_close_unconfirmed"),
        }
    }

    pub async fn sweep(&self) {
        let table = self.table.lock().await;
        self.sweep_locked(&table).await;
    }

    async fn sweep_locked(&self, table: &Table) {
        let listed = self.run(&["list-sessions", "-F", "#{session_name}"]).await;
        if listed.code != Some(0) {
            let stderr = listed.stderr.trim_start();
            let none = listed.code == Some(1) && ["no server", "no sessions", "error connecting to "].iter().any(|p| stderr.starts_with(p));
            if !none { self.note("lista_indisponivel"); }
            return;
        }
        for name in listed.stdout.lines().map(str::trim) {
            let Some(operation) = name.strip_prefix(PREFIX).filter(|op| valid_operation(op)) else { continue };
            if table.active.contains_key(operation) { continue; }
            let option = |option: &'static str| async move {
                let out = self.run(&["show-options", "-v", "-t", &Self::pane(name), option]).await;
                (out.code == Some(0)).then(|| out.stdout.trim().to_owned())
            };
            if option("@cp_hidden").await.as_deref() != Some("1") || option("@cp_login_owner").await.as_deref() != Some(self.owner.as_str()) {
                continue;
            }
            if self.kill(name).await.is_err() { self.note("nao_fechou"); }
        }
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

/// O multiplexador de verdade: `ProcessMux` sem entrada; com entrada, o mesmo cliente com stdin.
pub struct ProcessWindowMux { mux: ProcessMux, program: String }

impl WindowMux for ProcessWindowMux {
    fn scope(&self) -> BoxFuture<'_, Vec<String>> { self.mux.scope() }

    fn run(&self, scope: Vec<String>, args: Vec<String>, stdin: Vec<u8>) -> BoxFuture<'_, MuxOutput> {
        Box::pin(async move {
            if stdin.is_empty() {
                return self.mux.run(scope.into_iter().chain([self.program.clone()]).chain(args).collect()).await;
            }
            use tokio::io::AsyncWriteExt;
            let mut command = crate::session_open::terminal::client_command(&self.program);
            command.args(&args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped()).kill_on_drop(true);
            let run = async {
                let mut child = command.spawn()?;
                if let Some(mut input) = child.stdin.take() { input.write_all(&stdin).await?; }
                child.wait_with_output().await
            };
            match tokio::time::timeout(self.mux.timeout, run).await {
                Ok(Ok(out)) => MuxOutput { code: out.status.code(), stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned() },
                Ok(Err(error)) => MuxOutput { code: None, stdout: String::new(), stderr: error.to_string() },
                Err(_) => MuxOutput { code: None, stdout: String::new(), stderr: "o multiplexador não respondeu no prazo".into() },
            }
        })
    }
}

/// Programa que recebe o link e não abre nada: a tela do Hangar mostra o link e o botão. No Windows a
/// CLI roda o `BROWSER` no lugar do `rundll32`; o `hostname` recusa o argumento e sai sem janela.
const BROWSER: Option<&str> = Some(if cfg!(windows) { "hostname.exe" } else { "true" });

/// As janelas do multiplexador da máquina, no ambiente das contas. Sem as variáveis de tela do
/// `TerminalEnv::local`: o shell do login não as usa, e achá-las pode consultar o systemd.
pub fn local(accounts: &super::environment::AccountEnvironment) -> ClaudeWindows {
    // Na forma da chave da conta: a pasta padrão escrita de outro jeito ganharia o `-e` no psmux.
    let default = accounts.home.join(".claude");
    let default_config_dir = AccountKey::new(super::Provider::Claude, &default).map_or(default, |key| key.canonical_home);
    let env = TerminalEnv { windows: cfg!(windows), program: "tmux".into(), display: Vec::new(),
        server_config_dir: accounts.base.get("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()).cloned(),
        default_config_dir, birth: std::time::Duration::ZERO };
    let diary = crate::diag::diary_dir_in(accounts);
    let mux = ProcessWindowMux { mux: ProcessMux { diary: diary.clone(), ..ProcessMux::default() }, program: env.program.clone() };
    ClaudeWindows::new(Box::new(mux), env, BROWSER.map(str::to_owned), diary)
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

//! Janela escondida do multiplexador onde o servidor conduz um programa por conta da pessoa (login do Claude,
//! Termos do `agy`): nasce com as marcas antes de qualquer comando, recebe linha e tecla, é lida e fechada, e as
//! órfãs deste servidor saem na varredura. Cada uso tem prefixo de nome e dono próprios.
use crate::session_open::terminal::{Mux, MuxOutput, ProcessMux, TerminalEnv};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::{path::PathBuf, time::Duration};

const KILL_TRIES: usize = 3;
const KILL_PAUSE: Duration = Duration::from_millis(100);

/// Quem roda o multiplexador; os testes trocam por um falso.
pub trait WindowMux: Send + Sync {
    /// Prefixo do escopo do systemd (`systemd-run … --`); vazio sem ele.
    fn scope(&self) -> BoxFuture<'_, Vec<String>>;
    /// `scope` vem antes do programa (só no `new-session`); `args` sem o programa; `stdin` vazio = sem entrada.
    fn run(&self, scope: Vec<String>, args: Vec<String>, stdin: Vec<u8>) -> BoxFuture<'_, MuxOutput>;
}

pub struct PrivateWindows {
    mux: Box<dyn WindowMux>,
    windows: bool,
    prefix: &'static str,
    /// Marca as janelas deste servidor: a varredura nunca derruba a de outro processo (um teste com
    /// outra HOME, outra instalação) que use o mesmo multiplexador.
    owner: String,
    diary: Option<PathBuf>,
}

impl PrivateWindows {
    pub fn new(mux: Box<dyn WindowMux>, env: &TerminalEnv, prefix: &'static str, owner: String, diary: Option<PathBuf>) -> Self {
        Self { mux, windows: env.windows, prefix, owner, diary }
    }

    pub fn name(&self, id: &str) -> String { format!("{}{id}", self.prefix) }

    pub fn windows(&self) -> bool { self.windows }

    pub fn pane(name: &str) -> String { format!("={name}:") }

    pub async fn run(&self, args: &[&str]) -> MuxOutput {
        self.mux.run(Vec::new(), args.iter().map(|arg| (*arg).to_owned()).collect(), Vec::new()).await
    }

    pub async fn run_input(&self, args: Vec<String>, stdin: Vec<u8>) -> MuxOutput { self.mux.run(Vec::new(), args, stdin).await }

    pub fn note(&self, code: &'static str) {
        tracing::warn!(code, "janela escondida: limpeza de janela esquecida falhou");
        self.record("conta.janela_orfa", code);
    }

    /// Linha de aviso no diário de uso, quando há diário.
    pub fn record(&self, event: &'static str, code: &'static str) {
        if self.diary.is_some() {
            crate::diag::record(self.diary.clone(), vec![json!({"evento": event, "nivel": "aviso", "codigo": code})], "contas");
        }
    }

    /// `env` vai no `-e` do `new-session`.
    pub async fn spawn(&self, name: &str, cwd: &str, width: u16, env: &[(String, String)]) -> Result<(), &'static str> {
        let width = width.to_string();
        let mut args: Vec<String> = ["new-session", "-d", "-s", name, "-c", cwd, "-x", &width, "-y", "50"].map(str::to_owned).into();
        for (key, value) in env { args.extend(["-e".into(), format!("{key}={value}")]); }
        let scope = if self.windows { Vec::new() } else { self.mux.scope().await };
        let scoped = !scope.is_empty();
        let mut created = self.mux.run(scope, args.clone(), Vec::new()).await;
        // `systemd-run` que não cria mais escopo: sobe sem ele, como a sessão.
        if scoped && created.code != Some(0) && created.stderr.trim_start().starts_with("Failed to ") {
            created = self.mux.run(Vec::new(), args, Vec::new()).await;
        }
        if created.code != Some(0) && self.alive(name).await != Some(true) {
            let cause: String = created.stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or("").chars().take(200).collect();
            tracing::warn!(code = "window_mux_refused", retorno = ?created.code, %cause, "janela escondida não foi criada");
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
        if self.windows || out.code == Some(0) || out.stderr.trim() != "client is read-only" { return out; }
        let mut retry = args;
        retry.splice(1..1, ["-c".to_owned(), String::new()]);
        self.mux.run(Vec::new(), retry, Vec::new()).await
    }

    pub async fn enter(&self, name: &str) -> bool {
        // Fora do Windows, CR cru: com `extended-keys` o nome `Enter` sai codificado. No psmux o CR
        // dentro de `-l` é engolido, e lá vale a tecla nomeada.
        let tail: &[&str] = if self.windows { &["Enter"] } else { &["-l", "--", "\r"] };
        let args = ["send-keys", "-t", &Self::pane(name)].iter().chain(tail).map(|arg| (*arg).to_owned()).collect();
        self.keys(args).await.code == Some(0)
    }

    /// Tecla nomeada (`Up`, `Down`…); o Enter vai por `enter`.
    pub async fn key(&self, name: &str, key: &str) -> bool {
        self.keys(["send-keys", "-t", &Self::pane(name), key].map(str::to_owned).into()).await.code == Some(0)
    }

    pub async fn type_line(&self, name: &str, text: &str) -> Result<(), &'static str> {
        let typed = self.keys(["send-keys", "-t", &Self::pane(name), "-l", "--", text].map(str::to_owned).into()).await;
        if typed.code != Some(0) || !self.enter(name).await { return Err("window_type_failed"); }
        Ok(())
    }

    /// A tela visível. Decide pela saída, não pelo código: texto é a tela com qualquer código; sem texto, código 0 é
    /// tela vazia e o resto passa pelo `has-session`. `Ok(None)`: não se leu com a janela de pé.
    pub async fn capture(&self, name: &str) -> Result<Option<String>, &'static str> {
        let out = self.run(&["capture-pane", "-p", "-t", &Self::pane(name)]).await;
        if !out.stdout.is_empty() || out.code == Some(0) { return Ok(Some(out.stdout)); }
        match self.alive(name).await {
            Some(false) => Err("window_gone"),
            _ => Ok(None),
        }
    }

    /// `Some(true)` de pé, `Some(false)` sumiu, `None` o multiplexador não respondeu.
    pub async fn alive(&self, name: &str) -> Option<bool> {
        match self.run(&["has-session", "-t", &format!("={name}")]).await.code {
            Some(0) => Some(true),
            Some(1) => Some(false),
            _ => None,
        }
    }

    /// Só a releitura prova: o psmux responde 0 ao `kill-session` com a sessão viva, e a janela que não caiu
    /// recebe o pedido de novo. No psmux o `=` não vale no `kill-session` e o nome cru já é exato.
    pub async fn kill(&self, name: &str) -> Result<(), &'static str> {
        let mut result = Err("window_still_open");
        for attempt in 0..KILL_TRIES {
            if attempt > 0 { tokio::time::sleep(KILL_PAUSE).await; }
            let _ = self.run(&["kill-session", "-t", &crate::session_open::terminal::target(self.windows, name)]).await;
            result = match self.alive(name).await {
                Some(true) => Err("window_still_open"),
                Some(false) => return Ok(()),
                None => Err("window_close_unconfirmed"),
            };
        }
        result
    }

    /// Fecha as janelas escondidas deste servidor com o prefixo deste uso cujo id `orphan` aceita. `false`: alguma
    /// continuou aberta, ou a lista não veio.
    pub async fn sweep(&self, orphan: impl Fn(&str) -> bool) -> bool {
        let listed = self.run(&["list-sessions", "-F", "#{session_name}"]).await;
        if listed.code != Some(0) {
            let none = listed.code == Some(1) && hangar_core::terminal_process::mux_absence(listed.stderr.trim_start());
            if !none { self.note("lista_indisponivel"); }
            return none;
        }
        let mut closed = true;
        for name in listed.stdout.lines().map(str::trim) {
            if !name.strip_prefix(self.prefix).is_some_and(&orphan) { continue; }
            let option = |option: &'static str| async move {
                let out = self.run(&["show-options", "-v", "-t", &Self::pane(name), option]).await;
                (out.code == Some(0)).then(|| out.stdout.trim().to_owned())
            };
            if option("@cp_hidden").await.as_deref() != Some("1") || option("@cp_login_owner").await.as_deref() != Some(self.owner.as_str()) {
                continue;
            }
            if self.kill(name).await.is_err() {
                self.note("nao_fechou");
                closed = false;
            }
        }
        closed
    }
}

/// O multiplexador de verdade: `ProcessMux` sem entrada; com entrada, o mesmo cliente com stdin.
pub struct ProcessWindowMux { mux: ProcessMux, program: String }

impl ProcessWindowMux {
    pub fn new(env: &TerminalEnv, diary: Option<PathBuf>) -> Self {
        Self { mux: ProcessMux { diary, ..ProcessMux::default() }, program: env.program.clone() }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// O multiplexador que dá a mesma resposta a tudo.
    struct Answer(MuxOutput);

    impl WindowMux for Answer {
        fn scope(&self) -> BoxFuture<'_, Vec<String>> { Box::pin(async { Vec::new() }) }
        fn run(&self, _scope: Vec<String>, _args: Vec<String>, _stdin: Vec<u8>) -> BoxFuture<'_, MuxOutput> {
            let out = self.0.clone();
            Box::pin(async move { out })
        }
    }

    fn sweep_with(stderr: &str) -> bool {
        let env = TerminalEnv { windows: true, program: "tmux".into(), display: Vec::new(), server_config_dir: None,
            default_config_dir: PathBuf::from("/fixture/.claude"), birth: Duration::ZERO };
        let out = MuxOutput { code: Some(1), stdout: String::new(), stderr: stderr.into() };
        let win = PrivateWindows::new(Box::new(Answer(out)), &env, "term-x-", "owner".into(), None);
        futures_util::FutureExt::now_or_never(win.sweep(|_| true)).expect("o falso responde na hora")
    }

    #[test]
    fn sweep_reads_the_psmux_without_a_server_as_no_window() {
        assert!(sweep_with("psmux: no server running on session 'default'\n"));
        assert!(sweep_with("no server running on /tmp/tmux-0/default\n"));
        assert!(!sweep_with("psmux: unknown command\n"), "recusa desconhecida não é lista vazia");
    }
}

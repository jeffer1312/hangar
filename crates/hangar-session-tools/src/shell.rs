//! Shell separado do painel: a marca no multiplexador comprova que ele pertence ao Hangar.
use std::sync::Arc;

use futures_util::future::BoxFuture;
use serde_json::{Value, json};

#[derive(Clone, Debug, Default)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub trait Mux: Send + Sync {
    fn run(&self, args: Vec<String>) -> BoxFuture<'_, Output>;
    fn scope(&self) -> BoxFuture<'_, Vec<String>>;
    fn warning(&self, message: &str);
}

pub struct Service {
    pub mux: Arc<dyn Mux>,
    pub program: String,
    pub windows: bool,
}

impl Service {
    pub async fn open(&self, name: &str, cwd: &str) -> Result<Value, crate::Error> {
        let target = format!("term-{name}");
        if self.has(&target).await && !self.hidden(&target).await {
            return Err(crate::Error {
                status: 409,
                code: "erro_sessao_tmux_em_uso",
                msg: format!("ja existe uma sessao tmux chamada '{target}' sem a marca do painel -- pode ser uma sessao sua de mesmo nome, ou o shell deste painel que perdeu a marca. Encerre ou renomeie essa sessao antes de abrir o shell"),
                params: json!({"nome": target}),
            });
        }
        let mut created = false;
        if self.has(&target).await {
            let output = self.run(["display", "-p", "-t", &format!("={target}:"), "#{session_path}"]).await;
            let old = if output.code == Some(0) { output.stdout.trim() } else { "" };
            if output.code != Some(0) {
                self.mux.warning(&format!("não consegui ler o diretório do shell escondido {target:?}; reaproveito sem conferir se ele é de {cwd:?}"));
            }
            if !old.is_empty() && normalize(old, self.windows) != normalize(cwd, self.windows)
                && self.hidden(&target).await && !self.kill(&target).await {
                self.mux.warning(&format!("shell escondido {target:?} não saiu; não reaproveito o diretório antigo {old:?}"));
                return Err(failed());
            }
        }
        if !self.has(&target).await {
            let mut args = self.mux.scope().await;
            args.extend([self.program.clone(), "new-session".into(), "-d".into(), "-s".into(), target.clone(), "-c".into(), cwd.into()]);
            if self.mux.run(args).await.code != Some(0) {
                return if self.has(&target).await { Ok(json!({"ok": true, "shell": target})) } else { Err(failed()) };
            }
            created = true;
        }
        if self.run(["set-option", "-t", &format!("={target}:"), "@cp_hidden", "1"]).await.code != Some(0) {
            // Uma falha ao remarcar não pode matar um shell que já tinha trabalho em execução.
            if created { self.kill(&target).await; }
            return Err(failed());
        }
        Ok(json!({"ok": true, "shell": target}))
    }

    async fn run<const N: usize>(&self, args: [&str; N]) -> Output {
        let mut argv = vec![self.program.clone()];
        argv.extend(args.into_iter().map(str::to_owned));
        self.mux.run(argv).await
    }

    async fn has(&self, target: &str) -> bool {
        self.run(["has-session", "-t", &format!("={target}")]).await.code == Some(0)
    }

    async fn hidden(&self, target: &str) -> bool {
        let out = self.run(["show-options", "-v", "-t", &format!("={target}:"), "@cp_hidden"]).await;
        out.code == Some(0) && out.stdout.trim() == "1"
    }

    async fn kill(&self, target: &str) -> bool {
        let target_arg = if self.windows { target.into() } else { format!("={target}") };
        self.run(["kill-session", "-t", &target_arg]).await;
        !self.has(target).await
    }
}

fn failed() -> crate::Error {
    crate::Error { status: 500, code: "erro_shell_criacao_falhou", msg: "tmux recusou criar o shell".into(), params: json!({}) }
}

// Compara a pasta de nascimento sem consultar disco nem seguir symlinks, como os.path.normpath.
fn normalize(path: &str, windows: bool) -> String {
    let path = if windows { path.replace('/', "\\") } else { path.to_owned() };
    let separator = if windows { '\\' } else { '/' };
    let (prefix, rest) = if windows && path.starts_with("\\\\") {
        let host_end = path[2..].find('\\').map(|i| i + 2).unwrap_or(path.len());
        let share_end = path[host_end..].strip_prefix('\\').and_then(|v| v.find('\\')).map(|i| host_end + 1 + i).unwrap_or(path.len());
        (&path[..share_end], &path[share_end..])
    } else if windows && path.as_bytes().get(1) == Some(&b':') {
        (&path[..2], &path[2..])
    } else { ("", path.as_str()) };
    let root = if rest.starts_with(separator) {
        if !windows && rest.starts_with("//") && !rest.starts_with("///") { "//".to_owned() } else { separator.to_string() }
    } else { String::new() };
    let mut components = Vec::new();
    for part in rest.split(separator) {
        match part {
            "" | "." => {},
            ".." if components.last().is_some_and(|last| *last != "..") => { components.pop(); },
            ".." if !root.is_empty() || windows && prefix.starts_with("\\\\") => {},
            _ => components.push(part),
        }
    }
    let joined = components.join(&separator.to_string());
    let result = format!("{prefix}{root}{joined}");
    if result.is_empty() { ".".into() } else { result }
}

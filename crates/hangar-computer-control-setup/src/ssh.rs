//! SSH: hosts do `~/.ssh/config`, teste de conexão e o texto para preparar um Windows.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};

use hangar_core::pycompat::py;

use crate::targets::check_host;
use crate::{Error, Paths};

const NO_SSH: &str = "ssh não encontrado neste servidor";
const TEST_DEADLINE: Duration = Duration::from_secs(20);
const RESOLVE_DEADLINE: Duration = Duration::from_secs(10);

static HOST_LINE: LazyLock<Regex> = LazyLock::new(|| py::py_re(r"(?i)^\s*Host\s+(.+)$"));

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

enum RunError {
    Missing,
    Timeout,
    Other(std::io::Error),
}

/// Os `Host` do `~/.ssh/config`, sem coringas nem repetição: o que a pessoa já usa em `ssh <host>`.
pub fn ssh_hosts(paths: &Paths) -> Vec<String> {
    let Ok(bytes) = std::fs::read(paths.ssh_config()) else { return Vec::new() };
    hosts_of(&String::from_utf8_lossy(&bytes))
}

/// Entra por SSH sem senha e roda `echo ok`, como o agente vai precisar. `ssh` = o do PATH do servidor.
pub async fn test_host(host: &str, proxy_command: &str, ssh: Option<&Path>, env: &BTreeMap<String, String>) -> Result<Value, Error> {
    let host = check_host(host)?;
    let Some(ssh) = ssh else { return Ok(json!({"ok": false, "detail": NO_SSH})) };
    // accept-new: máquina nova não tem host key conhecida; chave que MUDOU continua recusada.
    let mut args: Vec<String> = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "-o", "StrictHostKeyChecking=accept-new"]
        .map(String::from).into();
    let proxy = py::strip(proxy_command);
    if !proxy.is_empty() {
        args.extend(["-o".into(), format!("ProxyCommand={proxy}")]);
    }
    args.extend(["--".into(), host, "echo".into(), "ok".into()]);
    let out = match run(ssh, &args, env, TEST_DEADLINE).await {
        Ok(out) => out,
        Err(RunError::Missing) => return Ok(json!({"ok": false, "detail": NO_SSH})),
        Err(RunError::Timeout) => return Ok(json!({"ok": false, "detail": "sem resposta em 20 s"})),
        Err(RunError::Other(error)) => return Err(Error::at(ssh, &error)),
    };
    let ok = out.code == 0 && out.stdout.contains("ok");
    let detail = if ok {
        String::new()
    } else {
        let tail = tail(if out.stderr.is_empty() { &out.stdout } else { &out.stderr });
        if tail.is_empty() { format!("saiu com {}", out.code) } else { tail }
    };
    Ok(json!({"ok": ok, "detail": detail}))
}

/// Os últimos 400 code points da saída aparada.
fn tail(text: &str) -> String {
    let text = py::strip(text);
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(400)).collect()
}

/// Texto para colar num agente rodando no Windows; host vazio vira `novo-windows`.
pub async fn windows_setup(paths: &Paths, host: &str, ssh: Option<&Path>, env: &BTreeMap<String, String>) -> Result<Value, Error> {
    let host = if py::strip(host).is_empty() { "novo-windows".to_owned() } else { check_host(host)? };
    let explicit = host.split_once('@').map_or("", |(user, _)| user).to_owned();
    let (resolved, key) = identity(paths, &host, ssh, env).await?;
    if key.is_empty() {
        return Err(Error::plain(400, "erro_computer_control_no_ssh_key",
            "esta máquina não tem chave SSH: crie uma com ssh-keygen -t ed25519"));
    }
    let user = if explicit.is_empty() { resolved } else { explicit };
    let user_line = if user.is_empty() {
        "que vai entrar por SSH (me diga qual é)".to_owned()
    } else {
        format!("{user} (é o usuário com que o Hangar vai entrar; se ele não existir aqui, use o administrador logado e me diga qual é)")
    };
    Ok(json!({"prompt": prompt(&user_line, &key), "user": user}))
}

fn hosts_of(text: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for line in py::splitlines(text) {
        let Some(found) = HOST_LINE.captures(line) else { continue };
        for host in found[1].split(py::is_space).filter(|h| !h.is_empty() && !h.contains(['*', '?', '!'])) {
            if !hosts.iter().any(|known| known == host) {
                hosts.push(host.to_owned());
            }
        }
    }
    hosts
}

/// (usuário, chave pública) que o `ssh` usaria para o host, pelo próprio `ssh -G`. Sem `ssh` não há
/// chave; `ssh -G` que não responde ou que falha é erro com código, nunca "sem chave".
async fn identity(paths: &Paths, host: &str, ssh: Option<&Path>, env: &BTreeMap<String, String>) -> Result<(String, String), Error> {
    let Some(ssh) = ssh else { return Ok(Default::default()) };
    let args = ["-G".to_owned(), "--".to_owned(), host.to_owned()];
    let out = match run(ssh, &args, env, RESOLVE_DEADLINE).await {
        Ok(out) => out,
        Err(RunError::Missing) => return Ok(Default::default()),
        Err(RunError::Timeout) => {
            return Err(Error {
                status: 504,
                code: "erro_computer_control_ssh_timeout",
                msg: format!("ssh -G {host} não respondeu em {} s", RESOLVE_DEADLINE.as_secs()),
                params: json!({"host": host}),
            });
        }
        Err(RunError::Other(error)) => return Err(Error::at(ssh, &error)),
    };
    if out.code != 0 {
        let detail = tail(if out.stderr.is_empty() { &out.stdout } else { &out.stderr });
        return Err(Error {
            status: 500,
            code: "erro_computer_control_ssh_resolve",
            msg: format!("ssh -G {host} saiu com {}: {detail}", out.code),
            params: json!({"host": host, "code": out.code, "error": detail}),
        });
    }
    let home = paths.home.clone();
    tokio::task::spawn_blocking(move || identity_of(&out.stdout, &home)).await.map_err(Error::io)?
}

/// Primeira linha `user`; primeira `identityfile` cujo `.pub` existe. Lê disco: fora do executor.
fn identity_of(stdout: &str, home: &Path) -> Result<(String, String), Error> {
    let (mut user, mut key) = (String::new(), String::new());
    for line in py::splitlines(stdout) {
        let (name, value) = line.split_once(' ').unwrap_or((line, ""));
        if name == "user" && user.is_empty() {
            user = py::strip(value).to_owned();
        } else if name == "identityfile" && key.is_empty() {
            let public = format!("{}.pub", expand_user(py::strip(value), home));
            let public = Path::new(&public);
            if public.is_file() {
                let bytes = std::fs::read(public).map_err(|e| Error::at(public, &e))?;
                key = py::strip(&String::from_utf8_lossy(&bytes)).to_owned();
            }
        }
    }
    Ok((user, key))
}

/// `os.path.expanduser` para `~` e `~/…`. shortcut: `~outro/…` fica literal; vale olhar o passwd se aparecer.
fn expand_user(path: &str, home: &Path) -> String {
    let rest = path.strip_prefix('~').filter(|rest| rest.is_empty() || rest.starts_with('/') || (cfg!(windows) && rest.starts_with('\\')));
    match rest {
        Some(rest) => format!("{}{rest}", home.to_string_lossy().trim_end_matches(['/', '\\'])),
        None => path.to_owned(),
    }
}

async fn run(program: &Path, args: &[String], env: &BTreeMap<String, String>, deadline: Duration) -> Result<Output, RunError> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).env_clear()
        // A autoridade do servidor não vai ao ssh nem ao ProxyCommand dele.
        .envs(env.iter().filter(|(k, _)| !hangar_core::terminal_process::PRIVATE_ENV_KEYS.contains(&k.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Grupo próprio: no prazo cai o ssh e o que ele abriu (ProxyCommand e afins), não só ele.
    #[cfg(unix)]
    let mut tree = hangar_core::terminal_process::CommandTree::configure(&mut command).map_err(RunError::Other)?;
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(RunError::Missing),
        Err(error) => return Err(RunError::Other(error)),
    };
    #[cfg(unix)]
    tree.attach(&child).map_err(RunError::Other)?;
    match tokio::time::timeout(deadline, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(Output { code: hangar_core::terminal_process::exit_code(output.status).unwrap_or(-1), stdout: text(&output.stdout), stderr: text(&output.stderr) }),
        Ok(Err(error)) => Err(RunError::Other(error)),
        Err(_) => {
            #[cfg(unix)]
            hangar_core::terminal_process::finish(&mut tree).await;
            Err(RunError::Timeout)
        }
    }
}

/// `text=True, errors="replace"`: bytes inválidos trocados e quebras universais.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n").replace('\r', "\n")
}

fn prompt(user_line: &str, key: &str) -> String {
    format!("Configure este Windows para ser controlado pelo Hangar (MCP hangar-computer-control) a partir de outra máquina, por SSH com chave. Faça na ordem, confira cada passo e pare pra me perguntar se algo não bater.

1. Rode tudo num PowerShell elevado (como Administrador). Se não estiver elevado, me peça pra abrir um.
2. OpenSSH Server instalado, ligado e iniciando sozinho:
   Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
   Set-Service -Name sshd -StartupType Automatic; Start-Service sshd
3. Porta 22 liberada no firewall, se ainda não houver regra de entrada pra ela:
   New-NetFirewallRule -Name OpenSSH-Server-In-TCP -DisplayName 'OpenSSH Server (sshd)' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
4. Autorize esta chave pública pro usuário {user_line}:
   {key}
   - Usuário administrador: a chave vai em C:\\ProgramData\\ssh\\administrators_authorized_keys, com permissão só pra Administradores e SYSTEM (use os SIDs, que valem em Windows de qualquer idioma):
     icacls.exe C:\\ProgramData\\ssh\\administrators_authorized_keys /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F'
   - Usuário comum: em C:\\Users\\<usuário>\\.ssh\\authorized_keys.
   Acrescente sem apagar as chaves que já estiverem lá.
5. Esse usuário precisa ser administrador (o agente sobe como tarefa agendada elevada) e ficar logado numa sessão gráfica ATIVA: tela desbloqueada e sessão RDP não desconectada. Pra deixar a sessão ativa sem cliente RDP: tscon <id> /dest:console.
6. Confira: o serviço sshd está Running, a regra de firewall existe e o arquivo de chaves contém a chave acima.

Não mude outras configurações de segurança além dessas. No fim, me diga o nome desta máquina na rede (ou o IP) e o usuário, pra eu cadastrar no Hangar.")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn ssh_config_reader_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let home = tempfile::tempdir().unwrap();
            let paths = Paths::new(home.path().to_owned(), vec![]);
            std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
            std::fs::write(paths.ssh_config(), &bytes).unwrap();
            let hosts = ssh_hosts(&paths);
            prop_assert!(hosts.iter().all(|h| !h.is_empty() && !h.contains(['*', '?', '!'])));
        }

        #[test]
        fn ssh_config_text_reader_never_panics(text in "(?s)\\PC{0,200}") {
            let _ = hosts_of(&format!("Host {text}\nhost a {text}"));
        }
    }
}

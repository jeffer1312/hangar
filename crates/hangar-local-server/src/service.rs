//! O serviço que sobe o servidor: unit do systemd do usuário, tarefa `hangar-backend` do Windows ou
//! agente do launchd. Lido pelo gerenciador, nunca pela porta.
use crate::host::Host;

pub const UNIT: &str = "hangar-backend.service";
pub const TASK: &str = "hangar-backend";
pub const LAUNCHD_LABEL: &str = "br.dev.hangar.backend";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
    Macos,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "linux") { Os::Linux } else if cfg!(windows) { Os::Windows } else if cfg!(target_os = "macos") { Os::Macos } else { Os::Other }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    /// Unit, tarefa ou agente não existe.
    Missing,
    /// O gerenciador diz que o processo está de pé.
    Running,
    /// Iniciando, ainda sem o processo pronto.
    Starting,
    /// Caiu e espera para religar (`auto-restart` do systemd, espera do lançador no Windows).
    Restarting,
    Stopped,
    Failed,
    /// O gerenciador não respondeu (barramento do usuário fora, PowerShell que não abriu).
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub state: ServiceState,
    /// Código de saída da última vida, quando o gerenciador o guarda.
    pub exit_status: Option<i64>,
    /// A leitura crua numa linha, para o retrato da falha (`unit: …`, `tarefa: …`, `launchd: …`).
    pub summary: String,
}

impl Service {
    fn unreadable(summary: String) -> Service { Service { state: ServiceState::Unreadable, exit_status: None, summary } }
}

pub fn read(host: &dyn Host, os: Os) -> Service {
    match os {
        Os::Linux => {
            let out = host.run("systemctl", &["--user", "show", UNIT, "--no-pager", "-p", "LoadState", "-p", "ActiveState", "-p", "SubState",
                "-p", "MainPID", "-p", "ExecMainStatus", "-p", "ExecMainCode", "-p", "NRestarts", "-p", "ExecMainStartTimestamp"]);
            match out {
                Ok(out) if out.code == Some(0) => parse_systemd(&out.stdout),
                Ok(out) => Service::unreadable(format!("unit: systemctl saiu com {:?}: {}", out.code, one_line(&format!("{}{}", out.stdout, out.stderr)))),
                Err(e) => Service::unreadable(format!("unit: systemctl não rodou: {e}")),
            }
        }
        Os::Windows => {
            // Sem acento nem aspas duplas: o texto volta na codepage do console.
            let script = concat!(
                "$n='hangar-backend'; $t=Get-ScheduledTask -TaskName $n -EA SilentlyContinue; ",
                "if(-not $t){'tarefa: ausente'; exit 0}; $i=Get-ScheduledTaskInfo -TaskName $n -EA SilentlyContinue; ",
                "if($i){$r=('{0:X8}' -f $i.LastTaskResult); $w=$i.LastRunTime.ToString('s')} else {$r='?'; $w='?'}; ",
                "'tarefa: estado=' + $t.State + ' ultima_execucao=' + $w + ' ultimo_resultado=0x' + $r + ' acao=' + $t.Actions[0].Execute + ' ' + $t.Actions[0].Arguments");
            match powershell(host, script) {
                Ok(out) if out.code == Some(0) => parse_task(&out.stdout),
                Ok(out) => Service::unreadable(format!("tarefa: PowerShell saiu com {:?}: {}", out.code, one_line(&format!("{}{}", out.stdout, out.stderr)))),
                Err(e) => Service::unreadable(format!("tarefa: PowerShell não rodou: {e}")),
            }
        }
        Os::Macos => {
            let Some(target) = launchd_target(host) else { return Service::unreadable("launchd: uid ilegível".to_owned()) };
            match host.run("launchctl", &["print", &target]) {
                Ok(out) if out.code == Some(0) => parse_launchd(&out.stdout),
                // 113: o launchd não conhece o serviço.
                Ok(out) if out.code == Some(113) || out.stdout.contains("Could not find service") || out.stderr.contains("Could not find service") =>
                    Service { state: ServiceState::Missing, exit_status: None, summary: format!("launchd: {target} não carregado") },
                Ok(out) => Service::unreadable(format!("launchd: launchctl saiu com {:?}: {}", out.code, one_line(&format!("{}{}", out.stdout, out.stderr)))),
                Err(e) => Service::unreadable(format!("launchd: launchctl não rodou: {e}")),
            }
        }
        Os::Other => Service::unreadable("serviço: sistema sem gerenciador conhecido".to_owned()),
    }
}

/// `gui/<uid>/br.dev.hangar.backend`.
pub fn launchd_target(host: &dyn Host) -> Option<String> {
    let out = host.run("id", &["-u"]).ok().filter(|out| out.code == Some(0))?;
    let uid: u32 = out.stdout.trim().parse().ok()?;
    Some(format!("gui/{uid}/{LAUNCHD_LABEL}"))
}

/// `-EncodedCommand`: o script vai inteiro, sem as aspas do argv do Windows no meio.
pub fn powershell(host: &dyn Host, script: &str) -> std::io::Result<crate::host::Output> {
    let encoded: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    host.run("powershell", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &base64(&encoded)])
}

/// O script de um `powershell` que lê o código de saída: erro de cmdlet vira `exit 1` com a linha.
pub fn guarded(script: &str) -> String {
    format!("try {{ {script} }} catch {{ Write-Output ($_ | Out-String); exit 1 }}")
}

fn one_line(text: &str) -> String {
    text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" | ")
}

pub fn parse_systemd(text: &str) -> Service {
    let field = |key: &str| text.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('=')).map(str::trim).unwrap_or("");
    let (load, active, sub) = (field("LoadState"), field("ActiveState"), field("SubState"));
    let state = match (load, active, sub) {
        ("not-found", _, _) => ServiceState::Missing,
        (_, "active" | "reloading", _) => ServiceState::Running,
        (_, "activating", "auto-restart") => ServiceState::Restarting,
        (_, "activating", _) => ServiceState::Starting,
        (_, "failed", _) => ServiceState::Failed,
        (_, "inactive" | "deactivating", _) => ServiceState::Stopped,
        _ => ServiceState::Unreadable,
    };
    let exit_status = field("ExecMainStatus").parse::<i64>().ok().filter(|code| *code != 0);
    let summary = format!("unit: {}", text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "));
    Service { state, exit_status, summary }
}

pub fn parse_task(text: &str) -> Service {
    let line = text.lines().map(str::trim).find(|l| l.starts_with("tarefa:")).unwrap_or("").to_owned();
    if line == "tarefa: ausente" {
        return Service { state: ServiceState::Missing, exit_status: None, summary: format!("tarefa: {TASK} não registrada") };
    }
    let word = |key: &str| line.split_whitespace().find_map(|w| w.strip_prefix(key)).unwrap_or("");
    let state = match word("estado=") {
        "Running" => ServiceState::Running,
        "Queued" => ServiceState::Starting,
        "Ready" | "Disabled" => ServiceState::Stopped,
        _ => ServiceState::Unreadable,
    };
    // `LastTaskResult` é um HRESULT: 0x00041301 é "rodando", 0x00041303 "nunca rodou".
    let exit_status = word("ultimo_resultado=0x").get(..8).and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .filter(|code| !matches!(code, 0 | 0x0004_1301 | 0x0004_1303)).map(i64::from);
    if line.is_empty() { return Service::unreadable(format!("tarefa: resposta sem estado: {}", one_line(text))); }
    Service { state, exit_status, summary: line }
}

pub fn parse_launchd(text: &str) -> Service {
    let field = |key: &str| text.lines().map(str::trim).find_map(|l| l.strip_prefix(key)?.trim_start().strip_prefix('=')).map(str::trim).unwrap_or("");
    let state = match field("state") {
        "running" => ServiceState::Running,
        "spawn scheduled" => ServiceState::Restarting,
        "" => ServiceState::Unreadable,
        _ => ServiceState::Stopped,
    };
    let exit_status = field("last exit code").parse::<i64>().ok().filter(|code| *code != 0);
    Service { state, exit_status, summary: format!("launchd: state={} last_exit={}", field("state"), field("last exit code")) }
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= chunk.len() { TABLE[(n >> shift) as usize & 63] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_examples() {
        for (plain, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
    }

    #[test]
    fn systemd_states_and_exit_code() {
        let unit = |active: &str, sub: &str, status: &str| parse_systemd(&format!("LoadState=loaded\nActiveState={active}\nSubState={sub}\nMainPID=0\nExecMainStatus={status}\n"));
        assert_eq!(unit("active", "running", "0").state, ServiceState::Running);
        assert_eq!(unit("activating", "start", "0").state, ServiceState::Starting);
        assert_eq!(unit("activating", "auto-restart", "3").state, ServiceState::Restarting);
        assert_eq!(unit("failed", "failed", "3").exit_status, Some(3));
        assert_eq!(unit("inactive", "dead", "0"), Service { state: ServiceState::Stopped, exit_status: None,
            summary: "unit: LoadState=loaded ActiveState=inactive SubState=dead MainPID=0 ExecMainStatus=0".to_owned() });
        assert_eq!(parse_systemd("LoadState=not-found\nActiveState=inactive\n").state, ServiceState::Missing);
    }

    #[test]
    fn task_states_ignore_the_running_and_never_ran_results() {
        let task = |state: &str, result: &str| parse_task(&format!("tarefa: estado={state} ultima_execucao=2026-11-17T10:00:00 ultimo_resultado=0x{result} acao=x launch backend\n"));
        assert_eq!(task("Running", "00041301"), Service { state: ServiceState::Running, exit_status: None,
            summary: "tarefa: estado=Running ultima_execucao=2026-11-17T10:00:00 ultimo_resultado=0x00041301 acao=x launch backend".to_owned() });
        assert_eq!(task("Ready", "00000001").exit_status, Some(1));
        assert_eq!(task("Ready", "00041303").exit_status, None);
        assert_eq!(parse_task("tarefa: ausente\n").state, ServiceState::Missing);
        assert_eq!(parse_task("").state, ServiceState::Unreadable);
    }

    #[test]
    fn launchd_state_and_last_exit() {
        let out = "gui/501/br.dev.hangar.backend = {\n\tstate = not running\n\tlast exit code = 3\n}\n";
        assert_eq!(parse_launchd(out).state, ServiceState::Stopped);
        assert_eq!(parse_launchd(out).exit_status, Some(3));
        assert_eq!(parse_launchd("\tstate = running\n\tlast exit code = (never exited)\n").state, ServiceState::Running);
    }
}

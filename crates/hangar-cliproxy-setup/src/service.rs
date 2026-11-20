//! Serviço próprio do proxy instalado pelo Hangar: unit de usuário no Linux, LaunchAgent no macOS e
//! tarefa ao logon no Windows. Fica de pé quando o Hangar reinicia ou atualiza.
use std::path::{Path, PathBuf};

pub const UNIT: &str = "hangar-cliproxy.service";
pub const LAUNCHD_LABEL: &str = "dev.hangar.cliproxy";
pub const TASK: &str = "hangar-cliproxy";

#[derive(Clone, Debug)]
pub struct Service {
    pub binary: PathBuf,
    pub config: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub step: String,
    pub detail: String,
}

impl Error {
    pub fn code(&self) -> &'static str {
        "cliproxy_servico_falhou"
    }
}

pub fn systemd_unit(s: &Service) -> String {
    // `%` é especificador do systemd (`%b`, `%h`): no caminho ele vale literal.
    let quote = |p: &Path| format!("\"{}\"", p.display().to_string().replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%"));
    format!(
        "[Unit]\nDescription=CLIProxyAPI (instalado pelo Hangar)\nAfter=network.target\n\n\
         [Service]\nType=simple\nExecStart={} --config {}\nRestart=always\nRestartSec=10\n\n\
         [Install]\nWantedBy=default.target\n",
        quote(&s.binary), quote(&s.config)
    )
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn launchd_plist(s: &Service) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         <key>Label</key><string>{LAUNCHD_LABEL}</string>\n\
         <key>ProgramArguments</key><array><string>{}</string><string>--config</string><string>{}</string></array>\n\
         <key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n\
         </dict></plist>\n",
        xml(&s.binary.display().to_string()), xml(&s.config.display().to_string())
    )
}

/// O WSH abre o proxy sem janela e espera por ele, para a tarefa religar quando o processo cai.
pub fn windows_vbs(s: &Service) -> String {
    let line = format!("\"{}\" --config \"{}\"", s.binary.display(), s.config.display());
    format!("WScript.Quit CreateObject(\"WScript.Shell\").Run(\"{}\", 0, True)\r\n", line.replace('"', "\"\""))
}

/// UTF-16LE com BOM, como o instalador grava os `.vbs` das tarefas do Hangar.
pub fn encode_vbs(text: &str) -> Vec<u8> {
    [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect()
}

fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Para a tarefa e o proxy da pasta do Hangar, regrava a tarefa apontando para o `.vbs` e liga.
pub fn windows_script(vbs: &Path, managed_dir: &Path) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         Stop-ScheduledTask -TaskName {task} -ErrorAction SilentlyContinue\n\
         Get-Process -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -and $_.Path.StartsWith({dir}, [StringComparison]::OrdinalIgnoreCase) }} | Stop-Process -Force\n\
         $a = New-ScheduledTaskAction -Execute 'wscript.exe' -Argument {arg}\n\
         $t = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME\n\
         $s = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew\n\
         Register-ScheduledTask -TaskName {task} -Action $a -Trigger $t -Settings $s -Force | Out-Null\n\
         Start-ScheduledTask -TaskName {task}\n",
        task = ps_quote(TASK),
        dir = ps_quote(&managed_dir.display().to_string()),
        arg = ps_quote(&format!("\"{}\"", vbs.display())),
    )
}

async fn run(step: &str, program: &str, args: &[&str]) -> Result<(), Error> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command.output().await.map_err(|e| Error { step: step.into(), detail: e.to_string() })?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().chars().take(400).collect::<String>();
    Err(Error { step: step.into(), detail: format!("{} ({})", detail, output.status) })
}

fn write(step: &str, path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let failed = |e: std::io::Error| Error { step: step.into(), detail: format!("{}: {e}", path.display()) };
    std::fs::create_dir_all(path.parent().expect("arquivo de serviço tem pasta")).map_err(failed)?;
    hangar_core::fs::atomic_write(path, bytes).map_err(failed)
}

/// Grava o serviço apontando para `s.binary` e o (re)inicia: serve para instalar e para atualizar.
pub async fn register(home: &Path, managed_dir: &Path, s: &Service) -> Result<(), Error> {
    if cfg!(target_os = "macos") {
        let plist = home.join("Library/LaunchAgents").join(format!("{LAUNCHD_LABEL}.plist"));
        write("plist", &plist, launchd_plist(s).as_bytes())?;
        let plist = plist.to_string_lossy().into_owned();
        // Descarregar o que não estava carregado falha e não importa.
        let _ = run("launchctl unload", "launchctl", &["unload", &plist]).await;
        return run("launchctl load", "launchctl", &["load", "-w", &plist]).await;
    }
    if cfg!(windows) {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData/Local"));
        let vbs = base.join("hangar").join(format!("{TASK}.vbs"));
        write("vbs", &vbs, &encode_vbs(&windows_vbs(s)))?;
        let script: Vec<u8> = windows_script(&vbs, managed_dir).encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &script);
        return run("Register-ScheduledTask", "powershell.exe", &["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded]).await;
    }
    let unit = home.join(".config/systemd/user").join(UNIT);
    write("unit", &unit, systemd_unit(s).as_bytes())?;
    run("systemctl daemon-reload", "systemctl", &["--user", "daemon-reload"]).await?;
    run("systemctl enable", "systemctl", &["--user", "enable", UNIT]).await?;
    run("systemctl restart", "systemctl", &["--user", "restart", UNIT]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> Service {
        Service { binary: "/home/u/.hangar/cliproxy/bin/8.0.79/cli-proxy-api".into(), config: "/home/u/.cli-proxy-api/config.yaml".into() }
    }

    #[test]
    fn the_unit_runs_the_managed_binary_with_the_config_and_restarts() {
        let unit = systemd_unit(&service());
        assert!(unit.contains("ExecStart=\"/home/u/.hangar/cliproxy/bin/8.0.79/cli-proxy-api\" --config \"/home/u/.cli-proxy-api/config.yaml\"\n"), "{unit}");
        assert!(unit.contains("Restart=always\n") && unit.contains("WantedBy=default.target\n"));
        let odd = Service { binary: "/home/a%b/cli-proxy-api".into(), config: "/home/a%b/config.yaml".into() };
        assert!(systemd_unit(&odd).contains("ExecStart=\"/home/a%%b/cli-proxy-api\" --config \"/home/a%%b/config.yaml\"\n"), "o `%` do caminho não vira especificador");
    }

    #[test]
    fn the_plist_escapes_paths_and_keeps_the_process_alive() {
        let plist = launchd_plist(&Service { binary: "/Users/a&b/cli-proxy-api".into(), config: "/Users/a&b/c.yaml".into() });
        assert!(plist.contains("<string>/Users/a&amp;b/cli-proxy-api</string><string>--config</string><string>/Users/a&amp;b/c.yaml</string>"));
        assert!(plist.contains("<key>KeepAlive</key><true/>"));
    }

    #[test]
    fn windows_launcher_waits_hidden_and_quotes_paths() {
        let s = Service { binary: r"C:\Users\O'Neil\.hangar\cliproxy\bin\8.0.79\cli-proxy-api.exe".into(), config: r"C:\Users\O'Neil\.cli-proxy-api\config.yaml".into() };
        assert_eq!(windows_vbs(&s), "WScript.Quit CreateObject(\"WScript.Shell\").Run(\"\"\"C:\\Users\\O'Neil\\.hangar\\cliproxy\\bin\\8.0.79\\cli-proxy-api.exe\"\" --config \"\"C:\\Users\\O'Neil\\.cli-proxy-api\\config.yaml\"\"\", 0, True)\r\n");
        assert_eq!(&encode_vbs("W")[..4], &[0xFF, 0xFE, b'W', 0]);
        let script = windows_script(Path::new(r"C:\Users\O'Neil\AppData\Local\hangar\hangar-cliproxy.vbs"), Path::new(r"C:\Users\O'Neil\.hangar\cliproxy"));
        assert!(script.contains(r#"-Argument '"C:\Users\O''Neil\AppData\Local\hangar\hangar-cliproxy.vbs"'"#), "{script}");
        assert!(script.contains("-RestartCount 999") && script.contains("StartsWith('C:\\Users\\O''Neil\\.hangar\\cliproxy'"));
    }
}

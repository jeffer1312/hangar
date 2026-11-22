//! Serviço próprio do proxy instalado pelo Hangar: unit de usuário no Linux, LaunchAgent no macOS e
//! tarefa ao logon no Windows, gravados pelo `hangar-managed-binary`. Fica de pé quando o Hangar
//! reinicia ou atualiza.
use std::path::{Path, PathBuf};

use hangar_managed_binary::service::{self as managed, Arg, ServiceId};
pub(crate) use managed::run;
pub use managed::{Error, encode_vbs};

pub const UNIT: &str = "hangar-cliproxy.service";
pub const LAUNCHD_LABEL: &str = "dev.hangar.cliproxy";
pub const TASK: &str = "hangar-cliproxy";
pub const ID: ServiceId = ServiceId { unit: UNIT, launchd_label: LAUNCHD_LABEL, task: TASK, description: "CLIProxyAPI (instalado pelo Hangar)", no_restart_exit: None };

#[derive(Clone, Debug)]
pub struct Service {
    pub binary: PathBuf,
    pub config: PathBuf,
}

impl Service {
    fn managed(&self) -> managed::Service {
        managed::Service { id: ID, executable: self.binary.clone(), args: vec![Arg::Plain("--config".into()), Arg::Path(self.config.clone())] }
    }
}

pub fn systemd_unit(s: &Service) -> String {
    managed::systemd_unit(&s.managed())
}

pub fn launchd_plist(s: &Service) -> String {
    managed::launchd_plist(&s.managed())
}

pub fn windows_vbs(s: &Service) -> String {
    managed::windows_vbs(&s.managed())
}

/// Para a tarefa e o proxy da pasta do Hangar, regrava a tarefa apontando para o `.vbs` e liga.
pub fn windows_script(vbs: &Path, managed_dir: &Path) -> String {
    managed::windows_script(&ID, vbs, managed_dir, true)
}

/// Tira o serviço do Hangar e para o proxy dele: a volta da entrega que não respondeu.
pub fn windows_unregister_script(managed_dir: &Path) -> String {
    managed::windows_unregister_script(&ID, managed_dir)
}

/// Grava o serviço apontando para `s.binary` e o (re)inicia: serve para instalar e para atualizar.
pub async fn register(home: &Path, managed_dir: &Path, s: &Service) -> Result<(), Error> {
    managed::register(home, managed_dir, &s.managed()).await
}

/// Para e apaga o serviço do Hangar, sem tocar no programa baixado nem no config.
pub async fn unregister(home: &Path, managed_dir: &Path) -> Result<(), Error> {
    managed::unregister(home, managed_dir, &ID).await
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

    /// Os arquivos de serviço que as instalações existentes já têm: o crate genérico não pode mudar um byte.
    #[test]
    fn service_files_stay_byte_for_byte_the_ones_already_installed() {
        assert_eq!(systemd_unit(&service()), "[Unit]\nDescription=CLIProxyAPI (instalado pelo Hangar)\nAfter=network.target\n\n\
            [Service]\nType=simple\nExecStart=\"/home/u/.hangar/cliproxy/bin/8.0.79/cli-proxy-api\" --config \"/home/u/.cli-proxy-api/config.yaml\"\n\
            Restart=always\nRestartSec=10\n\n[Install]\nWantedBy=default.target\n");
        assert_eq!(launchd_plist(&service()), "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
            <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
            <plist version=\"1.0\"><dict>\n\
            <key>Label</key><string>dev.hangar.cliproxy</string>\n\
            <key>ProgramArguments</key><array><string>/home/u/.hangar/cliproxy/bin/8.0.79/cli-proxy-api</string><string>--config</string><string>/home/u/.cli-proxy-api/config.yaml</string></array>\n\
            <key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n\
            </dict></plist>\n");
        let script = windows_script(Path::new(r"C:\h\hangar-cliproxy.vbs"), Path::new(r"C:\h\.hangar\cliproxy"));
        assert_eq!(script, "$ErrorActionPreference = 'Stop'\n\
            Stop-ScheduledTask -TaskName 'hangar-cliproxy' -ErrorAction SilentlyContinue\n\
            Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith('C:\\h\\.hangar\\cliproxy\\', [StringComparison]::OrdinalIgnoreCase) } | Stop-Process -Force\n\
            $a = New-ScheduledTaskAction -Execute 'wscript.exe' -Argument '\"C:\\h\\hangar-cliproxy.vbs\"'\n\
            $t = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME\n\
            $s = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew\n\
            Register-ScheduledTask -TaskName 'hangar-cliproxy' -Action $a -Trigger $t -Settings $s -Force | Out-Null\n\
            Start-ScheduledTask -TaskName 'hangar-cliproxy'\n");
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
        assert!(script.contains("-RestartCount 999") && script.contains("StartsWith('C:\\Users\\O''Neil\\.hangar\\cliproxy\\'"), "{script}");
        let undo = windows_unregister_script(Path::new(r"C:\Users\O'Neil\.hangar\cliproxy\"));
        assert!(undo.contains("StartsWith('C:\\Users\\O''Neil\\.hangar\\cliproxy\\'"), "barra no fim só uma vez: {undo}");
        assert!(undo.contains("-ErrorAction SilentlyContinue) { Unregister-ScheduledTask"), "só a tarefa ausente é ignorada: {undo}");
    }
}

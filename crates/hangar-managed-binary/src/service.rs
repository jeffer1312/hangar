//! Serviço próprio de um programa instalado pelo Hangar: unit de usuário no Linux, LaunchAgent no macOS e
//! tarefa ao logon no Windows. Fica de pé quando o Hangar reinicia ou atualiza.
//!
//! Instalar ([`install`]) grava e registra sem subir: o programa só sobe no próximo logon ou no [`start`].
//! [`register`] faz os dois, para instalar e para atualizar.
use std::path::{Path, PathBuf};

use crate::ErrorCodes;

/// Nomes fixos do serviço de um programa.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceId {
    /// Unit de usuário do systemd, com o `.service`.
    pub unit: &'static str,
    pub launchd_label: &'static str,
    /// Nome da tarefa agendada do Windows; também nomeia o `.vbs` que ela abre.
    pub task: &'static str,
    /// `Description=` da unit.
    pub description: &'static str,
    /// Código de saída em que o programa avisa que religar não adianta. Só a unit do systemd filtra
    /// (`RestartPreventExitStatus=`); o Agendador do Windows já não religa ação que sai com erro, e o
    /// launchd não filtra por código e segue religando no ritmo dele.
    pub no_restart_exit: Option<u8>,
}

/// Argumento da linha de comando do serviço.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    /// Palavra sem espaço nem aspas (`--config`), escrita como veio.
    Plain(String),
    /// Caminho, sempre entre aspas na unit e no `.vbs`.
    Path(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Service {
    pub id: ServiceId,
    pub executable: PathBuf,
    pub args: Vec<Arg>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Error {
    pub step: String,
    pub detail: String,
}

impl Error {
    pub fn code(&self, codes: &ErrorCodes) -> &'static str {
        codes.service_failed
    }
}

pub fn systemd_unit(s: &Service) -> String {
    // `%` é especificador do systemd (`%b`, `%h`): no caminho ele vale literal.
    let escape = |text: String| text.replace('%', "%%");
    let quote = |p: &Path| format!("\"{}\"", escape(p.display().to_string().replace('\\', "\\\\").replace('"', "\\\"")));
    let mut exec = quote(&s.executable);
    for arg in &s.args {
        exec.push(' ');
        exec.push_str(&match arg {
            Arg::Plain(word) => escape(word.clone()),
            Arg::Path(path) => quote(path),
        });
    }
    let prevent = s.id.no_restart_exit.map(|code| format!("RestartPreventExitStatus={code}\n")).unwrap_or_default();
    format!(
        "[Unit]\nDescription={}\nAfter=network.target\n\n\
         [Service]\nType=simple\nExecStart={exec}\nRestart=always\nRestartSec=10\n{prevent}\n\
         [Install]\nWantedBy=default.target\n",
        s.id.description
    )
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn launchd_plist(s: &Service) -> String {
    let mut args = format!("<string>{}</string>", xml(&s.executable.display().to_string()));
    for arg in &s.args {
        let text = match arg {
            Arg::Plain(word) => word.clone(),
            Arg::Path(path) => path.display().to_string(),
        };
        args.push_str(&format!("<string>{}</string>", xml(&text)));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         <key>Label</key><string>{}</string>\n\
         <key>ProgramArguments</key><array>{args}</array>\n\
         <key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n\
         </dict></plist>\n",
        s.id.launchd_label
    )
}

/// O WSH abre o programa sem janela e espera por ele, para a tarefa religar quando o processo cai.
pub fn windows_vbs(s: &Service) -> String {
    let mut line = format!("\"{}\"", s.executable.display());
    for arg in &s.args {
        line.push(' ');
        line.push_str(&match arg {
            Arg::Plain(word) => word.clone(),
            Arg::Path(path) => format!("\"{}\"", path.display()),
        });
    }
    format!("WScript.Quit CreateObject(\"WScript.Shell\").Run(\"{}\", 0, True)\r\n", line.replace('"', "\"\""))
}

/// UTF-16LE com BOM, como o instalador grava os `.vbs` das tarefas do Hangar.
pub fn encode_vbs(text: &str) -> Vec<u8> {
    [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect()
}

fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A pasta com a barra no fim: sem ela, `...\cliproxy` também casaria o programa de `...\cliproxy-x`.
fn process_prefix(managed_dir: &Path) -> String {
    ps_quote(&format!("{}\\", managed_dir.display().to_string().trim_end_matches(['\\', '/'])))
}

/// As linhas que param a tarefa `task` (já entre aspas) e o programa que roda da pasta do Hangar.
fn ps_stop(task: &str, managed_dir: &Path) -> String {
    format!(
        "Stop-ScheduledTask -TaskName {task} -ErrorAction SilentlyContinue\n\
         Get-Process -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -and $_.Path.StartsWith({dir}, [StringComparison]::OrdinalIgnoreCase) }} | Stop-Process -Force\n",
        dir = process_prefix(managed_dir),
    )
}

/// Para a tarefa e o programa da pasta do Hangar e regrava a tarefa apontando para o `.vbs`; com `start`, liga.
pub fn windows_script(id: &ServiceId, vbs: &Path, managed_dir: &Path, start: bool) -> String {
    let task = ps_quote(id.task);
    let mut script = format!(
        "$ErrorActionPreference = 'Stop'\n\
         {stop}\
         $a = New-ScheduledTaskAction -Execute 'wscript.exe' -Argument {arg}\n\
         $t = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME\n\
         $s = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew\n\
         Register-ScheduledTask -TaskName {task} -Action $a -Trigger $t -Settings $s -Force | Out-Null\n",
        stop = ps_stop(&task, managed_dir),
        arg = ps_quote(&format!("\"{}\"", vbs.display())),
    );
    if start {
        script.push_str(&format!("Start-ScheduledTask -TaskName {task}\n"));
    }
    script
}

pub fn windows_start_script(id: &ServiceId) -> String {
    format!("$ErrorActionPreference = 'Stop'\nStart-ScheduledTask -TaskName {}\n", ps_quote(id.task))
}

/// Tira o serviço e para o programa da pasta do Hangar.
pub fn windows_unregister_script(id: &ServiceId, managed_dir: &Path) -> String {
    let task = ps_quote(id.task);
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         {stop}\
         if (Get-ScheduledTask -TaskName {task} -ErrorAction SilentlyContinue) {{ Unregister-ScheduledTask -TaskName {task} -Confirm:$false }}\n",
        stop = ps_stop(&task, managed_dir),
    )
}

pub fn systemd_unit_path(home: &Path, id: &ServiceId) -> PathBuf {
    home.join(".config/systemd/user").join(id.unit)
}

pub fn launchd_plist_path(home: &Path, id: &ServiceId) -> PathBuf {
    home.join("Library/LaunchAgents").join(format!("{}.plist", id.launchd_label))
}

pub fn windows_vbs_path(home: &Path, id: &ServiceId) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData/Local"));
    base.join("hangar").join(format!("{}.vbs", id.task))
}

pub async fn run(step: &str, program: &str, args: &[&str]) -> Result<(), Error> {
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

/// O texto do `-EncodedCommand`, antes do base64.
fn utf16le(script: &str) -> Vec<u8> {
    script.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

async fn powershell(step: &str, script: &str) -> Result<(), Error> {
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, utf16le(script));
    run(step, "powershell.exe", &["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded]).await
}

/// Grava o `.vbs` e regrava a tarefa do Windows; com `start`, liga no mesmo script.
async fn register_task(home: &Path, managed_dir: &Path, s: &Service, start: bool) -> Result<(), Error> {
    let vbs = windows_vbs_path(home, &s.id);
    write("vbs", &vbs, &encode_vbs(&windows_vbs(s)))?;
    powershell("Register-ScheduledTask", &windows_script(&s.id, &vbs, managed_dir, start)).await
}

/// Grava e registra o serviço apontando para `s.executable`, sem subir o programa.
pub async fn install(home: &Path, managed_dir: &Path, s: &Service) -> Result<(), Error> {
    if cfg!(target_os = "macos") {
        // O plist na pasta dos LaunchAgents basta: o launchd o carrega no próximo logon.
        return write("plist", &launchd_plist_path(home, &s.id), launchd_plist(s).as_bytes());
    }
    if cfg!(windows) {
        return register_task(home, managed_dir, s, false).await;
    }
    write("unit", &systemd_unit_path(home, &s.id), systemd_unit(s).as_bytes())?;
    run("systemctl daemon-reload", "systemctl", &["--user", "daemon-reload"]).await?;
    run("systemctl enable", "systemctl", &["--user", "enable", s.id.unit]).await
}

/// (Re)inicia o serviço já instalado.
pub async fn start(home: &Path, id: &ServiceId) -> Result<(), Error> {
    if cfg!(target_os = "macos") {
        let plist = launchd_plist_path(home, id).to_string_lossy().into_owned();
        // Descarregar o que não estava carregado falha e não importa.
        let _ = run("launchctl unload", "launchctl", &["unload", &plist]).await;
        return run("launchctl load", "launchctl", &["load", "-w", &plist]).await;
    }
    if cfg!(windows) {
        return powershell("Start-ScheduledTask", &windows_start_script(id)).await;
    }
    run("systemctl restart", "systemctl", &["--user", "restart", id.unit]).await
}

/// Grava o serviço apontando para `s.executable` e o (re)inicia: serve para instalar e para atualizar.
pub async fn register(home: &Path, managed_dir: &Path, s: &Service) -> Result<(), Error> {
    // No Windows, registrar e ligar vão num script só: uma chamada ao PowerShell em vez de duas.
    if cfg!(windows) {
        return register_task(home, managed_dir, s, true).await;
    }
    install(home, managed_dir, s).await?;
    start(home, &s.id).await
}

/// Para e apaga o serviço, sem tocar no programa baixado nem nos dados dele.
pub async fn unregister(home: &Path, managed_dir: &Path, id: &ServiceId) -> Result<(), Error> {
    let remove = |step: &str, path: &Path| match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error { step: step.into(), detail: format!("{}: {e}", path.display()) }),
        _ => Ok(()),
    };
    if cfg!(target_os = "macos") {
        let plist = launchd_plist_path(home, id);
        let path = plist.to_string_lossy().into_owned();
        // Descarregar o que não estava carregado falha e não importa.
        let _ = run("launchctl unload", "launchctl", &["unload", "-w", &path]).await;
        return remove("plist", &plist);
    }
    if cfg!(windows) {
        return powershell("Unregister-ScheduledTask", &windows_unregister_script(id, managed_dir)).await;
    }
    let unit = systemd_unit_path(home, id);
    if unit.is_file() {
        run("systemctl disable", "systemctl", &["--user", "disable", "--now", id.unit]).await?;
    }
    remove("unit", &unit)?;
    run("systemctl daemon-reload", "systemctl", &["--user", "daemon-reload"]).await
}

/// O que o gerenciador diz do processo do serviço.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessState {
    /// Algum processo existe, ou vai existir logo (o `Restart=always` esperando o `RestartSec`). Sem como
    /// perguntar (Windows) ou sem a unit, `false`.
    pub running: bool,
    /// Sem processo de pé, o código com que o último saiu sozinho (morte por sinal não conta). `None` sem
    /// como saber.
    pub last_exit: Option<i32>,
}

/// Algum processo do serviço existe pelo gerenciador, ou vai existir logo: [`ProcessState::running`].
pub async fn process_running(home: &Path, id: &ServiceId) -> bool {
    // No Windows a resposta é sempre `false`: sem abrir o PowerShell à toa.
    !cfg!(windows) && process_state(home, id).await.running
}

/// Uma consulta ao gerenciador: processo de pé e, sem ele, o código da última saída.
pub async fn process_state(home: &Path, id: &ServiceId) -> ProcessState {
    if cfg!(target_os = "macos") {
        return stdout("launchctl", &["list", id.launchd_label]).await.map(|out| launchd_state(&out)).unwrap_or_default();
    }
    if cfg!(windows) {
        let task = ps_quote(id.task);
        let script = format!("$ErrorActionPreference = 'Stop'\n$t = Get-ScheduledTask -TaskName {task}\n\
            \"State=$($t.State)\"\n\"LastTaskResult=$(($t | Get-ScheduledTaskInfo).LastTaskResult)\"\n");
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, utf16le(&script));
        let out = stdout("powershell.exe", &["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded]).await;
        return out.map(|out| task_state(&out)).unwrap_or_default();
    }
    if !systemd_unit_path(home, id).is_file() {
        return ProcessState::default();
    }
    let show = stdout("systemctl", &["--user", "show", "-p", "MainPID", "-p", "ActiveState", "-p", "SubState",
        "-p", "ExecMainCode", "-p", "ExecMainStatus", id.unit]).await;
    show.map(|out| systemd_state(&out)).unwrap_or_default()
}

/// A saída de um comando que terminou bem; qualquer falha é `None`.
async fn stdout(program: &str, args: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command.output().await.ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Linhas `Chave=valor`, como o `systemctl show` e o script da tarefa escrevem.
fn key_value<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines().find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('=')).unwrap_or_default()
}

/// Saída do `systemctl show`: processo vivo, ou unit subindo, parando ou à espera de religar. `ExecMainCode=1`
/// é a saída normal (`CLD_EXITED`); os outros valores são sinal.
fn systemd_state(show: &str) -> ProcessState {
    let field = |key: &str| key_value(show, key);
    let no_pid = matches!(field("MainPID"), "" | "0");
    let running = !no_pid
        || matches!(field("ActiveState"), "active" | "activating" | "reloading" | "deactivating" | "refreshing")
        || field("SubState") == "auto-restart";
    let last_exit = if no_pid && field("ExecMainCode") == "1" { field("ExecMainStatus").parse().ok() } else { None };
    ProcessState { running, last_exit }
}

/// O dicionário do `launchctl list <label>`: `"PID" = n;` com o processo de pé e `"LastExitStatus" = n;` com o
/// status de espera cru, o código nos bits altos e o sinal nos baixos.
fn launchd_state(list: &str) -> ProcessState {
    let field = |key: &str| list.lines().find_map(|line| {
        line.trim().strip_prefix(&format!("\"{key}\" = "))?.trim_end_matches(';').trim().parse::<i64>().ok()
    });
    if field("PID").is_some() {
        return ProcessState { running: true, last_exit: None };
    }
    let last_exit = field("LastExitStatus").filter(|raw| *raw >= 0 && raw & 0xff == 0).and_then(|raw| i32::try_from(raw >> 8).ok());
    ProcessState { running: false, last_exit }
}

/// `State=` e `LastTaskResult=` da tarefa: o `.vbs` sai com o código do programa, que vira o resultado dela.
/// `running` continua `false`: a tarefa de pé não prova que o programa já abriu os dados.
fn task_state(out: &str) -> ProcessState {
    let last_exit = if key_value(out, "State") == "Running" { None } else { key_value(out, "LastTaskResult").parse().ok() };
    ProcessState { running: false, last_exit }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHATSAPP: ServiceId = ServiceId {
        unit: "hangar-whatsapp.service",
        launchd_label: "dev.hangar.whatsapp",
        task: "hangar-whatsapp",
        description: "WhatsApp do Hangar",
        no_restart_exit: None,
    };

    fn service(executable: &str, data: &str) -> Service {
        Service { id: WHATSAPP, executable: executable.into(), args: vec![Arg::Plain("--data".into()), Arg::Path(data.into())] }
    }

    #[test]
    fn the_unit_carries_the_description_and_quotes_only_paths() {
        let unit = systemd_unit(&service("/home/u/.hangar/whatsapp/bin/0.1.0/hangar-whatsapp", "/home/u/.hangar/whatsapp"));
        assert_eq!(unit, "[Unit]\nDescription=WhatsApp do Hangar\nAfter=network.target\n\n\
            [Service]\nType=simple\nExecStart=\"/home/u/.hangar/whatsapp/bin/0.1.0/hangar-whatsapp\" --data \"/home/u/.hangar/whatsapp\"\n\
            Restart=always\nRestartSec=10\n\n[Install]\nWantedBy=default.target\n");
        let odd = systemd_unit(&service("/home/a%b/hangar-whatsapp", "/home/a\"b/dados"));
        assert!(odd.contains("ExecStart=\"/home/a%%b/hangar-whatsapp\" --data \"/home/a\\\"b/dados\"\n"), "{odd}");
        let bare = systemd_unit(&Service { id: WHATSAPP, executable: "/x/p".into(), args: Vec::new() });
        assert!(bare.contains("ExecStart=\"/x/p\"\n"), "sem argumentos: {bare}");
    }

    #[test]
    fn the_plist_lists_each_argument_escaped() {
        let plist = launchd_plist(&service("/Users/a&b/hangar-whatsapp", "/Users/a&b/dados"));
        assert!(plist.contains("<key>Label</key><string>dev.hangar.whatsapp</string>\n"), "{plist}");
        assert!(plist.contains("<array><string>/Users/a&amp;b/hangar-whatsapp</string><string>--data</string><string>/Users/a&amp;b/dados</string></array>"), "{plist}");
        assert!(plist.contains("<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>"));
    }

    #[test]
    fn the_windows_launcher_and_scripts_use_the_task_name() {
        let s = service(r"C:\Users\O'Neil\.hangar\whatsapp\bin\0.1.0\hangar-whatsapp.exe", r"C:\Users\O'Neil\.hangar\whatsapp");
        assert_eq!(windows_vbs(&s), "WScript.Quit CreateObject(\"WScript.Shell\").Run(\"\"\"C:\\Users\\O'Neil\\.hangar\\whatsapp\\bin\\0.1.0\\hangar-whatsapp.exe\"\" --data \"\"C:\\Users\\O'Neil\\.hangar\\whatsapp\"\"\", 0, True)\r\n");
        let vbs = Path::new(r"C:\Users\O'Neil\AppData\Local\hangar\hangar-whatsapp.vbs");
        let dir = Path::new(r"C:\Users\O'Neil\.hangar\whatsapp");
        let installed = windows_script(&WHATSAPP, vbs, dir, false);
        assert!(installed.contains("Register-ScheduledTask -TaskName 'hangar-whatsapp' "), "{installed}");
        assert!(installed.contains(r#"-Argument '"C:\Users\O''Neil\AppData\Local\hangar\hangar-whatsapp.vbs"'"#), "{installed}");
        assert!(!installed.contains("Start-ScheduledTask"), "instalar não sobe: {installed}");
        let started = windows_script(&WHATSAPP, vbs, dir, true);
        assert_eq!(started, format!("{installed}Start-ScheduledTask -TaskName 'hangar-whatsapp'\n"));
        assert_eq!(windows_start_script(&WHATSAPP), "$ErrorActionPreference = 'Stop'\nStart-ScheduledTask -TaskName 'hangar-whatsapp'\n");
        let undo = windows_unregister_script(&WHATSAPP, dir);
        assert!(undo.contains("Unregister-ScheduledTask -TaskName 'hangar-whatsapp' -Confirm:$false"), "{undo}");
        assert!(undo.contains("StartsWith('C:\\Users\\O''Neil\\.hangar\\whatsapp\\'"), "{undo}");
    }

    #[test]
    fn service_files_live_where_each_system_reads_them() {
        let home = Path::new("/home/u");
        assert_eq!(systemd_unit_path(home, &WHATSAPP), Path::new("/home/u/.config/systemd/user/hangar-whatsapp.service"));
        assert_eq!(launchd_plist_path(home, &WHATSAPP), Path::new("/home/u/Library/LaunchAgents/dev.hangar.whatsapp.plist"));
        assert!(windows_vbs_path(home, &WHATSAPP).ends_with("hangar/hangar-whatsapp.vbs"));
    }

    #[test]
    fn a_code_that_must_not_restart_goes_into_the_unit_only_when_asked() {
        let id = ServiceId { no_restart_exit: Some(3), ..WHATSAPP };
        let s = |id| Service { id, executable: "/x/p".into(), args: Vec::new() };
        let unit = systemd_unit(&s(id));
        assert!(unit.contains("Restart=always\nRestartSec=10\nRestartPreventExitStatus=3\n\n[Install]\n"), "{unit}");
        assert!(!systemd_unit(&s(WHATSAPP)).contains("RestartPreventExitStatus"));
        // O launchd não filtra por código e o Agendador já não religa saída com erro: nada muda neles.
        assert_eq!(launchd_plist(&s(id)), launchd_plist(&s(WHATSAPP)));
        assert_eq!(windows_vbs(&s(id)), windows_vbs(&s(WHATSAPP)));
        let (vbs, dir) = (Path::new(r"C:\h\w.vbs"), Path::new(r"C:\h\w"));
        assert_eq!(windows_script(&id, vbs, dir, true), windows_script(&WHATSAPP, vbs, dir, true));
    }

    #[test]
    fn a_unit_waiting_to_restart_counts_as_busy() {
        let show = |pid: &str, active: &str, sub: &str| format!("MainPID={pid}\nActiveState={active}\nSubState={sub}\n");
        let busy = |text: &str| systemd_state(text).running;
        assert!(busy(&show("4321", "active", "running")));
        assert!(busy(&show("0", "activating", "auto-restart")), "na janela do RestartSec o MainPID é 0");
        assert!(busy(&show("0", "deactivating", "stop-sigterm")));
        assert!(!busy(&show("0", "inactive", "dead")));
        assert!(!busy(&show("0", "failed", "failed")));
        assert!(!busy(""), "sem resposta do systemd");
    }

    /// Valores lidos de `systemd-run --user -p Restart=always [-p RestartPreventExitStatus=3] sh -c 'exit 3'`.
    #[test]
    fn the_last_exit_code_comes_only_from_a_normal_exit_without_a_process() {
        let show = |pid: &str, active: &str, sub: &str, code: &str, status: &str|
            format!("ActiveState={active}\nSubState={sub}\nMainPID={pid}\nExecMainCode={code}\nExecMainStatus={status}\n");
        assert_eq!(systemd_state(&show("0", "failed", "failed", "1", "3")), ProcessState { running: false, last_exit: Some(3) });
        // Unit sem o filtro, à espera de religar depois da mesma saída.
        assert_eq!(systemd_state(&show("0", "activating", "auto-restart", "1", "3")), ProcessState { running: true, last_exit: Some(3) });
        assert_eq!(systemd_state(&show("0", "failed", "failed", "2", "9")).last_exit, None, "morte por sinal");
        assert_eq!(systemd_state(&show("4321", "active", "running", "0", "0")).last_exit, None, "processo de pé");
        assert_eq!(systemd_state(&show("0", "inactive", "dead", "0", "0")).last_exit, None, "nunca saiu");
        assert_eq!(systemd_state("").last_exit, None);
    }

    #[test]
    fn launchd_and_the_task_report_the_last_exit_code_too() {
        let gone = "{\n\t\"Label\" = \"dev.hangar.whatsapp\";\n\t\"LastExitStatus\" = 768;\n};\n";
        assert_eq!(launchd_state(gone), ProcessState { running: false, last_exit: Some(3) });
        let up = "{\n\t\"PID\" = 4321;\n\t\"LastExitStatus\" = 768;\n};\n";
        assert_eq!(launchd_state(up), ProcessState { running: true, last_exit: None });
        assert_eq!(launchd_state("{\n\t\"LastExitStatus\" = 9;\n};\n").last_exit, None, "sinal");
        assert_eq!(task_state("State=Ready\r\nLastTaskResult=3\r\n"), ProcessState { running: false, last_exit: Some(3) });
        assert_eq!(task_state("State=Running\r\nLastTaskResult=267009\r\n").last_exit, None);
        assert_eq!(task_state("State=Ready\r\nLastTaskResult=4294967295\r\n").last_exit, None, "Stop-Process -Force");
        assert_eq!(task_state("").last_exit, None);
    }

    #[test]
    fn the_service_error_takes_the_binary_code() {
        let error = Error { step: "unit".into(), detail: "x".into() };
        assert_eq!(error.code(&crate::error_codes!("whatsapp")), "whatsapp_servico_falhou");
    }
}

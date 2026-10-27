//! O script de instalação roda destacado do app e escreve num arquivo que o app acompanha (spec "Destacado do app"):
//! fechar o app não mata a instalação, e reabrir lê o arquivo do começo e retoma. O `state.json` guarda a pasta, as escolhas
//! e as execuções; nunca senha.
use std::{ffi::OsString, path::{Path, PathBuf}, process::{Command, Stdio}, time::Duration};
use serde::{Deserialize, Serialize};
use super::local::Mode;
use super::system::refreshed_path;

pub(crate) const REPO: &str = "jeffer1312/hangar";
/// Commit em que este app foi compilado (`build.rs`).
pub(crate) const COMMIT: &str = env!("HANGAR_NATIVE_COMMIT");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Kind { Check, Install }

impl Kind { fn name(self) -> &'static str { match self { Kind::Check => "check", Kind::Install => "install" } } }

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Options {
    pub agents: Vec<String>,
    pub outside: bool,
    /// `-ConsertarRoda` (Windows, só a instalação): reinicia o psmux e fecha as sessões; só depois de a pessoa confirmar.
    #[serde(default)]
    pub fix_mouse_wheel: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RunRecord {
    pub kind: Kind,
    pub log: PathBuf,
    pub pid: u32,
    /// Hora de início do processo (`identity`): sem ela, um pid reaproveitado depois de reiniciar passaria pelo script.
    /// Ausente num `state.json` antigo = não está vivo.
    #[serde(default)]
    pub started: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SetupState {
    /// Decidido no começo e nunca mudado na retomada.
    #[serde(default)]
    pub mode: Mode,
    pub dest: PathBuf,
    pub options: Options,
    /// O script que ainda roda pede senha com este código; reabrir o app tem de aceitá-lo.
    pub askpass_code: String,
    pub check: Option<RunRecord>,
    pub install: Option<RunRecord>,
}

pub(crate) struct Launch {
    pub kind: Kind,
    pub bootstrap: PathBuf,
    pub mode: Mode,
    pub dest: PathBuf,
    pub options: Options,
    pub log: PathBuf,
    pub token: Option<String>,
    pub askpass: Option<(PathBuf, String)>,
}

pub(crate) fn bootstrap_name(windows: bool) -> &'static str { if windows { "bootstrap.ps1" } else { "bootstrap.sh" } }

pub(crate) fn bootstrap_url(commit: &str, windows: bool) -> String {
    format!("https://raw.githubusercontent.com/{REPO}/{commit}/{}", bootstrap_name(windows))
}

/// O app da `beta` instala o pacote da `beta`; qualquer outro canal, o `latest`.
pub(crate) fn is_beta(built: Option<&str>) -> bool { built == Some("beta") }

/// Spec "Opções novas". A conferência leva as mesmas escolhas: ela lista os agentes escolhidos na tela 2.
/// Pasta no bootstrap = clone: o pacote não leva pasta nenhuma.
pub(crate) fn script_args(kind: Kind, options: &Options, mode: Mode, dest: &Path, windows: bool, beta: bool) -> Vec<String> {
    let agents = options.agents.join(",");
    let tailscale = if options.outside { "sim" } else { "nao" };
    let mut args = if windows {
        vec!["-App".to_owned(), "-Agentes".into(), agents, "-Tailscale".into(), tailscale.into(), "-SemNativo".into()]
    } else {
        let folder = (mode == Mode::Git).then(|| dest.to_string_lossy().into_owned());
        folder.into_iter().chain(["--app".into(), format!("--agentes={agents}"), format!("--tailscale={tailscale}"), "--sem-nativo".into()]).collect()
    };
    if kind == Kind::Check { args.push(if windows { "-SoChecar" } else { "--check" }.into()); }
    if windows && kind == Kind::Install && options.fix_mouse_wheel { args.push("-ConsertarRoda".into()); }
    if !windows && mode == Mode::Package && beta { args.push("--beta".into()); }
    args
}

/// Ambiente do script que depende do modo. No Windows a pasta do clone vai em `CP_DESTINO` e o canal do pacote em `CP_CANAL`.
pub(crate) fn script_env(mode: Mode, dest: &Path, windows: bool, beta: bool) -> Vec<(&'static str, OsString)> {
    match (windows, mode) {
        (false, _) => Vec::new(),
        (true, Mode::Git) => vec![("CP_DESTINO", dest.as_os_str().to_owned())],
        (true, Mode::Package) => if beta { vec![("CP_CANAL", "beta".into())] } else { Vec::new() },
    }
}

/// O ambiente que só o script recebe. O agente do plano 3 nunca recebe `HANGAR_TOKEN` nem o askpass.
pub(crate) fn child_env(token: Option<&str>, askpass: Option<(&Path, &str)>, path: &str) -> Vec<(String, String)> {
    let mut env = vec![("PATH".to_owned(), path.to_owned())];
    if let Some(token) = token { env.push(("HANGAR_TOKEN".into(), token.into())); }
    if let Some((wrapper, code)) = askpass {
        env.push(("HANGAR_ASKPASS".into(), wrapper.to_string_lossy().into_owned()));
        env.push((crate::app::ASKPASS_CODE_ENV.into(), code.into()));
    }
    env
}

pub(crate) fn spawn(launch: &Launch) -> Result<u32, String> {
    if let Some(dir) = launch.log.parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
    let out = std::fs::File::create(&launch.log).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut command = command_for(launch);
    command.stdin(Stdio::null()).stdout(out).stderr(err);
    let askpass = launch.askpass.as_ref().map(|(wrapper, code)| (wrapper.as_path(), code.as_str()));
    for (key, value) in child_env(launch.token.as_deref(), askpass, &refreshed_path()) { command.env(key, value); }
    detach(&mut command);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let pid = child.id();
    // Sem o `wait`, o filho que acaba vira zumbi enquanto o app estiver aberto.
    std::thread::spawn(move || { let _ = child.wait(); });
    Ok(pid)
}

#[cfg(windows)]
fn command_for(launch: &Launch) -> Command {
    let mut command = Command::new(super::system::powershell_exe());
    command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&launch.bootstrap);
    with_script(command, launch, true)
}

#[cfg(not(windows))]
fn command_for(launch: &Launch) -> Command {
    let mut command = Command::new("bash");
    command.arg(&launch.bootstrap);
    with_script(command, launch, false)
}

fn with_script(mut command: Command, launch: &Launch, windows: bool) -> Command {
    let beta = is_beta(crate::update::built_channel());
    command.args(script_args(launch.kind, &launch.options, launch.mode, &launch.dest, windows, beta));
    // Herdados do ambiente do app: `CP_DESTINO`/`CP_DEV` mandariam o bootstrap para o clone, e `CP_CANAL` trocaria o canal.
    if launch.mode == Mode::Package { for key in ["CP_DESTINO", "CP_DEV", "CP_CANAL"] { command.env_remove(key); } }
    command.envs(script_env(launch.mode, &launch.dest, windows, beta));
    command
}

/// Sessão nova: sem terminal de controle, o script nunca acha o terminal de onde o app foi aberto (`/dev/tty`).
#[cfg(target_os = "linux")]
pub(crate) fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe { command.pre_exec(|| { libc::setsid(); Ok(()) }); }
}

#[cfg(windows)]
pub(crate) fn detach(command: &mut Command) { super::system::hidden(command); }

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn detach(_: &mut Command) {}

/// Só para provar telas: `HANGAR_SETUP_BOOTSTRAP=<script>` usa este arquivo e o app pula a própria cópia.
pub(crate) fn test_bootstrap() -> Option<PathBuf> { std::env::var_os("HANGAR_SETUP_BOOTSTRAP").map(PathBuf::from).filter(|p| p.is_file()) }

pub(crate) async fn fetch_bootstrap(dir: &Path, windows: bool) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let target = dir.join(bootstrap_name(windows));
    if let Some(local) = test_bootstrap() {
        std::fs::copy(&local, &target).map_err(|e| e.to_string())?;
        return Ok(target);
    }
    if COMMIT.is_empty() { return Err("HANGAR_NATIVE_COMMIT".into()); }
    let url = bootstrap_url(COMMIT, windows);
    let client = reqwest::Client::builder().timeout(Duration::from_secs(60)).build().map_err(|e| e.to_string())?;
    let bytes = client.get(&url).send().await.and_then(reqwest::Response::error_for_status).map_err(|e| format!("{url}: {e}"))?
        .bytes().await.map_err(|e| e.to_string())?;
    std::fs::write(&target, &bytes).map_err(|e| e.to_string())?;
    Ok(target)
}

pub(crate) fn state_dir() -> Option<PathBuf> { Some(crate::appearance::dir()?.join("setup")) }

/// Saída de cada execução na pasta de logs do Hangar (`crate::log_dir()`), uma por execução.
pub(crate) fn log_path(kind: Kind) -> PathBuf {
    crate::log_dir().join("setup").join(format!("{}-{}.log", chrono::Local::now().format("%Y%m%d-%H%M%S"), kind.name()))
}

pub(crate) fn load_state() -> Option<SetupState> { load_state_at(&state_dir()?) }

pub(crate) fn load_state_at(dir: &Path) -> Option<SetupState> { serde_json::from_slice(&std::fs::read(dir.join("state.json")).ok()?).ok() }

pub(crate) fn save_state(state: &SetupState) -> std::io::Result<()> {
    save_state_at(&state_dir().ok_or_else(|| std::io::Error::other("sem pasta de configuração"))?, state)
}

pub(crate) fn save_state_at(dir: &Path, state: &SetupState) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("state.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        options.mode(0o600);
    }
    std::io::Write::write_all(&mut options.open(&tmp)?, serde_json::to_string(state).map_err(std::io::Error::other)?.as_bytes())?;
    std::fs::rename(tmp, dir.join("state.json"))
}

pub(crate) fn clear_state() {
    if let Some(dir) = state_dir() { let _ = std::fs::remove_file(dir.join("state.json")); }
}

/// O que se sabe do processo: a hora de início, a prova de que não existe mais, ou nada (leitura falhou).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Identity { Known(String), Gone, Unknown }

/// Identifica o processo além do número: o pid volta a ser usado, a hora de início não.
pub(crate) fn identity(pid: u32) -> Option<String> {
    match read_identity(pid) { Identity::Known(started) => Some(started), _ => None }
}

#[cfg(target_os = "linux")]
pub(crate) fn read_identity(pid: u32) -> Identity {
    if pid == 0 || pid > i32::MAX as u32 { return Identity::Gone; }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(text) => parse_start(&text).map_or(Identity::Unknown, Identity::Known),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Identity::Gone,
        Err(_) => Identity::Unknown,
    }
}

/// Campo 22 de `/proc/<pid>/stat`; o nome (campo 2) pode ter espaço e parêntese, então conta depois do último `)`.
#[cfg(any(target_os = "linux", test))]
fn parse_start(stat: &str) -> Option<String> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(19).map(str::to_owned)
}

/// Sentinela impressa pelo script, nunca o texto de erro (localizado) do PowerShell.
#[cfg(any(windows, test))]
fn parse_windows_identity(out: &str) -> Identity {
    match out.trim() {
        "gone" => Identity::Gone,
        number if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) => Identity::Known(number.to_owned()),
        _ => Identity::Unknown,
    }
}

#[cfg(windows)]
pub(crate) fn read_identity(pid: u32) -> Identity {
    if pid == 0 || pid > i32::MAX as u32 { return Identity::Gone; }
    let script = format!("$p = Get-Process -Id {pid} -ErrorAction SilentlyContinue; if ($p) {{ $p.StartTime.ToFileTimeUtc() }} else {{ 'gone' }}");
    super::system::powershell(&script).map_or(Identity::Unknown, |out| parse_windows_identity(&out))
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn read_identity(_: u32) -> Identity { Identity::Unknown }

/// O pid só vale se for o mesmo processo que o app iniciou; pid 0 e fora da faixa do sistema nunca valem.
fn same_process(pid: u32, started: &str) -> bool {
    pid != 0 && pid <= i32::MAX as u32 && !started.is_empty() && identity(pid).as_deref() == Some(started)
}

pub(crate) fn alive(pid: u32, started: &str) -> bool { same_process(pid, started) }

/// Pára a execução inteira: no Linux o `setsid` fez do script o líder do grupo; no Windows vai a árvore.
pub(crate) fn stop(pid: u32, started: &str) { signal(pid, started, false) }

/// Como `stop`, sem dar escolha ao processo (SIGKILL); no Windows o `taskkill /F` já é isso.
pub(crate) fn kill(pid: u32, started: &str) { signal(pid, started, true) }

/// Linux: algum processo ainda no grupo `pgid`. O `setsid` fez do agente o líder; um comando dele em andamento sobrevive ao
/// líder e segue no grupo. No Windows o `taskkill /T` já leva a árvore: sempre `false`.
pub(crate) fn group_alive(pgid: u32) -> bool {
    #[cfg(target_os = "linux")]
    if let Ok(entries) = std::fs::read_dir("/proc") {
        return entries.flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .any(|pid| std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|s| parse_pgrp(&s)) == Some(pgid));
    }
    let _ = pgid;
    false
}

/// SIGKILL ao grupo inteiro. Só para um grupo que já se sabe ser do app (o líder foi conferido pela identidade).
pub(crate) fn kill_group(pgid: u32) {
    #[cfg(target_os = "linux")]
    if pgid > 1 && pgid <= i32::MAX as u32 { unsafe { libc::kill(-(pgid as i32), libc::SIGKILL); } }
    let _ = pgid;
}

/// Campo 5 de `/proc/<pid>/stat` (o grupo), contado depois do último `)` como em `parse_start`.
#[cfg(any(target_os = "linux", test))]
fn parse_pgrp(stat: &str) -> Option<u32> { stat.rsplit_once(')')?.1.split_whitespace().nth(2)?.parse().ok() }

fn signal(pid: u32, started: &str, force: bool) {
    let _ = force;
    if !same_process(pid, started) { return; }
    #[cfg(target_os = "linux")]
    unsafe { libc::kill(-(pid as i32), if force { libc::SIGKILL } else { libc::SIGTERM }); }
    #[cfg(windows)]
    { let _ = super::system::hidden(&mut Command::new("taskkill")).args(["/PID", &pid.to_string(), "/T", "/F"]).output(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> Options { Options { agents: vec!["claude".into(), "codex".into()], outside: true, fix_mouse_wheel: false } }

    #[test]
    fn mouse_wheel_fix_only_on_the_windows_install() {
        let on = Options { fix_mouse_wheel: true, ..options() };
        let dest = Path::new(r"C:\Users\dev\hangar");
        assert_eq!(script_args(Kind::Install, &on, Mode::Git, dest, true, false).last().map(String::as_str), Some("-ConsertarRoda"));
        assert!(!script_args(Kind::Check, &on, Mode::Git, dest, true, false).contains(&"-ConsertarRoda".to_owned()));
        assert!(!script_args(Kind::Install, &on, Mode::Git, dest, false, false).contains(&"-ConsertarRoda".to_owned()));
        assert!(!script_args(Kind::Install, &options(), Mode::Git, dest, true, false).contains(&"-ConsertarRoda".to_owned()));
        // `state.json` de antes da opção continua lendo.
        let old: Options = serde_json::from_str(r#"{"agents":["claude"],"outside":false}"#).unwrap();
        assert!(!old.fix_mouse_wheel);
    }

    #[test]
    fn bootstrap_comes_from_the_build_commit() {
        assert_eq!(bootstrap_url("abc123", false), "https://raw.githubusercontent.com/jeffer1312/hangar/abc123/bootstrap.sh");
        assert_eq!(bootstrap_url("abc123", true), "https://raw.githubusercontent.com/jeffer1312/hangar/abc123/bootstrap.ps1");
    }

    #[test]
    fn linux_args_put_the_folder_first() {
        let dest = Path::new("/home/dev/Projetos/hangar");
        assert_eq!(script_args(Kind::Install, &options(), Mode::Git, dest, false, false),
            vec!["/home/dev/Projetos/hangar", "--app", "--agentes=claude,codex", "--tailscale=sim", "--sem-nativo"]);
        assert_eq!(script_args(Kind::Check, &Options { outside: false, ..options() }, Mode::Git, dest, false, false).last().map(String::as_str), Some("--check"));
        assert!(script_args(Kind::Check, &Options { outside: false, ..options() }, Mode::Git, dest, false, false).contains(&"--tailscale=nao".to_owned()));
        // O clone fica como hoje: o canal não entra.
        assert_eq!(script_args(Kind::Install, &options(), Mode::Git, dest, false, true), script_args(Kind::Install, &options(), Mode::Git, dest, false, false));
    }

    #[test]
    fn windows_args_use_powershell_switches() {
        assert_eq!(script_args(Kind::Install, &options(), Mode::Git, Path::new(r"C:\Users\dev\hangar"), true, false),
            vec!["-App", "-Agentes", "claude,codex", "-Tailscale", "sim", "-SemNativo"]);
        assert_eq!(script_args(Kind::Check, &options(), Mode::Git, Path::new(r"C:\Users\dev\hangar"), true, true).last().map(String::as_str), Some("-SoChecar"));
    }

    #[test]
    fn package_args_carry_no_folder_and_the_linux_channel() {
        let dest = Path::new("/home/dev/.hangar");
        let base = ["--app", "--agentes=claude,codex", "--tailscale=sim", "--sem-nativo"];
        assert_eq!(script_args(Kind::Install, &options(), Mode::Package, dest, false, false), base);
        assert_eq!(script_args(Kind::Install, &options(), Mode::Package, dest, false, true), [&base[..], &["--beta"]].concat());
        assert_eq!(script_args(Kind::Check, &options(), Mode::Package, dest, false, false), [&base[..], &["--check"]].concat());
        assert_eq!(script_args(Kind::Check, &options(), Mode::Package, dest, false, true), [&base[..], &["--check", "--beta"]].concat());
        // Windows: o canal vai no ambiente (`script_env`), não nos argumentos.
        let win = ["-App", "-Agentes", "claude,codex", "-Tailscale", "sim", "-SemNativo"];
        let on = Options { fix_mouse_wheel: true, ..options() };
        assert_eq!(script_args(Kind::Install, &options(), Mode::Package, dest, true, true), win);
        assert_eq!(script_args(Kind::Install, &on, Mode::Package, dest, true, false), [&win[..], &["-ConsertarRoda"]].concat());
        assert_eq!(script_args(Kind::Check, &on, Mode::Package, dest, true, true), [&win[..], &["-SoChecar"]].concat());
    }

    #[test]
    fn script_env_folder_only_for_the_windows_clone_and_channel_only_for_the_beta_package() {
        let dest = Path::new(r"C:\Users\dev\hangar");
        assert_eq!(script_env(Mode::Git, dest, true, true), vec![("CP_DESTINO", OsString::from(dest))]);
        assert_eq!(script_env(Mode::Git, dest, false, true), vec![]);
        assert_eq!(script_env(Mode::Package, dest, true, true), vec![("CP_CANAL", OsString::from("beta"))]);
        assert_eq!(script_env(Mode::Package, dest, true, false), vec![]);
        assert_eq!(script_env(Mode::Package, dest, false, true), vec![]);
    }

    #[test]
    fn package_never_inherits_the_variables_that_send_bootstrap_to_git() {
        let launch = |mode| Launch { kind: Kind::Install, bootstrap: "/cfg/bootstrap.sh".into(), mode, dest: "/home/dev/.hangar".into(),
            options: options(), log: "/logs/a.log".into(), token: None, askpass: None };
        let envs = |mode, windows| -> Vec<(String, Option<String>)> {
            with_script(Command::new("bash"), &launch(mode), windows).get_envs()
                .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect()
        };
        let package = envs(Mode::Package, false);
        for key in ["CP_DESTINO", "CP_DEV", "CP_CANAL"] { assert!(package.contains(&(key.into(), None)), "{key}: {package:?}"); }
        // O clone fica como hoje: nada removido.
        assert_eq!(envs(Mode::Git, false), vec![]);
        assert_eq!(envs(Mode::Git, true), vec![("CP_DESTINO".into(), Some("/home/dev/.hangar".into()))]);
    }

    #[test]
    fn only_the_beta_build_installs_the_beta_package() {
        assert!(is_beta(Some("beta")));
        assert!(!is_beta(Some("main")));
        assert!(!is_beta(Some("latest")));
        assert!(!is_beta(None));
    }

    #[test]
    fn child_env_carries_the_secrets_only_when_given() {
        let env = child_env(Some("t0ken"), Some((Path::new("/cfg/askpass.sh"), "c0de")), "/usr/bin");
        assert!(env.contains(&("PATH".into(), "/usr/bin".into())));
        assert!(env.contains(&("HANGAR_TOKEN".into(), "t0ken".into())));
        assert!(env.contains(&("HANGAR_ASKPASS".into(), "/cfg/askpass.sh".into())));
        // Nunca o SUDO_ASKPASS: o sudo-rs o ignora, e o script entrega a senha ao `sudo -S`.
        assert!(!env.iter().any(|(key, _)| key == "SUDO_ASKPASS"));
        assert!(env.contains(&("HANGAR_ASKPASS_CODE".into(), "c0de".into())));
        assert_eq!(child_env(None, None, "/usr/bin"), vec![("PATH".to_owned(), "/usr/bin".to_owned())]);
    }

    #[test]
    fn state_round_trip_keeps_runs_and_never_a_password() {
        let dir = std::env::temp_dir().join(format!("hangar-run-state-{}", std::process::id()));
        let state = SetupState { mode: Mode::Package, dest: "/home/dev/.hangar".into(), options: options(), askpass_code: "c0de".into(),
            check: Some(RunRecord { kind: Kind::Check, log: "/logs/a.log".into(), pid: 41, started: "100".into() }),
            install: Some(RunRecord { kind: Kind::Install, log: "/logs/b.log".into(), pid: 42, started: "200".into() }) };
        save_state_at(&dir, &state).unwrap();
        assert_eq!(load_state_at(&dir), Some(state));
        let text = std::fs::read_to_string(dir.join("state.json")).unwrap();
        let mut keys: Vec<String> = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text).unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, vec!["askpass_code", "check", "dest", "install", "mode", "options"]);
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join("state.json")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn state_from_before_the_package_resumes_as_the_clone() {
        let old: SetupState = serde_json::from_str(r#"{"dest":"/home/dev/hangar","options":{"agents":["claude"],"outside":false},
            "askpass_code":"c","check":null,"install":null}"#).unwrap();
        assert_eq!((old.mode, old.dest), (Mode::Git, PathBuf::from("/home/dev/hangar")));
    }

    #[test]
    fn start_time_is_field_22_even_with_a_tricky_name() {
        let tail = (4..=21).map(|n| n.to_string()).collect::<Vec<_>>().join(" ");
        assert_eq!(parse_start(&format!("7 (a b) c) S {tail} 9876 0")), Some("9876".to_owned()));
        assert_eq!(parse_start("garbage"), None);
        assert_eq!(parse_pgrp(&format!("7 (a b) c) S 1 4321 {tail}")), Some(4321));
        assert_eq!(parse_pgrp("garbage"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn group_of_this_test_is_alive() {
        let own = std::fs::read_to_string("/proc/self/stat").ok().and_then(|s| parse_pgrp(&s)).unwrap();
        assert!(group_alive(own));
    }

    #[test]
    fn a_pid_is_alive_only_with_the_same_identity() {
        assert!(!alive(0, "1"));
        assert!(!alive(u32::MAX, "1"));
        assert!(!alive(std::process::id(), ""));
        assert!(!alive(std::process::id(), "not-the-start-time"));
        #[cfg(any(target_os = "linux", windows))] {
            let own = identity(std::process::id()).unwrap();
            assert!(alive(std::process::id(), &own));
        }
    }

    #[test]
    fn identity_is_gone_for_pid_zero_and_missing_pids() {
        assert_eq!(read_identity(0), Identity::Gone);
        assert_eq!(read_identity(u32::MAX), Identity::Gone);
        #[cfg(target_os = "linux")]
        assert_eq!(read_identity(i32::MAX as u32), Identity::Gone);
        #[cfg(any(target_os = "linux", windows))]
        assert!(matches!(read_identity(std::process::id()), Identity::Known(_)));
    }

    #[test]
    fn windows_identity_reads_the_sentinel_not_the_error_text() {
        assert_eq!(parse_windows_identity("gone\r\n"), Identity::Gone);
        assert_eq!(parse_windows_identity("133456789012345678\r\n"), Identity::Known("133456789012345678".into()));
        assert_eq!(parse_windows_identity(""), Identity::Unknown);
        assert_eq!(parse_windows_identity("Get-Process : Não foi possível"), Identity::Unknown);
    }

    #[test]
    fn log_name_says_which_run() {
        assert!(log_path(Kind::Check).to_string_lossy().ends_with("-check.log"));
        assert!(log_path(Kind::Install).to_string_lossy().ends_with("-install.log"));
    }
}

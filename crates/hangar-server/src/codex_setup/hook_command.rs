//! O wrapper conserva os três fluxos e o resultado do processo do hook.
use std::{ffi::{OsStr, OsString}, io::{self, Read, Write}, process::{Command, Output, Stdio}};
use std::path::{Component, Path, PathBuf};

pub fn allow_wrapper() -> Result<(String, String), String> {
    let executable = persistent_server()?.into_os_string().into_string()
        .map_err(|_| "O caminho do servidor não está em UTF-8".to_owned())?;
    Ok((executable, "codex-hook-allow".into()))
}

/// O programa dos hooks gravados nos agentes: `~/.hangar/bin/hangar-launcher`, o mesmo binário renovado
/// a cada versão e preservado na volta de versão, então o comando novo não falha na versão antiga.
/// Só vale quando ele É este servidor (mesmos bytes): no checkout e no modo desenvolvedor a
/// atualização não o renova, e um lançador velho recusaria o subcomando novo. Fora disso, o
/// `persistent_server`.
pub fn hook_server(home: &Path) -> Result<PathBuf, String> {
    let running = std::env::current_exe().map_err(|error| error.to_string())?;
    hook_server_with(home, cfg!(windows), &running, persistent_server)
}

pub fn hook_server_with(home: &Path, windows: bool, running: &Path, fallback: impl FnOnce() -> Result<PathBuf, String>) -> Result<PathBuf, String> {
    let launcher = crate::update::migrate::launcher_path(home, windows);
    if same_bytes(&launcher, running) { Ok(launcher) } else { fallback() }
}

/// Os dois arquivos têm o mesmo conteúdo; ilegível ou ausente é "não". A resposta fica guardada por
/// caminho, tamanho e data dos dois: o reparo do Kimi pergunta uma vez por entrada.
fn same_bytes(a: &Path, b: &Path) -> bool {
    type Key = (PathBuf, PathBuf, u64, Option<std::time::SystemTime>, u64, Option<std::time::SystemTime>);
    static SEEN: std::sync::Mutex<Vec<(Key, bool)>> = std::sync::Mutex::new(Vec::new());
    let (Ok(meta_a), Ok(meta_b)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if !meta_a.is_file() || meta_a.len() != meta_b.len() {
        return false;
    }
    let key: Key = (a.to_owned(), b.to_owned(), meta_a.len(), meta_a.modified().ok(), meta_b.len(), meta_b.modified().ok());
    if let Some((_, same)) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key) {
        return *same;
    }
    let same = compare(a, b);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() > 16 { seen.clear(); }
    seen.push((key, same));
    same
}

fn compare(a: &Path, b: &Path) -> bool {
    let (Ok(file_a), Ok(file_b)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    let (mut file_a, mut file_b) = (io::BufReader::new(file_a), io::BufReader::new(file_b));
    let (mut buf_a, mut buf_b) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let Ok(n) = file_a.read(&mut buf_a) else { return false };
        if n == 0 {
            return true;
        }
        if file_b.read_exact(&mut buf_b[..n]).is_err() || buf_a[..n] != buf_b[..n] {
            return false;
        }
    }
}

/// `"<programa dos hooks>" <subcomando>`, sem sufixo.
pub fn hook_command(home: &Path, mode: &str) -> Result<String, String> {
    let program = hook_server(home)?;
    let program = program.to_str().ok_or("O caminho do servidor não está em UTF-8")?;
    Ok(format!("\"{program}\" {mode}"))
}

/// Este `hangar-server` por um caminho que sobrevive à troca de versão, para gravar em config de fora.
pub fn persistent_server() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Ok(persistent_executable(&executable, std::env::home_dir().as_deref()))
}

fn persistent_executable(executable: &Path, home: Option<&Path>) -> PathBuf {
    let Some(home) = home else { return executable.to_owned(); };
    let executable = crate::list::links::without_verbatim(executable.canonicalize().unwrap_or_else(|_| executable.to_owned()));
    let home = crate::list::links::without_verbatim(home.canonicalize().unwrap_or_else(|_| home.to_owned()));
    let current = crate::install_root::current(&home);
    let stable = || if cfg!(windows) { current.join("bin/hangar-server.exe") }
        else { current.join("scripts/hangar-server-command") };
    let versions = home.join(".hangar/versions");
    if let Ok(relative) = executable.strip_prefix(&versions) {
        let mut components = relative.components();
        if matches!(components.next(), Some(Component::Normal(_))) {
            let binary = components.as_path();
            if binary.parent() == Some(Path::new("bin"))
                && binary.file_name().is_some_and(|name| name == "hangar-server" || name == "hangar-server.exe")
            {
                return stable();
            }
        }
    }
    if let Ok(real) = current.canonicalize().map(crate::list::links::without_verbatim)
        && let Ok(relative) = executable.strip_prefix(real)
    { let _ = relative; return stable(); }
    if executable.parent() == Some(home.join(".hangar/bin").as_path()) && current.exists() {
        return stable();
    }
    executable.to_owned()
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn recorded_hooks_follow_current_even_while_the_link_is_absent() {
        let home = Path::new("fixture-home");
        for name in ["hangar-server", "hangar-server.exe"] {
            let executable = home.join(".hangar/versions/v1/bin").join(name);
            let expected = if cfg!(windows) { home.join(".hangar/current/bin/hangar-server.exe") }
                else { home.join(".hangar/current/scripts/hangar-server-command") };
            assert_eq!(persistent_executable(&executable, Some(home)), expected);
        }
        for executable in [home.join(".hangar/bin/hangar-server"), home.join(".hangar/current/bin/hangar-server"), PathBuf::from("other-checkout/crates/target/debug/hangar-server")] {
            assert_eq!(persistent_executable(&executable, Some(home)), executable);
        }
    }

    /// Revisão R1, item 1: lançador mais velho que o servidor (checkout ou modo desenvolvedor, que a
    /// atualização não renova) recusaria o subcomando novo; os hooks ficam no servidor em execução.
    #[test]
    fn hooks_use_the_launcher_only_when_it_is_this_server() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let launcher = crate::update::migrate::launcher_path(&home, cfg!(windows));
        std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        let running = temp.path().join("hangar-server");
        std::fs::write(&running, b"servidor novo com state-hook").unwrap();
        let fallback = || Ok(PathBuf::from("persistente"));
        assert_eq!(hook_server_with(&home, cfg!(windows), &running, fallback).unwrap(), PathBuf::from("persistente"), "sem lançador");
        std::fs::write(&launcher, b"servidor velho sem state-hook").unwrap();
        assert_eq!(hook_server_with(&home, cfg!(windows), &running, fallback).unwrap(), PathBuf::from("persistente"), "lançador velho");
        std::fs::write(&launcher, b"servidor novo com state-hook").unwrap();
        assert_eq!(hook_server_with(&home, cfg!(windows), &running, fallback).unwrap(), launcher, "o mesmo binário");
    }

    #[cfg(unix)]
    #[test]
    fn recorded_checkout_hook_keeps_the_current_link_unresolved() {
        let temp = tempfile::tempdir_in(crate::list::links::without_verbatim(std::env::temp_dir().canonicalize().unwrap())).unwrap();
        let home = temp.path().join("home");
        let checkout = temp.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(home.join(".hangar")).unwrap();
        std::os::unix::fs::symlink(&checkout, crate::install_root::current(&home)).unwrap();
        let binary = Path::new("crates/target/release/hangar-server");
        assert_eq!(persistent_executable(&checkout.join(binary), Some(&home)), crate::install_root::current(&home).join("scripts/hangar-server-command"));
    }
}

pub fn run(mode: &str, args: &[OsString]) -> i32 {
    let filter = mode == "codex-hook-allow" && args.is_empty();
    let valid = filter || (args.first().is_some_and(|arg| arg == "--")
        && if mode == "codex-hook-allow" { args.len() >= 2 } else { args.len() == 2 });
    if !valid && mode == "codex-hook-json" {
        eprintln!("Uso: hangar-server {mode} -- comando");
        return 2;
    }
    let mut input = Vec::new();
    if let Err(error) = io::stdin().read_to_end(&mut input) {
        eprintln!("Não foi possível ler a entrada do hook: {error}");
        return 1;
    }
    if !valid {
        eprintln!("Uso: hangar-server {mode} -- comando");
        return 2;
    }
    if filter {
        return if io::stdout().write_all(&super::hook_wrappers::complete_allow(&input)).is_ok() { 0 } else { 1 };
    }
    let output = match execute(mode, &args[1..], input) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("Não foi possível executar o hook: {error}");
            return 127;
        }
    };
    let stdout = if mode == "codex-hook-allow" {
        super::hook_wrappers::complete_allow(&output.stdout)
    } else {
        super::hook_wrappers::normalize_json(&output.stdout)
    };
    let result = if mode == "codex-hook-allow" {
        io::stderr().write_all(&output.stderr).and_then(|()| io::stdout().write_all(&stdout))
    } else {
        io::stdout().write_all(&stdout).and_then(|()| io::stderr().write_all(&output.stderr))
    };
    if result.is_err() { return 1; }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        output.status.code().unwrap_or_else(|| 128 + output.status.signal().unwrap_or(1))
    }
    #[cfg(not(unix))]
    { output.status.code().unwrap_or(1) }
}

fn execute(mode: &str, args: &[OsString], input: Vec<u8>) -> io::Result<Output> {
    let mut command = if mode == "codex-hook-allow" {
        let mut command = Command::new(&args[0]);
        command.args(&args[1..]);
        command
    } else { shell(&args[0])? };
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| io::Error::other("entrada do hook indisponível"))?;
    // A saída pode encher enquanto o hook ainda lê a entrada.
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(&input));
        let output = child.wait_with_output();
        let written = writer.join().map_err(|_| io::Error::other("escrita da entrada do hook interrompida"))?;
        if let Err(error) = written && error.kind() != io::ErrorKind::BrokenPipe { return Err(error); }
        output
    })
}

#[cfg(unix)]
fn shell(script: &OsStr) -> io::Result<Command> {
    let mut command = Command::new("/bin/sh");
    command.arg("-c").arg(script);
    Ok(command)
}

#[cfg(windows)]
fn shell(script: &OsStr) -> io::Result<Command> {
    use std::os::windows::process::CommandExt;
    let executable = std::env::var_os("ComSpec").filter(|value| !value.is_empty()).or_else(|| {
        std::env::var_os("SystemRoot").map(|root| std::path::Path::new(&root).join("System32/cmd.exe").into_os_string())
    }).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "interpretador de comandos indisponível"))?;
    let mut command = Command::new(executable);
    let mut argument = OsString::from("/c \"");
    argument.push(script);
    argument.push("\"");
    command.raw_arg(argument);
    Ok(command)
}

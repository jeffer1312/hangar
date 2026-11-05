//! O wrapper conserva os três fluxos e o resultado do processo do hook.
use std::{ffi::{OsStr, OsString}, io::{self, Read, Write}, process::{Command, Output, Stdio}};
use std::path::{Component, Path, PathBuf};

pub fn allow_wrapper() -> Result<(String, String), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let executable = persistent_executable(&executable, std::env::home_dir().as_deref());
    let executable = executable.into_os_string().into_string()
        .map_err(|_| "O caminho do servidor não está em UTF-8".to_owned())?;
    Ok((executable, "codex-hook-allow".into()))
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

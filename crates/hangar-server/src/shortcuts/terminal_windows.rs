//! Arquivos do atalho no psmux: o comando e a saída pertencem ao terminal, não ao servidor.
use regex::Regex;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::LazyLock,
};

const SUFFIXES: [&str; 4] = [".cmd", "-cmd.cmd", "-cmd.ps1", ".exit"];
const DYNAMIC: [&str; 8] = [
    "CD",
    "DATE",
    "TIME",
    "RANDOM",
    "ERRORLEVEL",
    "CMDCMDLINE",
    "CMDEXTVERSION",
    "HIGHESTNUMANODENUMBER",
];

pub fn directory(home: &Path, env: &BTreeMap<String, String>) -> PathBuf {
    env.get("LOCALAPPDATA")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.to_owned())
        .join("Hangar/shortcuts")
}
fn variable() -> &'static Regex {
    static VARIABLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^%([A-Za-z_][A-Za-z0-9_()]*)%").unwrap());
    &VARIABLE
}
pub fn escape(command: &str, env: &BTreeMap<String, String>) -> String {
    let known: std::collections::HashSet<String> = env
        .keys()
        .map(|s| s.to_uppercase())
        .chain(DYNAMIC.iter().map(|s| (*s).into()))
        .collect();
    let mut result = String::new();
    let mut rest = command;
    while !rest.is_empty() {
        if rest.starts_with('%') {
            if let Some(found) = variable()
                .captures(rest)
                .filter(|found| known.contains(&found[1].to_uppercase()))
            {
                let matched = found.get(0).unwrap().as_str();
                result.push_str(matched);
                rest = &rest[matched.len()..];
                continue;
            }
            result.push_str("%%");
            rest = &rest[1..];
        } else {
            let c = rest.chars().next().unwrap();
            result.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    result
}
pub fn forget(dir: &Path, id: &str) -> std::io::Result<()> {
    for suffix in SUFFIXES {
        match std::fs::remove_file(dir.join(format!("{id}{suffix}"))) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// O teste de arquivos injeta Latin-1; em produção a codepage OEM é a do Windows.
pub fn write_command(
    dir: &Path,
    id: &str,
    command: &str,
    powershell: bool,
    shell: Option<&str>,
    env: &BTreeMap<String, String>,
    encode: fn(&str) -> std::io::Result<Vec<u8>>,
) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    let inner = dir.join(format!("{id}-cmd.cmd"));
    let outer = dir.join(format!("{id}.cmd"));
    let exit = dir.join(format!("{id}.exit"));
    match std::fs::remove_file(&exit) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let launch = if powershell {
        let script = dir.join(format!("{id}-cmd.ps1"));
        let text = format!(
            "\u{feff}{}\r\n$__hangar_ok = $?\r\nif (-not $__hangar_ok) {{ if ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {{ exit $LASTEXITCODE }}; exit 1 }}\r\n",
            command.replace("\r\n", "\n").replace('\n', "\r\n")
        );
        std::fs::write(&script, text.as_bytes())?;
        let executable = shell
            .map(str::to_owned)
            .or_else(|| {
                crate::runtime::process::resolve_program("powershell.exe", env.iter())
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| {
                format!(
                    "{}/System32/WindowsPowerShell/v1.0/powershell.exe",
                    env.get("SystemRoot")
                        .map(String::as_str)
                        .unwrap_or("C:\\Windows")
                )
            });
        format!(
            "@\"{executable}\" -NoProfile -ExecutionPolicy Bypass -File \"{}\"",
            script.display()
        )
    } else {
        let body = escape(command, env)
            .replace("\r\n", "\n")
            .replace('\n', "\r\n");
        std::fs::write(&inner, encode(&format!("@echo off\r\n{body}\r\n"))?)?;
        format!("@cmd /d /c \"{}\"", inner.display())
    };
    std::fs::write(
        &outer,
        encode(&format!(
            "{launch}\r\n@>\"{}\" echo %ERRORLEVEL%\r\n:h\r\n@pause >nul\r\n@goto h\r\n",
            exit.display()
        ))?,
    )?;
    Ok(format!("cmd /d /c \"{}\"", outer.display()))
}

#[cfg(windows)]
pub fn encode(text: &str) -> std::io::Result<Vec<u8>> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn WideCharToMultiByte(
            page: u32,
            flags: u32,
            wide: *const u16,
            len: i32,
            out: *mut u8,
            size: i32,
            default: *const u8,
            used: *mut i32,
        ) -> i32;
    }
    let wide: Vec<u16> = text.encode_utf16().collect();
    if wide.is_empty() {
        return Ok(Vec::new());
    }
    let len = i32::try_from(wide.len()).map_err(std::io::Error::other)?;
    let size = unsafe {
        WideCharToMultiByte(
            1,
            0,
            wide.as_ptr(),
            len,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    if size == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut out = vec![0; size as usize];
    if unsafe {
        WideCharToMultiByte(
            1,
            0,
            wide.as_ptr(),
            len,
            out.as_mut_ptr(),
            size,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(out)
}

#[cfg(not(windows))]
pub fn encode(text: &str) -> std::io::Result<Vec<u8>> {
    Ok(text.as_bytes().to_vec())
}

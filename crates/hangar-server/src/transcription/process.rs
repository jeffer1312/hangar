use super::cloud::AttemptFailure;
use crate::terminal_process::{CommandTree, OwnedTree};
use std::process::Stdio;
use tokio::process::{Child, Command};

pub(crate) struct ManagedChild {
    pub child: Child,
    tree: CommandTree,
    finished: bool,
}

pub(crate) fn command(program: &std::path::Path) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    for key in crate::terminal_process::PRIVATE_ENV_KEYS.into_iter().chain([
        "CP_GROQ_API_KEY", "GROQ_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY", "ELEVENLABS_API_KEY",
    ]) { command.env_remove(key); }
    command
}

impl ManagedChild {
    pub async fn spawn(mut command: Command, code: &str, detail: &str) -> Result<Self, AttemptFailure> {
        let tree = CommandTree::configure(&mut command).map_err(|_| AttemptFailure::unavailable(code, detail))?;
        let child = command.spawn().map_err(|error| {
            let code = if error.kind() == std::io::ErrorKind::NotFound && code == "audio_conversion_failed" {
                "audio_converter_missing"
            } else { code };
            AttemptFailure::unavailable(code, detail)
        })?;
        let mut owned = Self { child, tree, finished: false };
        if owned.tree.attach(&owned.child).is_err() {
            let _ = owned.child.kill().await;
            return Err(AttemptFailure::unavailable(code, "Não foi possível controlar o processo iniciado."));
        }
        Ok(owned)
    }

    pub async fn stop(&mut self) {
        crate::terminal_process::finish(&mut self.tree).await;
        let _ = self.child.wait().await;
        self.finished = true;
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if !self.finished { let _ = self.tree.terminate(); let _ = self.child.start_kill(); }
    }
}

pub(crate) fn expand_path(value: &str) -> std::path::PathBuf {
    if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    value.into()
}

pub(crate) async fn owns_port(pid: u32, port: u16) -> Result<bool, ()> {
    tokio::task::spawn_blocking(move || listening(pid, port)).await.map_err(|_| ())?
}

#[cfg(target_os = "linux")]
fn listening(pid: u32, port: u16) -> Result<bool, ()> {
    let directory = std::path::PathBuf::from(format!("/proc/{pid}"));
    let Ok(entries) = std::fs::read_dir(directory.join("fd")) else { return Ok(false); };
    let sockets: std::collections::HashSet<_> = entries.flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter_map(|p| p.to_str().and_then(|p| p.strip_prefix("socket:[")?.strip_suffix(']')).map(str::to_owned)).collect();
    let Ok(table) = std::fs::read_to_string(directory.join("net/tcp")) else { return Ok(false); };
    Ok(table.lines().skip(1).any(|line| {
        let fields: Vec<_> = line.split_whitespace().collect();
        fields.len() > 9 && fields[3] == "0A" && sockets.contains(fields[9])
            && fields[1].split_once(':').is_some_and(|(address, value)| address == "0100007F"
                && u16::from_str_radix(value, 16) == Ok(port))
    }))
}

#[cfg(target_os = "windows")]
fn listening(pid: u32, port: u16) -> Result<bool, ()> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER};
    let mut size = 0u32;
    unsafe {
        let result = GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, 2, TCP_TABLE_OWNER_PID_LISTENER, 0);
        if result != 122 && result != 0 { return Err(()); }
        let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
        if GetExtendedTcpTable(buffer.as_mut_ptr().cast(), &mut size, 0, 2, TCP_TABLE_OWNER_PID_LISTENER, 0) != 0 { return Err(()); }
        let count = buffer.first().copied().unwrap_or(0) as usize;
        let row_size = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
        if count > (size as usize).saturating_sub(4) / row_size { return Err(()); }
        let rows = buffer.as_ptr().add(1).cast::<MIB_TCPROW_OWNER_PID>();
        for index in 0..count {
            let row = std::ptr::read(rows.add(index));
            if row.dwOwningPid == pid && row.dwLocalAddr == u32::from_ne_bytes([127, 0, 0, 1])
                && u16::from_be(row.dwLocalPort as u16) == port { return Ok(true); }
        }
    }
    Ok(false)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn listening(pid: u32, port: u16) -> Result<bool, ()> {
    let result = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-p"]).arg(pid.to_string()).arg(format!("-iTCP:{port}"))
        .args(["-sTCP:LISTEN", "-Fn"]).output().map_err(|_| ())?;
    Ok(result.status.success() && String::from_utf8_lossy(&result.stdout).lines()
        .any(|line| line == format!("n127.0.0.1:{port}")))
}

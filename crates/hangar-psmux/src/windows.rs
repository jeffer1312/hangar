use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, FILETIME, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};

use crate::{Orphan, data_dir, orphans};

pub fn scan_orphans(dir: &Path, settle: Duration) -> io::Result<Vec<Orphan>> {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let me = Pid::from_u32(std::process::id());
    let mut candidates: Vec<_> = system.processes().iter()
        .filter(|(_, process)| process.run_time() >= settle.as_secs() && super::server_program(&process.name().to_string_lossy()))
        .map(|(pid, _)| *pid).collect();
    candidates.push(me);
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&candidates), true, ProcessRefreshKind::nothing()
        .with_user(UpdateKind::Always).with_cmd(UpdateKind::Always).with_environ(UpdateKind::Always));
    let owner = system.process(me).and_then(|process| process.user_id())
        .ok_or_else(|| io::Error::other("usuário do servidor ilegível"))?;

    orphans(dir, candidates.into_iter().filter(|pid| *pid != me).filter_map(|pid| {
        let process = system.process(pid).filter(|process| process.run_time() >= settle.as_secs()
            && process.user_id() == Some(owner) && uses_registry(process.environ(), dir))?;
        let argv = process.cmd().iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        Some((pid.as_u32(), argv))
    }))
}

pub fn process_birth(pid: u32) -> io::Result<Option<u64>> {
    let Some(handle) = open_process(pid, false)? else { return Ok(None); };
    if exited(&handle, 0)? { Ok(None) } else { birth(&handle).map(Some) }
}

pub fn process_tree(root: u32, root_birth: u64) -> io::Result<Vec<(u32, u64)>> {
    let changed = || io::Error::other("a árvore de processos mudou durante a conferência");
    let root_handle = open_process(root, false)?.ok_or_else(changed)?;
    if birth(&root_handle)? != root_birth || exited(&root_handle, 0)? { return Err(changed()); }
    let edges = tree_edges(root)?;
    let mut handles = BTreeMap::new();
    for &pid in edges.keys() {
        if let Some(handle) = open_process(pid, false)? { handles.insert(pid, handle); }
    }
    // Os handles mantêm cada PID ligado à mesma vida enquanto o parentesco é relido.
    let confirmed = tree_edges(root)?;
    if exited(&root_handle, 0)? { return Err(changed()); }
    let mut times = BTreeMap::from([(root, root_birth)]);
    let mut active = Vec::new();
    for (pid, parent) in confirmed {
        if edges.get(&pid) != Some(&parent) { return Err(changed()); }
        let handle = handles.get(&pid).ok_or_else(changed)?;
        if exited(handle, 0)? { continue; }
        let started = birth(handle)?;
        times.insert(pid, started);
        active.push((pid, parent, started));
    }
    if active.iter().any(|&(_, parent, started)| times.get(&parent).is_none_or(|parent| started < *parent)) {
        return Err(changed());
    }
    Ok(active.into_iter().map(|(pid, _, started)| (pid, started)).collect())
}

fn tree_edges(root: u32) -> io::Result<BTreeMap<u32, u32>> {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    if system.process(Pid::from_u32(root)).is_none() {
        return Err(io::Error::other("o processo raiz não apareceu na varredura"));
    }
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (&pid, process) in system.processes() {
        if let Some(parent) = process.parent() { children.entry(parent.as_u32()).or_default().push(pid.as_u32()); }
    }
    let mut seen = BTreeSet::from([root]);
    let mut edges = BTreeMap::new();
    let mut parents = vec![root];
    while let Some(parent) = parents.pop() {
        for &child in children.get(&parent).into_iter().flatten() {
            if seen.insert(child) {
                edges.insert(child, parent);
                parents.push(child);
            }
        }
    }
    Ok(edges)
}

pub fn stop_processes(identities: &[(u32, u64)], timeout: Duration) -> io::Result<()> {
    let mut handles = Vec::new();
    for &(pid, expected) in identities {
        if let Some(handle) = open_process(pid, true)? && birth(&handle)? == expected && !exited(&handle, 0)? {
            handles.push(handle);
        }
    }
    let deadline = Instant::now() + timeout;
    for handle in &handles {
        if unsafe { TerminateProcess(handle.as_raw_handle(), 1) } == 0 && !exited(handle, 0)? {
            return Err(io::Error::last_os_error());
        }
    }
    for handle in &handles {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !exited(handle, remaining.as_millis().min(u128::from(u32::MAX - 1)) as u32)? {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "processo antigo não encerrou"));
        }
    }
    Ok(())
}

fn open_process(pid: u32, terminate: bool) -> io::Result<Option<OwnedHandle>> {
    let rights = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | if terminate { PROCESS_TERMINATE } else { 0 };
    let handle = unsafe { OpenProcess(rights, 0, pid) };
    if handle.is_null() {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) { Ok(None) } else { Err(error) };
    }
    // SAFETY: OpenProcess devolve um handle próprio, fechado pelo OwnedHandle.
    Ok(Some(unsafe { OwnedHandle::from_raw_handle(handle) }))
}

fn exited(handle: &OwnedHandle, timeout: u32) -> io::Result<bool> {
    match unsafe { WaitForSingleObject(handle.as_raw_handle(), timeout) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

fn birth(handle: &OwnedHandle) -> io::Result<u64> {
    let mut created = FILETIME::default();
    let (mut stopped, mut kernel, mut user) = (created, created, created);
    if unsafe { GetProcessTimes(handle.as_raw_handle(), &mut created, &mut stopped, &mut kernel, &mut user) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn uses_registry(environment: &[OsString], dir: &Path) -> bool {
    data_dir(|name| environment.iter().find_map(|entry| {
        let entry = entry.to_str()?;
        let (key, value) = entry.split_once('=')?;
        key.eq_ignore_ascii_case(name).then(|| OsString::from(value))
    })).is_some_and(|path| {
        let normalized = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let equal = |a: &OsStr, b: &OsStr| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
        path == dir || equal(normalized(&path).as_os_str(), normalized(dir).as_os_str())
    })
}

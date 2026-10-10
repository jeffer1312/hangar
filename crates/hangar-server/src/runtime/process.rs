//! Processo do cano, comum aos provedores: subir, matar conferindo a identidade, varrer órfãos e
//! achar o binário. Port de `subir_cano_processo`, `_matar_grupo` e `matar_orfaos`
//! (`backend/app/adapters/claude_headless/adapter.py`). Não grava o arquivo da sessão: quem grava é
//! o Python, pela política que o ator pede.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

pub use crate::session_write::table::Provider;

/// O que o Python entrega pela política `launch_env`.
pub struct LaunchSpec {
    pub provider: Provider,
    /// Os 16 primeiros caracteres dão nome ao socket e ao log (`cano-<key16>…`).
    pub key: String,
    pub cwd: PathBuf,
    /// Comando pronto que roda dentro do cano; o Rust não monta argv de provedor.
    pub program: Vec<String>,
    /// Ambiente completo. É segredo: nunca vai para log nem disco.
    pub env: Vec<(String, String)>,
    pub cano_extra: Map<String, Value>,
    pub sidecar_dir: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Cano {
    pub pid: u32,
    pub escuta: String,
    pub token: String,
    pub ts: f64,
    pub versao: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness { Dead, Ours, Foreign }

#[derive(Debug)]
pub enum ProcessError { NoCano, Spawn(String), NotListening, StillAlive }

impl ProcessError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoCano => "cano_ausente",
            Self::Spawn(_) => "cano_nao_subiu",
            Self::NotListening => "cano_nao_escutou",
            Self::StillAlive => "cano_continua_vivo",
        }
    }
}

fn key16(key: &str) -> &str { key.get(..16).unwrap_or(key) }

/// O cano se reconhece pelo argv: `--escuta` e `--log …/cano-<key16>.log`.
fn is_cano_of(argv: &[String], key: &str) -> bool {
    let log = argv.iter().position(|a| a == "--log").and_then(|i| argv.get(i + 1)).map(Path::new);
    argv.iter().any(|a| a == "--escuta")
        && log.and_then(Path::file_name).is_some_and(|n| n.to_string_lossy() == format!("cano-{}.log", key16(key)))
}

/// `None` = sem processo; senão o argv e, com argv vazio, se ele ainda é nosso (zumbi do Linux,
/// executável do cano nos outros).
#[cfg(target_os = "linux")]
fn probe(pid: u32) -> Option<(Vec<String>, bool)> {
    let proc = PathBuf::from(format!("/proc/{pid}"));
    let raw = std::fs::read(proc.join("cmdline")).ok()?;
    let argv: Vec<String> = raw.split(|b| *b == 0).filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned()).collect();
    if !argv.is_empty() { return Some((argv, false)); }
    // Zumbi tem cmdline vazio: só é nosso se ainda não foi colhido por este processo.
    let stat = String::from_utf8_lossy(&std::fs::read(proc.join("stat")).ok()?).into_owned();
    let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace();
    let zombie = fields.next() == Some("Z");
    let ppid: u32 = fields.next()?.parse().ok()?;
    Some((argv, zombie && ppid == std::process::id()))
}

#[cfg(not(target_os = "linux"))]
fn probe(pid: u32) -> Option<(Vec<String>, bool)> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    let target = Pid::from_u32(pid);
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[target]), true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_exe(UpdateKind::Always));
    let process = system.process(target)?;
    let argv: Vec<String> = process.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect();
    if !argv.is_empty() { return Some((argv, false)); }
    // Sem argv legível, o executável ser o binário do cano é a prova que resta.
    let same_exe = process.exe().zip(cano_binary().ok()).is_some_and(|(exe, bin)| exe == bin);
    Some((argv, same_exe))
}

fn identify(pid: u32, key: &str) -> Liveness {
    match probe(pid) {
        None => Liveness::Dead,
        Some((argv, _)) if !argv.is_empty() => if is_cano_of(&argv, key) { Liveness::Ours } else { Liveness::Foreign },
        Some((_, unreaped_ours)) => if unreaped_ours { Liveness::Ours } else { Liveness::Foreign },
    }
}

/// Pelo pid E pela identidade: número reaproveitado depois de reiniciar a máquina é `Foreign`.
pub fn liveness(pid: u32, key: &str) -> Liveness { identify(pid, key) }

static FOUND: OnceLock<PathBuf> = OnceLock::new();

/// Fixa o binário do cano antes da primeira busca, sem variável de ambiente: os testes de integração
/// rodam num processo só, e mudar o ambiente com outras threads lendo é indefinido em Unix. O mesmo
/// caminho de novo é aceito; outro devolve o que já valia.
#[doc(hidden)]
pub fn use_cano_binary(path: PathBuf) -> Result<(), PathBuf> {
    let current = FOUND.get_or_init(|| path.clone());
    if *current == path { Ok(()) } else { Err(current.clone()) }
}

/// `CP_RUST_CANO_BIN`, senão a pasta do `hangar-server`, senão `~/.hangar/bin`; sondado uma vez
/// por processo quando acha (sem argumentos o cano sai com 2); falha não fica guardada, a próxima chamada
/// sonda de novo. Caminho errado na variável não vira outro binário.
pub fn cano_binary() -> Result<PathBuf, ProcessError> {
    if let Some(path) = FOUND.get() { return Ok(path.clone()); }
    let found = {
        let name = if cfg!(windows) { "hangar-cano.exe" } else { "hangar-cano" };
        let chosen = std::env::var_os("CP_RUST_CANO_BIN").filter(|v| !v.is_empty()).map(PathBuf::from);
        let candidates: Vec<PathBuf> = match chosen {
            Some(path) => vec![path],
            None => [std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(name))),
                     std::env::home_dir().map(|h| h.join(".hangar").join("bin").join(name))]
                .into_iter().flatten().collect(),
        };
        let found = candidates.into_iter().find(|bin| probe_binary(bin));
        if found.is_none() && crate::warn_limit::allow(None, "cano_binary") { tracing::warn!("hangar-cano ausente ou não roda nesta máquina"); }
        found
    };
    found.map(|path| FOUND.get_or_init(|| path).clone()).ok_or(ProcessError::NoCano)
}

fn probe_binary(bin: &Path) -> bool {
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new(bin).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    else { return false };
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code() == Some(2),
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => { let _ = child.kill(); let _ = child.wait(); return false; }
        }
    }
}

/// Escopo transiente do systemd, fora do cgroup do serviço (senão o restart dele mata o cano).
/// `--scope` faz `exec`: o pid lançado continua sendo o do cano. Sondado uma vez, como `tmux._scope_probe`.
#[cfg(unix)]
fn scope_prefix() -> &'static [&'static str] {
    const SCOPE: &[&str] = &["systemd-run", "--user", "--scope", "--collect", "-q", "--"];
    static USABLE: OnceLock<bool> = OnceLock::new();
    let usable = *USABLE.get_or_init(|| {
        let ok = std::env::var_os("XDG_RUNTIME_DIR").is_some()
            && std::process::Command::new(SCOPE[0]).args(&SCOPE[1..]).arg("true")
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        if !ok { tracing::warn!("systemd-run --user --scope indisponível; cano nasce no cgroup do servidor"); }
        ok
    });
    if usable { SCOPE } else { &[] }
}

/// O executável pelo PATH do ambiente da sessão (PATHEXT no Windows: o `hangar-engine` é `.CMD`).
fn resolve_program(name: &str, env: &[(String, String)]) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 { return path.is_file().then(|| path.to_path_buf()); }
    let var = |k: &str| env.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
    // No Windows o nome cru só vale se já termina numa extensão do PATHEXT, como o `shutil.which`:
    // `codex` sem extensão é o shim de shell do npm, que o CreateProcess não roda.
    let exts: Vec<String> = if cfg!(windows) {
        let pathext: Vec<String> = var("PATHEXT").unwrap_or(".COM;.EXE;.BAT;.CMD").split(';')
            .filter(|e| !e.is_empty()).map(str::to_ascii_lowercase).collect();
        let lower = name.to_ascii_lowercase();
        if pathext.iter().any(|e| lower.ends_with(e.as_str())) { vec![String::new()] } else { pathext }
    } else { vec![String::new()] };
    std::env::split_paths(var("PATH")?).find_map(|dir| {
        exts.iter().map(|ext| dir.join(format!("{name}{ext}"))).find(|p| p.is_file())
    })
}

/// Socket unix na pasta do arquivo da sessão; TCP em loopback onde não há socket unix ou o caminho
/// passa do limite do kernel. Sufixo por subida: o cano anterior da mesma chave pode estar morrendo.
fn new_listen(key: &str, dir: &Path) -> String {
    let suffix = &crate::mods::state::random_hex(2);
    let socket = dir.join(format!("cano-{}-{suffix}.sock", key16(key)));
    if cfg!(unix) && socket.as_os_str().len() < 100 {
        return format!("unix:{}", socket.display());
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or_else(|e| {
        tracing::warn!(kind = ?e.kind(), "não consegui reservar porta de loopback para o cano; a escuta sobe com porta 0");
        0
    });
    format!("tcp:127.0.0.1:{port}")
}

/// Sobe o cano e só devolve quando ele escuta; sem escuta em 10 s, mata o que subiu.
pub async fn spawn(spec: &LaunchSpec) -> Result<Cano, ProcessError> {
    let bin = tokio::task::spawn_blocking(cano_binary).await.map_err(|_| ProcessError::NoCano)??;
    #[cfg(unix)]
    let prefix = tokio::task::spawn_blocking(scope_prefix).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "sondagem do escopo systemd falhou; o cano sobe sem o escopo");
        &[]
    });
    #[cfg(not(unix))]
    let prefix: &[&str] = &[];
    let program = spec.program.first().ok_or_else(|| ProcessError::Spawn("comando vazio".into()))?;
    let exe = resolve_program(program, &spec.env)
        .ok_or_else(|| ProcessError::Spawn(format!("binário não encontrado: {program}")))?;
    let escuta = new_listen(&spec.key, &spec.sidecar_dir);
    let token = crate::mods::state::random_hex(16);
    let log = spec.sidecar_dir.join(format!("cano-{}.log", key16(&spec.key)));
    let mut argv: Vec<std::ffi::OsString> = prefix.iter().map(Into::into).collect();
    argv.push(bin.into());
    for (flag, value) in [("--escuta", std::ffi::OsStr::new(&escuta)), ("--log", log.as_os_str()),
                          ("--cwd", spec.cwd.as_os_str()), ("--token", std::ffi::OsStr::new(&token))] {
        argv.push(flag.into());
        argv.push(value.to_owned());
    }
    argv.push("--".into());
    argv.push(exe.into());
    argv.extend(spec.program[1..].iter().map(Into::into));
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(&spec.cwd).env_clear().envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)]
    // SAFETY: `setsid` é async-signal-safe e não toca memória do processo pai.
    unsafe { cmd.pre_exec(|| if libc::setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }); }
    #[cfg(windows)]
    cmd.creation_flags(0x0000_0200 | 0x0800_0000); // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
    let mut child = cmd.spawn().map_err(|e| ProcessError::Spawn(format!("não subiu: {}", e.kind())))?;
    let pid = child.id().ok_or_else(|| ProcessError::Spawn("saiu ao nascer".into()))?;
    // Só para não deixar zumbi.
    tokio::spawn(async move { let _ = child.wait().await; });
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let mut cano = Cano { pid, escuta, token, ts, versao: 2, extra: spec.cano_extra.clone() };
    let binding = super::protocol::CanoBinding { pid, escuta: cano.escuta.clone(), token: cano.token.clone(), versao: 2 };
    match super::cano::connect(&binding).await {
        Ok(connection) => { cano.versao = connection.snapshot.versao; Ok(cano) }
        Err(error) => {
            tracing::warn!(pid, code = %error.code, "cano não escutou; matando o que subiu");
            if let Err(failure) = kill(&cano, &spec.key, &spec.sidecar_dir).await {
                tracing::warn!(pid, code = failure.code(), "cano que não escutou não foi encerrado");
            }
            Err(ProcessError::NotListening)
        }
    }
}

/// Mata o grupo do cano (cano + agente) só se o pid ainda for o cano da chave e, confirmada a saída,
/// apaga os rastros dele na pasta da sessão (`sidecar_dir`): o socket desta vida e, se nenhuma outra
/// vida da chave tem socket ali nem processo vivo, os demais `cano-<key16>*`. Pid de outro programa: não mata.
pub async fn kill(cano: &Cano, key: &str, sidecar_dir: &Path) -> Result<(), ProcessError> {
    let (pid, key_owned) = (cano.pid, key.to_owned());
    let state = tokio::task::spawn_blocking(move || identify(pid, &key_owned)).await
        .map_err(|_| ProcessError::StillAlive)?;
    match state {
        Liveness::Dead => {}
        Liveness::Foreign => tracing::warn!(pid, "pid do cano não é mais o cano da sessão; não matei"),
        Liveness::Ours => {
            signal_group(pid)?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while probe_alive(pid) {
                if tokio::time::Instant::now() >= deadline { return Err(ProcessError::StillAlive); }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
    remove_traces(sidecar_dir, key, &cano.escuta, cano.pid);
    Ok(())
}

/// Vivo e ainda não zumbi: zumbi já saiu, só falta ser colhido.
#[cfg(target_os = "linux")]
fn probe_alive(pid: u32) -> bool { probe(pid).is_some_and(|(argv, _)| !argv.is_empty()) }
#[cfg(not(target_os = "linux"))]
fn probe_alive(pid: u32) -> bool { probe(pid).is_some() }

#[cfg(unix)]
fn signal_group(pid: u32) -> Result<(), ProcessError> {
    // SAFETY: chamadas sem memória compartilhada; erro lido logo depois.
    let group = unsafe { libc::getpgid(pid as i32) };
    if group <= 0 || unsafe { libc::killpg(group, libc::SIGTERM) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) { return Ok(()); }
        tracing::warn!(pid, error = %error, "não foi possível encerrar o cano; arquivos conservados");
        return Err(ProcessError::StillAlive);
    }
    Ok(())
}

#[cfg(windows)]
fn signal_group(pid: u32) -> Result<(), ProcessError> {
    let status = std::process::Command::new("taskkill").args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().map_err(|_| ProcessError::StillAlive)?;
    // 128 = o processo já não existe.
    if matches!(status.code(), Some(0 | 128)) { Ok(()) } else { Err(ProcessError::StillAlive) }
}

fn remove_traces(dir: &Path, key: &str, escuta: &str, except: u32) {
    // Chave vazia ou curta casaria `cano-*` de outras sessões e levaria o socket delas.
    if key.len() < 16 { return; }
    let prefix = format!("cano-{}", key16(key));
    let remove = |path: &Path| if let Err(e) = std::fs::remove_file(path) && e.kind() != std::io::ErrorKind::NotFound {
        tracing::warn!(file = %path.display(), kind = ?e.kind(), "rastro do cano não removido");
    };
    // O socket desta vida, se é mesmo um rastro dela na pasta da sessão.
    if let Some(own) = escuta.strip_prefix("unix:").map(Path::new)
        && own.parent() == Some(dir) && own.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)) {
        remove(own);
    }
    let entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&prefix)).collect(),
        Err(e) => { tracing::debug!(dir = %dir.display(), kind = ?e.kind(), "rastros do cano: pasta ilegível"); return; }
    };
    // Outra vida da mesma chave (sufixo por subida) ainda tem socket, ou outro cano dela está vivo (a
    // que escuta por TCP não deixa socket): o log e o resto são dela também.
    let newer_socket = entries.iter().any(|e| { let name = e.file_name().to_string_lossy().into_owned();
        name.starts_with(&format!("{prefix}-")) && name.ends_with(".sock") });
    if newer_socket || other_cano_alive(key, except) { return; }
    for entry in entries { remove(&entry.path()); }
}

/// Outro cano vivo desta chave além de `except`. Sem como listar os processos, responde que sim: na
/// dúvida, os rastros ficam.
#[cfg(target_os = "linux")]
fn other_cano_alive(key: &str, except: u32) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else { return true };
    entries.flatten().filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| *pid != except)
        .any(|pid| probe(pid).is_some_and(|(argv, _)| is_cano_of(&argv, key)))
}
#[cfg(not(target_os = "linux"))]
fn other_cano_alive(key: &str, except: u32) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always));
    system.processes().iter().any(|(pid, process)| pid.as_u32() != except
        && is_cano_of(&process.cmd().iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>(), key))
}

/// Canos deste dono cuja sessão já não existe; `SIGTERM` por pid, como o Python. Dono é
/// `HANGAR_CANO_OWNER`, ou o `HOME` em cano antigo sem ele. Só Linux (`/proc`).
pub fn kill_orphans(live: &HashSet<String>, owner: &str) -> usize {
    #[cfg(target_os = "linux")]
    { linux_orphans(live, owner) }
    #[cfg(not(target_os = "linux"))]
    { let _ = (live, owner); 0 }
}

/// Varredura da subida do Rust (regra 10): vivas são as chaves de `claude-headless/`, as do Codex sem
/// terminal de `codex-sessions/` e as de `codex-sessions/prepared/` (transferência em curso). Pasta
/// ausente conta como vazia; pasta que não se lê cancela a varredura (`None`), porque a sessão viva
/// dela pareceria órfã. Arquivo ilegível também cancela.
pub fn sweep_orphans(claude_dir: &Path, codex_dir: &Path, owner: &str) -> Option<usize> {
    let mut live = HashSet::new();
    let prepared = codex_dir.join("prepared");
    for (dir, codex) in [(claude_dir, false), (codex_dir, true), (prepared.as_path(), false)] {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => { tracing::warn!(dir = %dir.display(), kind = ?e.kind(), "pasta de sessões ilegível; varredura de órfãos cancelada"); return None; }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                // Entrada perdida deixaria a chave dela fora do conjunto de vivas, e o cano real seria morto.
                Err(e) => { tracing::warn!(dir = %dir.display(), kind = ?e.kind(), "entrada da pasta de sessões ilegível; varredura de órfãos cancelada"); return None; }
            };
            if entry.path().extension().is_none_or(|ext| ext != "json") { continue; }
            let meta: Value = match std::fs::read(entry.path()).ok().and_then(|raw| serde_json::from_slice(&raw).ok()) {
                Some(meta) => meta,
                None => { tracing::warn!(file = %entry.path().display(), "arquivo de sessão ilegível; varredura de órfãos cancelada"); return None; }
            };
            if codex && meta["headless"] != true { continue; }
            if let Some(key) = meta["key"].as_str().filter(|key| !key.is_empty()) { live.insert(key.to_owned()); }
        }
    }
    Some(kill_orphans(&live, owner))
}

#[cfg(target_os = "linux")]
fn linux_orphans(live: &HashSet<String>, owner: &str) -> usize {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir("/proc") else { return 0 };
    // SAFETY: sem efeito colateral.
    let uid = unsafe { libc::getuid() };
    let me = std::process::id().to_string();
    let (mut killed, mut foreign, mut denied) = (0usize, 0usize, 0usize);
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit()) && *n != me) else { continue };
        if !entry.metadata().is_ok_and(|m| m.uid() == uid) { continue; }
        let environ = match std::fs::read(entry.path().join("environ")) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => { denied += 1; continue; }
            Err(_) => continue,
        };
        let var = |wanted: &[u8]| environ.split(|b| *b == 0)
            .find_map(|item| item.strip_prefix(wanted).and_then(|rest| rest.strip_prefix(b"=")));
        let Some(key) = var(b"HANGAR_CANO_KEY").filter(|k| !k.is_empty()) else { continue };
        if live.contains(String::from_utf8_lossy(key).as_ref()) { continue; }
        if var(b"HANGAR_CANO_OWNER").or_else(|| var(b"HOME")) != Some(owner.as_bytes()) { foreign += 1; continue; }
        // Nunca 0 nem negativo: seriam o grupo inteiro deste processo.
        let Some(pid) = pid.parse::<i32>().ok().filter(|p| *p > 0) else { continue };
        // SAFETY: sinal para um pid lido agora; falha só é registrada.
        if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 { killed += 1; }
        else { tracing::warn!(pid, "órfão do cano não recebeu o sinal"); }
    }
    if denied > 0 { tracing::info!(denied, "varredura de órfãos sem permissão em processos deste usuário"); }
    if foreign > 0 { tracing::info!(foreign, "processos de cano sem prova de que são deste servidor ficaram"); }
    killed
}

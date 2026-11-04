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

/// Subida de um processo pelo cano (o Codex sem terminal; o comando vem de `codex_launch`).
pub struct LaunchSpec {
    pub provider: Provider,
    /// Os 16 primeiros caracteres dão nome ao socket e ao log (`cano-<key16>…`).
    pub key: String,
    pub cwd: PathBuf,
    /// Comando pronto que roda dentro do cano; o Rust não monta argv de provedor.
    pub program: Vec<String>,
    /// Ambiente completo. É segredo: nunca vai para log nem disco.
    pub env: Vec<(String, String)>,
    pub sidecar_dir: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Cano {
    pub pid: u32,
    pub escuta: String,
    pub token: String,
    #[serde(deserialize_with = "crate::json_envelope::float")]
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
    identity_from(probe(pid), key)
}

fn identity_from(observation: Option<(Vec<String>, bool)>, key: &str) -> Liveness {
    match observation {
        None => Liveness::Dead,
        Some((argv, _)) if !argv.is_empty() => if is_cano_of(&argv, key) { Liveness::Ours } else { Liveness::Foreign },
        Some((_, unreaped_ours)) => if unreaped_ours { Liveness::Ours } else { Liveness::Foreign },
    }
}

/// Pelo pid E pela identidade: número reaproveitado depois de reiniciar a máquina é `Foreign`.
pub fn liveness(pid: u32, key: &str) -> Liveness {
    let observation = probe(pid);
    // O zumbi não serve para religar, mas sua propriedade ainda permite limpar o grupo no `kill`.
    #[cfg(target_os = "linux")]
    if observation.as_ref().is_some_and(|(argv, ours)| argv.is_empty() && *ours) {
        return Liveness::Dead;
    }
    identity_from(observation, key)
}

/// Argv do processo vivo (vazio quando ilegível); `None` sem processo.
pub fn argv(pid: u32) -> Option<Vec<String>> { probe(pid).map(|(argv, _)| argv) }

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

/// Recusa do systemd ao criar um escopo logo depois de outro ("Failed to start transient scope unit:
/// Unit run-….scope not found"). Só ela se repete, com estas esperas; qualquer outra falha responde na hora.
pub(crate) const SCOPE_RETRY: [Duration; 3] = [Duration::from_millis(200), Duration::from_millis(400), Duration::from_millis(800)];
pub(crate) const SCOPE_UNIT_MISSING: &str = "systemd_scope_unit_missing";
const SCOPE_REFUSED: &str = "systemd_scope_refused";
const SCOPE_ABSENT: &str = "systemd_scope_absent";

/// Primeira linha não vazia, truncada: vai ao log e ao erro sem trazer a saída inteira.
fn first_line(text: &[u8]) -> Option<String> {
    String::from_utf8_lossy(text).lines().map(str::trim).find(|line| !line.is_empty()).map(|line| line.chars().take(200).collect())
}

/// A recusa vem de escopos criados ao mesmo tempo: quem foi recusado junto não pode repetir junto.
/// Espera entre a base e o dobro dela.
pub(crate) fn scope_wait(base: Duration) -> Duration {
    let spread = u64::from_str_radix(&crate::mods::state::random_hex(2), 16).unwrap_or(0);
    base + base.mul_f64(spread as f64 / 65536.0)
}

pub(crate) fn scope_unit_missing(stderr: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().find(|line| line.contains("transient scope unit") && line.contains("not found"))?;
    Some(line.chars().take(200).collect())
}

/// Escopo transiente do systemd, fora do cgroup do serviço (senão o restart dele mata o cano).
/// `--scope` faz `exec`: o pid lançado continua sendo o do cano. Sondado como `tmux._scope_probe`.
/// Caminho absoluto pelo PATH do servidor: o processo nasce com o ambiente da sessão, cujo PATH não
/// precisa ter o `systemd-run`, e a sondagem só prova o que a subida vai rodar se for o mesmo binário.
/// Só fica gravado o que não muda com o processo vivo: escopo provado, ou sem `systemd-run`/`XDG_RUNTIME_DIR`.
/// Recusa (a transitória esgotada ou outra) deixa esta subida sem escopo e a próxima sonda de novo.
/// Bloqueante (processo e esperas): chamar fora do runtime.
#[cfg(unix)]
pub(crate) fn scope_prefix() -> &'static [std::ffi::OsString] {
    const ARGS: &[&str] = &["--user", "--scope", "--collect", "-q", "--"];
    static PREFIX: OnceLock<Vec<std::ffi::OsString>> = OnceLock::new();
    // Uma sondagem por vez: duas em paralelo criariam a colisão de escopos que a repetição tenta evitar.
    static PROBING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if let Some(prefix) = PREFIX.get() { return prefix; }
    let _probing = PROBING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(prefix) = PREFIX.get() { return prefix; }
    let absent = |reason: &str| PREFIX.get_or_init(|| {
        tracing::warn!(code = SCOPE_ABSENT, reason, "escopo do systemd indisponível; cano nasce no cgroup do servidor");
        Vec::new()
    });
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() { return absent("sem XDG_RUNTIME_DIR"); }
    let Some(bin) = std::env::var_os("PATH")
        .and_then(|path| std::env::split_paths(&path).map(|dir| dir.join("systemd-run")).find(|p| p.is_file()))
    else { return absent("sem systemd-run no PATH") };
    let mut waits = SCOPE_RETRY.iter();
    loop {
        let output = std::process::Command::new(&bin).args(ARGS).arg("true")
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped()).output();
        let output = match output {
            Ok(output) if output.status.success() =>
                return PREFIX.get_or_init(|| std::iter::once(bin.into_os_string()).chain(ARGS.iter().map(Into::into)).collect()),
            Ok(output) => output,
            Err(error) => {
                tracing::warn!(code = SCOPE_REFUSED, cause = %error.kind(), "sondagem do escopo systemd não rodou; esta subida segue sem escopo");
                return &[];
            }
        };
        let Some(refusal) = scope_unit_missing(&output.stderr) else {
            let cause = first_line(&output.stderr).unwrap_or_else(|| format!("saída {}", output.status));
            tracing::warn!(code = SCOPE_REFUSED, cause, "sondagem do escopo systemd recusada; esta subida segue sem escopo");
            return &[];
        };
        let Some(wait) = waits.next() else {
            tracing::warn!(code = SCOPE_UNIT_MISSING, refusal, "sondagem do escopo systemd recusada em todas as tentativas; esta subida segue sem escopo");
            return &[];
        };
        let wait = scope_wait(*wait);
        tracing::warn!(code = SCOPE_UNIT_MISSING, wait_ms = wait.as_millis() as u64, "sondagem do escopo systemd recusada; nova tentativa");
        std::thread::sleep(wait);
    }
}

/// O executável pelo PATH do ambiente da sessão (PATHEXT no Windows: o `hangar-engine` é `.CMD`).
pub(crate) fn resolve_program<'a>(name: &str, env: impl IntoIterator<Item = (&'a String, &'a String)> + Clone) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 { return path.is_file().then(|| path.to_path_buf()); }
    let var = |k: &str| env.clone().into_iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str());
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
    let prefix: &[std::ffi::OsString] = tokio::task::spawn_blocking(scope_prefix).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "sondagem do escopo systemd falhou; o cano sobe sem o escopo");
        &[]
    });
    #[cfg(not(unix))]
    let prefix: &[std::ffi::OsString] = &[];
    spawn_recipe(prepared(spec, bin, prefix.to_vec(), Map::new())?).await
}

fn prepared(spec: &LaunchSpec, bin: PathBuf, prefix: Vec<std::ffi::OsString>, extra: Map<String, Value>) -> Result<PreparedRecipe, ProcessError> {
    let program = spec.program.first().ok_or_else(|| ProcessError::Spawn("comando vazio".into()))?;
    let exe = resolve_program(program, spec.env.iter().map(|(k, v)| (k, v)))
        .ok_or_else(|| ProcessError::Spawn(format!("binário não encontrado: {program}")))?;
    let program=std::iter::once(exe.into_os_string()).chain(spec.program[1..].iter().map(std::ffi::OsString::from)).collect();
    let env=spec.env.iter().map(|(k,v)|(k.into(),v.into())).collect();
    Ok(PreparedRecipe {bin,prefix,program,env,cwd:spec.cwd.clone(),key:spec.key.clone(),sidecar_dir:spec.sidecar_dir.clone(),extra})
}

/// O relançamento: tudo conferido antes de encerrar a vida antiga (cano, escopo do systemd sem reserva,
/// executável no PATH da sessão), para a falha deixar a origem de pé. Bloqueante: chamar fora do runtime.
pub fn prepare_launch(spec: &LaunchSpec, extra: Map<String, Value>) -> Result<PreparedRecipe, ProcessError> {
    prepared(spec, cano_binary()?, scope_command()?, extra)
}

pub(crate) fn scope_command()->Result<Vec<std::ffi::OsString>,ProcessError> {
    #[cfg(unix)] let prefix=scope_prefix();
    #[cfg(not(unix))] let prefix:&[std::ffi::OsString]=&[];
    #[cfg(target_os="linux")]
    if Path::new("/run/systemd/system").exists() && prefix.is_empty() {return Err(ProcessError::Spawn("escopo da sessão indisponível; origem conservada".into()));}
    Ok(prefix.to_vec())
}

/// Prévia privada de um lançamento; ambiente só em memória, nunca no diário.
pub struct PreparedRecipe {bin:PathBuf,prefix:Vec<std::ffi::OsString>,program:Vec<std::ffi::OsString>,env:Vec<(std::ffi::OsString,std::ffi::OsString)>,cwd:PathBuf,key:String,sidecar_dir:PathBuf,extra:Map<String,Value>}
/// Mantém settings privados vivos até a vida nova assumir; falha encerra só o cano lançado aqui.
pub struct RelaunchGuard {cano:Cano,key:String,dir:PathBuf,settings:std::sync::Arc<super::claude_relaunch::Recipe>,armed:bool}
impl RelaunchGuard {
    pub fn new(cano:&Cano,key:&str,dir:&Path,settings:std::sync::Arc<super::claude_relaunch::Recipe>)->Self {Self {cano:cano.clone(),key:key.into(),dir:dir.into(),settings,armed:true}}
    pub fn disarm(&mut self) {self.armed=false;}
}
impl Drop for RelaunchGuard {
    fn drop(&mut self) {
        if !self.armed {return;}
        let (cano,key,dir,settings)=(self.cano.clone(),self.key.clone(),self.dir.clone(),self.settings.clone());
        tokio::spawn(async move {
            if let Err(error)=kill(&cano,&key,&dir).await {
                tracing::warn!(key,code=error.code(),"cano sem confirmação não foi encerrado; settings privados conservados");
                while liveness(cano.pid,&key)==Liveness::Ours && cano.extra.get("process_birth").and_then(Value::as_f64).is_some_and(|birth|crate::terminal_facts::agent_birth(cano.pid)==Some(birth)) {tokio::time::sleep(Duration::from_secs(1)).await;}
            }
            drop(settings);
        });
    }
}

pub async fn spawn_recipe(spec:PreparedRecipe)->Result<Cano,ProcessError> {
    let mut waits = SCOPE_RETRY.iter();
    loop {
        match spawn_once(&spec).await {
            Ok(cano) => return Ok(cano),
            Err(Started::Failed(error)) => return Err(error),
            Err(Started::UnitMissing(refusal)) => {
                let Some(wait) = waits.next() else {
                    tracing::warn!(code = SCOPE_UNIT_MISSING, "escopo do systemd recusado em todas as tentativas; o cano não subiu");
                    return Err(ProcessError::Spawn(refusal));
                };
                let wait = scope_wait(*wait);
                tracing::warn!(code = SCOPE_UNIT_MISSING, wait_ms = wait.as_millis() as u64, "escopo do systemd recusado ao subir o cano; nova tentativa");
                tokio::time::sleep(wait).await;
            }
        }
    }
}

enum Started { UnitMissing(String), Failed(ProcessError) }

async fn spawn_once(spec:&PreparedRecipe)->Result<Cano,Started> {
    use std::io::{Read,Seek};
    let escuta = new_listen(&spec.key, &spec.sidecar_dir);
    let token = crate::mods::state::random_hex(16);
    let log = spec.sidecar_dir.join(format!("cano-{}.log", key16(&spec.key)));
    let mut argv=spec.prefix.clone();
    argv.push(spec.bin.clone().into_os_string());
    for (flag, value) in [("--escuta", std::ffi::OsStr::new(&escuta)), ("--log", log.as_os_str()),
                          ("--cwd", spec.cwd.as_os_str()), ("--token", std::ffi::OsStr::new(&token))] {
        argv.push(flag.into());
        argv.push(value.to_owned());
    }
    argv.push("--".into());
    argv.extend(spec.program.iter().cloned());
    let mut env=spec.env.clone();
    // O `systemd-run` acha o gerenciador do usuário pelo barramento; a sondagem o provou com o do
    // servidor, e o ambiente da sessão não precisa trazê-lo.
    if !spec.prefix.is_empty() {
        for key in ["XDG_RUNTIME_DIR","DBUS_SESSION_BUS_ADDRESS"] {
            if !env.iter().any(|(name,_)|name==key) && let Some(value)=std::env::var_os(key) {env.push((key.into(),value));}
        }
    }
    // A recusa do `systemd-run` sai no stderr antes do exec; o cano escreve no `--log`. Arquivo já apagado
    // do disco: só este processo o lê, e nada sobra na pasta da sessão.
    let mut scope_err = None;
    let stderr = if spec.prefix.is_empty() { std::process::Stdio::null() } else {
        let path = spec.sidecar_dir.join(format!("cano-{}-{}.scope-err", key16(&spec.key), crate::mods::state::random_hex(2)));
        let file = std::fs::File::options().read(true).write(true).create_new(true).open(&path)
            .map_err(|e| Started::Failed(ProcessError::Spawn(format!("stderr do escopo: {}", e.kind()))))?;
        let _ = std::fs::remove_file(&path);
        let child_side = file.try_clone().map_err(|e| Started::Failed(ProcessError::Spawn(format!("stderr do escopo: {}", e.kind()))))?;
        scope_err = Some(file);
        child_side.into()
    };
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(&spec.cwd).env_clear().envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(stderr);
    #[cfg(unix)]
    // SAFETY: `setsid` é async-signal-safe e não toca memória do processo pai.
    unsafe { cmd.pre_exec(|| if libc::setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }); }
    #[cfg(windows)]
    cmd.creation_flags(0x0000_0200 | 0x0800_0000); // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
    let mut child = cmd.spawn().map_err(|e| Started::Failed(ProcessError::Spawn(format!("não subiu: {}", e.kind()))))?;
    let pid = child.id().ok_or_else(|| Started::Failed(ProcessError::Spawn("saiu ao nascer".into())))?;
    let birth=match crate::terminal_facts::agent_birth(pid) {
        Some(birth)=>birth,
        None=> {
            child.kill().await.map_err(|_|Started::Failed(ProcessError::StillAlive))?;
            child.wait().await.map_err(|_|Started::Failed(ProcessError::StillAlive))?;
            return Err(Started::Failed(ProcessError::Spawn("nascimento do cano não comprovado".into())));
        },
    };
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    let mut extra=spec.extra.clone();extra.insert("process_birth".into(),serde_json::json!(birth));
    let mut cano = Cano { pid, escuta, token, ts, versao: 2, extra };
    let binding = super::protocol::CanoBinding { pid, escuta: cano.escuta.clone(), token: cano.token.clone(), versao: 2 };
    // Processo que sai antes de escutar (o escopo recusado) responde na hora, sem esperar o prazo da escuta.
    let listened = tokio::select! {
        result = super::cano::connect(&binding) => Some(result),
        _ = child.wait() => None,
    };
    if let Some(Ok(connection)) = listened {
        // Só para não deixar zumbi.
        tokio::spawn(async move { let _ = child.wait().await; });
        cano.versao = connection.snapshot.versao;
        return Ok(cano);
    }
    let stderr = scope_err.map(|mut file| {
        let mut text = Vec::new();
        let _ = file.rewind().and_then(|()| file.read_to_end(&mut text));
        text
    }).unwrap_or_default();
    let refusal = scope_unit_missing(&stderr);
    // O arquivo junta o `systemd-run` (antes do exec) e o cano (depois): a causa vai ao log nos dois casos.
    let cause = first_line(&stderr);
    // ponytail: quem escreveu se reconhece pelo estilo do systemd ("Failed to …"); o cano escreve no `--log`.
    let scope_refused = refusal.is_none() && listened.is_none() && cause.as_deref().is_some_and(|line| line.starts_with("Failed to "));
    match &listened {
        _ if refusal.is_some() => {},
        _ if scope_refused => tracing::warn!(pid, code = SCOPE_REFUSED, cause, "escopo do systemd recusado ao subir o cano"),
        Some(Err(error)) => tracing::warn!(pid, code = %error.code, cause, "cano não escutou; matando o que subiu"),
        _ => tracing::warn!(pid, code = "cano_saiu_antes_de_escutar", cause, "cano saiu antes de escutar"),
    }
    if let Err(failure) = kill(&cano, &spec.key, &spec.sidecar_dir).await {
        tracing::warn!(pid, code = failure.code(), "cano que não escutou não foi encerrado");
    }
    if listened.is_some() { tokio::spawn(async move { let _ = child.wait().await; }); }
    Err(match (refusal, cause) {
        (Some(refusal), _) => Started::UnitMissing(refusal),
        (None, Some(cause)) if scope_refused => Started::Failed(ProcessError::Spawn(format!("escopo do systemd recusado: {cause}"))),
        _ => Started::Failed(ProcessError::NotListening),
    })
}

/// Mata o grupo do cano (cano + agente) só se o pid ainda for o cano da chave e, confirmada a saída,
/// apaga os rastros dele na pasta da sessão (`sidecar_dir`): o socket desta vida e, se nenhuma outra
/// vida da chave tem socket ali nem processo vivo, os demais `cano-<key16>*`. Pid de outro programa: não mata.
pub async fn kill(cano: &Cano, key: &str, sidecar_dir: &Path) -> Result<(), ProcessError> {
    let (pid, key_owned) = (cano.pid, key.to_owned());
    let state = tokio::task::spawn_blocking(move || identify(pid, &key_owned)).await
        .map_err(|_| ProcessError::StillAlive)?;
    if state==Liveness::Ours && cano.extra.get("process_birth").and_then(Value::as_f64).is_some_and(|birth|crate::terminal_facts::agent_birth(pid)!=Some(birth)) {
        return Err(ProcessError::StillAlive);
    }
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
pub(crate) fn probe_alive(pid: u32) -> bool { probe(pid).is_some_and(|(argv, _)| !argv.is_empty()) }
#[cfg(not(target_os = "linux"))]
pub(crate) fn probe_alive(pid: u32) -> bool { probe(pid).is_some() }

/// Encerra o app-server adotado do Codex com terminal. No Unix só o pid: ele mora no grupo do pane, e
/// `killpg` levaria a TUI junto. No Windows a árvore, que pega o `cmd.exe` do `codex.cmd`.
pub fn terminate(pid: u32) -> Result<(), ProcessError> {
    if pid <= 1 { return Err(ProcessError::StillAlive); }
    terminate_one(pid)
}

#[cfg(unix)]
fn terminate_one(pid: u32) -> Result<(), ProcessError> {
    // SAFETY: sinal num pid conferido por quem chama; erro lido logo depois.
    if unsafe { libc::kill(pid as i32, libc::SIGTERM) } == 0 { return Ok(()); }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(ProcessError::StillAlive) }
}

#[cfg(windows)]
fn terminate_one(pid: u32) -> Result<(), ProcessError> { signal_group(pid) }

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

#[cfg(test)]
mod tests {
    #[test]
    fn cano_with_a_non_canonical_timestamp_still_reconnects() {
        let raw = r#"{"pid":1,"escuta":"x","token":"t","versao":1,"ts":1.70e9,"outro":true}"#;
        let meta = serde_json::from_str::<serde_json::Value>(raw).unwrap();
        let cano = serde_json::from_value::<super::Cano>(meta).unwrap();
        assert_eq!(cano.ts, 1.7e9);
        assert_eq!(cano.extra["outro"], true);
    }
}

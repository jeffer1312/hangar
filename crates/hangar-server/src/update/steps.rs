//! Passos de máquina (`tipo: maquina`) de `docs/atualizacoes/` da versão nova. Leitura, registro e
//! aplicação compatíveis com `backend/app/atualizacoes.py`: o mesmo frontmatter, o mesmo
//! `aplicados.json` e a mesma regra de prova, para os dois motores concordarem sobre o que já rodou.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::state::StateFile;
use super::UpdateError;

/// Teto de um passo, o `_TIMEOUT` do Python.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub id: String,
    pub title: String,
    /// O comando deste sistema (`comando_windows`/`comando_posix`, senão `comando`); vazio = só texto.
    pub command: String,
    /// Caminhos que têm que existir depois do comando.
    pub proof: Vec<String>,
    pub destructive: bool,
    pub machine: bool,
}

/// Saída de um comando de passo: código (`None` = morto ou estourou o prazo) e o texto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRun {
    pub code: Option<i32>,
    pub output: String,
}

/// Quem roda o comando de um passo. O real é o shell do sistema; o teste troca.
pub trait StepRunner {
    fn run(&self, command: &str, cwd: &Path) -> StepRun;
}

/// O shell do sistema, como o `shell=True` do Python: `sh -c` no POSIX, `cmd /c` no Windows.
pub struct ShellRunner {
    pub timeout: Duration,
}

impl StepRunner for ShellRunner {
    fn run(&self, command: &str, cwd: &Path) -> StepRun {
        let mut cmd = shell(command);
        // O instalador chamado pelo passo não baixa o binário do Rust: dentro da atualização só ela baixa.
        cmd.current_dir(cwd).env("LC_ALL", "C").env("LANGUAGE", "C").env("HANGAR_UPDATE_OWNS_RUST", "1")
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Grupo próprio: o prazo estourado mata o passo e os filhos dele de uma vez.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => return StepRun { code: None, output: format!("não consegui rodar o shell: {e}") },
        };
        // Leitores com buffer compartilhado: o que chegou até o prazo vale, mesmo com o cano ainda aberto.
        let (closed, closes) = std::sync::mpsc::channel::<()>();
        let buffers = [child.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>), child.stderr.take().map(|e| Box::new(e) as Box<dyn Read + Send>)]
            .map(|pipe| {
                let buffer = Arc::new(Mutex::new(Vec::new()));
                let (sink, closed) = (buffer.clone(), closed.clone());
                std::thread::spawn(move || {
                    let mut chunk = [0u8; 8192];
                    if let Some(mut pipe) = pipe {
                        while let Ok(n @ 1..) = pipe.read(&mut chunk) {
                            sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner).extend_from_slice(&chunk[..n]);
                        }
                    }
                    let _ = closed.send(());
                });
                buffer
            });
        let deadline = Instant::now() + self.timeout;
        let code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code(),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                _ => {
                    kill_tree(&mut child);
                    let _ = child.wait();
                    break None;
                }
            }
        };
        // Um neto que herdou o cano (serviço que o passo sobe) o mantém aberto: espera um pouco e segue.
        let grace = Instant::now() + READ_GRACE;
        for _ in 0..buffers.len() {
            if closes.recv_timeout(grace.saturating_duration_since(Instant::now())).is_err() { break; }
        }
        let output = buffers.iter().map(|b| String::from_utf8_lossy(&b.lock().unwrap_or_else(std::sync::PoisonError::into_inner)).into_owned())
            .collect::<Vec<_>>().concat();
        StepRun { code, output }
    }
}

/// Quanto se espera o cano fechar depois que o passo saiu.
const READ_GRACE: Duration = Duration::from_secs(2);

/// O passo e tudo o que ele abriu: o grupo de processos próprio no Unix, a árvore no Windows.
#[cfg(unix)]
fn kill_tree(child: &mut std::process::Child) {
    if let Ok(group) = i32::try_from(child.id()) {
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    let _ = child.kill();
}

#[cfg(windows)]
fn kill_tree(child: &mut std::process::Child) {
    use std::os::windows::process::CommandExt;
    let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &child.id().to_string()])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(0x0800_0000).status();
    let _ = child.kill();
}

#[cfg(unix)]
fn shell(command: &str) -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(command);
    cmd
}

/// `cmd.exe /c "<linha>"`, a mesma linha que o `subprocess` monta com `shell=True`, sem janela.
#[cfg(windows)]
fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()));
    cmd.raw_arg(format!("/c \"{command}\"")).creation_flags(0x0800_0000);
    cmd
}

/// Frontmatter `chave: valor` entre `---`, e o corpo, como `_frontmatter` do Python.
pub fn frontmatter(text: &str) -> (BTreeMap<String, String>, String) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return (BTreeMap::new(), text.to_owned());
    }
    let Some(end) = lines.iter().skip(1).position(|l| *l == "---").map(|i| i + 1) else {
        return (BTreeMap::new(), text.to_owned());
    };
    let mut fields = BTreeMap::new();
    for line in &lines[1..end] {
        if line.trim().is_empty() || line.trim_start().starts_with('#') { continue; }
        if let Some((key, value)) = line.split_once(':') {
            fields.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    (fields, lines[end + 1..].join("\n").trim().to_owned())
}

pub(crate) fn step(fields: &BTreeMap<String, String>, stem: &str, windows: bool) -> Option<Step> {
    let get = |k: &str| fields.get(k).map(String::as_str).unwrap_or("");
    if get("titulo").is_empty() { return None; }
    let has_command = ["comando", "comando_posix", "comando_windows"].iter().any(|k| !get(k).is_empty());
    // Comando sem prova faria "saiu com 0" virar "deu certo": o arquivo é ignorado.
    if has_command && get("prova").is_empty() { return None; }
    let own = get(if windows { "comando_windows" } else { "comando_posix" });
    let command = if own.is_empty() { get("comando") } else { own }.trim().to_owned();
    Some(Step {
        id: Some(get("id")).filter(|id| !id.is_empty()).unwrap_or(stem).to_owned(),
        title: get("titulo").to_owned(),
        // Comando só do outro sistema: aqui é texto, e a prova é do efeito de lá.
        proof: if has_command && command.is_empty() { Vec::new() } else { get("prova").split_whitespace().map(str::to_owned).collect() },
        command,
        destructive: matches!(get("destrutivo").to_lowercase().as_str(), "true" | "sim" | "1"),
        machine: get("tipo") == "maquina",
    })
}

/// Os passos válidos de `dir` em ordem de nome e os arquivos ignorados por estarem malformados.
pub fn read_steps(dir: &Path, windows: bool) -> (Vec<Step>, Vec<String>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .filter(|p| !p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_uppercase().starts_with("README")))
        .collect();
    files.sort();
    let (mut steps, mut invalid) = (Vec::new(), Vec::new());
    for file in files {
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_owned();
        let stem = file.file_stem().and_then(|n| n.to_str()).unwrap_or_default();
        match std::fs::read_to_string(&file).ok().and_then(|text| step(&frontmatter(&text).0, stem, windows)) {
            Some(step) => steps.push(step),
            None => invalid.push(name),
        }
    }
    (steps, invalid)
}

pub fn registry_path(state_dir: &Path) -> PathBuf { state_dir.join("aplicados.json") }

/// Ids já aplicados nesta máquina; arquivo ausente = nenhum.
pub fn applied(state_dir: &Path) -> Result<BTreeSet<String>, String> {
    let path = registry_path(state_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(format!("não consegui ler {}: {e}", path.display())),
    };
    let ids = serde_json::from_slice::<Vec<serde_json::Value>>(&bytes)
        .map_err(|e| format!("{} ilegível ({e}); conserte ou apague o arquivo", path.display()))?;
    Ok(ids.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
}

/// Lista ordenada de strings, como o `_gravar` do Python.
pub(crate) fn mark(state_dir: &Path, id: &str) -> Result<(), UpdateError> {
    record(state_dir, [id.to_owned()]).map_err(|e| UpdateError::Step(format!("não consegui registrar o passo {id}: {e}")))
}

fn record(state_dir: &Path, new: impl IntoIterator<Item = String>) -> std::io::Result<()> {
    // Registro ilegível não é sobrescrito: levaria o histórico junto.
    let mut ids = applied(state_dir).map_err(std::io::Error::other)?;
    ids.extend(new);
    std::fs::create_dir_all(state_dir)
        .and_then(|()| crate::runtime::queue::atomic_write(&registry_path(state_dir), &serde_json::to_vec(&ids).unwrap_or_default()))
}

/// Registra todo passo válido de `root/docs/atualizacoes` sem rodar nenhum, como o `marcar_todos`
/// do Python: a instalação do zero já cumpre o que eles levam às máquinas antigas. Devolve quantos
/// marcou e os arquivos malformados, que ficaram de fora.
pub fn mark_all(root: &Path, state_dir: &Path) -> Result<(usize, Vec<String>), UpdateError> {
    let (steps, invalid) = read_steps(&root.join("docs").join("atualizacoes"), cfg!(windows));
    let count = steps.len();
    record(state_dir, steps.into_iter().map(|s| s.id))
        .map_err(|e| UpdateError::Step(format!("não consegui registrar os passos como aplicados: {e}")))?;
    Ok((count, invalid))
}

/// Caminho da prova: absoluto, com `~`, ou relativo à raiz da versão.
fn exists(proof: &str, root: &Path, home: &Path) -> bool {
    let path = match proof.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => home.join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(proof),
    };
    if path.is_absolute() { path.exists() } else { root.join(path).exists() }
}

fn failure(step: &Step, why: &str) -> UpdateError { UpdateError::Step(format!("{}: {why}", step.title)) }

/// Roda os passos de máquina pendentes de `root/docs/atualizacoes`, em ordem, e registra cada um
/// só depois da prova. Para no primeiro que falhar. Comando igual ao do passo imediatamente
/// anterior não roda de novo; a prova de cada um continua conferida.
pub fn apply_machine_steps(root: &Path, home: &Path, state_dir: &Path, runner: &dyn StepRunner, state: &StateFile)
    -> Result<Vec<String>, UpdateError> {
    let (steps, invalid) = read_steps(&root.join("docs").join("atualizacoes"), cfg!(windows));
    if !invalid.is_empty() {
        state.write(serde_json::json!({"passos_invalidos": invalid}));
    }
    let done_before = applied(state_dir).map_err(UpdateError::Step)?;
    let execute = |step: &Step| -> Result<(), UpdateError> {
        state.log(&format!("$ {}", step.command));
        let run = runner.run(&step.command, root);
        if !run.output.trim().is_empty() { state.log(run.output.trim_end()); }
        if run.code == Some(0) { return Ok(()); }
        let marked: Vec<&str> = run.output.lines().filter_map(|l| l.split_once("##HANGAR-FALHA##").map(|(_, why)| why.trim())).collect();
        let lines: Vec<&str> = run.output.trim().lines().collect();
        let tail = lines[lines.len().saturating_sub(8)..].join("\n");
        let why = if !marked.is_empty() { marked.join("\n") } else if !tail.is_empty() { tail } else {
            run.code.map_or_else(|| "estourou o prazo".to_owned(), |c| format!("saiu com {c}"))
        };
        Err(failure(step, &why))
    };
    let mut done = Vec::new();
    let mut previous: Option<String> = None;
    for step in steps.iter().filter(|s| s.machine && !done_before.contains(&s.id)) {
        let repeated = !step.command.is_empty() && previous.as_deref() == Some(step.command.as_str());
        if !step.command.is_empty() && !repeated { execute(step)?; }
        let missing = |s: &Step| s.proof.iter().filter(|p| !exists(p, root, home)).cloned().collect::<Vec<_>>();
        let mut absent = missing(step);
        if !absent.is_empty() && repeated {
            execute(step)?;
            absent = missing(step);
        }
        if !absent.is_empty() {
            return Err(failure(step, &format!("o passo rodou mas não deixou: {}", absent.join(" "))));
        }
        mark(state_dir, &step.id)?;
        if !step.command.is_empty() { previous = Some(step.command.clone()); }
        done.push(step.id.clone());
    }
    Ok(done)
}

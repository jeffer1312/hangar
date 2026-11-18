//! Raiz da instalação do Hangar, o Python dela e o lançador do backend. A mesma regra de
//! `backend/app/install_root.py`.
//!
//!   hangar-server launch backend [ARGS]   lê o `~/.hangar/current` uma vez, resolve a versão real e
//!                                         abre o `hangar-server` dela (`boot: direct`) ou, na versão
//!                                         que ainda sobe pelo Python, `-m app.main ARGS`
//!
//! Sem o `current` vale o checkout do lançador; sem nenhum dos dois sai com 3 e `launch_no_current`;
//! uso errado, 2. No Windows o lançador fica de pé e religa o `hangar-server` que cai (decisão 38).
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub use hangar_core::install_root::*;

/// `hangar-server launch …`: `args` sem o nome do programa nem o `launch`.
pub fn launch(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) != Some("backend") {
        eprintln!("uso: hangar-server launch backend [ARGS...]");
        return ExitCode::from(2);
    }
    let home = crate::accounts::environment::AccountEnvironment::capture().home;
    let log = args.iter().position(|a| a == "--log").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    // A versão vai em toda linha do lançador: na troca, o log recebe a saída da anterior e da nova.
    let version = version_of(&home);
    let explicit = std::env::var_os("CP_RUST_SERVER_BIN");
    let mut lines = vec![format!("launch_start: versão {version}, lançador pid {}", std::process::id())];
    let chosen = backend_noted(&home, &args[1..], cfg!(windows), explicit.as_deref(), &mut lines);
    // O direto no Windows gira o log antes de escrever: estas linhas abrem o arquivo da vida nova.
    let tell = |lines: &[String]| { for line in lines { report(log.as_deref(), line); } };
    match chosen {
        #[cfg(unix)]
        Ok(Backend::Python(command)) => { tell(&lines); run(command, log.as_deref(), &version) }
        Ok(backend) => {
            let mut again = |notes: &mut Vec<String>| relaunch_backend(&home, &args[1..], cfg!(windows), explicit.as_deref(), notes);
            run_direct(backend, log.as_deref(), &version, lines, &home.join(".hangar"), &mut again)
        }
        Err((code, path)) => {
            tell(&lines);
            report(log.as_deref(), &format!("{code}: {}", path.display()));
            failed(log.as_deref(), "erro", code, &path.display().to_string());
            ExitCode::from(3)
        }
    }
}

/// O nome da versão para onde o `current` aponta agora; `?` sem ele.
fn version_of(home: &Path) -> String {
    std::fs::canonicalize(current(home)).ok()
        .and_then(|real| real.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "?".to_owned())
}

/// O servidor da religada, resolvido de novo como na primeira subida: depois de uma atualização o
/// `current` aponta para outra versão, e o binário antigo pode já ter sido podado. A versão que
/// sobe pelo Python (a volta da atualização) é religada pelo Python dela: nada mais traz de volta
/// um lançador que sai.
pub(crate) fn relaunch_backend(home: &Path, args: &[String], windows: bool, explicit: Option<&std::ffi::OsStr>, notes: &mut Vec<String>)
    -> Result<(Backend, String), (&'static str, PathBuf)> {
    let version = version_of(home);
    Ok((backend_noted(home, args, windows, explicit, notes)?, version))
}

/// `lancador.falhou` no diário: o gerenciador religa sem fim, e a causa fica no diário que o
/// relatório leva. `nivel` aviso quando o servidor sobe mesmo assim. O lançador não tem log
/// próprio: a falha de gravar vai ao log da tarefa e ao stderr.
fn failed(log: Option<&Path>, level: &str, code: &str, detail: &str) {
    record(log, code, serde_json::json!({"evento": "lancador.falhou", "nivel": level, "codigo": code, "detalhe": detail}));
}

/// Uma linha do lançador no diário; a falha de gravar vai ao log da tarefa e ao stderr, com `what`.
fn record(log: Option<&Path>, what: &str, line: serde_json::Value) {
    let written = match hangar_core::diag::diary_dir() {
        Some(dir) => hangar_core::diag::write_diary(&dir, &[line], chrono::Local::now()).map_err(|error| format!("{:?}", error.kind())),
        None => Err("sem pasta de logs".to_owned()),
    };
    if let Err(cause) = written {
        report(log, &format!("launch_diary_unwritable: {what} não gravado no diário ({cause})"));
    }
}

/// A tarefa do Windows roda o lançador sem console: o erro dele vai também ao log do backend.
fn report(log: Option<&Path>, line: &str) {
    eprintln!("hangar-server launch: {line}");
    if let Some(log) = log {
        use std::io::Write;
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(log)
            .and_then(|mut f| writeln!(f, "[hangar] hangar-server launch: {line}"));
    }
}

/// No Unix o lançador vira o backend: o serviço acompanha o próprio Python.
#[cfg(unix)]
fn run(mut command: std::process::Command, log: Option<&Path>, version: &str) -> ExitCode {
    use std::os::unix::process::CommandExt;
    if let Err(error) = crate::boot_version::PythonLaunch::capture(&command).and_then(|launch| launch.record_exec()) {
        report(log, &format!("launch_python_version_unavailable: {error}"));
    }
    // A unit é `Type=notify` com `WatchdogSec` para o `hangar-server`; a versão que sobe pelo Python
    // não avisa: o lançador diz pronto e desliga o watchdog antes de virar o Python.
    #[cfg(target_os = "linux")]
    if let Err(error) = notify_from_env("READY=1\nWATCHDOG_USEC=0") {
        report(log, &format!("launch_notify_failed: {error}"));
    }
    let error = command.exec();
    report(log, &format!("launch_exec_failed: versão {version}: {error}"));
    failed(log, "erro", "launch_exec_failed", &Path::new(command.get_program()).display().to_string());
    ExitCode::from(126)
}

/// O servidor da religada (`relaunch_backend`), com as notas da escolha.
type Again<'a> = dyn FnMut(&mut Vec<String>) -> Result<(Backend, String), (&'static str, PathBuf)> + 'a;

/// O lançador vira o `hangar-server`: mesmo pid, então o `sd_notify` dele vale para a unit e a vida
/// é dele. Quem religa a queda é o systemd/launchd.
#[cfg(unix)]
fn run_direct(backend: Backend, log: Option<&Path>, version: &str, lines: Vec<String>, _hangar_dir: &Path,
              _again: &mut Again<'_>) -> ExitCode {
    use std::os::unix::process::CommandExt;
    let (Backend::Direct(mut command) | Backend::Python(mut command)) = backend;
    for line in &lines { report(log, line); }
    let error = command.exec();
    report(log, &format!("launch_exec_failed: versão {version}: {error}"));
    failed(log, "erro", "launch_exec_failed", &Path::new(command.get_program()).display().to_string());
    ExitCode::from(126)
}

/// Sem `NOTIFY_SOCKET` (unit `Type=simple`, rodando na mão) não há a quem avisar.
#[cfg(target_os = "linux")]
fn notify_from_env(state: &str) -> std::io::Result<()> {
    hangar_boot::notify::Notifier::from_lookup(|key| std::env::var(key).ok(), std::process::id()).send(state)
}

/// No Windows: o servidor no Job do lançador, com o log da tarefa como saída (o Python o abria sozinho).
/// O Agendador não religa quem cai depois de subir (decisão 38): o servidor que sai sem parada pedida
/// (`exit_is_requested_stop`) volta depois da espera de `Relaunch`, sem desistir, resolvido de novo pelo
/// `current`, também a versão que sobe pelo Python. O lançador só sai com a parada pedida ou morto;
/// morto, o Job leva o servidor.
#[cfg(windows)]
fn run_direct(backend: Backend, log: Option<&Path>, version: &str, lines: Vec<String>, hangar_dir: &Path,
              again: &mut Again<'_>) -> ExitCode {
    let (mut command, mut python) = split(backend);
    let mut version = version.to_owned();
    let launcher = std::process::id();
    let mut relaunch = Relaunch::new(relaunch_first());
    let (mut pending, mut previous_fall) = (lines, None::<u32>);
    loop {
        // Cada vida gira e abre o log de novo, num handle só e em acréscimo: o lançador escreve antes
        // de abrir o servidor e depois que ele sai, nunca junto com ele.
        let (file, held) = match log.map(|target| (target, open_log(target))) {
            None => (None, None),
            Some((target, Ok((file, used)))) => {
                let held = (used != target).then(|| format!("AVISO: {} está preso por outro processo; a saída vai para {}", target.display(), used.display()));
                (Some(file), held)
            }
            Some((target, Err(error))) => {
                report(log, &format!("launch_log_unavailable: {}: {error}", target.display()));
                failed(log, "aviso", "launch_log_unavailable", &format!("{:?}", error.kind()));
                (None, None)
            }
        };
        // `launch_start` e as notas da escolha abrem o arquivo da primeira vida.
        for line in pending.drain(..).chain(held) { say(file.as_ref(), log, &line); }
        let after = previous_fall.map(|fall| format!(", depois da queda {fall}")).unwrap_or_default();
        say(file.as_ref(), log, &format!("launch_backend_starting: versão {version}, lançador pid {launcher}{after}"));
        if previous_fall.is_none() && let Err(error) = die_with_the_launcher() {
            say(file.as_ref(), log, &format!("launch_job_failed: {error}"));
            failed(log, "aviso", "launch_job_failed", &format!("{:?}", error.kind()));
        }
        match file.as_ref().map(|file| (file.try_clone(), file.try_clone())) {
            Some((Ok(out), Ok(err))) => { command.stdout(out).stderr(err); }
            Some((Err(error), _) | (_, Err(error))) => {
                say(file.as_ref(), log, &format!("launch_log_unavailable: {error}"));
                failed(log, "aviso", "launch_log_unavailable", &format!("{:?}", error.kind()));
                command.stdout(std::process::Stdio::inherit()).stderr(std::process::Stdio::inherit());
            }
            None => { command.stdout(std::process::Stdio::inherit()).stderr(std::process::Stdio::inherit()); }
        }
        // A versão que sobe pelo Python precisa do interpretador efetivo registrado, como na subida dela.
        let snapshot = python.then(|| crate::boot_version::PythonLaunch::capture(&command)
            .inspect_err(|error| say(file.as_ref(), log, &format!("launch_python_version_unavailable: {error}"))).ok()).flatten();
        let started = std::time::Instant::now();
        let (ended, exit) = match command.spawn() {
            Err(error) => {
                failed(log, "erro", "launch_exec_failed", &format!("{}: {:?}", Path::new(command.get_program()).display(), error.kind()));
                (format!("launch_exec_failed: versão {version}: {error}"), None)
            }
            Ok(mut child) => {
                let pid = child.id();
                let who = if python { "python" } else { "hangar-server" };
                if let Some(snapshot) = snapshot { record_python_launch(&snapshot, &mut child, file.as_ref(), log); }
                match wait_watching(&mut child, hangar_dir, file.as_ref(), log) {
                    // Sem saber se o servidor saiu, ele é encerrado antes da religada: dois na mesma porta não sobem.
                    Err(error) => {
                        failed(log, "erro", "launch_wait_failed", &format!("{:?}", error.kind()));
                        let killed = child.kill().and_then(|()| child.wait().map(drop));
                        let killed = killed.err().map(|e| format!("; encerrar falhou: {e}")).unwrap_or_default();
                        (format!("launch_wait_failed: versão {version}, {who} pid {pid}: {error}{killed}"), None)
                    }
                    Ok(status) => {
                        let exited = format!("launch_backend_exited: versão {version}, {who} pid {pid}, {status}, depois de {} s", started.elapsed().as_secs());
                        if exit_is_requested_stop(status.code()) {
                            if !status.success() { say(file.as_ref(), log, &exited); }
                            let life = close_life(hangar_dir, pid, file.as_ref(), log);
                            record(log, "lancador.parada", serde_json::json!({"evento": "lancador.parada", "nivel": "ok",
                                "saida": format!("0x{:08X}", status.code().unwrap_or(0) as u32), "vida_s": started.elapsed().as_secs(), "vida": life}));
                            std::process::exit(status.code().unwrap_or(1));
                        }
                        (exited, status.code())
                    }
                }
            }
        };
        // O `Command` guarda as cópias do handle: soltas aqui, o log gira sem arquivo aberto.
        command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let lived = started.elapsed();
        let (fall, wait) = relaunch.fall(lived);
        say(file.as_ref(), log, &format!("{ended}; religa em {} ms (queda {fall})", wait.as_millis()));
        // A queda vai ao diário que o relatório leva: o servidor só anota a vida que não parou limpa
        // na subida seguinte, sem o código de saída.
        if let Some(code) = exit {
            record(log, "lancador.queda", serde_json::json!({"evento": "lancador.queda", "nivel": "erro", "codigo": "server_exited",
                "saida": format!("0x{:08X}", code as u32), "vida_s": lived.as_secs(), "queda": fall, "espera_ms": wait.as_millis() as u64}));
        }
        drop(file);
        std::thread::sleep(wait);
        previous_fall = Some(fall);
        // Depois da espera, quando uma atualização pode ter trocado o `current`. Falha pode ser a
        // troca no meio: tenta de novo.
        loop {
            let mut notes = Vec::new();
            match again(&mut notes) {
                Ok((fresh, fresh_version)) => {
                    pending.extend(notes);
                    ((command, python), version) = (split(fresh), fresh_version);
                    break;
                }
                Err((code, path)) => {
                    for note in &notes { report(log, note); }
                    failed(log, "erro", code, &path.display().to_string());
                    let (fall, wait) = relaunch.fall(std::time::Duration::ZERO);
                    report(log, &format!("{code}: {}; tenta de novo em {} ms", path.display(), wait.as_millis()));
                    std::thread::sleep(wait);
                    previous_fall = Some(fall);
                }
            }
        }
    }
}

/// Espera o servidor sair conferindo a saúde dele, como o `WatchdogSec` do systemd no Linux: travado,
/// o lançador o encerra (código 1, que não é parada pedida) e a queda segue para a religada.
#[cfg(windows)]
fn wait_watching(child: &mut std::process::Child, hangar_dir: &Path, file: Option<&std::fs::File>, log: Option<&Path>)
    -> std::io::Result<std::process::ExitStatus> {
    let pid = child.id();
    let mut watch = crate::watchdog::LauncherWatch::new(std::time::Instant::now());
    let mut hung = false;
    loop {
        if let Some(status) = child.try_wait()? { return Ok(status); }
        let verdict = watch.tick(std::time::Instant::now(), || hangar_boot::life::health(hangar_dir, pid),
            |address| crate::watchdog::health_answers_blocking(address, crate::watchdog::TIMEOUT));
        if let Some(crate::watchdog::Verdict::Withheld(rounds)) = verdict {
            if !hung {
                hung = true;
                say(file, log, &format!("launch_backend_hung: hangar-server pid {pid} sem responder a saúde em {rounds} rodadas seguidas; encerrando"));
                record(log, "lancador.travado", serde_json::json!({"evento": "lancador.travado", "nivel": "erro", "codigo": "server_hung", "rodadas": rounds}));
            }
            // O reinício só é contado para o `hangar-doctor` com o filho de fato fora.
            match child.kill().and_then(|()| child.wait()) {
                Ok(status) => {
                    note_restart_for_doctor(pid, rounds, file, log);
                    return Ok(status);
                }
                // Sem encerrar não há religada: a próxima rodada tenta de novo.
                Err(error) => say(file, log, &format!("launch_kill_failed: hangar-server pid {pid}: {error}")),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// O `hangar-doctor` conta os reinícios do vigia por esta linha, no log que a tarefa `hangar-vigia`
/// escrevia antes de o vigia morar no lançador.
#[cfg(windows)]
fn note_restart_for_doctor(pid: u32, rounds: u32, file: Option<&std::fs::File>, log: Option<&Path>) {
    use std::io::Write;
    let Some(base) = hangar_core::paths::log_base() else { return };
    let path = base.join("privado").join("hangar-vigia.log");
    let line = format!("{} vigia: hangar-backend sem resposta HTTP; o lançador encerrou o hangar-server pid {pid} depois de {rounds} rodadas\n",
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"));
    let written = std::fs::create_dir_all(base.join("privado"))
        .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(&path))
        .and_then(|mut target| target.write_all(line.as_bytes()));
    if let Err(error) = written { say(file, log, &format!("launch_vigia_log_unwritable: {}: {error}", path.display())); }
}

/// A parada pedida que não passou pelos tratadores do servidor (`Stop-Process -Force` é `TerminateProcess`)
/// deixa a marca de pé dele: o lançador a fecha como parada limpa, só se ela for desse filho. Com o
/// handle do filho ainda aberto, o pid dele não foi reusado.
#[cfg(windows)]
fn close_life(hangar_dir: &Path, pid: u32, file: Option<&std::fs::File>, log: Option<&Path>) -> &'static str {
    match hangar_boot::life::end(hangar_dir, pid) {
        Ok(hangar_boot::life::Ended::Cleared) => "limpa",
        Ok(hangar_boot::life::Ended::OtherLife) => "outra",
        Ok(hangar_boot::life::Ended::Reset) => "ilegivel",
        Err(error) => {
            say(file, log, &format!("launch_life_unwritable: marca de pé do hangar-server pid {pid} não fechada: {error}"));
            failed(log, "aviso", "launch_life_unwritable", &format!("{:?}", error.kind()));
            "erro"
        }
    }
}

/// A primeira espera da religada. `HANGAR_LAUNCH_RELAUNCH_FIRST_MS` só existe para os testes, que
/// não esperam segundos por queda.
#[cfg(windows)]
fn relaunch_first() -> std::time::Duration {
    std::env::var("HANGAR_LAUNCH_RELAUNCH_FIRST_MS").ok().and_then(|ms| ms.parse().ok())
        .map(std::time::Duration::from_millis).unwrap_or(RELAUNCH_FIRST)
}

/// Linha do lançador pelo handle do log; sem ele, pelo `report` de sempre.
#[cfg(windows)]
fn say(file: Option<&std::fs::File>, log: Option<&Path>, line: &str) {
    use std::io::Write;
    match file {
        Some(mut file) => {
            eprintln!("hangar-server launch: {line}");
            if writeln!(file, "[hangar] hangar-server launch: {line}").is_err() { report(log, line); }
        }
        None => report(log, line),
    }
}

/// O log da tarefa da subida anterior vai para `<nome>.1` (e os outros andam uma casa, até `.5`): a
/// saída de uma queda fica para ler. Aberto em acréscimo e com escrita compartilhada (o padrão do
/// `std` no Windows), um neto que sobreviveu com a cópia herdada não impede a subida; preso mesmo
/// assim, a saída vai para o primeiro `.N` que abrir.
#[cfg_attr(not(windows), allow(dead_code))]
fn open_log(target: &Path) -> std::io::Result<(std::fs::File, std::path::PathBuf)> {
    if let Some(parent) = target.parent() { let _ = std::fs::create_dir_all(parent); }
    let (stem, ext) = (target.file_stem().unwrap_or_default().to_string_lossy(), target.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default());
    let numbered = |n: u32| target.with_file_name(format!("{stem}.{n}{ext}"));
    let _ = std::fs::remove_file(numbered(5));
    for n in (1..5).rev() { let _ = std::fs::rename(numbered(n), numbered(n + 1)); }
    let _ = std::fs::rename(target, numbered(1));
    let mut last = std::io::Error::other("sem candidato");
    for candidate in std::iter::once(target.to_path_buf()).chain((1..=5).map(numbered)) {
        match std::fs::OpenOptions::new().append(true).create(true).open(&candidate) {
            Ok(file) => return Ok((file, candidate)),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// O comando da vida e se ela sobe pelo Python.
#[cfg(windows)]
fn split(backend: Backend) -> (std::process::Command, bool) {
    match backend {
        Backend::Direct(command) => (command, false),
        Backend::Python(command) => (command, true),
    }
}

/// O interpretador que o Python da versão de fato usa, conferido pelo nascimento do filho, até 5 s.
#[cfg(windows)]
fn record_python_launch(snapshot: &crate::boot_version::PythonLaunch, child: &mut std::process::Child, file: Option<&std::fs::File>, log: Option<&Path>) {
    let pid = child.id();
    let mut birth = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let confirmed = loop {
        if birth.is_none() { birth = crate::terminal_facts::agent_birth(pid); }
        if let Some(birth) = birth {
            match snapshot.record_windows(pid, birth) {
                Ok(true) => break true,
                Err(error) => { say(file, log, &format!("launch_python_version_unavailable: {error}")); break false; }
                Ok(false) => {}
            }
        }
        if std::time::Instant::now() >= deadline || child.try_wait().ok().flatten().is_some() { break false; }
        std::thread::park_timeout(std::time::Duration::from_millis(50));
    };
    if !confirmed { say(file, log, "launch_python_version_unavailable: interpretador efetivo não confirmado"); }
}

/// Põe o lançador num Job que mata o grupo quando o último handle fecha: matar o lançador
/// (`Stop-ScheduledTask`, `Stop-Process`) leva o backend junto e a porta não fica presa com um órfão.
/// O handle fica aberto até o processo sair. `BREAKAWAY_OK`: o que nasce com
/// `CREATE_BREAKAWAY_FROM_JOB` (terminal de sessão) sobrevive, como no Job do `runtime_process`.
#[cfg(windows)]
fn die_with_the_launcher() -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() { return Err(std::io::Error::last_os_error()); }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        let set = SetInformationJobObject(job, JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void, std::mem::size_of_val(&info) as u32);
        // O próprio lançador entra antes de criar o filho: ele já nasce dentro, sem janela de corrida.
        if set == 0 || AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            let error = std::io::Error::last_os_error();
            CloseHandle(job);
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{current, root, Backend};

    /// B-36: cada subida começa um log novo e a saída da anterior (a de uma queda) fica em `.1`; as
    /// mais velhas andam uma casa até `.5`.
    #[test]
    fn the_task_log_keeps_the_previous_start_in_the_numbered_files() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let target = logs.join("hangar-backend.log");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(&target, "queda").unwrap();
        for n in 1..=5 { std::fs::write(logs.join(format!("hangar-backend.{n}.log")), format!("velho {n}")).unwrap(); }
        let (mut file, used) = super::open_log(&target).unwrap();
        assert_eq!(used, target);
        file.write_all(b"nova").unwrap();
        file.write_all(b" subida").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "nova subida");
        assert_eq!(std::fs::read_to_string(logs.join("hangar-backend.1.log")).unwrap(), "queda");
        assert_eq!(std::fs::read_to_string(logs.join("hangar-backend.2.log")).unwrap(), "velho 1");
        assert_eq!(std::fs::read_to_string(logs.join("hangar-backend.5.log")).unwrap(), "velho 4", "o mais velho sai");
    }

    /// P2: a versão que sobe pelo Python avisa pronto e desliga o watchdog; sem socket, nada a fazer.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_launcher_tells_systemd_ready_and_turns_the_watchdog_off_for_the_python() {
        use std::os::linux::net::SocketAddrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify");
        let listener = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        let to = |socket: Option<String>| hangar_boot::notify::Notifier::from_lookup(move |key| (key == "NOTIFY_SOCKET").then(|| socket.clone()).flatten(), 1);
        to(Some(path.to_string_lossy().into_owned())).send("READY=1\nWATCHDOG_USEC=0").unwrap();
        let mut buf = [0u8; 64];
        let n = listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1\nWATCHDOG_USEC=0");
        let name = format!("hangar-notify-{}", std::process::id());
        let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let abstract_listener = std::os::unix::net::UnixDatagram::bind_addr(&address).unwrap();
        to(Some(format!("@{name}"))).send("READY=1").unwrap();
        let n = abstract_listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1");
        to(None).send("READY=1").unwrap();
    }

    /// Decisão 38: a religada resolve o `current` de novo. Depois da troca de versão ela abre o servidor
    /// da nova (com o nome dela para o log), e a versão que sobe pelo Python volta pelo Python dela.
    #[test]
    fn the_relaunch_follows_the_current_for_direct_and_python_versions() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let windows = cfg!(windows);
        let exe = if windows { "hangar-server.exe" } else { "hangar-server" };
        let version = |name: &str, manifest: &str| {
            let real = home.join(".hangar").join("versions").join(name);
            std::fs::create_dir_all(real.join("backend")).unwrap();
            std::fs::write(real.join("manifest.json"), manifest).unwrap();
            real
        };
        let runnable = |path: &std::path::Path| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
            #[cfg(unix)]
            { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap(); }
        };
        let relink = |target: &std::path::Path| {
            let link = current(home);
            #[cfg(unix)]
            { let _ = std::fs::remove_file(&link); std::os::unix::fs::symlink(target, &link).unwrap(); }
            #[cfg(windows)]
            { let _ = std::fs::remove_dir(&link); crate::update::make_link(target, &link).unwrap(); }
        };
        let (v1, v2) = (version("v1", r#"{"boot":"direct"}"#), version("v2", r#"{"boot":"direct"}"#));
        let v3 = version("v3", r#"{"version":"v3"}"#);
        for real in [&v1, &v2] { runnable(&real.join("bin").join(exe)); }
        runnable(&super::python_exe(&v3, windows));
        for (real, name) in [(&v1, "v1"), (&v2, "v2")] {
            relink(real);
            let (Backend::Direct(command), version) = super::relaunch_backend(home, &[], windows, None, &mut Vec::new()).unwrap()
                else { panic!("{name} sobe direto") };
            assert_eq!(version, name);
            assert!(std::path::Path::new(command.get_program()).ends_with(std::path::Path::new(name).join("bin").join(exe)), "{command:?}");
        }
        // Sem a vigia, quem desiste fica fora até o próximo logon: a versão que sobe pelo Python
        // (a volta da atualização) é religada pelo Python dela.
        relink(&v3);
        let (Backend::Python(command), version) = super::relaunch_backend(home, &[], windows, None, &mut Vec::new()).unwrap()
            else { panic!("v3 sobe pelo Python") };
        assert_eq!(version, "v3");
        assert!(std::path::Path::new(command.get_program()).ends_with(super::python_exe(std::path::Path::new("v3"), windows)), "{command:?}");
    }

    #[test]
    fn install_root_is_the_unresolved_current_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        assert_ne!(root(home), Some(current(home)), "sem o current a raiz é a cópia do Hangar");
        let real = home.join(".hangar").join("versions").join("1.0.0.5");
        std::fs::create_dir_all(&real).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, current(home)).unwrap();
        #[cfg(windows)]
        crate::update::make_link(&real, &current(home)).unwrap();
        assert_eq!(root(home), Some(home.join(".hangar").join("current")));
    }
}

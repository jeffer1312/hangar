//! Quem reinicia o servidor depois da troca, conforme a máquina o roda: unit do systemd, tarefas
//! agendadas do Windows ou à mão. A mesma regra do `_topologia`/`_reiniciar` do motor `git`.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Restarter, StepRunner};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topology {
    Systemd,
    Windows,
    Manual,
}

impl Topology {
    /// O nome que a tela lê em `pre_voo.topologia`.
    pub fn name(self) -> &'static str {
        match self {
            Topology::Systemd => "systemd",
            Topology::Windows => "windows",
            Topology::Manual => "manual",
        }
    }

    /// `windows` vem de fora para o teste. Fora do Windows, a unit do backend existir no systemd do
    /// usuário (pelo código de saída: barramento fora não vira "não existe").
    pub fn detect(runner: &dyn StepRunner, cwd: &Path, windows: bool) -> Topology {
        if windows { return Topology::Windows; }
        if unit_exists(runner, cwd, "hangar-backend.service") { Topology::Systemd } else { Topology::Manual }
    }
}

fn unit_exists(runner: &dyn StepRunner, cwd: &Path, unit: &str) -> bool {
    runner.run(&format!("systemctl --user list-unit-files {unit}"), cwd).code == Some(0)
}

pub struct SystemRestarter {
    pub topology: Topology,
    pub runner: Arc<dyn StepRunner + Send + Sync>,
    pub home: PathBuf,
    /// A porta do backend que as tarefas do Windows sobem.
    pub port: u16,
}

impl SystemRestarter {
    /// O comando de reinício desta máquina; `None` na mão.
    fn command(&self) -> Result<Option<String>, String> {
        Ok(Some(match self.topology {
            Topology::Manual => return Ok(None),
            // O backend por último: sem escopo próprio, o reinício dele leva quem pediu junto.
            Topology::Systemd => {
                let front = unit_exists(&*self.runner, &self.home, "hangar-frontend.service");
                format!("systemctl --user restart {}hangar-backend.service", if front { "hangar-frontend.service " } else { "" })
            }
            // O `windows-tasks.ps1` da versão para onde o `current` aponta agora (a troca já
            // aconteceu): derruba os processos das tarefas e as sobe de novo, sem o instalador.
            Topology::Windows => {
                let current = crate::install_root::current(&self.home);
                let root = std::fs::canonicalize(&current).map(crate::list::links::without_verbatim)
                    .map_err(|e| format!("{} ilegível: {e}", current.display()))?;
                // O PowerShell fecha a string em qualquer aspa simples tipográfica também.
                let quote = |p: &Path| p.display().to_string().chars()
                    .flat_map(|c| if matches!(c, '\'' | '‘' | '’' | '‚' | '‛') { vec![c, c] } else { vec![c] }).collect::<String>();
                format!("powershell -NoProfile -ExecutionPolicy Bypass -Command \". '{}'; Restart-HangarTasks '{}' {} {}\"",
                    quote(&root.join("scripts").join("windows-tasks.ps1")), quote(&root), self.port, front_port(&self.home, &root))
            }
        }))
    }
}

/// `CP_FRONT_PORT` do `.env` dos dados do usuário; 0 sem a tarefa do front.
fn front_port(home: &Path, root: &Path) -> u16 { env_port(home, root, "CP_FRONT_PORT").unwrap_or(0) }

/// Uma porta (`CP_PORT`, `CP_FRONT_PORT`) do `.env` dos dados do usuário.
pub(crate) fn env_port(home: &Path, root: &Path, key: &str) -> Option<u16> {
    let env = crate::user_data::path(home, Some(&root.join("backend")), ".env")?;
    std::fs::read_to_string(env).ok()?.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('=')).and_then(|port| port.trim().parse().ok())
}

/// As últimas linhas da saída: o motivo da recusa (mutex ocupado, unit negada) vai ao estado.
fn tail(output: &str) -> String {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(6)..].join(" | ")
}

impl Restarter for SystemRestarter {
    async fn restart(&self) -> Result<(), String> {
        let Some(command) = self.command()? else { return Ok(()) };
        let (runner, home) = (self.runner.clone(), self.home.clone());
        let run = tokio::task::spawn_blocking(move || runner.run(&command, &home)).await.map_err(|e| e.to_string())?;
        match run.code {
            Some(0) => Ok(()),
            code => Err(format!("o reinício saiu com {code:?}: {}", tail(&run.output))),
        }
    }

    fn manual(&self) -> bool { self.topology == Topology::Manual }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::update::StepRun;

    /// Responde por prefixo do comando e anota tudo o que rodou.
    struct Script {
        ran: Mutex<Vec<String>>,
        answer: Box<dyn Fn(&str) -> StepRun + Send + Sync>,
    }

    impl StepRunner for Script {
        fn run(&self, command: &str, _cwd: &Path) -> StepRun {
            self.ran.lock().unwrap().push(command.to_owned());
            (self.answer)(command)
        }
    }

    fn script(answer: impl Fn(&str) -> StepRun + Send + Sync + 'static) -> Arc<Script> {
        Arc::new(Script { ran: Mutex::default(), answer: Box::new(answer) })
    }

    fn code(code: i32, output: &str) -> StepRun { StepRun { code: Some(code), output: output.into() } }

    fn restarter(topology: Topology, runner: Arc<Script>, home: &Path) -> SystemRestarter {
        SystemRestarter { topology, runner, home: home.to_path_buf(), port: 8765 }
    }

    #[test]
    fn update_topology_is_windows_systemd_or_manual() {
        let dir = tempfile::tempdir().unwrap();
        let unit = script(|c| code(if c == "systemctl --user list-unit-files hangar-backend.service" { 0 } else { 1 }, ""));
        assert_eq!(Topology::detect(&*unit, dir.path(), false), Topology::Systemd);
        assert_eq!(Topology::detect(&*unit, dir.path(), true), Topology::Windows);
        assert_eq!(Topology::detect(&*script(|_| code(127, "systemctl: not found")), dir.path(), false), Topology::Manual);
        assert_eq!([Topology::Systemd, Topology::Windows, Topology::Manual].map(Topology::name), ["systemd", "windows", "manual"]);
    }

    #[tokio::test]
    async fn update_systemd_restart_takes_the_frontend_first_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let both = script(|_| code(0, ""));
        restarter(Topology::Systemd, both.clone(), dir.path()).restart().await.unwrap();
        assert_eq!(both.ran.lock().unwrap().last().unwrap(), "systemctl --user restart hangar-frontend.service hangar-backend.service");

        let backend = script(|c| code(if c.contains("list-unit-files hangar-frontend") { 1 } else { 0 }, ""));
        restarter(Topology::Systemd, backend.clone(), dir.path()).restart().await.unwrap();
        assert_eq!(backend.ran.lock().unwrap().last().unwrap(), "systemctl --user restart hangar-backend.service");

        let refused = script(|c| if c.contains("restart") { code(1, "Failed to restart hangar-backend.service: Access denied") } else { code(1, "") });
        let error = restarter(Topology::Systemd, refused, dir.path()).restart().await.unwrap_err();
        assert!(error.contains("Access denied"), "{error}");
    }

    /// `~/.hangar/current` para a versão nova, com `CP_FRONT_PORT` no `.env` dos dados do usuário.
    fn windows_home(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let version = dir.path().join(".hangar/versions").join(name);
        std::fs::create_dir_all(&version).unwrap();
        crate::update::swap(&crate::update::Layout::new(dir.path()), &version).unwrap();
        let config = dir.path().join(".hangar/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(".env"), "CP_AUTH_TOKEN=x\nCP_FRONT_PORT=5173\n").unwrap();
        let real = crate::list::links::without_verbatim(std::fs::canonicalize(&version).unwrap());
        (dir, real)
    }

    #[tokio::test]
    async fn update_windows_restart_runs_the_tasks_helper_of_the_new_root() {
        let (dir, root) = windows_home("0.1.0.11");
        let runner = script(|_| code(0, ""));
        restarter(Topology::Windows, runner.clone(), dir.path()).restart().await.unwrap();
        let helper = root.join("scripts").join("windows-tasks.ps1");
        assert_eq!(runner.ran.lock().unwrap().as_slice(), [format!(
            "powershell -NoProfile -ExecutionPolicy Bypass -Command \". '{}'; Restart-HangarTasks '{}' 8765 5173\"",
            helper.display(), root.display())]);
    }

    #[tokio::test]
    async fn update_windows_restart_quotes_the_path_and_reports_the_refusal() {
        let (dir, root) = windows_home("0.1.0.11");
        // Toda aspa simples que o PowerShell reconhece (' ‘ ’ ‚ ‛) dobra dentro da string dele.
        let odd = root.parent().unwrap().join("o'v ’x‘y‚z‛");
        std::fs::rename(&root, &odd).unwrap();
        crate::update::swap(&crate::update::Layout::new(dir.path()), &odd).unwrap();
        let refused = script(|_| code(1, "Instalacao ou recuperacao em andamento; tente de novo em instantes"));
        let error = restarter(Topology::Windows, refused.clone(), dir.path()).restart().await.unwrap_err();
        assert!(error.contains("Instalacao ou recuperacao em andamento"), "{error}");
        assert!(refused.ran.lock().unwrap()[0].contains("o''v ’’x‘‘y‚‚z‛‛"), "{:?}", refused.ran.lock().unwrap());
    }

    #[tokio::test]
    async fn update_manual_restart_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let runner = script(|_| code(0, ""));
        let manual = restarter(Topology::Manual, runner.clone(), dir.path());
        assert!(manual.manual());
        manual.restart().await.unwrap();
        assert!(runner.ran.lock().unwrap().is_empty());
        assert!(!restarter(Topology::Systemd, runner, dir.path()).manual());
    }
}

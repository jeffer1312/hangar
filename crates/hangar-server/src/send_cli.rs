//! `hangar-server send`: liga o `hangar-send` (crate `hangar-send`) ao livro de peers, às contas e ao
//! tmux desta máquina. A raiz vem do atalho (`--root`), para cada cópia ler a própria configuração.
use hangar_send::{Captured, Host, Invocation};
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct MachineHost {
    home: PathBuf,
    root: Option<PathBuf>,
}

impl MachineHost {
    /// A mesma regra do `hangar-server peers path`, com a `backend/` da cópia que chamou.
    fn peers_path(&self) -> std::io::Result<PathBuf> {
        let backend = match &self.root {
            Some(root) => Some(root.join("backend")),
            None => crate::session_open::cli::backend_dir(),
        };
        Ok(crate::user_data::path(&self.home, backend.as_deref(), "peers.json")?
            .unwrap_or_else(|| self.home.join(".hangar/config/peers.json")))
    }
}

impl Host for MachineHost {
    fn peers(&self, args: &[&str]) -> Captured {
        let Ok(path) = self.peers_path() else {
            return Captured { code: 1, stdout: Vec::new(), stderr: "erro: não foi possível resolver peers.json\n".into() };
        };
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = crate::machine_peers::cli::run(&path, &args, &mut stdout, &mut stderr);
        Captured { code: code.into(), stdout, stderr }
    }

    fn account_prep(&self, name: &str) -> Captured {
        // Processo próprio: a reconciliação escreve no stdout e no stderr do processo.
        let output = std::env::current_exe().and_then(|exe| {
            Command::new(exe).args(["conta", "--prep", name]).stdin(Stdio::null()).output()
        });
        match output {
            Ok(output) => Captured { code: output.status.code().unwrap_or(1), stdout: output.stdout, stderr: output.stderr },
            Err(error) => Captured { code: 126, stdout: Vec::new(), stderr: format!("hangar-conta: falha ao executar: {error}\n").into() },
        }
    }

    fn tmux(&self, args: &[&str]) -> Option<Captured> {
        let output = Command::new("tmux").args(args).stdin(Stdio::null()).output().ok()?;
        Some(Captured { code: output.status.code().unwrap_or(1), stdout: output.stdout, stderr: output.stderr })
    }
}

#[cfg(test)]
mod tests {
    use super::MachineHost;

    /// Arquivo a arquivo: o `peers.json` da cópia vale até existir o do lugar novo.
    #[test]
    fn peers_file_follows_the_copy_until_the_new_place_exists() {
        let dir = tempfile::tempdir().unwrap();
        let (home, root) = (dir.path().join("casa"), dir.path().join("copia"));
        std::fs::create_dir_all(root.join("backend")).unwrap();
        let host = MachineHost { home: home.clone(), root: Some(root.clone()) };
        assert_eq!(host.peers_path().unwrap(), root.join("backend/peers.json"));
        std::fs::create_dir_all(home.join(".hangar/config")).unwrap();
        std::fs::write(home.join(".hangar/config/peers.json"), "{}").unwrap();
        assert_eq!(host.peers_path().unwrap(), home.join(".hangar/config/peers.json"));
    }

    /// Máquina migrada (`~/.hangar/current`) usa o lugar novo mesmo sem o arquivo ali.
    #[test]
    fn migrated_machine_never_reads_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let (home, root) = (dir.path().join("casa"), dir.path().join("copia"));
        std::fs::create_dir_all(home.join(".hangar/current")).unwrap();
        std::fs::create_dir_all(root.join("backend")).unwrap();
        std::fs::write(root.join("backend/peers.json"), "{}").unwrap();
        let host = MachineHost { home: home.clone(), root: Some(root) };
        assert_eq!(host.peers_path().unwrap(), home.join(".hangar/config/peers.json"));
    }
}

pub fn cli(args: &[String]) -> std::process::ExitCode {
    let (root, args) = hangar_send::split_root(args);
    let vars: std::collections::BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let home = hangar_core::account_env::AccountEnvironment::from_map(vars.clone()).home;
    let host = MachineHost { home: home.clone(), root: root.clone() };
    let invocation = Invocation { args, root, home, vars };
    let code = hangar_send::run(&invocation, &host, &mut std::io::stdout(), &mut std::io::stderr());
    std::process::ExitCode::from(code)
}

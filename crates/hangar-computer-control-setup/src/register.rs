//! Registro sozinho do MCP: com máquina-alvo e o binário do pacote, a entrada nasce ligada em todas as
//! contas; desligada à mão (ausente no principal e guardada) fica desligada. Entrada ativa com outro
//! programa (checkout, `uvx`, `servidor_mcp.py`) passa para o binário do pacote com o mesmo env.
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::files::installed;
use crate::writer::register_entry;
use crate::{Error, Paths, targets};

const FAILED: &str = "computer_control_register_failed";

#[derive(Debug, PartialEq, Eq)]
pub enum Registered {
    /// Plataforma sem release, binário ausente, sem alvo ou desligado à mão: nada gravado.
    Skipped,
    /// A entrada já estava igual em todo arquivo.
    Unchanged,
    Written,
}

/// Roda na subida, no relógio de 1 h, depois de criar alvo e depois de trocar o binário. `hyprland` só é
/// perguntado sem alvo cadastrado, antes da trava; `jev` dá o env do Jev, lido sob a trava. Falha vai ao
/// diário com código.
pub fn register(
    paths: &Paths,
    hyprland: impl FnOnce() -> bool,
    jev: impl FnOnce() -> Result<Map<String, Value>, Error>,
    diary: Option<&Path>,
) -> Result<Registered, Error> {
    let outcome = register_on(paths, crate::supported(), hyprland, jev);
    if let Err(error) = &outcome {
        tracing::warn!(code = FAILED, erro = error.code, msg = %error.msg, "Computer Use: registro do MCP falhou");
        let line = json!({"evento": "computer_control.registrar", "nivel": "erro", "codigo": FAILED,
            "erro": error.code, "detalhe": error.msg});
        hangar_core::diag::record_blocking(diary, &[line], "computer control");
    }
    outcome
}

fn register_on(
    paths: &Paths,
    supported: bool,
    hyprland: impl FnOnce() -> bool,
    jev: impl FnOnce() -> Result<Map<String, Value>, Error>,
) -> Result<Registered, Error> {
    if !supported || !installed(paths)? {
        return Ok(Registered::Skipped);
    }
    // O alvo `linux` automático do Hyprland não tem arquivo: alvo padrão vazio. A sonda pode levar
    // segundos, e o Claude Code espera a trava do `.claude.json` só 1 s: roda antes dela.
    let target = first_target(paths);
    let hyprland = target.is_none() && hyprland();
    // Desligar no meio daqui até a trava: a decisão abaixo, toda sob a trava, já o vê.
    #[cfg(test)]
    crate::writer::race::fire(&paths.parked_file());
    Ok(match register_entry(paths, target, hyprland, jev)? {
        None => Registered::Skipped,
        Some(false) => Registered::Unchanged,
        Some(true) => Registered::Written,
    })
}

fn first_target(paths: &Paths) -> Option<String> {
    let file = targets::target_files(paths).into_iter().next()?;
    Some(paths.targets_dir().join(file).to_string_lossy().into_owned())
}

/// Há Hyprland vivo, mesmo com o servidor nascido antes do login gráfico (sem a variável no ambiente).
pub fn hyprland_found() -> bool {
    cfg!(target_os = "linux")
        && hyprland_running(&hangar_core::display_env::for_launch(|key| std::env::var(key).ok().filter(|v| !v.is_empty())))
}

/// A instância do Hyprland da assinatura atende no socket dela: pasta que sobrou de uma instância morta
/// não conta.
pub fn hyprland_running(vars: &[(String, String)]) -> bool {
    let get = |key: &str| vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty());
    match (get("HYPRLAND_INSTANCE_SIGNATURE"), get("XDG_RUNTIME_DIR")) {
        (Some(signature), Some(runtime)) => answers(&Path::new(runtime).join("hypr").join(signature).join(".socket.sock")),
        _ => false,
    }
}

#[cfg(unix)]
fn answers(socket: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

#[cfg(not(unix))]
fn answers(_socket: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::NAME;

    struct Home {
        dir: tempfile::TempDir,
        paths: Paths,
    }

    impl Home {
        /// Pacote instalado (`install.json` com tag e binário) e uma conta aberta (`acc`) e outra nunca
        /// aberta (`vazia`).
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let paths = Paths::new(dir.path().to_owned(), vec![dir.path().join("acc"), dir.path().join("vazia")]);
            std::fs::create_dir_all(paths.targets_dir()).unwrap();
            std::fs::write(paths.binary(), b"\x7fELF").unwrap();
            std::fs::write(paths.install_json(), br#"{"tag": "v2.0.0"}"#).unwrap();
            std::fs::create_dir_all(dir.path().join("acc")).unwrap();
            std::fs::write(dir.path().join("acc/.claude.json"), br#"{"x": 1}"#).unwrap();
            std::fs::create_dir_all(dir.path().join("vazia")).unwrap();
            std::fs::write(dir.path().join("vazia/.claude.json"), b"").unwrap();
            Self { dir, paths }
        }

        fn target(&self, name: &str) -> String {
            let path = self.paths.targets_dir().join(format!("{name}-agent.json"));
            std::fs::write(&path, br#"{"transport": "ssh", "host": "h"}"#).unwrap();
            path.to_string_lossy().into_owned()
        }

        fn run(&self, hyprland: bool) -> Result<Registered, Error> {
            register(&self.paths, || hyprland, || Ok(jev()), Some(&self.diary()))
        }

        fn diary(&self) -> std::path::PathBuf {
            self.dir.path().join("diario")
        }

        fn entry(&self, file: &Path) -> Option<Value> {
            let text = std::fs::read_to_string(file).ok()?;
            let data: Value = serde_json::from_str(&text).unwrap();
            data.get("mcpServers").and_then(|s| s.get(NAME)).cloned()
        }

        fn main_entry(&self) -> Option<Value> {
            self.entry(&self.paths.main_file())
        }

        fn binary(&self) -> String {
            self.paths.binary().to_string_lossy().into_owned()
        }

        fn agents_dir(&self) -> String {
            self.paths.targets_dir().to_string_lossy().into_owned()
        }
    }

    fn jev() -> Map<String, Value> {
        Map::from_iter([("TYPESAFE_API_KEY".to_owned(), json!("fixture-key"))])
    }

    /// D4.1: sem alvo e sem Hyprland, nada.
    #[test]
    fn without_target_nothing_is_registered() {
        let home = Home::new();
        assert_eq!(home.run(false).unwrap(), Registered::Skipped);
        assert!(!home.paths.main_file().exists());
    }

    /// D4.1, ROT-38: alvo em `targets/` liga em todas as contas abertas, com o primeiro alvo como padrão;
    /// o Hyprland nem é perguntado.
    #[test]
    fn a_target_file_registers_in_every_opened_account() {
        let home = Home::new();
        home.target("b");
        let first = home.target("a");
        let outcome = register(&home.paths, || -> bool { panic!("Hyprland perguntado com alvo") }, || Ok(jev()), None).unwrap();
        assert_eq!(outcome, Registered::Written);
        let wanted = json!({"command": home.binary(), "args": [], "env": {
            "HCC_AGENT_CONFIG": first, "HCC_AGENTS_DIR": home.agents_dir(), "TYPESAFE_API_KEY": "fixture-key"}});
        assert_eq!(home.main_entry(), Some(wanted.clone()));
        assert_eq!(home.entry(&home.dir.path().join("acc/.claude.json")), Some(wanted));
        assert_eq!(std::fs::read(home.dir.path().join("vazia/.claude.json")).unwrap(), b"", "conta nunca aberta fica com o espelho");
    }

    /// D4.1, P1: só o alvo `linux` automático do Hyprland liga, com alvo padrão vazio.
    #[test]
    fn hyprland_alone_registers_with_an_empty_default_target() {
        let home = Home::new();
        assert_eq!(home.run(true).unwrap(), Registered::Written);
        let wanted = json!({"command": home.binary(), "args": [], "env": {
            "HCC_AGENTS_DIR": home.agents_dir(), "TYPESAFE_API_KEY": "fixture-key"}});
        assert_eq!(home.main_entry(), Some(wanted));
    }

    /// P1: alvo SSH cadastrado depois do registro pelo Hyprland não vira o padrão mostrado do MCP ligado.
    #[test]
    fn a_target_added_after_hyprland_does_not_become_the_shown_default() {
        let home = Home::new();
        assert_eq!(home.run(true).unwrap(), Registered::Written);
        home.target("vm");
        assert_eq!(home.run(true).unwrap(), Registered::Unchanged);
        let state = crate::state(&home.paths, &crate::Facts::default()).unwrap();
        assert_eq!((state["enabled"].clone(), state["agent_config"].clone()), (json!(true), json!("")));
    }

    /// D4.1: sem o binário do pacote, nada, nem com alvo.
    #[test]
    fn without_the_package_binary_nothing_is_registered() {
        let home = Home::new();
        home.target("a");
        std::fs::remove_file(home.paths.binary()).unwrap();
        assert_eq!(home.run(true).unwrap(), Registered::Skipped);
        assert!(!home.paths.main_file().exists());
    }

    /// Mesma regra de "instalado" do PUT: binário sem `install.json` com tag nem registra nem liga.
    #[test]
    fn a_binary_without_install_json_neither_registers_nor_turns_on_by_put() {
        for install in [None, Some(&br#"{"tag": ""}"#[..])] {
            let home = Home::new();
            let target = home.target("a");
            match install {
                None => std::fs::remove_file(home.paths.install_json()).unwrap(),
                Some(bytes) => std::fs::write(home.paths.install_json(), bytes).unwrap(),
            }
            assert_eq!(home.run(true).unwrap(), Registered::Skipped);
            assert!(!home.paths.main_file().exists());
            let body: crate::SaveRequest = serde_json::from_value(json!({"enabled": true, "agent_config": target})).unwrap();
            let refused =
                crate::save(&home.paths, &crate::Facts::default(), &crate::Aux::default(), &body, || Ok(jev())).unwrap_err();
            assert_eq!((refused.status, refused.code), (400, "erro_computer_control_not_installed"));
            assert!(!home.paths.main_file().exists());
        }
    }

    /// D4.1, 1B refinado: ausente no principal e guardada = desligado à mão, fica desligado.
    #[test]
    fn turned_off_by_hand_stays_off() {
        let home = Home::new();
        home.target("a");
        std::fs::write(home.paths.main_file(), br#"{"mcpServers": {"outro": {"command": "x"}}}"#).unwrap();
        std::fs::create_dir_all(home.paths.parked_file().parent().unwrap()).unwrap();
        std::fs::write(home.paths.parked_file(), br#"{"command": "/velho", "env": {}}"#).unwrap();
        let before = std::fs::read(home.paths.main_file()).unwrap();
        assert_eq!(home.run(true).unwrap(), Registered::Skipped);
        assert_eq!(std::fs::read(home.paths.main_file()).unwrap(), before);
        assert_eq!(home.entry(&home.dir.path().join("acc/.claude.json")), None);
    }

    /// Idempotente: a segunda passada não grava; a guardada antiga não desliga uma entrada ativa.
    #[test]
    fn an_equal_entry_is_not_written_again() {
        let home = Home::new();
        home.target("a");
        assert_eq!(home.run(false).unwrap(), Registered::Written);
        std::fs::create_dir_all(home.paths.parked_file().parent().unwrap()).unwrap();
        std::fs::write(home.paths.parked_file(), br#"{"command": "/velho"}"#).unwrap();
        let main = std::fs::read(home.paths.main_file()).unwrap();
        let account = std::fs::read(home.dir.path().join("acc/.claude.json")).unwrap();
        assert_eq!(home.run(false).unwrap(), Registered::Unchanged);
        assert_eq!(std::fs::read(home.paths.main_file()).unwrap(), main);
        assert_eq!(std::fs::read(home.dir.path().join("acc/.claude.json")).unwrap(), account);
    }

    /// D4.2, ROT-75, HIS-26: entrada ativa com outro programa vira o binário do pacote; o alvo padrão e o
    /// env alheio ficam, `PYTHONPATH` sai. Vale mesmo sem alvo cadastrado.
    #[test]
    fn an_active_entry_with_another_program_is_rewritten_keeping_env() {
        for (command, args) in [
            ("/home/x/Projetos/hangar-computer-control/target/release/hangar-computer-control", json!([])),
            ("uvx", json!(["--from", "git+https://github.com/jeffer1312/hangar-computer-control", "hcc"])),
            ("/home/x/Projetos/hangar-computer-control/.venv/bin/python", json!(["servidor_mcp.py"])),
        ] {
            let home = Home::new();
            let old = json!({"mcpServers": {"outro": {"command": "y"}, NAME: {"command": command, "args": args, "env": {
                "HCC_AGENT_CONFIG": "/alvos/vm-agent.json", "VIRTUAL_ENV": "", "MEU": "fica", "PYTHONPATH": "/sai"}}}});
            std::fs::write(home.paths.main_file(), old.to_string()).unwrap();
            assert_eq!(home.run(false).unwrap(), Registered::Written, "{command}");
            let entry = home.main_entry().unwrap();
            assert_eq!(entry["command"], json!(home.binary()));
            assert_eq!(entry["args"], json!([]));
            assert_eq!(entry["env"], json!({"VIRTUAL_ENV": "", "MEU": "fica", "HCC_AGENT_CONFIG": "/alvos/vm-agent.json",
                "HCC_AGENTS_DIR": home.agents_dir(), "TYPESAFE_API_KEY": "fixture-key"}));
            let data: Value = serde_json::from_str(&std::fs::read_to_string(home.paths.main_file()).unwrap()).unwrap();
            assert_eq!(data["mcpServers"]["outro"], json!({"command": "y"}));
        }
    }

    /// D4.2, HIS-27: sem o binário do pacote, a entrada ativa antiga fica como está.
    #[test]
    fn an_active_entry_stays_when_the_binary_is_missing() {
        let home = Home::new();
        std::fs::remove_file(home.paths.binary()).unwrap();
        let old = json!({"mcpServers": {NAME: {"command": "uvx", "args": ["hcc"], "env": {"PYTHONPATH": "/p"}}}}).to_string();
        std::fs::write(home.paths.main_file(), &old).unwrap();
        assert_eq!(home.run(true).unwrap(), Registered::Skipped);
        assert_eq!(std::fs::read_to_string(home.paths.main_file()).unwrap(), old);
    }

    /// D4.4, P5: plataforma sem release nunca registra, nem com alvo, binário e Hyprland.
    #[test]
    fn an_unsupported_platform_never_registers() {
        let home = Home::new();
        home.target("a");
        assert_eq!(register_on(&home.paths, false, || true, || Ok(jev())).unwrap(), Registered::Skipped);
        assert!(!home.paths.main_file().exists());
    }

    /// Falha vai ao diário com código, e nada é gravado.
    #[test]
    fn a_failure_goes_to_the_diary() {
        let home = Home::new();
        home.target("a");
        std::fs::write(home.paths.main_file(), b"{quebrado").unwrap();
        assert_eq!(home.run(false).unwrap_err().code, "erro_computer_control_read");
        assert_eq!(std::fs::read(home.paths.main_file()).unwrap(), b"{quebrado");
        let lines: Vec<Value> = std::fs::read_dir(home.diary()).unwrap()
            .flat_map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect::<Vec<Value>>())
            .collect();
        assert!(lines.iter().any(|line| line["codigo"] == FAILED && line["erro"] == "erro_computer_control_read"), "{lines:?}");
    }

    #[test]
    #[cfg(unix)]
    fn hyprland_needs_a_live_instance() {
        let run = tempfile::tempdir().unwrap();
        let vars = |signature: &str| {
            vec![("HYPRLAND_INSTANCE_SIGNATURE".to_owned(), signature.to_owned()),
                ("XDG_RUNTIME_DIR".to_owned(), run.path().to_string_lossy().into_owned())]
        };
        std::fs::create_dir_all(run.path().join("hypr/viva_1")).unwrap();
        let _listener = std::os::unix::net::UnixListener::bind(run.path().join("hypr/viva_1/.socket.sock")).unwrap();
        assert!(hyprland_running(&vars("viva_1")));
        std::fs::create_dir_all(run.path().join("hypr/pasta_2")).unwrap();
        assert!(!hyprland_running(&vars("pasta_2")), "pasta velha sem socket");
        std::fs::create_dir_all(run.path().join("hypr/morta_3")).unwrap();
        drop(std::os::unix::net::UnixListener::bind(run.path().join("hypr/morta_3/.socket.sock")).unwrap());
        assert!(!hyprland_running(&vars("morta_3")), "socket que ficou sem ninguém escutando");
        assert!(!hyprland_running(&vars("nenhuma_4")));
        assert!(!hyprland_running(&vars("")));
        assert!(!hyprland_running(&[("HYPRLAND_INSTANCE_SIGNATURE".to_owned(), "viva_1".to_owned())]), "sem pasta de runtime");
    }

    /// A sonda do Hyprland (até segundos) roda antes de tomar a trava do `.claude.json`.
    #[test]
    fn hyprland_is_asked_before_the_lock() {
        let home = Home::new();
        let lock = hangar_core::config_lock::lock_path(&home.paths.main_file());
        let asked = || {
            assert!(!lock.exists(), "Hyprland sondado com a trava do .claude.json tomada");
            true
        };
        assert_eq!(register(&home.paths, asked, || Ok(jev()), None).unwrap(), Registered::Written);
    }

    /// Pasta de uma instância morta não é Hyprland: sem alvo, nada é registrado.
    #[test]
    fn a_stale_hyprland_folder_registers_nothing() {
        let home = Home::new();
        let run = home.dir.path().join("run");
        std::fs::create_dir_all(run.join("hypr/morta_1")).unwrap();
        let vars = vec![("HYPRLAND_INSTANCE_SIGNATURE".to_owned(), "morta_1".to_owned()),
            ("XDG_RUNTIME_DIR".to_owned(), run.to_string_lossy().into_owned())];
        assert_eq!(register(&home.paths, || hyprland_running(&vars), || Ok(jev()), None).unwrap(), Registered::Skipped);
        assert!(!home.paths.main_file().exists());
    }

    /// Um desligar que chega depois das conferências de fora da trava e antes da gravação vence: nada volta.
    #[test]
    fn turning_off_just_before_the_lock_is_not_undone() {
        let home = Home::new();
        home.target("a");
        assert_eq!(home.run(false).unwrap(), Registered::Written);
        let paths = home.paths.clone();
        let outcome = crate::writer::race::with(
            &home.paths.parked_file(),
            move || crate::remove_entry(&paths).unwrap(),
            || home.run(true),
        );
        assert_eq!(outcome.unwrap(), Registered::Skipped);
        assert_eq!(home.main_entry(), None);
        assert_eq!(home.entry(&home.dir.path().join("acc/.claude.json")), None);
        assert!(home.paths.parked_file().is_file());
    }

    /// Tirada por fora do Hangar (`claude mcp remove`, sem guardada) fica fora; o PUT de ligar religa.
    #[test]
    fn removed_outside_hangar_stays_out_until_the_screen_turns_it_on() {
        let home = Home::new();
        let target = home.target("a");
        assert_eq!(home.run(true).unwrap(), Registered::Written);
        assert!(home.paths.registered_marker().is_file());
        std::fs::write(home.paths.main_file(), br#"{"mcpServers": {}}"#).unwrap();
        assert_eq!(home.run(true).unwrap(), Registered::Skipped);
        assert_eq!(std::fs::read(home.paths.main_file()).unwrap(), br#"{"mcpServers": {}}"#);
        assert!(!home.paths.parked_file().exists());

        let body: crate::SaveRequest = serde_json::from_value(json!({"enabled": true, "agent_config": target})).unwrap();
        crate::save(&home.paths, &crate::Facts::default(), &crate::Aux::default(), &body, || Ok(jev())).unwrap();
        assert_eq!(home.main_entry().unwrap()["command"], json!(home.binary()));
        assert!(home.paths.registered_marker().is_file());
        assert_eq!(home.run(false).unwrap(), Registered::Unchanged);
    }

    /// Desligado pela tela antes do primeiro registro fica desligado quando o alvo aparece; o PUT de ligar religa.
    #[test]
    fn turned_off_before_the_first_registration_stays_off() {
        let home = Home::new();
        crate::remove_entry(&home.paths).unwrap();
        let target = home.target("a");
        assert_eq!(home.run(true).unwrap(), Registered::Skipped);
        assert_eq!(home.main_entry(), None);

        let body: crate::SaveRequest = serde_json::from_value(json!({"enabled": true, "agent_config": target})).unwrap();
        crate::save(&home.paths, &crate::Facts::default(), &crate::Aux::default(), &body, || Ok(jev())).unwrap();
        assert_eq!(home.main_entry().unwrap()["command"], json!(home.binary()));
    }

    /// Máquina com a entrada ativa de antes da marca: a passada só grava a marca.
    #[test]
    fn an_active_entry_without_the_marker_only_gets_the_marker() {
        let home = Home::new();
        let entry = json!({"command": home.binary(), "args": [], "env": {"HCC_AGENT_CONFIG": "/alvos/d-agent.json"}});
        let file = json!({"mcpServers": {NAME: entry.clone()}}).to_string();
        std::fs::write(home.paths.main_file(), &file).unwrap();
        std::fs::write(home.dir.path().join("acc/.claude.json"), &file).unwrap();
        assert_eq!(home.run(false).unwrap(), Registered::Unchanged);
        assert!(home.paths.registered_marker().is_file());
        assert_eq!(std::fs::read_to_string(home.paths.main_file()).unwrap(), file);
        assert_eq!(std::fs::read_to_string(home.dir.path().join("acc/.claude.json")).unwrap(), file);
    }

    /// Entrada que já roda o binário do pacote fica intocada: `args`, campo extra e o alvo próprio de
    /// cada conta. Conta aberta sem a entrada recebe a do principal como está.
    #[test]
    fn a_package_entry_is_left_as_the_user_made_it() {
        let home = Home::new();
        home.target("a");
        let main = json!({"command": home.binary(), "args": ["--verbose"], "type": "stdio", "env": {"HCC_AGENT_CONFIG": "/alvos/b-agent.json"}});
        std::fs::write(home.paths.main_file(), json!({"mcpServers": {NAME: main.clone()}}).to_string()).unwrap();
        let own = json!({"command": home.binary(), "env": {"HCC_AGENT_CONFIG": "/alvos/c-agent.json"}});
        std::fs::write(home.dir.path().join("acc/.claude.json"), json!({"mcpServers": {NAME: own.clone()}}).to_string()).unwrap();
        let before = std::fs::read(home.paths.main_file()).unwrap();
        assert_eq!(home.run(false).unwrap(), Registered::Unchanged);
        assert_eq!(std::fs::read(home.paths.main_file()).unwrap(), before);
        assert_eq!(home.entry(&home.dir.path().join("acc/.claude.json")), Some(own));

        std::fs::write(home.dir.path().join("acc/.claude.json"), br#"{"x": 1}"#).unwrap();
        assert_eq!(home.run(false).unwrap(), Registered::Written);
        assert_eq!(std::fs::read(home.paths.main_file()).unwrap(), before);
        assert_eq!(home.entry(&home.dir.path().join("acc/.claude.json")), Some(main));
    }

    proptest! {
        /// Qualquer `.claude.json` principal: registra ou volta erro sem tocar no arquivo, nunca pânico.
        #[test]
        fn any_main_file_registers_or_leaves_it_intact(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
            let home = Home::new();
            home.target("a");
            std::fs::write(home.paths.main_file(), &bytes).unwrap();
            if register(&home.paths, || false, || Ok(jev()), None).is_err() {
                prop_assert_eq!(std::fs::read(home.paths.main_file()).unwrap(), bytes);
            }
        }

        #[test]
        fn hyprland_reader_never_panics(signature in "\\PC{0,40}", runtime in "\\PC{0,40}") {
            let _ = hyprland_running(&[("HYPRLAND_INSTANCE_SIGNATURE".into(), signature), ("XDG_RUNTIME_DIR".into(), runtime)]);
        }
    }
}

//! Os passos de atualização publicados rodam sem Python: comando e prova, nos dois sistemas.
use std::path::{Path, PathBuf};

fn steps_dir() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/atualizacoes") }

fn repo() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("../..") }

/// O passo que apaga os atalhos do Python é o único que pode citá-los.
const REMOVES_THE_SHIMS: &str = "2026-11-17-atalhos-python-removidos";

/// Os passos reescritos, com o título que eles tinham: o id e o título nunca mudam.
const REWRITTEN: &[(&str, &str)] = &[
    ("2026-08-26-orquestracao-e-arquivos-citados", "Modal Orquestração (papéis e contas do time) e aba Arquivos com \"citados\" e ícones"),
    ("2026-09-01-politica-de-contas-no-cofre", "A política de contas da orquestração mudou de ~/.claude para o cofre ~/.hangar"),
    ("2026-10-02-codex-mcp-hangar-headers", "O MCP hangar no Codex volta a saber de qual sessão vem cada chamada"),
    ("2026-10-03-claude-to-codex-transfer", "O seletor de contas permite continuar a sessão Claude no Codex"),
    ("2026-10-09-python3-antes-da-loja", "No Windows, os hooks em Python do Claude voltam a rodar"),
    ("2026-10-10-chaves-do-push-no-celular", "As notificações no celular voltam a funcionar"),
    ("2026-10-10-recado-entre-sessoes-sem-aprovacao", "Mandar recado para outra sessão deixa de pedir aprovação no Claude"),
    ("2026-10-12-mcp-hangar-leva-o-jev-da-sessao", "As ferramentas do navegador ganham o objetivo do Jev nas sessões abertas com ele"),
    ("2026-10-21-orq-durable-cleanup", "Orquestrações encerradas liberam builds e caches, preservando os relatórios"),
    ("2026-10-22-python-install-manager-windows", "No Windows, o Python instalado pelo winget volta a servir aos comandos do Hangar"),
    ("2026-10-24-atalho-hangar-python", "Os scripts do Hangar passam a usar o Python do próprio Hangar pelo atalho hangar-python"),
    ("2026-10-30-mcp-hangar-prazo-longo", "As sessões Claude e Codex esperam até 10 minutos pela sua resposta no cofre de senhas"),
    ("2026-10-31-links-do-pacote", "Atalhos, skills e bloco do shell voltam a apontar para a versão instalada"),
];

/// O que denuncia Python num comando ou numa prova: o interpretador, o `uv`, um `.py`, um módulo
/// `app.*`, o atalho `hangar-python` ou o lançador Python antigo do Codex.
fn python_word(text: &str) -> Option<String> {
    let words = text.split(|c: char| c.is_whitespace() || ";&|()<>\"'{}=,`$".contains(c)).filter(|w| !w.is_empty());
    for word in words {
        let base = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
        let hit = matches!(base.as_str(), "uv" | "uv.exe" | "hangar-python" | "hangar-python.cmd" | "hangar-codex-tui")
            || base.starts_with("python")
            || base.ends_with(".py")
            || word.starts_with("app.");
        if hit { return Some(word.to_owned()); }
    }
    None
}

#[test]
fn python_word_catches_the_old_forms_and_not_the_new_ones() {
    for old in ["python3 scripts/registrar-mcp.py", "cd backend && uv run python -m app.vapid_setup",
                "prova ~/.local/bin/hangar-python", "~/.local/bin/python3.cmd", "scripts/hangar-codex-tui",
                "%USERPROFILE%\\.local\\bin\\hangar-python.cmd"] {
        assert!(python_word(old).is_some(), "não pegou: {old}");
    }
    for new in ["scripts/hangar-server-command mcp-register --root .", "~/.hangar/passos/2026-11-16-servidor-sobe-sem-python-linux",
                "~/.local/bin/hangar-codex-tui.cmd", "~/.claude/skills/migrar-python-rust"] {
        assert_eq!(python_word(new), None, "pegou à toa: {new}");
    }
}

#[test]
fn no_published_step_calls_python_on_either_system() {
    for windows in [false, true] {
        let (steps, invalid) = hangar_server::update::steps::read_steps(&steps_dir(), windows);
        assert!(invalid.is_empty(), "passos inválidos: {invalid:?}");
        for step in steps.iter().filter(|step| step.id != REMOVES_THE_SHIMS) {
            if let Some(word) = python_word(&step.command) {
                panic!("{} (windows={windows}) chama Python no comando: {word}", step.id);
            }
            for proof in &step.proof {
                assert_eq!(python_word(proof), None, "{} (windows={windows}) prova com Python: {proof}", step.id);
            }
        }
    }
}

#[test]
fn rewritten_steps_keep_id_and_title_and_prove_with_what_exists() {
    for windows in [false, true] {
        let (steps, _) = hangar_server::update::steps::read_steps(&steps_dir(), windows);
        for (id, title) in REWRITTEN {
            let step = steps.iter().find(|step| step.id == *id).unwrap_or_else(|| panic!("passo {id} sumiu"));
            assert_eq!(step.title, *title, "{id}: título mudou");
            // Prova relativa é da raiz da cópia: tem que existir no repositório.
            for proof in step.proof.iter().filter(|p| !p.starts_with('~') && !p.starts_with('/')) {
                assert!(repo().join(proof).exists(), "{id}: prova {proof} não existe no repositório");
            }
        }
    }
}

#[test]
fn rewritten_commands_use_the_hangar_server_subcommands() {
    let (posix, _) = hangar_server::update::steps::read_steps(&steps_dir(), false);
    let (windows, _) = hangar_server::update::steps::read_steps(&steps_dir(), true);
    let command = |steps: &[hangar_server::update::steps::Step], id: &str| {
        steps.iter().find(|step| step.id == id).map(|step| step.command.clone()).unwrap_or_default()
    };
    for id in ["2026-10-02-codex-mcp-hangar-headers", "2026-10-10-recado-entre-sessoes-sem-aprovacao",
               "2026-10-12-mcp-hangar-leva-o-jev-da-sessao", "2026-10-30-mcp-hangar-prazo-longo"] {
        assert!(command(&posix, id).starts_with("scripts/hangar-server-command mcp-register"), "{id}");
    }
    let push = "2026-10-10-chaves-do-push-no-celular";
    assert!(command(&posix, push).starts_with("scripts/hangar-server-command vapid-setup"));
    assert!(command(&windows, push).contains("hangar-server.exe vapid-setup"));
    // A prova do push não depende de onde o `.env` mora (pacote ou checkout).
    let proof = posix.iter().find(|step| step.id == push).unwrap().proof.clone();
    assert_eq!(proof, vec![format!("~/.hangar/passos/{push}")]);
    for steps in [&posix, &windows] {
        let prune = steps.iter().find(|step| step.id == "2026-10-21-orq-durable-cleanup").unwrap();
        assert!(prune.command.contains("orq-prune-request-v1") && prune.machine && prune.destructive);
    }
}

#[test]
fn the_shim_removal_step_is_a_machine_step_on_both_systems() {
    for windows in [false, true] {
        let (steps, _) = hangar_server::update::steps::read_steps(&steps_dir(), windows);
        let step = steps.iter().find(|step| step.id == REMOVES_THE_SHIMS).expect("passo que tira os atalhos");
        assert!(step.machine && step.destructive);
        assert!(!step.command.is_empty(), "windows={windows}: sem comando");
        assert_eq!(step.proof, vec![format!("~/.hangar/passos/{REMOVES_THE_SHIMS}")]);
        assert!(step.command.contains("hangar-python"));
    }
}

/// O comando POSIX do passo, numa casa de mentira: tira só os atalhos do Hangar e deixa a prova.
#[cfg(unix)]
#[test]
fn the_shim_removal_keeps_a_real_python_and_removes_ours() {
    let (steps, _) = hangar_server::update::steps::read_steps(&steps_dir(), false);
    let step = steps.iter().find(|step| step.id == REMOVES_THE_SHIMS).unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".local/bin");
    let cache = home.path().join(".hangar/cache/install-bin");
    let scripts = home.path().join("repo/scripts");
    for dir in [&bin, &cache, &scripts] { std::fs::create_dir_all(dir).unwrap(); }
    std::fs::write(scripts.join("hangar-python"), "#!/bin/sh\n").unwrap();
    std::os::unix::fs::symlink(scripts.join("hangar-python"), bin.join("hangar-python")).unwrap();
    std::fs::write(cache.join("python3"), format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", scripts.join("hangar-python").display())).unwrap();
    std::os::unix::fs::symlink("/usr/bin/python3", bin.join("python3")).unwrap();
    let status = std::process::Command::new("/bin/sh").arg("-c").arg(&step.command)
        .env("HOME", home.path()).current_dir(home.path()).status().unwrap();
    assert!(status.success());
    assert!(std::fs::symlink_metadata(bin.join("hangar-python")).is_err(), "atalho hangar-python ficou");
    assert!(!cache.join("python3").exists(), "atalho python3 do instalador ficou");
    assert!(std::fs::symlink_metadata(bin.join("python3")).is_ok(), "o python3 da pessoa saiu");
    let proof = std::fs::read_to_string(home.path().join(".hangar/passos").join(REMOVES_THE_SHIMS)).unwrap();
    assert_eq!(proof, "sobrou:\n");
}

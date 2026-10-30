//! Instaladores usam o mesmo serviço em processo, inclusive com o backend parado.
use crate::accounts::{AccountService, environment::AccountEnvironment};
use std::{path::PathBuf, process::ExitCode};

pub fn run(args: &[String]) -> ExitCode {
    if args.first().map(String::as_str) == Some("install") {
        return install(&args[1..]);
    }
    let Some(mode) = args
        .first()
        .map(String::as_str)
        .filter(|v| matches!(*v, "skills" | "hooks" | "persona"))
    else {
        eprintln!("uso: hangar-server harness-setup skills|hooks|persona [--repo CAMINHO]");
        return ExitCode::from(2);
    };
    let env = AccountEnvironment::capture();
    let repo = match &args[1..] {
        [] => crate::install_root::root(&env.home),
        [flag, path] if flag == "--repo" => Some(PathBuf::from(path)),
        _ => {
            eprintln!("argumentos de instalação inválidos");
            return ExitCode::from(2);
        }
    };
    let Some(repo) = repo.filter(|p| p.join("backend").is_dir()) else {
        eprintln!("raiz da instalação do Hangar não encontrada");
        return ExitCode::FAILURE;
    };
    if mode == "persona" {
        let settings = env.home.join(".claude/settings.json");
        let installed = env.home.join(".claude/plugins/installed_plugins.json");
        if !settings.is_file() || !installed.is_file() {
            println!(
                "pulado: sem {} ou {} (Claude Code não instalado nesta conta)",
                settings.display(),
                installed.display()
            );
            return ExitCode::SUCCESS;
        }
        match super::persona::sync(&env.home) {
            Ok(lines) => {
                for line in lines {
                    println!("{line}");
                }
            }
            Err(error) => {
                eprintln!("instalação de persona falhou: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    let result = if mode == "hooks" {
        super::sync_hooks(&AccountService::new(env), &repo)
    } else {
        super::sync_skills(&env, &repo)
    };
    if mode != "persona" {
        println!("{result}");
    }
    if result
        .as_object()
        .is_some_and(|map| map.values().any(|v| v.get("erro").is_some()))
    {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn install(args: &[String]) -> ExitCode {
    let [cli] = args else {
        eprintln!("uso: hangar-server harness-setup install codex|pi|omp|kimi");
        return ExitCode::from(64);
    };
    if !super::commands::NAMES.contains(&cli.as_str()) {
        eprintln!("uso: hangar-server harness-setup install codex|pi|omp|kimi");
        return ExitCode::from(64);
    }
    let Some(argv) = super::commands::command(cli, cfg!(windows)) else {
        eprintln!(
            "{cli}: sem instalador conferido para este sistema; veja {}",
            super::commands::manual()[cli].as_str().unwrap_or("")
        );
        return ExitCode::from(2);
    };
    let full = match super::commands::resolve(argv.clone(), &AccountEnvironment::capture()) {
        Ok(full) => full,
        Err(error) => {
            eprintln!("{cli}: {error}");
            return ExitCode::from(3);
        }
    };
    println!("$ {}", super::commands::display(&argv));
    match std::process::Command::new(&full[0])
        .args(&full[1..])
        .status()
    {
        Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
        Err(error) => {
            eprintln!("{cli}: {error}");
            ExitCode::FAILURE
        }
    }
}

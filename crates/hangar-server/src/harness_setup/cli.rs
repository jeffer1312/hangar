//! Instaladores usam o mesmo serviço em processo, inclusive com o backend parado.
use crate::accounts::{AccountService, environment::AccountEnvironment};
use std::{path::PathBuf, process::ExitCode};

pub fn run(args: &[String]) -> ExitCode {
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

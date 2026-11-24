//! Instaladores usam o mesmo serviço em processo, inclusive com o backend parado.
use crate::accounts::{AccountService, environment::AccountEnvironment};
use std::{path::PathBuf, process::ExitCode};

pub fn run(args: &[String]) -> ExitCode {
    if args == ["repair-codex"] { return repair_codex(); }
    if args.first().map(String::as_str) == Some("install") {
        return install(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("extensions") {
        return extensions(&args[1..]);
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
    // A versão do pacote se reconhece pelo manifesto, mesmo sem `backend/`.
    let Some(repo) = repo.filter(|p| hangar_core::paths::is_install_root(p) || p.join("backend").is_dir()) else {
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
        eprintln!("uso: hangar-server harness-setup install claude|codex|pi|omp|kimi|antigravity");
        return ExitCode::from(64);
    };
    if !super::commands::NAMES.contains(&cli.as_str()) {
        eprintln!("uso: hangar-server harness-setup install claude|codex|pi|omp|kimi|antigravity");
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

/// Uso errado de `extensions`.
const EXTENSIONS_USAGE: u8 = 2;
/// O Windows recusou o symlink: falta o Modo Desenvolvedor (ou admin).
const EXTENSIONS_DEVELOPER_MODE: u8 = 4;

/// `extensions pi|omp [--repo CAMINHO]`; `None` é uso errado.
fn parse_extensions(args: &[String]) -> Option<(&str, Option<PathBuf>)> {
    let (cli, repo) = match args {
        [cli] => (cli, None),
        [cli, flag, path] if flag == "--repo" && !path.is_empty() => (cli, Some(PathBuf::from(path))),
        _ => return None,
    };
    matches!(cli.as_str(), "pi" | "omp").then_some((cli.as_str(), repo))
}

/// O mesmo conserto `extensoes:pi|omp` do painel, para o instalador do Windows.
fn extensions(args: &[String]) -> ExitCode {
    let Some((cli, repo)) = parse_extensions(args) else {
        eprintln!("uso: hangar-server harness-setup extensions pi|omp [--repo CAMINHO]");
        return ExitCode::from(EXTENSIONS_USAGE);
    };
    let env = AccountEnvironment::capture();
    let Some(repo) = repo
        .or_else(|| crate::install_root::root(&env.home))
        .filter(|p| hangar_core::paths::is_install_root(p) || p.join("backend").is_dir())
    else {
        eprintln!("raiz da instalação do Hangar não encontrada");
        return ExitCode::FAILURE;
    };
    // Sem as fontes o conserto não liga nada e diria "0 ligadas" como se estivesse certo.
    if !repo.join("scripts/pi/hangar-state.ts").is_file() {
        eprintln!("{cli}: {} não tem as extensões (scripts/pi)", repo.display());
        return ExitCode::FAILURE;
    }
    match super::repair::link_extensions(&env, &repo, cli) {
        Ok(count) => {
            println!("{cli}: {count} extensões ligadas a {}", repo.join("scripts/pi").display());
            ExitCode::SUCCESS
        }
        Err(error) if error.needs_developer_mode => {
            eprintln!("{cli}: {error}");
            ExitCode::from(EXTENSIONS_DEVELOPER_MODE)
        }
        Err(error) => {
            eprintln!("{cli}: extensões não ligadas: {error}");
            ExitCode::FAILURE
        }
    }
}

fn repair_codex()->ExitCode {
    let result=(||->Result<(),String>{
        let env=AccountEnvironment::capture();
        let repo=crate::install_root::root(&env.home).ok_or("raiz da instalação do Hangar não encontrada")?;
        let config=crate::config_sync::runtime::RuntimeConfig::new(env.clone())?;
        let service=crate::codex_setup::service::Service::new(AccountService::new(env),config,repo,None);
        let runtime=tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|error|error.to_string())?;
        runtime.block_on(async{let result=service.repair_all().await;let closed=service.shutdown().await;result.and(closed)})
    })();
    match result {Ok(())=>ExitCode::SUCCESS,Err(error)=>{eprintln!("Reparo das contas Codex falhou: {error}");ExitCode::FAILURE}}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn extensions_takes_pi_or_omp_and_an_optional_repo() {
        assert_eq!(parse_extensions(&args(&["pi"])), Some(("pi", None)));
        assert_eq!(parse_extensions(&args(&["omp"])), Some(("omp", None)));
        assert_eq!(
            parse_extensions(&args(&["omp", "--repo", "/x/hangar"])),
            Some(("omp", Some(PathBuf::from("/x/hangar"))))
        );
    }

    #[test]
    fn extensions_refuses_anything_else() {
        for wrong in [
            &[][..],
            &["codex"][..],
            &["kimi"][..],
            &["pi", "omp"][..],
            &["pi", "--repo"][..],
            &["pi", "--repo", ""][..],
            &["pi", "--root", "/x"][..],
            &["pi", "--repo", "/x", "mais"][..],
        ] {
            assert_eq!(parse_extensions(&args(wrong)), None, "{wrong:?}");
        }
    }

    #[test]
    fn extensions_usage_is_refused_with_its_code() {
        assert_eq!(extensions(&args(&["kimi"])), ExitCode::from(EXTENSIONS_USAGE));
    }
}

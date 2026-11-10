//! Arquivos que permitem ao CLI encontrar o plugin e a identidade da máquina.
use super::{bundle::{Blob,Error},files};
use serde_json::json;
use std::{io,path::{Path,PathBuf},sync::Arc};

/// Primeira versão do `claude` com os mods ligados por padrão.
const MODS_BY_DEFAULT:(u64,u64,u64)=(2,1,287);

/// Onde publicar e com que identidade. `repo`: a raiz da instalação, `None` quando não foi achada.
pub struct Machine {
    pub home:PathBuf,
    pub repo:Option<PathBuf>,
    pub port:u16,
    pub secret:String,
    pub probe:crate::cli_probe::Probe,
    pub env:crate::accounts::environment::AccountEnvironment,
}

/// Uma publicação por vez: a da subida, que sonda o `claude` por até 20 s, não pode gravar por cima da
/// que uma troca de `claude_function_hooks` pediu depois.
static TURN:tokio::sync::Mutex<()>=tokio::sync::Mutex::const_new(());

/// Publica os dois arquivos com a lista de plugins valendo só se o `claude` desta máquina a aceita:
/// `--plugin-dir` desconhecido mata a sessão ao nascer. `hooks` é lido já na vez, para valer o estado
/// mais novo; hooks desligados não sondam nada. Leitura que não termina não publica nada.
pub async fn publish_in_turn(machine:Machine,hooks:impl FnOnce()->bool+Send+'static)->Result<(),Error> {
    let _turn=TURN.lock().await;
    let hooks=tokio::task::spawn_blocking(hooks).await
        .map_err(|error|Error::new("plugin_hooks_unreadable",format!("leitura de claude_function_hooks interrompida: {error}"),json!({})))?;
    let enabled=hooks&&takes_plugin_dir(&machine.probe,&machine.env).await;
    let Machine{home,repo,port,secret,..}=machine;
    tokio::task::spawn_blocking(move||publish(&home,repo.as_deref(),port,&secret,enabled)).await
        .map_err(|error|Error::new("plugin_publish_interrupted",format!("publicação do plugin interrompida: {error}"),json!({})))?
}

/// Pelo estado do servidor, na subida e quando `claude_function_hooks` muda. A falha vai ao log.
pub async fn publish_for_state(state:Arc<crate::routes::AppState>) {
    let service=match super::runtime::RuntimeConfig::from_state(&state) {
        Ok(service)=>service,
        Err(error)=>return tracing::warn!(code="plugin_publish_failed",%error,"caminho do plugin não foi publicado"),
    };
    let home=service.env.home.clone();
    let machine=Machine{repo:crate::install_root::root(&home),home,port:state.cfg.listen.port(),secret:state.cfg.auth_token.clone(),
        probe:state.harness_probe.clone(),env:service.env.clone()};
    if let Err(error)=publish_in_turn(machine,move||crate::orchestration::truthy(&service.get("claude_function_hooks"))).await {
        tracing::warn!(code=error.code,%error,"caminho do plugin não foi publicado");
    }
}

async fn takes_plugin_dir(probe:&crate::cli_probe::Probe,env:&crate::accounts::environment::AccountEnvironment)->bool {
    let Some(version)=probe.version_text(env,"claude").await else {return false};
    if parse_version(&version).is_some_and(|version|version>=MODS_BY_DEFAULT) {return true;}
    probe.help_text(env,"claude").await.is_some_and(|help|help.contains("--plugin-dir"))
}

/// `X.Y.Z` no começo, como o `re.match(r"\s*(\d+)\.(\d+)\.(\d+)")` do Python.
pub fn parse_version(text:&str)->Option<(u64,u64,u64)> {
    let number=|part:&str|(!part.is_empty()&&part.bytes().all(|b|b.is_ascii_digit())).then(||part.parse().ok()).flatten();
    let mut parts=text.trim_start().splitn(3,'.');
    let (major,minor)=(number(parts.next()?)?,number(parts.next()?)?);
    let patch=parts.next()?;
    Some((major,minor,number(&patch[..patch.bytes().take_while(u8::is_ascii_digit).count()])?))
}

pub fn publish(home:&Path,repo:Option<&Path>,port:u16,secret:&str,enabled:bool)->Result<(),Error> {
    let directory=home.join(".hangar");
    let machine=json!({"url":format!("http://127.0.0.1:{port}/api/plugin"),"chave":crate::mods::bridge::machine_key(if secret.is_empty(){"hangar"}else{secret})});
    let address=files::write_file(&directory.join("plugin.json"),&Blob{data:crate::transcript::pyjson::dumps(&machine,false).into_bytes(),mode:0o600});
    // O endereço e a lista são independentes: uma falha não conserva o interruptor antigo.
    publish_directories(&directory.join("plugin-dir"),repo,enabled)?;
    address
}

fn publish_directories(target:&Path,repo:Option<&Path>,enabled:bool)->Result<(),Error> {
    if !enabled {
        return match std::fs::remove_file(target) {Err(error) if error.kind()==io::ErrorKind::NotFound=>Ok(()),result=>result.map_err(Error::from)};
    }
    // Lista que aponta para pasta que não existe mataria toda sessão nova; a que já está lá fica.
    let Some(repo)=repo.filter(|repo|repo.join("plugins/hangar").is_dir()) else {
        return Err(Error::new("plugin_root_missing","raiz da instalação sem plugins/hangar; plugin-dir mantido",json!({})));
    };
    let plugins=repo.join("plugins");let own=plugins.join("hangar");
    let others=(||->io::Result<Vec<_>> {
        std::fs::read_dir(&plugins)?.map(|entry|entry.map(|entry|entry.path())).collect()
    })();
    let mut others=match others {
        Ok(paths)=>paths.into_iter().filter(|path|path!=&own&&path.join(".claude-plugin/plugin.json").is_file()).collect::<Vec<_>>(),
        Err(error)=>{tracing::warn!(%error,"Não foi possível listar os plugins; mantido o plugin do Hangar");vec![]},
    };
    others.sort();
    let data=std::iter::once(own).chain(others).map(|path|format!("{}\n",path.display())).collect::<String>();
    files::write_file(target,&Blob{data:data.into_bytes(),mode:files::mode(target).unwrap_or(0o644)})
}

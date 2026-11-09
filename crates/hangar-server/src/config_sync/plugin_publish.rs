//! Arquivos que permitem ao CLI encontrar o plugin e a identidade da máquina.
use super::{bundle::{Blob,Error},files};
use serde_json::json;
use std::{io,path::Path};

/// Primeira versão do `claude` com os mods ligados por padrão.
const MODS_BY_DEFAULT:(u64,u64,u64)=(2,1,287);

/// Publica os dois arquivos com a lista de plugins valendo só se o `claude` desta máquina a aceita:
/// `--plugin-dir` desconhecido mata a sessão ao nascer. Hooks desligados não sondam nada.
pub async fn publish_for_machine(home:&Path,repo:&Path,port:u16,secret:&str,hooks:bool,
    probe:&crate::cli_probe::Probe,env:&crate::accounts::environment::AccountEnvironment)->Result<(),Error> {
    let enabled=hooks&&takes_plugin_dir(probe,env).await;
    let (home,repo,secret)=(home.to_owned(),repo.to_owned(),secret.to_owned());
    tokio::task::spawn_blocking(move||publish(&home,&repo,port,&secret,enabled)).await
        .map_err(|error|Error::invalid(format!("publicação do plugin interrompida: {error}")))?
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

pub fn publish(home:&Path,repo:&Path,port:u16,secret:&str,enabled:bool)->Result<(),Error> {
    let directory=home.join(".hangar");
    let machine=json!({"url":format!("http://127.0.0.1:{port}/api/plugin"),"chave":crate::mods::bridge::machine_key(if secret.is_empty(){"hangar"}else{secret})});
    let address=files::write_file(&directory.join("plugin.json"),&Blob{data:crate::transcript::pyjson::dumps(&machine,false).into_bytes(),mode:0o600});
    // O endereço e a lista são independentes: uma falha não conserva o interruptor antigo.
    publish_directories(&directory.join("plugin-dir"),repo,enabled)?;
    address
}

fn publish_directories(target:&Path,repo:&Path,enabled:bool)->Result<(),Error> {
    if !enabled {
        return match std::fs::remove_file(target) {Err(error) if error.kind()==io::ErrorKind::NotFound=>Ok(()),result=>result.map_err(Error::from)};
    }
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

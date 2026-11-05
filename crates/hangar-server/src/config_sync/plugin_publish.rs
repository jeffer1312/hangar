//! Arquivos que permitem ao CLI encontrar o plugin e a identidade da máquina.
use super::{bundle::{Blob,Error},files};
use serde_json::json;
use std::{io,path::Path};

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

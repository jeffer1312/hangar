//! Artefatos pertencem à fonte comprovada; arquivos particulares são conservados.
use super::files;
use serde_json::{Value,json};
use std::{collections::{BTreeMap,BTreeSet},path::{Path,PathBuf}};
pub trait Storage {
    fn read(&mut self,path:&Path)->Result<Option<Vec<u8>>,files::Error>;
    fn write(&mut self,path:&Path,data:&[u8],expected:Option<&[u8]>,backups:&Path)->Result<bool,files::Error>;
    fn backup(&mut self,path:&Path,data:&[u8],backups:&Path)->Result<(),files::Error>;
    fn remove(&mut self,path:&Path)->Result<(),files::Error>;
}
pub struct Disk;
impl Storage for Disk {
    fn read(&mut self,path:&Path)->Result<Option<Vec<u8>>,files::Error>{files::read(path)}
    fn write(&mut self,path:&Path,data:&[u8],expected:Option<&[u8]>,backups:&Path)->Result<bool,files::Error>{files::write(path,data,expected,Some(backups))}
    fn backup(&mut self,path:&Path,data:&[u8],backups:&Path)->Result<(),files::Error>{files::backup(path,data,backups)}
    fn remove(&mut self,path:&Path)->Result<(),files::Error>{std::fs::remove_file(path).map_err(Into::into)}
}
fn warning(code:&str,path:&Path,error:Option<&files::Error>)->Value {
    let mut params=serde_json::Map::from_iter([("path".into(),json!(path.to_string_lossy()))]);
    if let Some(error)=error {params.insert("erro".into(),json!(error.to_string()));}
    super::messages::message(code,&params).expect("mensagem de artefato versionada")
}
pub fn reconcile(desired:&BTreeMap<PathBuf,Vec<u8>>,previous:&Value,backups:&Path,trusted:&BTreeSet<PathBuf>,storage:&mut impl Storage)->Result<Value,files::Error>{
    let previous=previous.as_object().ok_or_else(||files::Error::Invalid("Manifesto de artefatos precisa ser um objeto".into()))?;
    let mut manifest=serde_json::Map::new();let mut warnings=Vec::new();
    for (path,data) in desired {
        let name=path.to_string_lossy().into_owned();let old=previous.get(&name);
        let expected=old.and_then(Value::as_object).and_then(|v|v.get("hash")).filter(|v|!v.is_null());
        let result=(||->Result<(),files::Error>{
            for attempt in 0..3 {
                let current=storage.read(path)?;
                if current.as_ref().is_some_and(|v|v!=data)&&expected.is_none()&&!trusted.contains(path){
                    warnings.push(warning("aviso_artefato_sem_proveniencia",path,None));break;
                }
                match storage.write(path,data,current.as_deref(),backups){
                    Ok(_)=>{manifest.insert(name.clone(),json!({"hash":files::hash(data)}));break;},
                    Err(files::Error::Changed(_)) if attempt<2=>continue,
                    Err(error)=>return Err(error),
                }
            }
            Ok(())
        })();
        if let Err(error)=result {
            warnings.push(warning("aviso_artefato_falha",path,Some(&error)));
            if expected.is_some(){manifest.insert(name,crate::orchestration::codec::clone_value(old.unwrap()));}
        }
    }
    let mut names:Vec<_>=previous.keys().collect();names.sort();
    for name in names {
        let old=&previous[name];let path=Path::new(name);
        let Some(expected)=old.as_object().and_then(|v|v.get("hash")).and_then(Value::as_str) else{continue;};
        if desired.contains_key(path){continue;}
        let result=(||->Result<(),files::Error>{
            let Some(current)=storage.read(path)? else{return Ok(());};
            if files::hash(&current)!=expected {
                warnings.push(warning("aviso_artefato_obsoleto_alterado",path,None));
                manifest.insert(name.clone(),crate::orchestration::codec::clone_value(old));return Ok(());
            }
            storage.backup(path,&current,backups)?;
            if storage.read(path)?.as_deref()!=Some(current.as_slice()){
                return Err(files::Error::Changed(format!("Artefato alterado durante a retirada: {}",path.display())));
            }
            storage.remove(path)
        })();
        if let Err(error)=result {
            warnings.push(warning("aviso_artefato_obsoleto_falha",path,Some(&error)));
            manifest.insert(name.clone(),crate::orchestration::codec::clone_value(old));
        }
    }
    Ok(Value::Object(serde_json::Map::from_iter([("manifest".into(),Value::Object(manifest)),("warnings".into(),Value::Array(warnings))])))
}

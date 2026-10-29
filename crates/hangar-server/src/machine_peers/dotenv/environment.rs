//! Ambiente congelado preservando a ordem de precedência e a grafia da interpolação.
use std::collections::{BTreeMap,BTreeSet};

#[derive(Clone,Default)]
pub struct SettingsEnvironment {
    exact:BTreeMap<String,String>,
    effective:BTreeMap<String,String>,
    invalid:BTreeSet<String>,
}
impl SettingsEnvironment {
    pub fn from_pairs<K:Into<String>,V:Into<String>>(pairs:impl IntoIterator<Item=(K,V)>)->Self {
        let mut captured=Self::default();
        for (name,value) in pairs {
            let name=name.into();let value=value.into();
            captured.effective.insert(name.to_lowercase(),value.clone());
            captured.exact.insert(name,value);
        }
        captured
    }
    pub fn capture()->Self {
        let mut captured=Self::default();
        for (name,value) in std::env::vars_os() {
            let Ok(name)=name.into_string() else {continue;};
            match value.into_string() {
                Ok(value)=>{captured.effective.insert(name.to_lowercase(),value.clone());captured.exact.insert(name,value);},
                Err(_)=>{captured.invalid.insert(name.to_lowercase());}
            }
        }
        captured
    }
    /// Para consumidores que já validaram os nomes relevantes; `value` conserva a recusa UTF-8.
    pub fn settings(&self,raw:&str)->BTreeMap<String,Option<String>> {
        let mut values=super::file_settings(raw,&self.exact);
        values.extend(self.effective.iter().map(|(name,value)|(name.clone(),Some(value.clone()))));
        values
    }
    /// Interpolação usa grafia exata; o valor final usa a última entrada sem distinção de caixa.
    pub fn value(&self,raw:&str,key:&str)->Result<Option<String>,String> {
        let normalized=key.to_lowercase();
        if self.invalid.contains(&normalized){return Err(format!("configuração {key} inválida: valor fora de UTF-8"));}
        if let Some(value)=self.effective.get(&normalized){return Ok(Some(value.clone()));}
        Ok(super::file_settings(raw,&self.exact).remove(&normalized).flatten())
    }
}

/// Configuração congelada por serviço: o arquivo e o ambiente pertencem à mesma leitura inicial.
#[derive(Clone)]
pub struct SettingsSnapshot {
    environment:SettingsEnvironment,
    file:BTreeMap<String,Option<String>>,
}
impl SettingsSnapshot {
    pub fn from_parts(environment:SettingsEnvironment,raw:String)->Self {
        let file=super::file_settings(&raw,&environment.exact);
        Self{environment,file}
    }
    pub fn capture(home:&std::path::Path,backend:Option<&std::path::Path>,environment:SettingsEnvironment)->Result<Self,String>{
        let raw=match crate::user_data::path(home,backend,".env").map_err(|e|e.to_string())? {
            Some(path)=>match crate::runtime::queue::read_replaced_to_string(&path){
                Ok(raw)=>raw,
                Err(error) if error.kind()==std::io::ErrorKind::NotFound=>String::new(),
                Err(error)=>return Err(error.to_string()),
            },
            None=>String::new(),
        };
        Ok(Self::from_parts(environment,raw))
    }
    pub fn value(&self,key:&str)->Result<Option<String>,String>{self.aliases(&[key])}
    /// A fonte tem prioridade sobre o alias: qualquer alias no ambiente vence os do arquivo.
    pub fn aliases(&self,keys:&[&str])->Result<Option<String>,String>{
        for key in keys {if let Some(value)=self.environment.value("",key)?{return Ok(Some(value));}}
        for key in keys {if let Some(value)=self.file.get(&key.to_lowercase()).cloned().flatten(){return Ok(Some(value));}}
        Ok(None)
    }
}

//! Manifesto efetivo do Help fornecido com o servidor; o cadastro original permanece reversível.
use std::path::{Path,PathBuf};
use hangar_connectors::{AdminError,manifest::Manifest,host::ManifestResolver};
use serde_json::json;

pub struct Provided {executable: PathBuf}
impl Provided {
    pub fn new(executable: PathBuf) -> Self {Self {executable}}
}

impl ManifestResolver for Provided {
    fn resolve(&self,registered: &Manifest) -> Result<Option<Manifest>,AdminError> {
        let Some(program)=registered.command.first().and_then(|program|Path::new(program).file_name()).and_then(|name|name.to_str()) else {return Ok(None);};
        let name=program.to_ascii_lowercase();
        let offset=match name.as_str() {
            "hangar-help-test-connector"|"hangar-help-test-connector.exe"=>1,
            "hangar-server"|"hangar-server.exe" if registered.command.get(1).is_some_and(|arg|arg=="help-connector")=>2,
            _=>return Ok(None),
        };
        if registered.protocol_version!=1 || registered.capabilities.get("work_tracking")!=Some(&1)
            || registered.capabilities.iter().any(|(name,version)|!matches!(name.as_str(),"work_tracking"|"work_actions") || *version!=1) {return Ok(None);}
        let args=&registered.command[offset..];
        if args.len()%2!=0 || args.chunks_exact(2).any(|pair|!matches!(pair[0].as_str(),"--id"|"--help-url"|"--config-dir"|"--claude-config-dir")) {return Ok(None);}
        if args.chunks_exact(2).filter(|pair|pair[0]=="--id").last().is_some_and(|pair|pair[1]!=registered.id) {return Ok(None);}
        let executable=self.executable.to_str().ok_or_else(||AdminError::invalid("o caminho do servidor não é UTF-8"))?;
        let mut command=vec![executable.to_owned(),"help-connector".into()];command.extend_from_slice(args);
        if !args.chunks_exact(2).any(|pair|pair[0]=="--id") {command.extend(["--id".into(),registered.id.clone()]);}
        let mut capabilities=registered.capabilities.clone();capabilities.insert("work_actions".into(),1);
        let raw=json!({"manifest_version":1,"id":registered.id,"name":registered.name,"version":registered.version,
            "protocol_version":registered.protocol_version,"capabilities":capabilities,"command":command,"start_prompt":registered.start_prompt});
        Manifest::parse(raw.to_string().as_bytes(),&registered.cwd).map(Some)
    }
}

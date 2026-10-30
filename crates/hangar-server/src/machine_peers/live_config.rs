//! Valores publicados ao vivo pelo dono; o ambiente herdado só cede à publicação conferida.
use super::{dotenv::SettingsEnvironment,store};
use std::path::Path;
const CHANNEL_PUBLICATION:&str="# hangar-update-channel-publication=";

pub fn update_channel(home:&Path,backend:Option<&Path>,environment:&SettingsEnvironment,key:&str)->Result<String,String> {
    let inherited=environment.value("",key)?;
    let Some(path)=crate::user_data::path(home,backend,".env").map_err(|error|error.to_string())? else {return Ok(inherited.unwrap_or_default());};
    let _lock=store::lock(&path).map_err(|error|error.msg)?;
    let raw=crate::user_data::read_env_at(&path)?;
    let disk=environment.file_value(&raw,key).unwrap_or_default();
    let marker=std::fs::read(path.with_extension("env.update-channel.json")).ok().and_then(|bytes|serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    if let Some(marker)=marker {
        let nonce=marker["nonce"].as_str().unwrap_or("");
        if !nonce.is_empty() && inherited.as_deref()==marker["inherited"][key].as_str()
            && marker["published"][key].as_str()==Some(disk.as_str())
            && raw.lines().any(|line|line.strip_prefix(CHANNEL_PUBLICATION)==Some(nonce)) {
            return Ok(disk);
        }
    }
    Ok(inherited.unwrap_or(disk))
}

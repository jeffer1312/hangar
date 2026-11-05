//! Metadados de ambiente exibidos sem divulgar valores secretos.
use super::runtime::RuntimeConfig;
use serde_json::{Value,json};

impl RuntimeConfig {
    pub fn metadata(&self)->Result<Value,String> {
        let home=&self.env.home;
        let fields=[
            ("lan_bind_ip",json!("127.0.0.1")),("port",json!(8765)),("auth_token",json!("change-me")),
            ("projects_dir",json!(self.env.base.get("CLAUDE_CONFIG_DIR").filter(|value|!value.is_empty()).map(std::path::PathBuf::from).unwrap_or_else(||home.join(".claude")).join("projects"))), ("reload",json!(false)),("diag_term_input",json!(false)),
            ("front_port",json!(0)),("public_url",json!("")),("server_id",json!("")),("vapid_public",json!("")),("vapid_private",json!("")),
            ("vapid_subject",json!("mailto:push@hangar.dev.br")),("stall_poll_seconds",json!(30)),("auto_resume",json!(false)),
            ("omp_plugin_sync_enabled",json!(false)),("omp_plugin_sync_interval",json!(300.0)),("omp_claude_context_enabled",json!(false)),
            ("sync_bootstrap",json!("")),("sync_data",json!(home.join(".hangar/sync-vault.json"))),("sync_session_secret",json!("")),
            ("sync_rate_max",json!(10)),("sync_rate_window",json!(300)),("forwarded_allow_ips",json!("127.0.0.1")),
            ("rust_server",json!(true)),("deploy_secret",json!("")),("update_branch",json!("")),("update_last_branch",json!("")),
        ];
        let mut result=Vec::new();
        for (key,default) in fields {
            let name=format!("CP_{}",key.to_ascii_uppercase());
            let raw=self.settings.value(&name)?;
            let value=match raw {
                None=>default,
                Some(raw) if raw.trim().is_empty()&&matches!(key,"front_port"|"omp_plugin_sync_interval")=>default,
                Some(raw) if default.is_boolean()=>crate::value_coercion::boolean(&json!(raw))
                    .map(Value::Bool).ok_or_else(||format!("{name}: booleano inválido"))?,
                Some(raw) if key=="omp_plugin_sync_interval"=>positive_interval(&raw)
                    .map(|value|json!(value)).ok_or_else(||format!("{name}: número inválido"))?,
                Some(raw) if default.is_number()=>crate::value_coercion::integer(&json!(raw))
                    .ok_or_else(||format!("{name}: número inválido"))?,
                Some(raw) if matches!(key,"update_branch"|"update_last_branch")=> {
                    let branch=raw.trim();
                    let valid=branch.len()<=100 && !branch.starts_with('-') && !branch.contains("..")
                        && branch.bytes().all(|byte|byte.is_ascii_alphanumeric()||b"._/-".contains(&byte));
                    json!(if valid {branch}else{""})
                },
                Some(raw)=>json!(raw),
            };
            let secret=["auth_token","vapid_private","sync_bootstrap","sync_session_secret","deploy_secret"].contains(&key)
                ||["secret","token","password","senha","_key","apikey","private"].iter().any(|part|key.contains(part));
            let description=match key {"omp_plugin_sync_enabled"=>"omp_plugin_sync","omp_claude_context_enabled"=>"omp_claude_context",_=>key};
            result.push(json!({"nome":name,"valor":if secret{Value::Null}else{value.clone()},"definida":!value.is_null()&&value!="","segredo":secret,"descricao":description,"alerta":null}));
        }
        for (name,description) in [("CP_TERMINAL","terminal"),("CP_PRICING_OFFLINE","pricing_offline"),("CP_CLAUDE_CONFIG_DIRS","claude_config_dirs"),("CP_ENGINES_FILE","engines_file"),("CP_CODEX_SYNC_ENABLED","codex_sync_enabled")] {
            let raw=self.env.base.get(name).cloned().unwrap_or_default();
            let warning=if name=="CP_CODEX_SYNC_ENABLED"&&["0","false","no"].contains(&raw.to_ascii_lowercase().as_str()){Some("codex_sync_desligado")}else{None};
            result.push(json!({"nome":name,"valor":raw,"definida":!raw.is_empty(),"segredo":false,"descricao":description,"alerta":warning}));
        }
        Ok(json!(result))
    }
}

fn positive_interval(raw:&str)->Option<f64> {
    let raw=raw.trim();
    if raw.starts_with('_')||raw.ends_with('_')||raw.contains("__") {return None;}
    let value=raw.replace('_',"").parse::<f64>().ok()?;
    (value.is_finite()&&value>0.0).then_some(value)
}

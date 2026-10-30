//! Operações de serviço do checkout; a instalação manual nunca encerra processo alheio.
use super::{Service, tail};
impl Service {
    pub fn command(&self, args: &[&str], seconds: u64) -> Result<super::ProcessOutput, String> {
        self.run(
            &args.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
            &self.root,
            seconds,
        )
    }
    pub fn topology(&self) -> &'static str {
        if cfg!(windows) {
            return "windows";
        }
        #[cfg(target_os = "macos")]
        {
            return if self
                .command(&["launchctl", "print", &launchd_target()], 15)
                .is_ok_and(|v| v.code == 0)
            {
                "launchd"
            } else {
                "manual"
            };
        }
        #[cfg(not(target_os = "macos"))]
        {
            if self.binary("systemctl").is_some()
                && self
                    .command(
                        &[
                            "systemctl",
                            "--user",
                            "list-unit-files",
                            "hangar-backend.service",
                        ],
                        15,
                    )
                    .is_ok_and(|v| v.code == 0)
            {
                "systemd"
            } else {
                "manual"
            }
        }
    }
    fn frontend_port(&self) -> String {
        let Ok(raw) = std::fs::read_to_string(self.root.join("backend/.env")) else {
            return "0".into();
        };
        for line in crate::transcript::py::splitlines(&raw) {
            let Some(value) = line.strip_prefix("CP_FRONT_PORT=") else { continue };
            let Some(integer) = crate::engines::decimal::integer(crate::transcript::py::strip(value)) else {
                return "0".into();
            };
            let digits = integer.trim_start_matches(['+', '-']);
            // A conversão original recusa inteiros além do limite decimal do Python.
            if digits.len() > 4300 { return "0".into(); }
            let digits = digits.trim_start_matches('0');
            if digits.is_empty() { return "0".into(); }
            return format!("{}{digits}", if integer.starts_with('-') { "-" } else { "" });
        }
        "0".into()
    }
    pub fn restart_service(&self, topology: &str, port: u16) -> Result<(), String> {
        match topology {
            "systemd" => {
                let front = self
                    .command(
                        &[
                            "systemctl",
                            "--user",
                            "list-unit-files",
                            "hangar-frontend.service",
                        ],
                        15,
                    )?
                    .code
                    == 0;
                let mut args = vec!["systemctl", "--user", "restart"];
                if front {
                    args.push("hangar-frontend.service");
                }
                args.push("hangar-backend.service");
                let output = self.command(&args, 120)?;
                if output.code != 0 {
                    return Err(format!(
                        "nao consegui reiniciar o servidor: {}",
                        tail(&output, 12)
                    ));
                }
            }
            "windows" => {
                let helper = quote(&self.root.join("scripts/windows-tasks.ps1"));
                let root = quote(&self.root);
                let front = self.frontend_port();
                let command = format!(". '{helper}'; Restart-HangarTasks '{root}' {port} {front}");
                let output = self.command(
                    &[
                        "powershell",
                        "-NoProfile",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-Command",
                        &command,
                    ],
                    180,
                )?;
                if output.code != 0 {
                    return Err(format!(
                        "nao consegui reiniciar as tarefas: {}",
                        tail(&output, 6)
                    ));
                }
            }
            "launchd" => {
                let output =
                    self.command(&["launchctl", "kickstart", "-k", &launchd_target()], 120)?;
                if output.code != 0 {
                    return Err(format!(
                        "nao consegui reiniciar o servico: {}",
                        tail(&output, 12)
                    ));
                }
            }
            _ => self
                .state()
                .try_write(serde_json::json!({"reiniciar_manual":true}))?,
        }
        Ok(())
    }
    pub fn stop_windows_front(&self) {
        let command = format!(
            ". '{}'; Stop-HangarFrontend '{}'",
            quote(&self.root.join("scripts/windows-tasks.ps1")),
            quote(&self.root)
        );
        let _ = self.command(
            &[
                "powershell",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &command,
            ],
            60,
        );
    }
    pub fn refresh_chromium(&self) {
        if !cfg!(target_os = "linux") {
            return;
        }
        let marker = self.home.join(".hangar/native/chromium-ok");
        let Ok(metadata) = marker.metadata() else {
            return;
        };
        if !metadata.is_file() {
            return;
        }
        if metadata
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() < 86400)
            && std::fs::read_to_string(&marker).is_ok_and(|text| !text.contains("falhou"))
        {
            return;
        }
        let script = self.root.join("scripts/install-chromium.sh");
        if let Err(error) = self.run(&[script.display().to_string()], &self.root, 900) {
            tracing::warn!(code="update_chromium_failed",%error,"atualização do Chromium falhou");
        }
    }
}
fn quote(path: &std::path::Path) -> String {
    path.display()
        .to_string()
        .chars()
        .flat_map(|c| {
            if matches!(c, '\'' | '‘' | '’' | '‚' | '‛') {
                vec![c, c]
            } else {
                vec![c]
            }
        })
        .collect()
}
fn launchd_target() -> String {
    #[cfg(unix)]
    {
        format!("gui/{}/br.dev.hangar.backend", unsafe { libc::getuid() })
    }
    #[cfg(not(unix))]
    {
        String::new()
    }
}
impl Service {
    pub fn server_pid(&self, topology: &str, port: u16) -> Option<u32> {
        let value = match topology {
            "systemd" => self
                .command(
                    &[
                        "systemctl",
                        "--user",
                        "show",
                        "-p",
                        "MainPID",
                        "--value",
                        "hangar-backend.service",
                    ],
                    15,
                )
                .ok()?
                .stdout
                .trim()
                .parse()
                .ok(),
            "launchd" => self
                .command(&["launchctl", "print", &launchd_target()], 15)
                .ok()?
                .stdout
                .lines()
                .find_map(|line| line.trim().strip_prefix("pid = ")?.parse().ok()),
            "windows" => {
                let output=self.command(&["powershell","-NoProfile","-Command",&format!("Get-NetTCPConnection -State Listen -LocalPort {port} -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty OwningProcess")],15).ok()?;
                output.stdout.trim().parse().ok()
            }
            _ => None,
        };
        value.filter(|pid| *pid != 0)
    }
    pub fn up(&self, port: u16, seconds: u64) -> bool {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return false;
        };
        runtime.block_on(async {
            let Ok(client) = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3))
                .build()
            else {
                return false;
            };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
            while std::time::Instant::now() < deadline {
                if client
                    .get(format!("http://127.0.0.1:{port}/"))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
                {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            false
        })
    }
}
impl Service {
    pub fn native_command(&self, windows: bool) -> Vec<String> {
        if windows {
            vec![
                "powershell".into(),
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                self.root
                    .join("scripts/install-native.ps1")
                    .display()
                    .to_string(),
            ]
        } else {
            vec![
                self.root
                    .join("scripts/install-native.sh")
                    .display()
                    .to_string(),
            ]
        }
    }
    fn native_contract(&self, port:u16, source:&dyn super::releases::Source)->Result<Option<(String,String)>,serde_json::Value> {
        let marker=self.home.join(".hangar/native/release.json");
        if !marker.is_file(){return Ok(None);}
        let bytes=std::fs::read(&marker).map_err(|error|native_io_error(&error))?;
        let installed=native_document(&bytes,false)?;
        if !installed.is_object(){return Err(native_error("ValueError"));}
        if crate::orchestration::truthy(&installed["sem_build"]){return Ok(None);}
        let release=self.environment.get("HANGAR_NATIVE_UPDATE_URL").filter(|value|!value.is_empty()).cloned()
            .unwrap_or_else(||format!("{}/native-latest",super::releases::BASE));
        let bytes=source.get_release(&format!("{}/native-latest.json",release.trim_end_matches('/')),std::time::Duration::from_secs(30))
            .map_err(|error|native_error(if error.status.is_some(){"HTTPError"}else{error.kind}))?;
        let manifest=native_document(&bytes,true)?;
        if !manifest.is_object(){return Err(native_error("ValueError"));}
        let required=manifest.get("requires_api").map_or(Some("1".into()),native_positive_integer);
        let token=self.value("CP_AUTH_TOKEN").map_err(|_|native_error("ValueError"))?;
        let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|error|native_io_error(&error))?;
        let api=runtime.block_on(async {
            let client=reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(3)).read_timeout(std::time::Duration::from_secs(3)).build().map_err(|_|native_error("ValueError"))?;
            let response=client.get(format!("http://127.0.0.1:{port}/api/client-api")).bearer_auth(token).send().await.map_err(|error|native_error(if error.is_timeout(){"TimeoutError"}else if error.is_builder(){"ValueError"}else{"URLError"}))?;
            if response.status()==reqwest::StatusCode::NOT_FOUND{return Ok(Some("1".into()));}
            if !response.status().is_success(){return Err(native_error("HTTPError"));}
            let bytes=response.bytes().await.map_err(|error|native_error(if error.is_timeout(){"TimeoutError"}else{"OSError"}))?;
            let contract=native_document(&bytes,true)?;
            Ok(contract.get("api").and_then(native_positive_integer))
        })?;
        match (required,api){(Some(required),Some(api))=>Ok(Some((required,api))),_=>Err(native_error("ValueError"))}
    }
    pub fn update_native(&self, target: &str, port: u16, source: &dyn super::releases::Source) {
        if target!="main"{return;}
        let (required,api)=match self.native_contract(port,source) {
            Ok(Some(versions))=>versions,
            Ok(None)=>return,
            Err(mut event)=>{event["evento"]=serde_json::json!("atualizacao.nativo");event["nivel"]=serde_json::json!("aviso");event["codigo"]=serde_json::json!("contrato_ilegivel");self.native_diagnostic(event);return;}
        };
        if (required.len(),required.as_str())>(api.len(),api.as_str()) {
            self.native_diagnostic(serde_json::json!({"evento":"atualizacao.nativo","nivel":"ok","codigo":"api_insuficiente","requires_api":serde_json::from_str::<serde_json::Value>(&required).expect("inteiro validado"),"api":serde_json::from_str::<serde_json::Value>(&api).expect("inteiro validado")}));
            return;
        }
        let command=self.native_command(cfg!(windows));
        match self.run(&command,&self.root,900) {
            Ok(output) if output.code==0=>{},
            Ok(output)=>self.state().warn(&format!("app nativo nao atualizou: {}",super::failure_reason(&output))),
            Err(error)=>self.state().warn(&format!("app nativo nao atualizou: {error}")),
        }
    }
    fn native_diagnostic(&self,event:serde_json::Value) {
        let mut environment=crate::accounts::environment::AccountEnvironment::from_map(self.environment.clone());
        environment.home=self.home.clone();
        crate::diag::record_blocking(crate::diag::diary_dir_in(&environment).as_deref(),&[event],"atualização do app nativo");
    }
}

fn native_document(bytes:&[u8],binary:bool)->Result<crate::orchestration::codec::ValueGuard,serde_json::Value> {
    let decoded;
    let text=if binary {
        decoded=crate::orchestration::body_encoding::decode(bytes).map_err(|_|native_error("UnicodeDecodeError"))?;
        decoded.text.as_ref()
    }else{std::str::from_utf8(bytes).map_err(|_|native_error("UnicodeDecodeError"))?};
    if let Some(value)=crate::orchestration::codec::loads(text){return Ok(crate::orchestration::codec::ValueGuard(value));}
    // O marcador já foi decodificado como texto; o prefixo impede uma segunda detecção de encoding.
    let text_input=(!binary).then(||format!("    {text}"));
    let diagnostic=text_input.as_ref().map_or(bytes,|text|text.as_bytes());
    let kind=match crate::orchestration::body::parse(diagnostic) {
        Ok(value)=>{crate::orchestration::codec::discard(value);"JSONDecodeError"},
        Err((status,value))=>{crate::orchestration::codec::discard(value);if status==axum::http::StatusCode::BAD_REQUEST{"ValueError"}else{"JSONDecodeError"}},
    };
    Err(native_error(kind))
}
fn native_positive_integer(value:&serde_json::Value)->Option<String> {
    let serde_json::Value::Number(number)=value else{return None;};
    let text=number.to_string();
    (text!="0"&&text.bytes().all(|byte|byte.is_ascii_digit())).then_some(text)
}
fn native_error(kind:&str)->serde_json::Value {serde_json::json!({"erro_tipo":kind})}
fn native_io_error(error:&std::io::Error)->serde_json::Value {
    let kind=match error.kind(){std::io::ErrorKind::NotFound=>"FileNotFoundError",std::io::ErrorKind::PermissionDenied=>"PermissionError",std::io::ErrorKind::TimedOut=>"TimeoutError",_=>"OSError"};
    let mut fields=native_error(kind);
    if let Some(code)=error.raw_os_error(){
        #[cfg(unix)] {fields["errno"]=serde_json::json!(code);}
        #[cfg(windows)] {fields["winerror"]=serde_json::json!(code);fields["errno"]=serde_json::json!(match error.kind(){std::io::ErrorKind::NotFound=>2,std::io::ErrorKind::PermissionDenied=>13,_=>22});}
    }
    fields
}

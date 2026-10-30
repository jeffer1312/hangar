//! Lança o motor fora do serviço que ele vai reiniciar.
use super::{Service, releases};
use crate::update::executor::{Detached, Spawner};
use serde_json::{Value, json};
impl Service {
    pub fn start(
        &self,
        port: u16,
        expected: Option<&str>,
        restart: bool,
        spawner: &dyn Spawner,
    ) -> Result<Value, String> {
        self.start_with_owner_writer(port,expected,restart,spawner,&crate::runtime::queue::atomic_write)
    }
    pub fn start_with_owner_writer(
        &self,port:u16,expected:Option<&str>,restart:bool,spawner:&dyn Spawner,
        publish_owner:&dyn Fn(&std::path::Path,&[u8])->std::io::Result<()>,
    )->Result<Value,String>{
        let topology = self.topology();
        if restart && topology == "manual" {
            return Ok(json!({"ok":false,"erro":"topologia","topologia":topology}));
        }
        let Some(turn) = self.take_turn().map_err(|e| e.to_string())? else {
            return Ok(json!({"ok":false,"erro":"ja_rodando"}));
        };
        if let Some(branch)=expected && self.target()? != branch {
            return Ok(json!({"ok":false,"erro":"canal_mudou"}));
        }
        let request = if restart {
            Some(releases::nonce().map_err(|e| e.to_string())?)
        } else {
            None
        };
        self.state().try_write(if restart{json!({"fase":"rodando","ok":null,"erro":null,"pid":null,"etapa":null,"passo":0,"total":2,"texto":"Começando o reinício","commit_de":"","commit_para":null,"backend_pid":null,"reinicio_erro":null,"avisos":[],"waiting_for_server_build":false,"request_id":request})}else{json!({"fase":"rodando","ok":null,"erro":null,"pid":null,"passo":0,"total":6,"texto":"Começando"})})?;
        let authorization = releases::nonce().map_err(|e| e.to_string())?;
        let args = vec![
            "checkout-update".into(),
            self.root.display().to_string(),
            self.home.display().to_string(),
            self.state_dir.parent().unwrap().display().to_string(),
            port.to_string(),
            std::process::id().to_string(),
            if restart {
                "restart".into()
            } else {
                "update".into()
            },
            request.clone().unwrap_or_default(),
            authorization.clone(),
        ];
        let pid = match spawner.spawn(&args) {
            Ok(pid) => pid,
            Err(e) => {
                self.state().write(json!({"fase":"pronto","ok":false,"erro":if restart{"não consegui lançar o processo do reinício"}else{"não consegui lançar o processo da atualização"}}));
                return Err(e.to_string());
            }
        };
        let ready = self.state_dir.join(format!(".launch-{authorization}.ready"));
        let publication = publish_owner(&self.state_dir.join("rodando.lock"), pid.to_string().as_bytes())
            .and_then(|_| publish_authorization(&ready, pid));
        if let Err(error) = publication {
            let error = format!("update_lock_unavailable: {error}");
            let _ = std::fs::remove_file(&ready);
            self.state().write(json!({"fase":"pronto","ok":false,"erro":error}));
            drop(turn);
            return Err(error);
        }
        turn.transferred();
        Ok(if let Some(request) = request {
            json!({"ok":true,"pid":pid,"request_id":request})
        } else {
            json!({"ok":true,"pid":pid})
        })
    }
    /// O filho não executa nada antes da confirmação da tentativa, mesmo se o pai cair.
    pub fn await_launch_authorization(&self, authorization: &str) -> Result<(), String> {
        if authorization.is_empty() || !authorization.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("update_lock_unavailable: autorização de lançamento inválida".into());
        }
        let ready = self.state_dir.join(format!(".launch-{authorization}.ready"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match self.transition_guard().map_err(|error| format!("update_lock_unavailable: {error}"))? {
                Some(_guard) => {
                    let pid = std::process::id().to_string();
                    let allowed = std::fs::read_to_string(&ready).is_ok_and(|value| value == pid)
                        && std::fs::read_to_string(self.state_dir.join("rodando.lock")).is_ok_and(|value| value == pid);
                    match std::fs::remove_file(&ready) {
                        Ok(()) => {},
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                        Err(error) => return Err(format!("update_lock_unavailable: {error}")),
                    }
                    return if allowed { Ok(()) } else { Err("update_lock_unavailable: lançamento sem posse autorizada".into()) };
                },
                None if std::time::Instant::now() >= deadline => return Err("update_lock_unavailable: prazo da autorização de lançamento esgotado".into()),
                None => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
    }
    pub fn start_detached(
        &self,
        port: u16,
        expected: Option<&str>,
        restart: bool,
    ) -> Result<Value, String> {
        let spawner = Detached::current(self.topology() == "systemd", self.state_dir.clone())
            .map_err(|e| e.to_string())?;
        self.start(port, expected, restart, &spawner)
    }
}
pub fn cli(args: &[String]) -> std::process::ExitCode {
    if args.len() != 8 {
        return std::process::ExitCode::from(2);
    }
    let Ok(port) = args[3].parse::<u16>() else {
        return std::process::ExitCode::from(2);
    };
    let mut environment = std::env::vars().collect::<std::collections::BTreeMap<_, _>>();
    environment.insert("HOME".into(), args[1].clone());
    environment.insert("USERPROFILE".into(), args[1].clone());
    environment.insert("HANGAR_UPDATE_BACKEND_PID".into(), args[4].clone());
    let service = Service::new(
        args[1].clone().into(),
        args[0].clone().into(),
        args[2].clone().into(),
        environment,
    );
    if let Err(error) = service.await_launch_authorization(&args[7]) {
        eprintln!("{error}");
        return std::process::ExitCode::from(1);
    }
    let platform = crate::update::platform();
    let source = releases::HttpSource;
    let result = if args[5] == "restart" {
        service.execute_restart(port, &args[6], &source)
    } else {
        service.execute(port, &source, Some(&platform))
    };
    std::process::ExitCode::from(if result.get("ok") == Some(&Value::Bool(true)) {
        0
    } else {
        1
    })
}

fn publish_authorization(ready: &std::path::Path, pid: u32) -> std::io::Result<()> {
    use std::io::Write;
    let pending = ready.with_extension("pending");
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&pending)?;
        write!(file,"{pid}")?;
        file.sync_all()?;
        drop(file);
        // O rename é o ponto de autorização; nenhuma falha posterior o anuncia como recusado.
        std::fs::rename(&pending,ready)
    })();
    if result.is_err() { let _ = std::fs::remove_file(pending); }
    result
}

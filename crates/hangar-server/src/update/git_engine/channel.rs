//! Canal vivo do checkout, publicado no mesmo arquivo que o próximo processo vai ler.
use super::*;
use crate::machine_peers::{Error, store};
const PUBLICATION: &str = "# hangar-update-channel-publication=";
fn error(status: u16, code: &'static str, msg: &str) -> Error {
    Error {
        status,
        code,
        msg: msg.into(),
    }
}
fn busy() -> Error {
    error(
        409,
        "update_channel_busy",
        "Aguarde a atualização terminar antes de mudar o canal.",
    )
}
impl Service {
    pub fn channel_value(&self, key: &str) -> Result<String, String> {
        crate::machine_peers::live_config::update_channel(&self.home,Some(&self.root.join("backend")),&self.settings_environment(),key)
    }
    fn channel_git(&self,args:&[&str])->Result<super::ProcessOutput,String> {
        let mut command=vec!["git".to_owned()];command.extend(args.iter().map(|value|(*value).to_owned()));
        crate::update::process::run_capture_with_env(&command,&self.root,std::time::Duration::from_secs(20),&self.environment).map_err(|error|error.to_string())
    }
    fn checkout_branch(&self) -> Result<String, Error> {
        let output = self
            .channel_git(&["rev-parse", "--abbrev-ref", "HEAD"])
            .map_err(|_| {
                error(
                    503,
                    "update_channel_checkout",
                    "Não foi possível ler a branch instalada.",
                )
            })?;
        if output.code != 0 || output.stdout.contains('\u{fffd}') {
            return Err(error(
                503,
                "update_channel_checkout",
                "Não foi possível ler a branch instalada.",
            ));
        }
        Ok(output.stdout.trim().into())
    }
    fn channel_snapshot(&self, checkout: &str) -> Result<Value, Error> {
        let branch = self.channel_value("CP_UPDATE_BRANCH").map_err(|_| {
            error(
                503,
                "update_channel_checkout",
                "Não foi possível ler a branch instalada.",
            )
        })?;
        let last = self.channel_value("CP_UPDATE_LAST_BRANCH").map_err(|_| {
            error(
                503,
                "update_channel_checkout",
                "Não foi possível ler a branch instalada.",
            )
        })?;
        Ok(
            json!({"branch":branch,"checkout_branch":checkout,"last_branch":if last.is_empty(){branch}else{last}}),
        )
    }
    pub fn read_channel(&self) -> Result<Value, Error> {
        self.channel_snapshot(&self.checkout_branch()?)
    }
    pub fn write_channel(&self, branch: &str) -> Result<Value, Error> {
        let branch = branch.trim();
        if !valid_branch(branch) {
            return Err(error(
                400,
                "update_channel_invalid",
                "Nome de branch inválido.",
            ));
        }
        std::fs::create_dir_all(&self.state_dir).map_err(|_| {
            error(
                500,
                "update_channel_write",
                "Não foi possível gravar o canal. O canal anterior continua valendo.",
            )
        })?;
        let _serialization = store::lock(&self.state_dir.join("channel")).map_err(|_| busy())?;
        if self.screen_state().get("fase").and_then(Value::as_str) == Some("rodando") {
            return Err(busy());
        }
        let checkout = self.checkout_branch()?;
        if !branch.is_empty() {
            let reference = format!("refs/heads/{branch}");
            let output = self
                .channel_git(&["ls-remote", "--exit-code", "--heads", "origin", &reference])
                .map_err(|_| {
                    error(
                        503,
                        "update_channel_origin",
                        "Não foi possível conferir a branch no origin. Tente novamente.",
                    )
                })?;
            if output.code == 2 {
                return Err(error(
                    400,
                    "update_channel_missing",
                    "A branch não existe no origin.",
                ));
            }
            if output.code != 0 || output.stdout.contains('\u{fffd}') {
                return Err(error(
                    503,
                    "update_channel_origin",
                    "Não foi possível conferir a branch no origin. Tente novamente.",
                ));
            }
            if !output
                .stdout
                .lines()
                .any(|line| line.split('\t').next_back() == Some(&reference))
            {
                return Err(error(
                    400,
                    "update_channel_missing",
                    "A branch não existe no origin.",
                ));
            }
        }
        let _turn = self.take_turn().map_err(|cause| error(500,"update_lock_unavailable",&cause.to_string()))?.ok_or_else(busy)?;
        let old = self.channel_snapshot(&checkout)?;
        let last = if branch.is_empty() {
            old["last_branch"].as_str().unwrap_or("")
        } else {
            branch
        };
        self.publish_channel(branch, last).map_err(|_| {
            error(
                500,
                "update_channel_write",
                "Não foi possível gravar o canal. O canal anterior continua valendo.",
            )
        })?;
        self.channel_snapshot(&checkout)
    }
    pub fn publish_channel(&self, branch: &str, last: &str) -> Result<(), String> {
        let path = crate::user_data::path(&self.home, Some(&self.root.join("backend")), ".env")
            .map_err(|e| e.to_string())?
            .ok_or(".env indisponível")?;
        let _lock = store::lock(&path).map_err(|e| e.msg)?;
        let raw = crate::user_data::read_env_at(&path)?;
        let raw = raw
            .split_inclusive('\n')
            .filter(|line| !line.starts_with(PUBLICATION))
            .collect::<String>();
        let values = [
            ("CP_UPDATE_BRANCH", branch),
            ("CP_UPDATE_LAST_BRANCH", last),
        ];
        let mut output = crate::machine_peers::dotenv::replace(&raw, &values);
        let settings=self.settings_environment();
        let inherited: serde_json::Map<String,Value>=values.iter().map(|(key,_)|Ok(((*key).to_owned(),settings.value("",key)?)))
            .collect::<Result<Vec<_>,String>>()?.into_iter().filter_map(|(key,value)|value.map(|value|(key,json!(value)))).collect();
        if !inherited.is_empty() {
            let nonce = super::releases::nonce().map_err(|e| e.to_string())?;
            let published: serde_json::Map<String, Value> = values
                .iter()
                .map(|(key, value)| ((*key).into(), json!(value)))
                .collect();
            crate::runtime::queue::atomic_write(
                &path.with_extension("env.update-channel.json"),
                &serde_json::to_vec(
                    &json!({"nonce":nonce,"inherited":inherited,"published":published}),
                )
                .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let newline = if raw.contains("\r\n") { "\r\n" } else { "\n" };
            if !output.ends_with('\n') {
                output.push_str(newline);
            }
            output.push_str(&format!("{PUBLICATION}{nonce}{newline}"));
        }
        store::clean_temporary(&path).map_err(|e| e.msg)?;
        crate::runtime::queue::atomic_write(&path, output.as_bytes()).map_err(|e| e.to_string())
    }
    pub(super) fn transition_guard(&self) -> std::io::Result<Option<std::fs::File>> {
        Self::transition_guard_at(&self.state_dir)
    }
    pub(crate) fn transition_guard_at(state_dir: &std::path::Path) -> std::io::Result<Option<std::fs::File>> {
        std::fs::create_dir_all(state_dir)?;
        // A identidade da guarda permanece: removê-la permite travar dois arquivos diferentes.
        let file=std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .open(state_dir.join("rodando.guard"))?;
        match file.try_lock() {
            Ok(())=>Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock)=>Ok(None),
            Err(std::fs::TryLockError::Error(error))=>Err(error),
        }
    }
    pub fn take_turn(&self)->std::io::Result<Option<Turn>> {
        self.take_turn_with_publisher(&|source,target|std::fs::hard_link(source,target))
    }
    pub fn take_turn_with_publisher(&self,publish:&dyn Fn(&std::path::Path,&std::path::Path)->std::io::Result<()>)->std::io::Result<Option<Turn>> {
        let result=(|| {
            use std::io::Write;
            let Some(guard)=self.transition_guard()? else{return Ok(None)};
            let path=self.state_dir.join("rodando.lock");
            match std::fs::read_to_string(&path) {
                Ok(raw)=>{
                    let owner=raw.trim().parse::<u32>().unwrap_or(0);
                    if crate::terminal_process::pid_alive(owner)? {return Ok(None);}
                    std::fs::remove_file(&path)?;
                },
                Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},
                Err(error) if error.kind()==std::io::ErrorKind::InvalidData=>{std::fs::remove_file(&path)?;},
                Err(error)=>return Err(error),
            }
            let temporary=self.state_dir.join(format!(".rodando.{}.tmp",super::releases::nonce()?));
            let published=(|| {
                let mut file=std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
                write!(file,"{}",std::process::id())?;file.sync_all()?;drop(file);
                publish(&temporary,&path)
            })();
            let cleanup=std::fs::remove_file(&temporary);
            published?;cleanup?;
            Ok(Some(Turn{path,guard:Some(guard),cleanup:true}))
        })();
        result.map_err(lock_error)
    }
    pub fn release_turn(&self)->std::io::Result<()> {
        let result=(|| {
            let Some(_guard)=self.transition_guard()? else{return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock,"transição ocupada"))};
            let path=self.state_dir.join("rodando.lock");
            match std::fs::read_to_string(&path) {
                Ok(raw) if raw.trim()==std::process::id().to_string()=>std::fs::remove_file(path),
                Ok(_)=>Ok(()),
                Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Ok(()),
                Err(error)=>Err(error),
            }
        })();result.map_err(lock_error)
    }
}
fn lock_error(error:std::io::Error)->std::io::Error {
    std::io::Error::new(error.kind(),format!("update_lock_unavailable: {error}"))
}
pub struct Turn {path:PathBuf,guard:Option<std::fs::File>,cleanup:bool}
impl Turn {
    pub fn transferred(mut self) {self.cleanup=false;drop(self.guard.take());}
}
impl Drop for Turn {
    fn drop(&mut self) {
        if self.cleanup {
            if let Err(error)=std::fs::remove_file(&self.path) {
                if error.kind()!=std::io::ErrorKind::NotFound {tracing::warn!(code="update_lock_unavailable",%error,"trava da atualização não removida");}
            }
        }
    }
}

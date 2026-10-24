//! Reabertura conserva a receita do processo vivo, não os padrões atuais da conta.
use crate::list::procs::{ProcessRecipe,ProcessView};
use super::protocol::RuntimeError;
use std::{ffi::{OsStr,OsString},io,path::{Path,PathBuf},time::Duration};

pub const RELAUNCH_FLAG:&str="--session-relaunch";
/// Comando montado, sem fingir identidade de um processo que ainda não nasceu.
#[derive(Clone)]
pub struct LaunchRecipe {pub executable:PathBuf,pub cwd:PathBuf,pub argv:Vec<OsString>,pub environ:Vec<OsString>}
impl From<&ProcessRecipe> for LaunchRecipe {
    fn from(recipe:&ProcessRecipe)->Self {Self {executable:recipe.executable.clone(),cwd:recipe.cwd.clone(),argv:recipe.argv.clone(),environ:recipe.environ.clone()}}
}

fn failure(code:&str)->RuntimeError {RuntimeError::new(code,"não há prova suficiente para reabrir a sessão")}
fn variable<'a>(recipe:&'a ProcessRecipe,key:&str)->Option<&'a OsStr> {
    let prefix=format!("{key}=");
    recipe.environ.iter().find_map(|entry| {
        let raw=entry.as_encoded_bytes();
        let head=raw.get(..prefix.len())?;
        // No Windows o nome da variável não tem caixa (o Python a repassa em maiúsculas).
        let same=if cfg!(windows) {head.eq_ignore_ascii_case(prefix.as_bytes())} else {head==prefix.as_bytes()};
        if !same {return None;}
        let value=&raw[prefix.len()..];
        // SAFETY: `=` ASCII delimita uma fronteira do encoding de OsStr.
        Some(unsafe {OsStr::from_encoded_bytes_unchecked(value)})
    })
}
pub(crate) fn flag(argv:&[OsString],name:&str)->Result<Option<(usize,usize,OsString)>,RuntimeError> {
    let mut found=None;
    for (i,arg) in argv.iter().enumerate().skip(1) {
        if arg=="--" { break; }
        let value=if arg==name {
            Some((i,2,argv.get(i+1).filter(|v|!v.is_empty()).ok_or_else(||failure("relaunch_flag"))?.clone()))
        } else {
            arg.to_str().and_then(|s|s.strip_prefix(&format!("{name}="))).map(|v|(i,1,OsString::from(v)))
        };
        if let Some(value)=value {
            if found.replace(value).is_some() {return Err(failure("relaunch_duplicate_flag"));}
        }
    }
    Ok(found)
}

/// A receita original permanece para reconferir identidade; cópias privadas são só do próximo lançamento.
pub struct Recipe {
    chain:Vec<ProcessRecipe>,
    agent:ProcessRecipe,
    envelope:Option<ProcessRecipe>,
    settings:Vec<(usize,tempfile::NamedTempFile)>,
    owner:std::sync::Mutex<Option<(i64,f64)>>,
}
impl Drop for Recipe {
    fn drop(&mut self) {
        let owner=self.owner.lock().ok().and_then(|owner|*owner);
        let still_used=owner.is_some_and(|(pid,birth)|match crate::list::procs::SystemProcs::default().recipe(pid) {
            Ok(current)=>current.birth==birth,
            Err(error)=>error.kind()!=io::ErrorKind::NotFound,
        });
        if still_used && !self.settings.is_empty() {
            let mut paths=Vec::new();
            for (_,file) in self.settings.drain(..) {
                match file.keep() {Ok((file,path))=>{drop(file);paths.push(path);},Err(_)=>tracing::warn!(code="relaunch_settings_retention","não conservei um arquivo privado ainda usado pelo processo")}
            }
            if let (Some((pid,birth)),Ok(runtime))=(owner,tokio::runtime::Handle::try_current()) {
                runtime.spawn(async move {
                    let procs=crate::list::procs::SystemProcs::default();
                    loop {
                        match super::lifecycle::process_absent(&procs,pid,birth) {
                            Ok(true)=>break,
                            Ok(false)=>{},
                            Err(error)=>if crate::warn_limit::allow(None,&error.code) {tracing::warn!(code=%error.code,"não consegui conferir a saída do dono dos settings privados");},
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    for path in paths {if let Err(error)=std::fs::remove_file(path) && error.kind()!=io::ErrorKind::NotFound {tracing::warn!(code="relaunch_settings_cleanup",kind=?error.kind(),"arquivo privado próprio não foi removido");}}
                });
            } else {tracing::warn!(code="relaunch_settings_process_alive","settings privados conservados sem vigia porque o runtime terminou");}
        }
    }
}
impl Recipe {
    pub fn root_identity(&self)->Result<(i64,f64),RuntimeError> {self.chain.first().map(|process|(process.pid,process.birth)).ok_or_else(||failure("relaunch_process_binding"))}
    pub fn agent_identity(&self)->(i64,f64) {(self.agent.pid,self.agent.birth)}
    pub fn pin_settings(&self,pid:i64,birth:f64)->Result<(),RuntimeError> {
        *self.owner.lock().map_err(|_|failure("relaunch_settings_owner"))?=Some((pid,birth));Ok(())
    }
    pub fn argument_conversation(process:&ProcessRecipe)->Result<String,RuntimeError> {
        let sid=flag(&process.argv,"--session-id")?;let resume=flag(&process.argv,"--resume")?;
        if sid.is_some() && resume.is_some() {return Err(failure("relaunch_conversation"));}
        sid.or(resume).and_then(|(_,_,sid)|sid.into_string().ok()).filter(|sid|!sid.is_empty()).ok_or_else(||failure("relaunch_conversation"))
    }
    pub fn capture(procs:&dyn ProcessView,pane_pid:i64,agent_pid:i64,birth:f64,conversation:&str,read_only:bool)->Result<Self,RuntimeError> {
        let children=procs.children(Duration::ZERO).map_err(|_|failure("relaunch_process_read"))?;
        let mut paths=vec![vec![pane_pid]];
        let mut chain=None;
        while let Some(path)=paths.pop() {
            if path.len()>64 {return Err(failure("relaunch_process_tree"));}
            let last=*path.last().unwrap();
            if last==agent_pid {chain=Some(path);break;}
            for child in children.get(&last).into_iter().flatten() {
                if path.contains(child) {return Err(failure("relaunch_process_tree"));}
                let mut next=path.clone();next.push(*child);paths.push(next);
            }
        }
        let chain=chain.ok_or_else(||failure("relaunch_process_binding"))?;
        let mut snapshots=Vec::new();
        for pid in chain {
            snapshots.push(procs.recipe(pid).map_err(|_|failure("relaunch_process_read"))?);
        }
        let agent=snapshots.last().cloned().ok_or_else(||failure("relaunch_process_binding"))?;
        if !crate::terminal_facts::same_birth(agent.birth,birth) || agent.argv.is_empty() || agent.environ.is_empty() {return Err(failure("relaunch_process_binding"));}
        let executable=Path::new(&agent.argv[0]).file_stem().and_then(OsStr::to_str).unwrap_or("");
        let native_version=agent.executable.parent().is_some_and(|p|p.file_name()==Some(OsStr::new("versions"))
            && p.parent().is_some_and(|p|p.file_name()==Some(OsStr::new("claude"))))
            && agent.executable.file_name().and_then(OsStr::to_str).is_some_and(|name|!name.is_empty() && name.bytes().all(|b|b.is_ascii_digit() || b==b'.'));
        let claude=executable.eq_ignore_ascii_case("claude") || native_version || (executable.eq_ignore_ascii_case("node") && agent.argv.get(1).is_some_and(|a|a.to_str().is_some_and(|s|s.ends_with("/claude-code/cli.js") || s.ends_with("\\claude-code\\cli.js"))));
        if !claude {return Err(failure("relaunch_not_claude"));}
        let sid=flag(&agent.argv,"--session-id")?;
        let resume=flag(&agent.argv,"--resume")?;
        if sid.is_some() && resume.is_some() || sid.as_ref().or(resume.as_ref()).is_none_or(|(_,_,sid)|sid!=conversation) {return Err(failure("relaunch_conversation"));}
        let protected=read_only || variable(&agent,"HANGAR_ORQ_READ_ONLY")==Some(OsStr::new("1"));
        let envelope=snapshots.iter().rev().skip(1).find(|p|p.executable.file_stem()==Some(OsStr::new("bwrap"))).cloned();
        if protected {
            let bwrap=envelope.as_ref().ok_or_else(||failure("relaunch_read_only"))?;
            let cut=bwrap.argv.iter().position(|a|a=="--").ok_or_else(||failure("relaunch_read_only"))?;
            let prefix=&bwrap.argv[..cut];
            if !prefix.windows(3).any(|w|w==["--bind","/","/"].map(OsString::from))
                || !prefix.windows(3).any(|w|w==["--dev-bind","/dev","/dev"].map(OsString::from))
                || !prefix.windows(3).any(|w|w==["--setenv","HANGAR_ORQ_READ_ONLY","1"].map(OsString::from))
                || !prefix.windows(3).any(|w|w[0]=="--ro-bind" && Path::new(&w[1])==agent.cwd && w[1]==w[2]) {
                return Err(failure("relaunch_read_only"));
            }
        } else if envelope.is_some() {return Err(failure("relaunch_read_only"));}
        Ok(Self {chain:snapshots,agent,envelope,settings:vec![],owner:std::sync::Mutex::new(None)})
    }
    fn settings_path(&self,value:&OsStr,windows:bool)->Result<PathBuf,RuntimeError> {
        let raw=value.as_encoded_bytes();
        if windows {
            let drive=raw.get(1)==Some(&b':') && raw.first().is_some_and(|b|b.is_ascii_alphabetic());
            let separator=|byte:Option<&u8>|byte.is_some_and(|b|matches!(*b,b'\\'|b'/'));
            if drive && !separator(raw.get(2)) || separator(raw.first()) && !separator(raw.get(1)) {return Err(failure("relaunch_settings_drive_relative"));}
            if drive || raw.starts_with(b"\\\\") {return Ok(PathBuf::from(value));}
        }
        let path=Path::new(value);Ok(if path.is_absolute() {path.to_owned()} else {self.agent.cwd.join(path)})
    }
    /// Copia antes do kill do pai do hangar-engine; o original nunca é removido por este código.
    pub fn preserve_settings(&mut self,dir:&Path)->Result<(),RuntimeError> {
        if let Some((at,width,value))=flag(&self.agent.argv,"--settings")? {
            if value.to_str().is_some_and(|s|s.trim_start().starts_with('{')) {return Ok(());}
            let path=self.settings_path(&value,cfg!(windows))?;
            let bytes=std::fs::read(path).map_err(|_|failure("relaunch_settings_read"))?;
            let mut copy=tempfile::Builder::new().prefix("claude-settings-").suffix(".json").tempfile_in(dir).map_err(|_|failure("relaunch_settings_copy"))?;
            #[cfg(windows)] restrict_file(copy.path()).map_err(|_|failure("relaunch_settings_private"))?;
            use std::io::Write;
            copy.write_all(&bytes).and_then(|_|copy.as_file().sync_all()).map_err(|_|failure("relaunch_settings_copy"))?;
            self.settings.push((at+usize::from(width==2),copy));
        }
        Ok(())
    }
    pub fn verify(&self,procs:&dyn ProcessView)->Result<(),RuntimeError> {
        for (at,copy) in &self.settings {
            let value=&self.agent.argv[*at];
            let value=if let Some(raw)=value.to_str().and_then(|s|s.strip_prefix("--settings=")) {OsStr::new(raw)} else {value.as_os_str()};
            let path=self.settings_path(value,cfg!(windows))?;
            if std::fs::read(path).map_err(|_|failure("relaunch_settings_read"))?!=std::fs::read(copy.path()).map_err(|_|failure("relaunch_settings_copy"))? {return Err(failure("relaunch_settings_changed"));}
        }
        for expected in &self.chain {
            let actual=procs.recipe(expected.pid).map_err(|_|failure("relaunch_process_read"))?;
            if actual!=*expected {return Err(failure("relaunch_process_changed"));}
        }
        Ok(())
    }
    pub fn config_dir(&self)->Result<PathBuf,RuntimeError> {
        if let Some(path)=variable(&self.agent,"CLAUDE_CONFIG_DIR").filter(|p|!p.is_empty()) {return Ok(PathBuf::from(path));}
        let home=variable(&self.agent,if cfg!(windows) {"USERPROFILE"} else {"HOME"}).filter(|p|!p.is_empty()).ok_or_else(||failure("relaunch_config_missing"))?;
        Ok(PathBuf::from(home).join(".claude"))
    }
    pub fn bypass_enabled(&self)->Result<bool,RuntimeError> {
        Ok(self.agent.argv.iter().any(|a|a=="--allow-dangerously-skip-permissions" || a=="--dangerously-skip-permissions")
            || flag(&self.agent.argv,"--permission-mode")?.is_some_and(|(_,_,v)|v=="bypassPermissions"))
    }
    pub fn policy_preflight(&self,config:&Path)->Result<(),RuntimeError> {
        let mut paths=vec![config.join("settings.json"),self.agent.cwd.join(".claude/settings.json"),self.agent.cwd.join(".claude/settings.local.json")];
        #[cfg(target_os="linux")] paths.push(PathBuf::from("/etc/claude-code/managed-settings.json"));
        #[cfg(target_os="macos")] paths.push(PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"));
        #[cfg(windows)] {
            let program_files=variable(&self.agent,"ProgramFiles").ok_or_else(||failure("relaunch_policy_source"))?;
            paths.push(PathBuf::from(program_files).join("ClaudeCode/managed-settings.json"));
        }
        for path in paths {
            let bytes=match std::fs::read(&path) {Ok(bytes)=>bytes,Err(error) if error.kind()==io::ErrorKind::NotFound=>continue,Err(_)=>return Err(failure("relaunch_policy_read"))};
            let value:serde_json::Value=serde_json::from_slice(&bytes).map_err(|_|failure("relaunch_policy_read"))?;
            if value["permissions"]["disableBypassPermissionsMode"]=="disable" {return Err(failure("relaunch_policy_forbids_bypass"));}
        }
        if let Some((_,_,settings))=flag(&self.agent.argv,"--settings")? {
            let bytes=if settings.to_str().is_some_and(|s|s.trim_start().starts_with('{')) {settings.to_str().unwrap().as_bytes().to_vec()}
                else {std::fs::read(self.settings_path(&settings,cfg!(windows))?).map_err(|_|failure("relaunch_policy_read"))?};
            let value:serde_json::Value=serde_json::from_slice(&bytes).map_err(|_|failure("relaunch_policy_read"))?;
            if value["permissions"]["disableBypassPermissionsMode"]=="disable" {return Err(failure("relaunch_policy_forbids_bypass"));}
        }
        Ok(())
    }
    pub fn apply_choices(&self,launch:&mut ProcessRecipe,model:Option<&str>,effort:Option<&str>)->Result<(),RuntimeError> {
        let start=if self.envelope.is_some() {launch.argv.iter().position(|a|a=="--").ok_or_else(||failure("relaunch_read_only"))?+1} else {0};
        for (name,value) in [("--model",model),("--effort",effort)] {
            let Some(value)=value.filter(|v|!v.is_empty()) else {continue};
            if value.as_bytes().contains(&0) || name=="--effort" && !["low","medium","high","xhigh","max"].contains(&value) {return Err(failure("relaunch_choice_unsupported"));}
            let found=flag(&launch.argv[start..],name)?;
            if let Some((at,width,_))=found {launch.argv.splice(start+at..start+at+width,[OsString::from(name),OsString::from(value)]);}
            else {
                let end=launch.argv[start..].iter().position(|a|a=="--").map_or(launch.argv.len(),|at|start+at);
                launch.argv.splice(end..end,[OsString::from(name),OsString::from(value)]);
            }
        }
        Ok(())
    }
    pub fn launch(&self,conversation:&str,mode:&str)->Result<ProcessRecipe,RuntimeError> {
        let mut recipe=self.agent.clone();
        for (at,copy) in &self.settings {
            if recipe.argv[*at].to_str().is_some_and(|s|s.starts_with("--settings=")) {
                let mut value=OsString::from("--settings=");value.push(copy.path());recipe.argv[*at]=value;
            } else {recipe.argv[*at]=copy.path().as_os_str().to_owned();}
        }
        let mut replace=Vec::new();
        for name in ["--session-id","--resume","--permission-mode"] {
            if let Some((at,width,_))=flag(&recipe.argv,name)? {replace.push((at,width,name));}
        }
        replace.sort_by_key(|(at,_,_)|std::cmp::Reverse(*at));
        let mut resumed=false;let mut mode_set=false;
        for (at,width,name) in replace {
            let values=if name=="--permission-mode" {mode_set=true;vec![OsString::from(name),OsString::from(mode)]}
                else {resumed=true;vec![OsString::from("--resume"),OsString::from(conversation)]};
            recipe.argv.splice(at..at+width,values);
        }
        let cut=recipe.argv.iter().position(|a|a=="--").unwrap_or(recipe.argv.len());
        let mut extra=Vec::new();
        if !resumed {extra.extend([OsString::from("--resume"),conversation.into()]);}
        if !mode_set {extra.extend([OsString::from("--permission-mode"),mode.into()]);}
        if mode=="bypassPermissions" && !recipe.argv.iter().take(cut).any(|a|a=="--allow-dangerously-skip-permissions") {
            extra.push("--allow-dangerously-skip-permissions".into());
        }
        recipe.argv.splice(cut..cut,extra);
        if mode!="bypassPermissions" {
            recipe.argv.retain(|arg|arg!="--dangerously-skip-permissions");
            if self.agent.argv.iter().any(|arg|arg=="--dangerously-skip-permissions") && !recipe.argv.iter().any(|arg|arg=="--allow-dangerously-skip-permissions") {
                recipe.argv.push("--allow-dangerously-skip-permissions".into());
            }
        }
        if let Some(envelope)=&self.envelope {
            let cut=envelope.argv.iter().position(|a|a=="--").ok_or_else(||failure("relaunch_read_only"))?;
            let mut argv=envelope.argv[..=cut].to_vec();
            argv.push(recipe.executable.as_os_str().to_owned());argv.extend(recipe.argv.into_iter().skip(1));
            recipe.executable=envelope.executable.clone();recipe.argv=argv;
        }
        Ok(recipe)
    }
}

#[cfg(unix)]
type Native=Vec<u8>;
#[cfg(windows)]
type Native=Vec<u16>;
#[cfg(unix)]
fn native(value:&OsStr)->Native {use std::os::unix::ffi::OsStrExt;value.as_bytes().to_vec()}
#[cfg(windows)]
fn native(value:&OsStr)->Native {use std::os::windows::ffi::OsStrExt;value.encode_wide().collect()}
#[cfg(unix)]
fn os(value:Native)->OsString {use std::os::unix::ffi::OsStringExt;OsString::from_vec(value)}
#[cfg(windows)]
fn os(value:Native)->OsString {use std::os::windows::ffi::OsStringExt;OsString::from_wide(&value)}

#[derive(serde::Serialize,serde::Deserialize)]
struct LaunchFrame {executable:Native,cwd:Native,argv:Vec<Native>,environ:Vec<Native>}
impl LaunchFrame {
    fn from_recipe(recipe:&LaunchRecipe)->Self {
        Self {executable:native(recipe.executable.as_os_str()),cwd:native(recipe.cwd.as_os_str()),
            argv:recipe.argv.iter().map(|a|native(a)).collect(),environ:recipe.environ.iter().map(|a|native(a)).collect()}
    }
    fn command(self)->io::Result<std::process::Command> {
        let executable=os(self.executable);let cwd=os(self.cwd);
        let argv:Vec<_>=self.argv.into_iter().map(os).collect();
        if executable.is_empty() || cwd.is_empty() || argv.is_empty()
            || executable.as_encoded_bytes().contains(&0) || cwd.as_encoded_bytes().contains(&0)
            || argv.iter().any(|arg|arg.as_encoded_bytes().contains(&0)) {return Err(io::ErrorKind::InvalidData.into());}
        let mut command=std::process::Command::new(executable);
        command.current_dir(cwd).env_clear().args(&argv[1..]);
        #[cfg(unix)] {use std::os::unix::process::CommandExt;command.arg0(&argv[0]);}
        for entry in self.environ {
            let entry=os(entry);let bytes=entry.as_encoded_bytes();
            if bytes.contains(&0) {return Err(io::ErrorKind::InvalidData.into());}
            let at=bytes.iter().enumerate().find(|(i,b)|*i>0 && **b==b'=').map(|(i,_)|i).ok_or(io::ErrorKind::InvalidData)?;
            // SAFETY: os() reconstrói encoding nativo, cortado só no `=` ASCII.
            let key=unsafe {OsStr::from_encoded_bytes_unchecked(&bytes[..at])};
            let value=unsafe {OsStr::from_encoded_bytes_unchecked(&bytes[at+1..])};
            if ["HANGAR_INTERNAL_SECRET","HANGAR_RUNTIME_INSTANCE","CP_AUTH_TOKEN"].contains(&key.to_str().unwrap_or("")) {continue;}
            command.env(key,value);
        }
        // A receita é da vida antiga; o elo do novo pane vem do multiplexador que lançou o helper.
        for key in ["TMUX","TMUX_PANE","PSMUX_SESSION"] {
            if let Some(value)=std::env::var_os(key) {command.env(key,value);}
        }
        Ok(command)
    }
}
const IPC_WAIT:Duration=Duration::from_secs(15);
const IPC_LIMIT:usize=16*1024*1024;
async fn write_frame<W:tokio::io::AsyncWrite+Unpin>(stream:&mut W,frame:&LaunchFrame)->io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let bytes=serde_json::to_vec(frame).map_err(io::Error::other)?;
    if bytes.len()>IPC_LIMIT {return Err(io::ErrorKind::InvalidData.into());}
    stream.write_u32(bytes.len() as u32).await?;stream.write_all(&bytes).await?;stream.flush().await
}
async fn read_frame<R:tokio::io::AsyncRead+Unpin>(stream:&mut R)->io::Result<LaunchFrame> {
    use tokio::io::AsyncReadExt;
    let len=stream.read_u32().await? as usize;
    if len>IPC_LIMIT {return Err(io::ErrorKind::InvalidData.into());}
    let mut bytes=vec![0;len];stream.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}
#[cfg(unix)]
type Server=tokio::net::UnixListener;
#[cfg(unix)]
type Channel=tokio::net::UnixStream;
#[cfg(windows)]
type Server=tokio::net::windows::named_pipe::NamedPipeServer;
#[cfg(windows)]
type Channel=tokio::net::windows::named_pipe::NamedPipeServer;

/// Um consumo, em memória, com confirmação antes de autorizar a parada da origem.
pub struct Handoff {endpoint:String,server:Option<Server>,private:tempfile::TempDir}
pub struct Consumed {channel:Channel,pub pid:u32,_private:tempfile::TempDir}
impl Handoff {
    pub fn prepare()->io::Result<Self> {
        let private=tempfile::Builder::new().prefix("hangar-relaunch-").tempdir()?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(private.path(),std::fs::Permissions::from_mode(0o700))?;
            let path=private.path().join("launch.sock");
            let endpoint=path.to_str().ok_or(io::ErrorKind::InvalidData)?.to_owned();
            let server=tokio::net::UnixListener::bind(path)?;
            Ok(Self {endpoint,server:Some(server),private})
        }
        #[cfg(windows)] {
            let endpoint=format!(r"\\.\pipe\hangar-relaunch-{}",crate::mods::state::random_hex(16));
            let server=private_pipe(&endpoint)?;
            Ok(Self {endpoint,server:Some(server),private})
        }
    }
    pub fn endpoint(&self)->&str {&self.endpoint}
    pub async fn consume(mut self,expected:&ProcessRecipe,procs:&dyn ProcessView,recipe:&LaunchRecipe)->io::Result<Consumed> {
        use tokio::io::{AsyncReadExt,AsyncWriteExt};
        let expected_pid=u32::try_from(expected.pid).map_err(|_|io::ErrorKind::InvalidData)?;
        if procs.recipe(expected.pid)?!=*expected || expected.executable!=std::env::current_exe()?
            || expected.argv.len()!=3 || expected.argv[1]!=RELAUNCH_FLAG || expected.argv[2]!=*self.endpoint {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let server=self.server.take().ok_or(io::ErrorKind::NotConnected)?;
        let work=async {
            #[cfg(unix)] let mut channel={
                let (channel,_)=server.accept().await?;
                let cred=channel.peer_cred()?;
                // SAFETY: geteuid lê a identidade do chamador.
                if cred.uid()!=unsafe {libc::geteuid()} || cred.pid().and_then(|p|u32::try_from(p).ok())!=Some(expected_pid) {return Err(io::ErrorKind::PermissionDenied.into());}
                channel
            };
            #[cfg(windows)] let mut channel={
                use std::os::windows::io::AsRawHandle;
                server.connect().await?;
                let mut pid=0;
                // SAFETY: pipe vivo e ponteiro de saída local.
                if unsafe {windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId(server.as_raw_handle(),&mut pid)}==0 || pid!=expected_pid {return Err(io::ErrorKind::PermissionDenied.into());}
                server
            };
            if channel.read_u32().await?!=expected_pid || procs.recipe(expected.pid)?!=*expected {return Err(io::ErrorKind::PermissionDenied.into());}
            let frame=LaunchFrame::from_recipe(recipe);
            write_frame(&mut channel,&frame).await?;
            if channel.read_u8().await?!=1 {return Err(io::ErrorKind::InvalidData.into());}
            channel.write_u8(1).await?;channel.flush().await?;
            Ok(Consumed {channel,pid:expected_pid,_private:self.private})
        };
        tokio::time::timeout(IPC_WAIT,work).await.map_err(|_|io::ErrorKind::TimedOut)?
    }
}
impl Consumed {
    pub async fn start(mut self)->io::Result<()> {
        use tokio::io::{AsyncReadExt,AsyncWriteExt};
        let work=async {
            self.channel.write_u8(2).await?;self.channel.flush().await?;
            if self.channel.read_u8().await?!=2 {return Err(io::ErrorKind::InvalidData.into());}
            Ok(())
        };
        tokio::time::timeout(IPC_WAIT,work).await.map_err(|_|io::ErrorKind::TimedOut)?
    }
}
#[cfg(windows)]
struct SecurityDescriptor(windows_sys::Win32::Security::PSECURITY_DESCRIPTOR);
#[cfg(windows)]
impl Drop for SecurityDescriptor {
    fn drop(&mut self) {unsafe {windows_sys::Win32::Foundation::LocalFree(self.0);}}
}
#[cfg(windows)]
fn owner_security()->io::Result<SecurityDescriptor> {
    use windows_sys::Win32::{Foundation::{CloseHandle,LocalFree},Security::{TOKEN_QUERY,TOKEN_USER,TokenUser,GetTokenInformation,Authorization::{ConvertSidToStringSidW,ConvertStringSecurityDescriptorToSecurityDescriptorW}},System::Threading::{GetCurrentProcess,OpenProcessToken}};
    let mut token=std::ptr::null_mut();
    // SAFETY: saída local, token só para leitura da identidade corrente.
    if unsafe {OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY,&mut token)}==0 {return Err(io::Error::last_os_error());}
    let mut size=0;
    unsafe {GetTokenInformation(token,TokenUser,std::ptr::null_mut(),0,&mut size);}
    let mut storage=vec![0usize;(size as usize).div_ceil(std::mem::size_of::<usize>())];
    let result=unsafe {GetTokenInformation(token,TokenUser,storage.as_mut_ptr().cast(),size,&mut size)};
    unsafe {CloseHandle(token);}
    if result==0 {return Err(io::Error::last_os_error());}
    let user=unsafe {&*(storage.as_ptr().cast::<TOKEN_USER>())};
    let mut sid=std::ptr::null_mut();
    if unsafe {ConvertSidToStringSidW(user.User.Sid,&mut sid)}==0 {return Err(io::Error::last_os_error());}
    let mut len=0;while unsafe {*sid.add(len)}!=0 {len+=1;}
    let sid_text=String::from_utf16(unsafe {std::slice::from_raw_parts(sid,len)}).map_err(|_|io::ErrorKind::InvalidData);
    unsafe {LocalFree(sid.cast());}
    let sid_text=sid_text?;
    let sddl:Vec<u16>=format!("O:{sid_text}D:P(A;;GA;;;{sid_text})\0").encode_utf16().collect();
    let mut descriptor=std::ptr::null_mut();
    // SAFETY: SDDL terminado em NUL; o descritor vive até CreateNamedPipe voltar.
    if unsafe {ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(),1,&mut descriptor,std::ptr::null_mut())}==0 {return Err(io::Error::last_os_error());}
    Ok(SecurityDescriptor(descriptor))
}
#[cfg(windows)]
fn private_pipe(endpoint:&str)->io::Result<Server> {
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    let descriptor=owner_security()?;
    let mut attrs=SECURITY_ATTRIBUTES {nLength:std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,lpSecurityDescriptor:descriptor.0,bInheritHandle:0};
    unsafe {tokio::net::windows::named_pipe::ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).max_instances(1).create_with_security_attributes_raw(endpoint,&mut attrs as *mut _ as *mut _)}
}
#[cfg(windows)]
fn restrict_file(path:&Path)->io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::{SetFileSecurityW,OWNER_SECURITY_INFORMATION,DACL_SECURITY_INFORMATION,PROTECTED_DACL_SECURITY_INFORMATION};
    let descriptor=owner_security()?;
    let path:Vec<_>=path.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {SetFileSecurityW(path.as_ptr(),OWNER_SECURITY_INFORMATION|DACL_SECURITY_INFORMATION|PROTECTED_DACL_SECURITY_INFORMATION,descriptor.0)}==0 {return Err(io::Error::last_os_error());}
    Ok(())
}

/// Despacho do binário antes de iniciar servidor, logs ou varredura. O endpoint não carrega credencial.
pub fn helper_args(args:&[OsString])->io::Result<Option<&OsStr>> {
    if args.get(1).is_none_or(|a|a!=RELAUNCH_FLAG) {return Ok(None);}
    if args.len()!=3 || args[2].is_empty() {return Err(io::ErrorKind::InvalidInput.into());}
    Ok(Some(&args[2]))
}
pub async fn helper(endpoint:&OsStr)->io::Result<i32> {
    use tokio::io::{AsyncReadExt,AsyncWriteExt};
    let work=async {
        #[cfg(unix)] let mut channel=tokio::net::UnixStream::connect(Path::new(endpoint)).await?;
        #[cfg(unix)] if channel.peer_cred()?.uid()!=unsafe {libc::geteuid()} {return Err(io::ErrorKind::PermissionDenied.into());}
        #[cfg(windows)] let mut channel=tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint)?;
        channel.write_u32(std::process::id()).await?;
        let frame=read_frame(&mut channel).await?;
        let command=frame.command()?;
        channel.write_u8(1).await?;channel.flush().await?;
        if channel.read_u8().await?!=1 || channel.read_u8().await?!=2 {return Err(io::Error::from(io::ErrorKind::PermissionDenied));}
        Ok::<_,io::Error>((command,channel))
    };
    // O prazo cobre o IPC, não o tempo de vida do Claude depois do compromisso.
    let (mut command,mut channel)=tokio::time::timeout(IPC_WAIT,work).await.map_err(|_|io::ErrorKind::TimedOut)??;
    let mut child=command.spawn()?;
    channel.write_u8(2).await?;channel.flush().await?;
    drop(channel);
    Ok(child.wait()?.code().unwrap_or(1))
}

pub fn quote(arg:&str,windows:bool)->String {
    if !arg.is_empty() && arg.chars().all(|c|c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c)) {return arg.into();}
    format!("'{}'",arg.replace('\'',if windows {"''"} else {"'\"'\"'"}))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::procs::{ChildrenMap,ProcessRecipe,ProcessView};
    use std::{collections::HashMap,sync::Arc};

    struct Processes { recipes:HashMap<i64,ProcessRecipe>,children:ChildrenMap }
    impl ProcessView for Processes {
        fn children(&self,_:Duration)->io::Result<Arc<ChildrenMap>> {Ok(Arc::new(self.children.clone()))}
        fn argv(&self,_:i64)->Vec<String> {vec![]}
        fn cwd(&self,pid:i64)->Option<PathBuf> {self.recipes.get(&pid).map(|r|r.cwd.clone())}
        fn env_var(&self,_:i64,_:&str)->io::Result<Option<OsString>> {Err(io::ErrorKind::Unsupported.into())}
        fn start_time(&self,pid:i64)->Option<f64> {self.recipes.get(&pid).map(|r|r.birth)}
        fn recipe(&self,pid:i64)->io::Result<ProcessRecipe> {self.recipes.get(&pid).cloned().ok_or(io::ErrorKind::NotFound.into())}
    }
    fn process(pid:i64,args:&[&str])->ProcessRecipe {
        ProcessRecipe {pid,birth:1000.0,executable:args[0].into(),cwd:"/work".into(),argv:args.iter().map(OsString::from).collect(),
            environ:vec!["CLAUDE_CONFIG_DIR=/account".into(),"CP_ENGINE_ACCOUNT=chosen".into()],cgroup:None}
    }
    fn processes()->Processes {
        Processes {recipes:HashMap::from([(10,process(10,&["/bin/fish"])),(11,process(11,&["/bin/claude","--session-id","sid","--model","chosen"]))]),children:HashMap::from([(10,vec![11])])}
    }
    #[test]
    fn relative_settings_use_agent_cwd_in_copy_verify_and_policy() {
        let server=tempfile::tempdir().unwrap();let agent=tempfile::tempdir().unwrap();let private=tempfile::tempdir().unwrap();
        std::fs::write(server.path().join("settings.json"),b"{}").unwrap();
        let restrictive=br#"{"permissions":{"disableBypassPermissionsMode":"disable"}}"#;
        std::fs::write(agent.path().join("settings.json"),restrictive).unwrap();
        let mut source=processes();source.recipes.get_mut(&11).unwrap().cwd=agent.path().into();
        source.recipes.get_mut(&11).unwrap().argv.extend(["--settings".into(),"settings.json".into()]);
        source.recipes.get_mut(&11).unwrap().environ.push(format!("ProgramFiles={}",server.path().display()).into());
        let mut recipe=Recipe::capture(&source,10,11,1000.0,"sid",false).unwrap();
        assert_eq!(recipe.settings_path(OsStr::new("settings.json"),false).unwrap(),agent.path().join("settings.json"));
        recipe.preserve_settings(private.path()).unwrap();
        let launch=recipe.launch("sid","bypassPermissions").unwrap();
        let at=launch.argv.iter().position(|a|a=="--settings").unwrap();
        assert_eq!(std::fs::read(Path::new(&launch.argv[at+1])).unwrap(),restrictive);
        recipe.verify(&source).unwrap();
        assert_eq!(recipe.policy_preflight(server.path()).unwrap_err().code,"relaunch_policy_forbids_bypass");
        std::fs::write(agent.path().join("settings.json"),b"{}").unwrap();
        assert_eq!(recipe.verify(&source).unwrap_err().code,"relaunch_settings_changed");
        assert!(recipe.settings_path(OsStr::new("C:settings.json"),true).is_err());
        assert!(recipe.settings_path(OsStr::new("\\settings.json"),true).is_err());
    }
    #[test]
    fn fake_process_tree_includes_the_root_receipt() {
        let source=processes();assert_eq!(source.recipe(10).unwrap().pid,10);
        assert_eq!(Recipe::capture(&source,10,11,1000.0,"sid",false).unwrap().root_identity().unwrap(),(10,1000.0));
    }
    #[test]
    fn explicit_resume_keeps_choices_and_environment() {
        let source=processes();
        let recipe=Recipe::capture(&source,10,11,1000.0,"sid",false).unwrap();
        let launch=recipe.launch("sid","bypassPermissions").unwrap();
        assert_eq!(launch.argv,vec!["/bin/claude","--resume","sid","--model","chosen","--permission-mode","bypassPermissions","--allow-dangerously-skip-permissions"].into_iter().map(OsString::from).collect::<Vec<_>>());
        assert_eq!(launch.environ,source.recipes[&11].environ);
    }
    #[test]
    fn absent_proof_never_returns_a_launch_recipe() {
        let mut source=processes();
        let code=|result:Result<Recipe,RuntimeError>|result.err().unwrap().code;
        assert_eq!(code(Recipe::capture(&source,10,11,999.0,"sid",false)),"relaunch_process_binding");
        assert_eq!(code(Recipe::capture(&source,10,11,1000.0,"other",false)),"relaunch_conversation");
        assert_eq!(code(Recipe::capture(&source,10,11,1000.0,"sid",true)),"relaunch_read_only");
        source.recipes.get_mut(&11).unwrap().environ.clear();
        assert_eq!(code(Recipe::capture(&source,10,11,1000.0,"sid",false)),"relaunch_process_binding");
    }
    #[test]
    fn read_only_reuses_proved_envelope_not_unprotected_child() {
        let mut source=processes();
        source.recipes.get_mut(&11).unwrap().environ.push("HANGAR_ORQ_READ_ONLY=1".into());
        source.recipes.insert(10,process(10,&["/bin/bwrap","--bind","/","/","--dev-bind","/dev","/dev","--setenv","HANGAR_ORQ_READ_ONLY","1","--ro-bind","/work","/work","--","/bin/claude","--session-id","sid"]));
        let recipe=Recipe::capture(&source,10,11,1000.0,"sid",true).unwrap();
        let launch=recipe.launch("sid","bypassPermissions").unwrap();
        assert_eq!(launch.executable,PathBuf::from("/bin/bwrap"));
        assert!(launch.argv.windows(3).any(|w|w==["--ro-bind","/work","/work"].map(OsString::from)));
        assert!(launch.argv.windows(2).any(|w|w==["--resume","sid"].map(OsString::from)));
    }
    #[test]
    fn windows_settings_are_copied_before_the_parent_can_remove_them() {
        let source_dir=tempfile::tempdir().unwrap();
        let private=tempfile::tempdir().unwrap();
        let settings=source_dir.path().join("hangar-engine-choice.json");
        std::fs::write(&settings,b"{\"serviceTier\":\"priority\"}").unwrap();
        let mut source=processes();
        source.recipes.get_mut(&11).unwrap().argv.extend([OsString::from("--settings"),settings.into_os_string()]);
        let mut recipe=Recipe::capture(&source,10,11,1000.0,"sid",false).unwrap();
        recipe.preserve_settings(private.path()).unwrap();
        std::fs::remove_dir_all(source_dir.path()).unwrap();
        let launch=recipe.launch("sid","bypassPermissions").unwrap();
        let path=PathBuf::from(launch.argv[launch.argv.iter().position(|a|a=="--settings").unwrap()+1].clone());
        assert_eq!(std::fs::read(path).unwrap(),b"{\"serviceTier\":\"priority\"}");
    }
    #[test]
    fn unused_settings_copy_is_removed_without_touching_source() {
        let source_dir=tempfile::tempdir().unwrap();let private=tempfile::tempdir().unwrap();
        let path=source_dir.path().join("source.json");std::fs::write(&path,b"{}").unwrap();
        let mut processes=processes();processes.recipes.get_mut(&11).unwrap().argv.extend([OsString::from("--settings"),path.as_os_str().to_owned()]);
        let mut recipe=Recipe::capture(&processes,10,11,1000.0,"sid",false).unwrap();recipe.preserve_settings(private.path()).unwrap();
        let launch=recipe.launch("sid","bypassPermissions").unwrap();
        let copy=PathBuf::from(launch.argv[launch.argv.iter().position(|a|a=="--settings").unwrap()+1].clone());
        assert!(copy.exists());drop(recipe);
        assert!(!copy.exists());assert_eq!(std::fs::read(path).unwrap(),b"{}");
    }
    #[test]
    fn settings_are_not_deleted_while_the_confirmed_owner_is_alive() {
        let source_dir=tempfile::tempdir().unwrap();let private=tempfile::tempdir().unwrap();
        let path=source_dir.path().join("source.json");std::fs::write(&path,b"{}").unwrap();
        let mut processes=processes();processes.recipes.get_mut(&11).unwrap().argv.extend([OsString::from("--settings"),path.as_os_str().to_owned()]);
        let mut recipe=Recipe::capture(&processes,10,11,1000.0,"sid",false).unwrap();recipe.preserve_settings(private.path()).unwrap();
        let launch=recipe.launch("sid","bypassPermissions").unwrap();
        let copy=PathBuf::from(launch.argv[launch.argv.iter().position(|a|a=="--settings").unwrap()+1].clone());
        // O dono é o próprio processo de teste, nunca uma sessão Claude.
        let pid=std::process::id();let birth=crate::terminal_facts::agent_birth(pid).unwrap();recipe.pin_settings(i64::from(pid),birth).unwrap();drop(recipe);
        assert!(copy.exists());assert_eq!(std::fs::read(path).unwrap(),b"{}");
    }
    #[test]
    fn confirmed_choices_replace_only_model_and_effort() {
        let processes=processes();let recipe=Recipe::capture(&processes,10,11,1000.0,"sid",false).unwrap();
        let mut launch=recipe.launch("sid","bypassPermissions").unwrap();let env=launch.environ.clone();
        recipe.apply_choices(&mut launch,Some("new-model"),Some("high")).unwrap();
        assert!(launch.argv.windows(2).any(|w|w==["--model","new-model"].map(OsString::from)));
        assert!(launch.argv.windows(2).any(|w|w==["--effort","high"].map(OsString::from)));
        assert_eq!(launch.environ,env);
        assert!(recipe.apply_choices(&mut launch,None,Some("ultracode")).is_err());
    }
    #[test]
    fn private_bootstrap_never_selects_server_initialization() {
        let args=["hangar-server",RELAUNCH_FLAG,"private-endpoint"].map(OsString::from);
        let mut server_started=false;
        match helper_args(&args).unwrap() {Some(endpoint)=>assert_eq!(endpoint,"private-endpoint"),None=>server_started=true}
        assert!(!server_started);
        assert!(helper_args(&args[..2]).is_err());
        assert!(helper_args(&[OsString::from("hangar-server")]).unwrap().is_none());
    }
    #[test]
    fn mounted_launch_dto_has_no_fabricated_process_identity() {
        let launch=LaunchRecipe {executable:"/bin/claude".into(),cwd:"/work".into(),argv:vec!["claude".into(),"--resume".into(),"sid".into()],environ:vec!["A=value".into()]};
        let command=LaunchFrame::from_recipe(&launch).command().unwrap();
        assert_eq!(command.get_program(),"/bin/claude");
        assert_eq!(command.get_args().collect::<Vec<_>>(),vec![OsStr::new("--resume"),OsStr::new("sid")]);
        let mut invalid=launch.clone();invalid.argv.push(OsString::from("bad\0argument"));
        assert!(LaunchFrame::from_recipe(&invalid).command().is_err());
    }
    #[test]
    fn wire_command_preserves_environment_and_never_keeps_internal_authority() {
        let mut recipe=process(11,&["/bin/claude","--session-id","sid"]);
        recipe.environ.push("CP_AUTH_TOKEN=must-not-inherit".into());
        let command=LaunchFrame::from_recipe(&LaunchRecipe::from(&recipe)).command().unwrap();
        let env:HashMap<_,_>=command.get_envs().filter_map(|(k,v)|v.map(|v|(k.to_owned(),v.to_owned()))).collect();
        assert_eq!(env.get(OsStr::new("CP_ENGINE_ACCOUNT")),Some(&OsString::from("chosen")));
        assert!(!env.contains_key(OsStr::new("CP_AUTH_TOKEN")));
    }
    #[test]
    fn quote_respects_each_mux_dialect() {
        assert_eq!(quote("C:\\a b\\o'neil$",true),"'C:\\a b\\o''neil$'");
        assert_eq!(quote("o'neil$",false),"'o'\"'\"'neil$'");
        assert_eq!(quote("",true),"''");
    }
}

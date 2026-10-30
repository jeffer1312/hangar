//! Painel local e remoto: resolve o livro e executa ações sem carregar o backend.
mod actions;
mod book_write;
pub mod projection;
mod projects;
use super::{store, RequestValue};
use crate::{accounts::environment::AccountEnvironment, orchestration::codec};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub trait Transport: Send + Sync {
    fn request<'a>(
        &'a self,
        base: &'a str,
        token: &'a str,
        method: &'a str,
        path: &'a str,
        body: Option<Value>,
        timeout: u64,
    ) -> BoxFuture<'a, Result<Value, String>>;
}
pub struct NativeTransport;
impl Transport for NativeTransport {
    fn request<'a>(
        &'a self, base:&'a str, token:&'a str, method:&'a str, path:&'a str,
        body:Option<Value>, timeout:u64,
    )->BoxFuture<'a,Result<Value,String>> {
        Box::pin(async move {
            let client=crate::http_client().connect_timeout(Duration::from_secs(timeout)).read_timeout(Duration::from_secs(timeout))
                .redirect(reqwest::redirect::Policy::none()).build().map_err(|error|format!("servidor inacessível: {}",error.without_url()))?;
            let mut url=reqwest::Url::parse(&format!("{}{path}",base.trim_end_matches('/'))).map_err(|error|format!("servidor inacessível: {error}"))?;
            let mut method=method.parse::<reqwest::Method>().map_err(|error|error.to_string())?;
            let mut bytes=body.as_ref().map(|value|codec::dumps(value,false));
            if let Some(body)=body{codec::discard(body);}
            let mut authorization=true;let mut content_type=true;
            let mut visited=std::collections::HashMap::<String,usize>::new();
            loop {
                let mut request=client.request(method.clone(),url.clone());
                if authorization{request=request.bearer_auth(token);}
                if content_type{request=request.header("content-type","application/json");}
                if let Some(bytes)=&bytes{request=request.body(bytes.clone());}
                let response=request.send().await.map_err(|error|format!("servidor inacessível: {}",error.without_url()))?;
                let status=response.status();
                let reason=response.extensions().get::<hyper::ext::ReasonPhrase>().map(|reason|String::from_utf8_lossy(reason.as_bytes()).into_owned())
                    .unwrap_or_else(||status.canonical_reason().unwrap_or("").into());
                let location=response.headers().get(reqwest::header::LOCATION).or_else(||response.headers().get("uri"))
                    .map(|value|value.as_bytes().iter().map(|byte|char::from(*byte)).collect::<String>());
                let redirect=matches!(status.as_u16(),301|302|303|307|308)
                    && ((method==reqwest::Method::GET||method==reqwest::Method::HEAD)
                        || method==reqwest::Method::POST&&matches!(status.as_u16(),301|302|303));
                let response_bytes=response.bytes().await.map_err(|error|format!("servidor inacessível: {}",error.without_url()))?;
                if redirect&&let Some(location)=location {
                    let next=url.join(&location).map_err(|error|format!("servidor inacessível: {error}"))?;
                    if visited.get(next.as_str()).copied().unwrap_or(0)>=4||visited.len()>=10 {
                        return Err(format!("{}: The HTTP server returned a redirect error that would lead to an infinite loop.\nThe last 30x error message was:\n{reason}",status.as_u16()));
                    }
                    *visited.entry(next.to_string()).or_default()+=1;
                    if url.origin()!=next.origin(){authorization=false;}
                    url=next;content_type=false;bytes=None;
                    if method!=reqwest::Method::HEAD{method=reqwest::Method::GET;}
                    continue;
                }
                return decode_response(base,status.as_u16(),&reason,&response_bytes);
            }
        })
    }
}
pub fn decode_response(
    base: &str,
    status: u16,
    reason: &str,
    bytes: &[u8],
) -> Result<Value, String> {
    if !(200..300).contains(&status) {
        let parsed=std::str::from_utf8(bytes).ok().and_then(codec::loads);
        let detail=parsed.as_ref().and_then(|value|value.get("detail")).filter(|value|store::truthy(value))
            .map(codec::python_string).unwrap_or_else(||reason.into());
        if let Some(value)=parsed{codec::discard(value);}
        return Err(format!("{status}: {detail}"));
    }
    let text=std::str::from_utf8(bytes).map_err(|error|format!("resposta não-JSON de {base}: {error}"))?;
    if text.is_empty() {
        return Ok(json!({}));
    }
    codec::loads(text).ok_or_else(|| format!("resposta não-JSON de {base}: JSON inválido"))
}
#[derive(Clone)]
pub struct Panel {
    pub environment: AccountEnvironment,
    pub backend: Option<PathBuf>,
    pub cwd: PathBuf,
    pub transport: Arc<dyn Transport>,
}
impl Panel {
    pub fn new(environment: AccountEnvironment, backend: Option<PathBuf>, cwd: PathBuf) -> Self {
        Self {
            environment,
            backend,
            cwd,
            transport: Arc::new(NativeTransport),
        }
    }
    fn path(&self, name: &str) -> Result<PathBuf, String> {
        crate::user_data::path(&self.environment.home, self.backend.as_deref(), name)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{name} indisponível"))
    }
    // O painel lê a primeira atribuição literal do arquivo; não é um consumidor de Settings.
    pub fn env(&self, key: &str) -> Result<String,String> {
        let Some(bytes)=self.path(".env").ok().and_then(|path|std::fs::read(path).ok()) else{return Ok(String::new());};
        let text=std::str::from_utf8(&bytes).map_err(|_|ENV_UNICODE.to_owned())?;
        Ok(crate::transcript::py::splitlines(text).into_iter().find_map(|line|line.strip_prefix(&format!("{key}=")).map(|value|crate::transcript::py::strip(value).into())).unwrap_or_default())
    }
    pub fn local_base(&self) -> Result<String,String> {
        let port = self.env("CP_PORT")?;
        Ok(format!("http://127.0.0.1:{}",if port.is_empty(){"8765"}else{&port}))
    }
    fn peers(&self) -> Result<RequestValue, String> {
        let path = self.path("peers.json")?;
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(RequestValue(json!({}))),
            Err(e) => return Err(format!("peers.json inválido: {e}")),
        };
        let value = codec::loads(&raw).ok_or("peers.json inválido: JSON inválido")?;
        match value {
            Value::Object(map) => Ok(RequestValue(Value::Object(map))),
            value => {codec::discard(value);Err("peers.json inválido: raiz deve ser um objeto {id: {...}}".into())},
        }
    }
    pub fn resolve(&self, address: &str) -> Result<(String, String, String), String> {
        let Some((server, name)) = address.split_once("::") else {
            return Ok((self.local_base()?, self.env("CP_AUTH_TOKEN")?, address.into()));
        };
        let peers = self.peers()?;
        let cfg = peers
            .get(server)
            .filter(|v| store::truthy(v))
            .ok_or_else(|| {
                format!(
                    "servidor '{server}' desconhecido (conhecidos: {})",
                    if peers.as_object().unwrap().is_empty() {
                        "nenhum".into()
                    } else {
                        peers.as_object().unwrap().keys().cloned().collect::<Vec<_>>().join(", ")
                    }
                )
            })?;
        let (base, token, _) = projection::fields(cfg);
        if base.is_empty() || token.is_empty() {
            return Err(format!(
                "peers.json: entrada '{server}' sem base_url/token válidos (string)"
            ));
        }
        Ok((base.trim_end_matches('/').into(), token, name.into()))
    }
    pub(super) async fn api(
        &self,
        address: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        timeout: u64,
    ) -> Result<RequestValue, String> {
        let (base, token, name) = self.resolve(address)?;
        const COMPONENT:&percent_encoding::AsciiSet=&percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');
        let encoded=percent_encoding::utf8_percent_encode(&name,COMPONENT).to_string();
        self.transport
            .request(
                &base,
                &token,
                method,
                &path.replace("{name}", &encoded),
                body,
                timeout,
            )
            .await.map(RequestValue)
    }
    fn binary(&self, name: &str) -> Option<PathBuf> {
        self.environment
            .base
            .get("PATH")
            .into_iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join(name))
            .find(|path| {
                if !path.is_file() {
                    return false;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    path.metadata()
                        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
                }
                #[cfg(not(unix))]
                {
                    true
                }
            })
    }
    fn command(
        &self,
        args: &[String],
        seconds: u64,
    ) -> Result<crate::update::process::ProcessOutput, String> {
        let mut resolved=args.to_vec();
        if let Some(program)=args.first().and_then(|name|self.binary(name)){resolved[0]=program.to_string_lossy().into_owned();}
        crate::update::process::run_capture_with_env(
            &resolved,&self.cwd,Duration::from_secs(seconds),&self.environment.base,
        ).map_err(|error|match error{
            crate::update::process::ProcessError::Spawn(error)=>spawn_error(&error,args.first().map(String::as_str).unwrap_or("")),
            crate::update::process::ProcessError::Timeout{..}=>format!("Command '{}' timed out after {seconds} seconds",codec::python_string(&json!(args))),
            error=>error.to_string(),
        })
    }

    pub fn realpath(&self, path: &str) -> String {
        realpath(&self.cwd, Path::new(path)).display().to_string()
    }
}
fn realpath(cwd: &Path, path: &Path) -> PathBuf {
    use std::{collections::{HashSet,VecDeque},ffi::OsString,path::Component};
    enum Part { Path(OsString), End(PathBuf) }
    fn parts(path:&Path)->VecDeque<Part>{path.components().map(|part|Part::Path(part.as_os_str().to_owned())).collect()}
    let absolute=if path.is_absolute(){path.to_owned()}else{cwd.join(path)};
    let mut pending=parts(&absolute);let mut active=HashSet::new();let mut output=PathBuf::new();
    while let Some(part)=pending.pop_front(){
        let Part::Path(part)=part else{if let Part::End(path)=part{active.remove(&path);}continue;};
        match Path::new(&part).components().next(){
            Some(Component::ParentDir)=>{output.pop();}
            Some(Component::CurDir)|None=>{}
            Some(Component::Normal(_))=>{
                output.push(&part);
                if let Ok(target)=std::fs::read_link(&output){
                    let link=output.clone();
                    if active.insert(link.clone()){
                        output.pop();
                        let mut expansion=parts(&target);expansion.push_back(Part::End(link));expansion.append(&mut pending);pending=expansion;
                    }
                }
            }
            _=>output.push(part),
        }
    }
    output
}

pub fn cli(command: &str, args: &[String]) -> std::process::ExitCode {
    let environment = AccountEnvironment::capture();
    let backend = crate::session_open::cli::backend_dir_for_home(&environment.home);
    let panel = Panel::new(
        environment,
        backend,
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    );
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            println!("{}", json!({"ok":false,"message":error.to_string()}));
            return 1.into();
        }
    };
    let (code, result) = runtime.block_on(async {
        if command == "panel-data" {
            (0, panel.snapshot().await)
        } else {
            panel.action(args).await
        }
    });
    if result["message"]==ENV_UNICODE {eprintln!("erro: {ENV_UNICODE}");codec::discard(result);return 1.into();}
    if command=="panel-action"&&args.get(1).is_some_and(|arg|arg=="peer")&&result["message"]==book_write::UNICODE_ERROR {
        eprintln!("erro: {}",book_write::UNICODE_ERROR);codec::discard(result);return 1.into();
    }
    if command=="panel-action"&&args.len()<2 {println!("{}",codec::dumps(&result,false));codec::discard(result);return code.into();}
    match codec::dumps_utf8(&result) { Ok(text)=>println!("{text}"), Err(_)=>{eprintln!("erro: saída do painel contém Unicode inválido");codec::discard(result);return 1.into();} }
    codec::discard(result);
    code.into()
}

fn spawn_error(error:&std::io::Error,program:&str)->String{
    if let Some(code)=error.raw_os_error(){
        let detail=error.to_string();let suffix=format!(" (os error {code})");
        format!("[Errno {code}] {}: {}",detail.strip_suffix(&suffix).unwrap_or(&detail),crate::transcript::py::py_repr(&json!(program)))
    }else{error.to_string()}
}

const ENV_UNICODE:&str="configuração do painel contém Unicode inválido";

//! Um tique do checkout: os efeitos e avisos seguem a mesma ordem do botão automático.
use serde_json::Value;
use std::future::Future;
#[derive(Debug)]
pub enum Event { FetchFailed(i32,String), Launch, Refused(String), Skipped(String), Failed(String) }
pub trait Source {
    fn package(&self)->bool;
    fn sleep(&self)->impl Future<Output=Result<(),()>>+Send;
    fn enabled(&self)->bool;
    fn fetch(&self)->impl Future<Output=Result<(i32,String),String>>+Send;
    fn reason(&self)->impl Future<Output=Result<Option<String>,String>>+Send;
    fn working(&self)->impl Future<Output=Result<bool,String>>+Send;
    fn launch(&self)->impl Future<Output=Result<Value,String>>+Send;
    fn report(&self,event:Event);
}
pub async fn run(source:&impl Source) {
    if source.package(){return;}
    while source.sleep().await.is_ok(){tick(source).await;}
}
pub async fn tick(source:&impl Source) {
    if let Err(error)=attempt(source).await { source.report(Event::Failed(error)); }
}
async fn attempt(source:&impl Source)->Result<(),String> {
    if !source.enabled(){return Ok(());}
    let (code,tail)=source.fetch().await?;
    if code!=0 {source.report(Event::FetchFailed(code,tail));}
    let mut reason=source.reason().await?;
    if reason.is_none() && source.working().await? {reason=Some("sessao trabalhando".into());}
    match reason {
        None=>{
            source.report(Event::Launch);
            let result=source.launch().await?;
            if !crate::config_sync::apply::truthy(&result["ok"]) {
                source.report(Event::Refused(crate::orchestration::codec::python_string(&result["erro"])));
            }
            crate::orchestration::codec::discard(result);
        }
        Some(reason) if reason!="em dia"=>source.report(Event::Skipped(reason)),
        _=>{}
    }
    Ok(())
}
pub struct Live {
    pub service:super::Service,
    pub runtime:crate::config_sync::runtime::RuntimeConfig,
    pub list:std::sync::Arc<crate::list::bridge::ListBridge>,
    pub port:u16,
}
impl Source for Live {
    fn package(&self)->bool {matches!(crate::update::http::mode(&self.service.home),crate::update::http::Mode::Package(_))}
    async fn sleep(&self)->Result<(),()>{tokio::time::sleep(std::time::Duration::from_secs(3600)).await;Ok(())}
    fn enabled(&self)->bool {
        let value=self.runtime.get("automations");let enabled=crate::config_sync::apply::truthy(&value);
        crate::orchestration::codec::discard(value);enabled
    }
    async fn fetch(&self)->Result<(i32,String),String>{
        let service=self.service.clone();
        tokio::task::spawn_blocking(move||service.git(&["fetch","origin"],120).map(|output|(output.code,super::tail(&output,12)))).await.map_err(|error|error.to_string())?
    }
    async fn reason(&self)->Result<Option<String>,String>{
        let service=self.service.clone();
        tokio::task::spawn_blocking(move||service.auto_reason(&super::releases::HttpSource,Some(&crate::update::platform()),chrono::Local::now().fixed_offset())).await.map_err(|error|error.to_string())?
    }
    async fn working(&self)->Result<bool,String>{
        self.list.snapshot().await.map(|snapshot|snapshot.rows.iter().any(|row|row.state=="working")).map_err(|error|error.code.to_owned())
    }
    async fn launch(&self)->Result<Value,String>{
        let service=self.service.clone();let port=self.port;
        tokio::task::spawn_blocking(move||service.start_detached(port,Some("main"),false)).await.map_err(|error|error.to_string())?
    }
    fn report(&self,event:Event){match event{
        Event::FetchFailed(code,tail)=>tracing::warn!("auto-update: fetch falhou (rc={code}): {tail}"),
        Event::Launch=>tracing::info!("auto-update: disparando atualizacao"),
        Event::Refused(reason)=>tracing::warn!("auto-update: iniciar recusou ({reason})"),
        Event::Skipped(reason)=>tracing::info!("auto-update: pulando ({reason})"),
        Event::Failed(error)=>tracing::error!(%error,"auto-update: tick falhou"),
    }}
}

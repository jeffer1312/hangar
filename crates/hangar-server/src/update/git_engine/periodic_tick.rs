//! Um tique do checkout: os efeitos e avisos seguem a mesma ordem do botão automático.
use serde_json::Value;
#[derive(Debug)]
pub enum Event { FetchFailed(i32,String), Launch, Refused(String), Skipped(String), Failed(String) }
use crate::update::periodic::Busy;
type Time=chrono::DateTime<chrono::FixedOffset>;
pub async fn run(source:&Live) {
    if source.package(){return;}
    let mut since=None;
    while source.sleep().await.is_ok(){tick(source,&mut since).await;}
}
async fn tick(source:&Live,since:&mut Option<Time>) {
    if let Err(error)=attempt(source,since).await { source.report(Event::Failed(error)); }
}
/// O motivo de adiar, com `since` = desde quando a máquina segura a atualização.
fn hold(busy:Option<Busy>,since:&mut Option<Time>,now:Time)->Option<String> {
    let Some(busy)=busy else { *since=None; return None };
    let first=*since.get_or_insert(now);
    if now-first<crate::update::periodic::DEFER_CEILING { return Some(busy.why().into()); }
    tracing::warn!("auto-update: adiada há 24 h ({}); atualiza assim mesmo",busy.why());
    *since=None;
    None
}
async fn attempt(source:&Live,since:&mut Option<Time>)->Result<(),String> {
    if !source.enabled(){return Ok(());}
    let (code,tail)=source.fetch().await?;
    if code!=0 {source.report(Event::FetchFailed(code,tail));}
    let mut reason=source.reason().await?;
    if reason.is_none() {reason=hold(source.busy().await?,since,chrono::Local::now().fixed_offset());} else {*since=None;}
    match reason {
        None=>{
            source.report(Event::Launch);
            let result=source.launch().await?;
            if !crate::orchestration::truthy(&result["ok"]) {
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
    pub state:std::sync::Arc<crate::state::live::StateEnv>,
    pub port:u16,
}
impl Live {
    fn package(&self)->bool {matches!(crate::update::http::mode(&self.service.home),crate::update::http::Mode::Package(_))}
    async fn sleep(&self)->Result<(),()>{tokio::time::sleep(std::time::Duration::from_secs(3600)).await;Ok(())}
    fn enabled(&self)->bool {
        let value=self.runtime.get("automations");let enabled=crate::orchestration::truthy(&value);
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
    async fn busy(&self)->Result<Option<crate::update::periodic::Busy>,String>{crate::update::periodic::busy_now(&self.list,&self.state).await}
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

#[cfg(test)]
mod tests {
    use super::*;
    fn now()->Time {chrono::DateTime::parse_from_rfc3339("2026-10-24T12:00:00-03:00").unwrap()}
    /// Mesma regra do motor de pacote: fila e sessão trabalhando seguram até 24 h; a entrega incerta, sempre.
    #[test]
    fn git_engine_deferral_has_the_same_24h_ceiling() {
        let mut since=None;
        assert_eq!(hold(Some(Busy::Queue),&mut since,now()).as_deref(),Some(Busy::Queue.why()));
        assert_eq!(since,Some(now()),"o adiamento conta do primeiro");
        let later=now()+chrono::TimeDelta::hours(23);
        assert!(hold(Some(Busy::Working),&mut since,later).is_some());
        assert_eq!(since,Some(now()));
        let day=now()+chrono::TimeDelta::hours(24);
        assert_eq!(hold(Some(Busy::Working),&mut since.clone(),day),None,"passou de 24 h: atualiza");
        assert!(hold(Some(Busy::Uncertain),&mut since,later).is_some(),"incerta há 23 h segura");
        assert_eq!(hold(Some(Busy::Uncertain),&mut since,day),None,"incerta há 24 h solta");
        since=Some(now());
        assert_eq!(hold(None,&mut since,day),None);
        assert_eq!(since,None,"livre, o adiamento acaba");
    }
}

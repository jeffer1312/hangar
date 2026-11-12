//! Um tique do checkout: os efeitos e avisos seguem a mesma ordem do botão automático.
use serde_json::Value;
#[derive(Debug)]
pub enum Event { FetchFailed(i32,String), Launch, Refused(String), Skipped(String), Failed(String) }
use crate::update::periodic::Busy;
type Time=chrono::DateTime<chrono::FixedOffset>;
pub async fn run(source:&Live) {
    if source.package(){return;}
    let mut since=Held::default();
    while source.sleep().await.is_ok(){tick(source,&mut since).await;}
}
/// Desde quando a atualização está adiada e desde quando uma entrega incerta a segura.
#[derive(Clone,Copy,Debug,Default,PartialEq)]
struct Held { since:Option<Time>, uncertain:Option<Time> }
async fn tick(source:&Live,since:&mut Held) {
    if let Err(error)=attempt(source,since).await { source.report(Event::Failed(error)); }
}
/// O motivo de adiar. A entrega incerta conta as 24 h dela, da primeira vez que apareceu.
fn hold(busy:Option<Busy>,held:&mut Held,now:Time)->Option<String> {
    let Some(busy)=busy else { *held=Held::default(); return None };
    let first=*held.since.get_or_insert(now);
    let start=if busy==Busy::Uncertain { *held.uncertain.get_or_insert(now) } else { held.uncertain=None; first };
    if now-start<crate::update::periodic::DEFER_CEILING { return Some(busy.why().into()); }
    tracing::warn!("auto-update: adiada há 24 h ({}); atualiza assim mesmo",busy.why());
    *held=Held::default();
    None
}
async fn attempt(source:&Live,since:&mut Held)->Result<(),String> {
    if !source.enabled(){return Ok(());}
    let (code,tail)=source.fetch().await?;
    if code!=0 {source.report(Event::FetchFailed(code,tail));}
    let mut reason=source.reason().await?;
    if reason.is_none() {reason=hold(source.busy().await?,since,chrono::Local::now().fixed_offset());} else {*since=Held::default();}
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
    /// Mesma regra do motor de pacote: 24 h para fila e sessão; a entrega incerta conta as dela.
    #[test]
    fn git_engine_deferral_has_the_same_24h_ceiling() {
        let mut held=Held::default();
        assert_eq!(hold(Some(Busy::Queue),&mut held,now()).as_deref(),Some(Busy::Queue.why()));
        assert_eq!(held.since,Some(now()),"o adiamento conta do primeiro");
        let later=now()+chrono::TimeDelta::hours(23);
        assert!(hold(Some(Busy::Working),&mut held,later).is_some());
        let day=now()+chrono::TimeDelta::hours(24);
        assert_eq!(hold(Some(Busy::Working),&mut held.clone(),day),None,"passou de 24 h: atualiza");
        // Adiada há 24 h por outro motivo, a incerta que acaba de aparecer conta as próprias 24 h.
        assert!(hold(Some(Busy::Uncertain),&mut held,day).is_some(),"incerta nova segura");
        assert_eq!(held.uncertain,Some(day));
        assert!(hold(Some(Busy::Uncertain),&mut held,day+chrono::TimeDelta::hours(23)).is_some(),"incerta há 23 h segura");
        assert_eq!(hold(Some(Busy::Uncertain),&mut held,day+chrono::TimeDelta::hours(24)),None,"incerta há 24 h solta");
        assert_eq!(held,Held::default());
        hold(Some(Busy::Queue),&mut held,now());
        assert_eq!(hold(None,&mut held,day),None);
        assert_eq!(held,Held::default(),"livre, o adiamento acaba");
    }
}

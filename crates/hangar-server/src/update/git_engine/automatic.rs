//! Impedimentos do automático: a ordem evita rede e efeitos antes das recusas locais.
use super::{Service,releases::Source,switches};
use serde_json::Value;
use std::time::Duration;
const DIST_SHA_URL:&str="https://github.com/jeffer1312/hangar/releases/download/dist-latest/frontend-dist.sha";
pub fn reason(source:&Checkout<'_>,now:chrono::DateTime<chrono::FixedOffset>)->Result<Option<String>,String>{
    let skip=|why:&str|Ok(Some(why.to_owned()));
    if source.target()?!="main"{return skip("branch de teste configurada (CP_UPDATE_BRANCH)");}
    let pre=source.preflight()?;
    if pre["pode"]!=true{return skip("dependencias faltando");}
    if pre["branch_de_trabalho"]==true{return skip(&format!("checkout na branch {}",pre["branch"].as_str().unwrap_or("")));}
    if source.switches(&pre){return skip(&format!("checkout na branch {}, fora do alvo {}",pre["branch"].as_str().unwrap_or(""),pre["alvo"].as_str().unwrap_or("")));}
    if pre["divergiu"]==true{return skip("checkout divergiu de origin/main");}
    if pre["ahead"].as_u64().unwrap_or(0)>0{return skip("checkout adiante de origin/main (commits locais nao pushados)");}
    if pre["ahead_incerto"]==true{return skip("nao deu pra contar os commits locais");}
    if pre["behind"].as_u64().unwrap_or(0)==0{return skip("em dia");}
    if pre["sujo"].as_u64().unwrap_or(0)>0{return skip("arvore suja (trabalho nao commitado)");}
    let owned=crate::orchestration::codec::ValueGuard(source.state());
    let state=&owned.0;
    if state["fase"]=="rodando"{return skip("atualizacao ja rodando");}
    if state["ok"]==false {
        let at=timestamp(&state["ts"]);
        if at.is_none(){source.invalid_timestamp(&state["ts"]);}
        if at.is_some_and(|at|now-at<chrono::TimeDelta::hours(24)){return skip("ultima atualizacao falhou");}
    }
    let Some(selected)=source.pinned() else{return skip("nao deu pra conferir o binario do Rust publicado");};
    if selected!="origin/main"{return skip("binario do Rust do topo ainda nao publicado para este sistema");}
    let sha=match source.download().ok().and_then(|bytes|String::from_utf8(bytes).ok()){Some(sha)=>sha,None=>return skip("sem acesso ao dist do CI")};
    let (code,head)=source.target_head()?;
    if code!=0||head.trim().is_empty(){return skip("rev-parse origin/main falhou");}
    if sha.trim()!=head.trim(){return skip("dist do CI ainda nao e deste commit");}
    Ok(None)
}
pub struct Checkout<'a>{pub service:&'a Service,pub source:&'a dyn Source,pub platform:Option<&'a str>}
impl Checkout<'_>{
    fn target(&self)->Result<String,String>{self.service.target()}
    fn preflight(&self)->Result<Value,String>{self.service.preflight()}
    fn switches(&self,pre:&Value)->bool{switches(pre["branch"].as_str().unwrap_or(""),pre["alvo"].as_str().unwrap_or("main"))}
    fn state(&self)->Value{self.service.screen_state().into_value()}
    fn pinned(&self)->Option<String>{self.service.pinned_target("main",self.source,self.platform).0}
    fn download(&self)->Result<Vec<u8>,String>{self.source.get(DIST_SHA_URL,Duration::from_secs(15)).map_err(|error|error.message)}
    fn target_head(&self)->Result<(i32,String),String>{let output=self.service.git(&["rev-parse","origin/main"],30)?;Ok((output.code,output.stdout))}
    fn invalid_timestamp(&self,value:&Value){tracing::warn!("{}",invalid_timestamp_notice(value));}
}

fn timestamp(value:&Value)->Option<chrono::DateTime<chrono::FixedOffset>>{
    use std::sync::LazyLock;
    static WEEK:LazyLock<regex::Regex>=LazyLock::new(||regex::Regex::new(r"^(\d{4})-?W(\d{2})(?:-?([1-7]))?").unwrap());
    let text=value.as_str()?;
    let normalized=if let Some(found)=WEEK.captures(text){
        let weekday=match found.get(3).map(|v|v.as_str()).unwrap_or("1") {"1"=>chrono::Weekday::Mon,"2"=>chrono::Weekday::Tue,"3"=>chrono::Weekday::Wed,"4"=>chrono::Weekday::Thu,"5"=>chrono::Weekday::Fri,"6"=>chrono::Weekday::Sat,_=>chrono::Weekday::Sun};
        let date=chrono::NaiveDate::from_isoywd_opt(found[1].parse().ok()?,found[2].parse().ok()?,weekday)?;
        format!("{}{}",date.format("%Y-%m-%d"),&text[found.get(0)?.end()..])
    }else{text.to_owned()};
    let (micros,offset)=crate::transcript::py::fromisoformat(&normalized)?;
    chrono::DateTime::from_timestamp_micros(micros.checked_sub(offset?)?).map(|date|date.fixed_offset())
}

fn timestamp_repr(value:&Value)->String{
    let wrapped=Value::Array(vec![crate::orchestration::codec::clone_value(value)]);
    let text=crate::orchestration::codec::python_string(&wrapped);
    crate::orchestration::codec::discard(wrapped);
    text[1..text.len()-1].to_owned()
}

pub fn invalid_timestamp_notice(value:&Value)->String{
    format!("auto-update: ts invalido no estado da atualizacao ({})",timestamp_repr(value))
}


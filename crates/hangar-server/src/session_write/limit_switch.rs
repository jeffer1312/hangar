//! A sessão Claude que para no limite de uso da conta reabre sozinha na conta de mais folga, pela
//! mesma reabertura do `/conta`, e recebe pela fila um recado para continuar de onde parou.
use std::collections::BTreeMap;
use std::sync::{Arc,Mutex};

use futures_util::future::BoxFuture;
use serde_json::Value;

use crate::routes::AppState;
use crate::runtime::gateway::{LimitHit,RuntimeRegistry};
use crate::runtime::protocol::UsageLimit;
use crate::session_open::{Account,AccountSource,AgentProvider,ChangeReason,SessionSpec};

/// O que fazer com a sessão que bateu o limite.
#[derive(Debug,PartialEq)]
pub enum Decision { Switch { path:String, label:String }, Stay(&'static str) }

/// Contas que já bateram o limite, até a janela delas voltar: não são escolhidas de novo nesse prazo.
/// Gravadas ao lado do cache de cotas: o cache velho depois de um reinício mostraria a conta esgotada com folga.
#[derive(Default)]
pub struct Spent { spent:Mutex<BTreeMap<String,f64>>, file:Option<std::path::PathBuf> }

impl Spent {
    /// Ilegível ou ausente começa vazio: no pior caso a conta esgotada recusa de novo e é marcada outra vez.
    pub fn at(file:std::path::PathBuf) -> Self {
        let spent = std::fs::read(&file).ok().and_then(|raw|serde_json::from_slice(&raw).ok()).unwrap_or_default();
        Self { spent:Mutex::new(spent),file:Some(file) }
    }
}

impl Spent {
    pub fn mark(&self,dir:&str,until:f64) {
        let Ok(mut spent) = self.spent.lock() else { return };
        spent.insert(crate::list::discover::real(dir),until);
        let Some(file) = &self.file else { return };
        let saved = file.parent().map_or(Ok(()),std::fs::create_dir_all)
            .and_then(|()|crate::runtime::queue::atomic_write(file,&serde_json::to_vec(&*spent).unwrap_or_default()));
        if let Err(error) = saved { tracing::warn!(code="limit_spent_write_failed",kind=?error.kind(),"conta gasta não gravada; vale até o reinício"); }
    }
    pub fn is_spent(&self,dir:&str,now:f64) -> bool {
        self.spent.lock().is_ok_and(|spent|spent.get(&crate::list::discover::real(dir)).is_some_and(|until|*until > now))
    }
}

/// A última mensagem da conversa principal no fim do transcript, se for o aviso de limite do plano ainda valendo.
/// `apiErrorParams` é do CLI de hoje; `quotaLimits` recusado, do antigo. O aperto temporário não traz nenhum.
pub fn transcript_limit(tail:&str,now:f64) -> Option<UsageLimit> { transcript_limit_at(tail,now).map(|(limit,_)|limit) }

/// O aviso e a hora em que a linha foi gravada (segundos), quando ela diz.
pub fn transcript_limit_at(tail:&str,now:f64) -> Option<(UsageLimit,Option<f64>)> {
    let last = tail.lines().rev().filter_map(|line|serde_json::from_str::<Value>(line).ok())
        .find(|line|matches!(line["type"].as_str(),Some("user" | "assistant")) && line["isSidechain"] != true)?;
    if last["type"] != "assistant" || last["isApiErrorMessage"] != true { return None; }
    let params = &last["apiErrorParams"]["rate_limit_info"];
    let info = if last["apiError"] == "usage_limit_reached" && params.is_object() { params }
        else if last["apiError"].is_null() && last["error"] == "rate_limit" && last["quotaLimits"]["status"] == "rejected" { &last["quotaLimits"] }
        else { return None };
    let limit = UsageLimit::from_info(info);
    let at = last["timestamp"].as_str().and_then(|ts|chrono::DateTime::parse_from_rfc3339(ts).ok()).map(|ts|ts.timestamp_millis() as f64 / 1000.0);
    limit.resets_at.is_none_or(|reset|reset > now).then_some((limit,at))
}

/// O fim do arquivo basta: depois do aviso só vêm linhas de controle curtas.
const TAIL_BYTES:u64 = 64 * 1024;

pub fn file_limit(path:&std::path::Path,now:f64) -> Option<UsageLimit> { file_limit_at(path,now).map(|(limit,_)|limit) }

pub fn file_limit_at(path:&std::path::Path,now:f64) -> Option<(UsageLimit,Option<f64>)> {
    use std::io::{Read,Seek,SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES))).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    transcript_limit_at(&String::from_utf8_lossy(&tail),now)
}

/// A mesma escolha da abertura (`quotas::suggestion`): a conta Claude lida de mais folga, fora a atual e as
/// gastas nesta janela; cheia (99% ou mais) não serve.
pub fn decide(spec:&SessionSpec,current:&str,catalog:&Value,quotas:&[Value],spent:&Spent,now:f64) -> Decision {
    if spec.provider != AgentProvider::Claude { return Decision::Stay("limit_switch_not_claude"); }
    if matches!(spec.account,Account::Engine{..}) { return Decision::Stay("limit_switch_engine"); }
    let known:Vec<String> = catalog.as_array().into_iter().flatten().filter_map(|row|row["path"].as_str().map(crate::list::discover::real)).collect();
    let current = crate::list::discover::real(current);
    let rows:Vec<Value> = quotas.iter().filter(|row|row["id"].as_str().and_then(|id|id.strip_prefix("claude:")).is_some_and(|path| {
        let path = crate::list::discover::real(path);
        path != current && known.contains(&path) && !spent.is_spent(&path,now)
    })).cloned().collect();
    match crate::accounts::quotas::suggestion(&rows) {
        Some(best) if best["folga"].as_f64().is_some_and(|room|room > 100.0 - super::account::FULL_PCT) =>
            Decision::Switch { path:best["path"].as_str().unwrap_or_default().to_owned(),label:best["label"].as_str().unwrap_or_default().to_owned() },
        _=>Decision::Stay("limit_switch_no_room"),
    }
}

/// Vai à sessão pela fila, depois de reaberta: sem ele a conversa ficaria parada no aviso de limite.
pub const CONTINUE:&str = "Continue de onde parou.";

/// Janela sem reset conhecido fica gasta por cinco horas, a menor das janelas.
const UNKNOWN_RESET_S:f64 = 5.0 * 3600.0;

/// Resultado da reabertura pedida ao servidor.
#[derive(Debug,PartialEq)]
pub enum Reopen { Done, NotRust, Busy(String), Failed(String) }

/// Tentativas da reabertura recusada por sessão ocupada, com `Host::pause` entre elas.
const BUSY_TRIES:usize = 20;

/// O que a troca usa do servidor; nos testes, um falso que registra as chamadas.
pub trait Host: Send + Sync {
    fn spec<'a>(&'a self,name:&'a str) -> BoxFuture<'a,Option<Result<SessionSpec,String>>>;
    /// A pasta de conta Claude de verdade (`None` = a padrão do servidor).
    fn account_dir(&self,config_dir:Option<&str>) -> String;
    fn catalog(&self) -> Result<Value,String>;
    fn quotas(&self) -> BoxFuture<'_,Result<Vec<Value>,String>>;
    fn reopen<'a>(&'a self,name:&'a str,spec:&'a SessionSpec,path:&'a str) -> BoxFuture<'a,Reopen>;
    /// A fila da sessão, segurada desde o aviso de limite, volta a entregar.
    fn release<'a>(&'a self,name:&'a str) -> BoxFuture<'a,()>;
    /// A espera entre duas tentativas com a sessão ocupada.
    fn pause(&self) -> BoxFuture<'_,()>;
    /// Por que a sessão ficou parada, no estado ao vivo dela; `Clear` apaga depois da troca feita.
    fn show_problem<'a>(&'a self,name:&'a str,problem:LimitProblem) -> BoxFuture<'a,()>;
    fn diary(&self,name:&str,code:&str,reason:&'static str);
    fn now(&self) -> f64;
    /// O recado para continuar, pelo mesmo `/input` do app, depois da reabertura e da fila solta: a vida nova
    /// o recebe como mensagem dela. `Ok(true)`: entregue; `Ok(false)`: na fila dela, esperando a vez.
    fn send_continue<'a>(&'a self,name:&'a str) -> BoxFuture<'a,Result<bool,String>>;
    /// Fecha com Esc o menu do Claude Code ao bater o limite, se ele está na tela. `Ok(true)`: fechou.
    fn dismiss_limit_menu<'a>(&'a self,name:&'a str) -> BoxFuture<'a,Result<bool,String>>;
}

/// O menu "What do you want to do?" do Claude Code ao bater o limite, no rodapé do pane.
pub fn limit_menu_open(screen:&str) -> bool {
    // Só o rodapé: o menu que subiu com a conversa já foi respondido.
    let tail:Vec<&str> = screen.trim_end().lines().rev().take(12).collect();
    tail.iter().any(|line|line.contains("What do you want to do?")) && tail.iter().any(|line|line.contains("Stop and wait for limit to reset"))
}

#[derive(Debug,PartialEq)]
pub enum LimitProblem { NoRoom { resets_at:Option<f64> }, Failed { code:String }, Clear }

#[derive(Default)]
struct Switcher {
    spent:Spent,
    /// O último limite tratado por sessão: o mesmo aviso relido não troca de conta duas vezes.
    handled:Mutex<BTreeMap<String,UsageLimit>>,
}

enum Outcome { Switched { label:String, explicit:bool }, Stayed(&'static str), Failed(String), Busy(String) }

impl Switcher {
    fn first_time(&self,name:&str,limit:&UsageLimit) -> bool {
        self.handled.lock().is_ok_and(|mut handled|handled.insert(name.to_owned(),limit.clone()).as_ref() != Some(limit))
    }

    async fn switch(&self,host:&dyn Host,hit:&LimitHit) -> Outcome {
        if hit.provider != "claude" { return Outcome::Stayed("limit_switch_not_claude"); }
        let spec = match host.spec(&hit.name).await {
            None=>return Outcome::Stayed("limit_switch_not_rust"),
            Some(Err(code))=>return Outcome::Failed(code),
            Some(Ok(spec))=>spec,
        };
        if matches!(spec.account,Account::Engine{..}) { return Outcome::Stayed("limit_switch_engine"); }
        let now = host.now();
        let current = host.account_dir(spec.account.config_dir());
        // Aviso de uma vida que já trocou de conta (o `/conta` passou na frente): a conta de agora não bateu nada.
        if crate::list::discover::real(&host.account_dir(hit.account.as_deref())) != crate::list::discover::real(&current) { return Outcome::Stayed("limit_switch_stale"); }
        self.spent.mark(&current,hit.limit.resets_at.filter(|reset|*reset > now).unwrap_or(now + UNKNOWN_RESET_S));
        let catalog = match host.catalog() { Ok(catalog)=>catalog, Err(code)=>return Outcome::Failed(code) };
        let quotas = match host.quotas().await { Ok(quotas)=>quotas, Err(code)=>return Outcome::Failed(code) };
        let (path,label) = match decide(&spec,&current,&catalog,&quotas,&self.spent,now) {
            Decision::Switch { path,label }=>(path,label),
            Decision::Stay(code)=>return Outcome::Stayed(code),
        };
        // A fila está segurada, mas a sessão pode estar terminando o turno ou com uma pergunta aberta.
        let mut tries = 0;
        let refused = loop {
            tries += 1;
            match host.reopen(&hit.name,&spec,&path).await {
                Reopen::NotRust=>break Outcome::Stayed("limit_switch_not_rust"),
                Reopen::Failed(code)=>break Outcome::Failed(code),
                Reopen::Busy(code)=>{
                    host.diary(&hit.name,&super::input::diary_code(&format!("limit_switch_busy:{code}")),"limite de uso: a sessão recusou a reabertura por estar ocupada; a troca tenta de novo");
                    if tries >= BUSY_TRIES { break Outcome::Busy(code); }
                    // Parada no menu do limite, a sessão recusaria até a pessoa responder: o Esc é o Cancelar dele.
                    match host.dismiss_limit_menu(&hit.name).await {
                        Ok(true)=>host.diary(&hit.name,"limit_switch_menu_dismissed","limite de uso: a troca fechou o menu do limite do Claude Code com Esc"),
                        Ok(false)=>{},
                        Err(code)=>host.diary(&hit.name,&super::input::diary_code(&format!("limit_switch_menu_failed:{code}")),"limite de uso: o menu do limite não fechou"),
                    }
                    host.pause().await;
                    // A pessoa pode ter trocado de conta pelo `/conta` na espera: a escolha dela vence.
                    match host.spec(&hit.name).await {
                        None=>break Outcome::Stayed("limit_switch_not_rust"),
                        Some(Err(code))=>break Outcome::Failed(code),
                        Some(Ok(now)) if crate::list::discover::real(&host.account_dir(now.account.config_dir())) != crate::list::discover::real(&current)=>break Outcome::Stayed("limit_switch_stale"),
                        Some(Ok(_))=>{},
                    }
                },
                Reopen::Done=>return Outcome::Switched { label,explicit:spec.account_source == AccountSource::Explicit },
            }
        };
        refused
    }

    async fn handle(&self,host:&dyn Host,hit:LimitHit) {
        let outcome = if self.first_time(&hit.name,&hit.limit) { Some(self.switch(host,&hit).await) } else { None };
        // Todo aviso segura a fila, inclusive o repetido: qualquer fim a solta.
        host.release(&hit.name).await;
        let Some(outcome) = outcome else {
            host.diary(&hit.name,"limit_switch_repeated","limite de uso: o mesmo aviso já tratado chegou de novo e não troca outra vez");
            return;
        };
        let report = |code:&str,reason:&'static str|host.diary(&hit.name,&super::input::diary_code(code),reason);
        // O recado que não saiu: a sessão segue parada no aviso, e a linha mostra isso em vez de limpar.
        let mut continue_failed = None;
        // Só o aviso que não pede troca fica guardado. Na conta nova o limite é outro (mesmo com o mesmo reset
        // arredondado); a troca que falhou, sem folga ou ocupada tenta de novo no próximo turno parado, e o aviso
        // velho relido é barrado pela conta gasta e pelo `limit_switch_stale`.
        // A conta trocada pela pessoa no meio da espera também esquece: o limite dela é outro.
        if !matches!(outcome,Outcome::Stayed(code) if !matches!(code,"limit_switch_no_room" | "limit_switch_stale"))
            && let Ok(mut handled) = self.handled.lock() {
            handled.remove(&hit.name);
        }
        match &outcome {
            Outcome::Switched { label,explicit } => {
                tracing::info!(session=%hit.name,window=%hit.limit.window,to=%label,explicit,"limite de uso: sessão reaberta em outra conta");
                report("switched","limite de uso: a sessão reabriu na conta de mais folga");
                // O limite vence a conta escolhida pela pessoa, e isso fica registrado.
                if *explicit { report("explicit_account","limite de uso: a conta escolhida pela pessoa foi trocada"); }
                // Depois da fila solta (acima): retida, a vida nova não entregaria nada.
                match host.send_continue(&hit.name).await {
                    Ok(true) => report("limit_switch_continue_delivered","limite de uso: a vida nova recebeu o recado para continuar"),
                    Ok(false) => report("limit_switch_continue_pending","limite de uso: o recado para continuar está na fila da vida nova, esperando a vez"),
                    Err(code) => {
                        tracing::warn!(session=%hit.name,%code,"limite de uso: o recado para continuar não foi enviado");
                        let code = format!("limit_switch_continue_failed:{code}");
                        report(&code,"limite de uso: o recado para continuar não foi enviado à vida nova");
                        continue_failed = Some(code);
                    }
                }
            }
            Outcome::Stayed("limit_switch_no_room") => {
                tracing::warn!(session=%hit.name,window=%hit.limit.window,"limite de uso: nenhuma conta com folga; a sessão fica parada");
                report("limit_switch_no_room","limite de uso: nenhuma conta Claude com folga; a sessão fica parada até o reset");
            }
            Outcome::Stayed(code @ "limit_switch_not_rust") => {
                tracing::warn!(session=%hit.name,code,"limite de uso: a sessão saiu do Rust antes da troca");
                report(code,"limite de uso: a sessão fechou ou mudou de nome antes da troca de conta");
            }
            Outcome::Stayed(code) => {
                tracing::info!(session=%hit.name,code,"limite de uso sem troca de conta");
                report(code,"limite de uso: a sessão não troca de conta por este motivo");
            }
            Outcome::Failed(code) => {
                tracing::warn!(session=%hit.name,%code,"limite de uso: a troca de conta falhou");
                report(&code,"limite de uso: a troca de conta falhou; a sessão fica parada");
            }
            Outcome::Busy(code) => {
                tracing::warn!(session=%hit.name,%code,"limite de uso: a sessão seguiu ocupada; a troca fica para o próximo aviso");
                report(&code,"limite de uso: a sessão seguiu ocupada e a troca de conta não aconteceu");
            }
        }
        let problem = match &outcome {
            Outcome::Switched { .. }=>Some(continue_failed.map_or(LimitProblem::Clear,|code|LimitProblem::Failed { code })),
            Outcome::Stayed("limit_switch_no_room")=>Some(LimitProblem::NoRoom { resets_at:hit.limit.resets_at }),
            Outcome::Failed(code) | Outcome::Busy(code)=>Some(LimitProblem::Failed { code:code.clone() }),
            Outcome::Stayed(_)=>None,
        };
        if let Some(problem) = problem { host.show_problem(&hit.name,problem).await; }
    }
}

/// O vigia que recebe do runtime as sessões paradas no limite, uma troca por vez: duas sessões batendo o
/// limite juntas não escolhem a mesma conta sem ver a outra gasta.
pub fn spawn(st:Arc<AppState>) -> Option<tokio::task::JoinHandle<()>> {
    let runtime = st.state.runtime.get()?.clone();
    let (sink,mut hits) = tokio::sync::mpsc::unbounded_channel();
    runtime.set_limit_sink(sink);
    let switcher = Switcher { spent:Spent::at(st.accounts.quota_path().with_file_name("limit-spent.json")),..Switcher::default() };
    let host = Native { st,runtime };
    Some(tokio::spawn(async move {
        while let Some(hit) = hits.recv().await { switcher.handle(&host,hit).await; }
    }))
}

struct Native { st:Arc<AppState>, runtime:Arc<RuntimeRegistry> }

impl Host for Native {
    fn dismiss_limit_menu<'a>(&'a self,name:&'a str) -> BoxFuture<'a,Result<bool,String>> {
        Box::pin(async move { self.runtime.dismiss_claude_limit_menu(name).await.map_err(|error|error.code) })
    }
    fn spec<'a>(&'a self,name:&'a str) -> BoxFuture<'a,Option<Result<SessionSpec,String>>> {
        Box::pin(async move { self.runtime.claude_spec(name).await.map(|spec|spec.map_err(|error|error.code)) })
    }
    fn account_dir(&self,config_dir:Option<&str>) -> String {
        config_dir.map_or_else(||self.runtime.codex_environment().claude_base.to_string_lossy().into_owned(),str::to_owned)
    }
    fn catalog(&self) -> Result<Value,String> { self.st.accounts.claude_catalog().map_err(|error|error.code.into()) }
    fn quotas(&self) -> BoxFuture<'_,Result<Vec<Value>,String>> {
        Box::pin(async move {
            let quotas = self.st.accounts.quotas(None,false,true).await.map_err(|error|error.code.to_string())?;
            Ok(quotas.as_array().into_iter().flatten().filter(|row|row["provedor"]=="claude").cloned().collect())
        })
    }
    fn reopen<'a>(&'a self,name:&'a str,spec:&'a SessionSpec,path:&'a str) -> BoxFuture<'a,Reopen> {
        Box::pin(async move {
            match super::account::reopen_in(&self.st,&self.runtime,name,spec,path,false,ChangeReason::LimitSwitch).await {
                None=>Reopen::NotRust,
                Some(Err(failure)) if !failure.stopped && (super::busy_refusal(&failure.error).is_some() || failure.error.code=="lifecycle_busy") => Reopen::Busy(failure.error.code),
                Some(Err(failure))=>Reopen::Failed(failure.error.code),
                Some(Ok(_))=>Reopen::Done,
            }
        })
    }
    fn release<'a>(&'a self,name:&'a str) -> BoxFuture<'a,()> { self.runtime.release_limit(name); Box::pin(async {}) }
    fn pause(&self) -> BoxFuture<'_,()> { Box::pin(tokio::time::sleep(std::time::Duration::from_secs(3))) }
    fn show_problem<'a>(&'a self,name:&'a str,problem:LimitProblem) -> BoxFuture<'a,()> {
        let shown = match problem {
            LimitProblem::NoRoom { resets_at }=>Some(("limit_switch_no_room".to_owned(),
                resets_at.and_then(|at|crate::runtime::local_policy::local_hhmm(at.floor() as i64)).unwrap_or_default())),
            LimitProblem::Failed { code }=>Some(("limit_switch_failed".to_owned(),code)),
            LimitProblem::Clear=>None,
        };
        Box::pin(self.runtime.show_limit_problem(name,shown))
    }
    fn diary(&self,name:&str,code:&str,reason:&'static str) { self.st.diag.report("session.limit_switch",name,code,reason); }
    fn now(&self) -> f64 { crate::state::monitor::wall_now() }
    fn send_continue<'a>(&'a self,name:&'a str) -> BoxFuture<'a,Result<bool,String>> {
        Box::pin(async move {
            let (status,reply) = super::input::write_input(&self.st,name,CONTINUE,false).await;
            if status.is_success() { return Ok(reply["delivered"] == true); }
            Err(reply["detail"]["code"].as_str().unwrap_or("limit_switch_continue_refused").to_owned())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::session_open::{AccountSource,SPEC_VERSION,Surface};

    const FIVE_HOUR:&str = include_str!("../../tests/it/fixtures/usage_limit/claude_five_hour_sdk.jsonl");
    const SEVEN_DAY:&str = include_str!("../../tests/it/fixtures/usage_limit/claude_seven_day_tui.jsonl");
    const OLD_CLI:&str = include_str!("../../tests/it/fixtures/usage_limit/claude_old_cli.jsonl");

    #[test]
    fn the_limit_line_at_the_end_of_the_transcript_gives_window_and_reset() {
        assert_eq!(transcript_limit(FIVE_HOUR,1791570000.0),Some(UsageLimit { window:"five_hour".into(),resets_at:Some(1791577800.0) }));
        assert_eq!(transcript_limit(SEVEN_DAY,1791900000.0),Some(UsageLimit { window:"seven_day".into(),resets_at:Some(1791928800.0) }));
        assert_eq!(transcript_limit(OLD_CLI,1789560000.0),Some(UsageLimit { window:"five_hour".into(),resets_at:Some(1789566600.0) }));
        // O limite semanal do CLI de 10/2026, entre `api-request` e `turn_duration`.
        let weekly = include_str!("../../tests/it/fixtures/usage_limit/claude_weekly_limit_tui_2026_10.jsonl");
        assert_eq!(transcript_limit(weekly,1792249428.0),Some(UsageLimit { window:"seven_day".into(),resets_at:Some(1792270800.0) }));
    }

    #[test]
    fn bookkeeping_lines_after_the_limit_do_not_hide_it() {
        let tail = format!("{FIVE_HOUR}{}\n{}\n{}\n",
            json!({"type":"last-prompt","lastPrompt":"x","sessionId":"s"}),
            json!({"type":"assistant","isSidechain":true,"message":{"content":[]}}),
            json!({"type":"queue-operation","operation":"enqueue"}));
        assert!(transcript_limit(&tail,1791570000.0).is_some());
    }

    #[test]
    fn a_passed_reset_or_a_later_message_is_not_the_current_limit() {
        assert_eq!(transcript_limit(FIVE_HOUR,1791577801.0),None,"o reset já passou");
        let later = format!("{FIVE_HOUR}{}\n",json!({"type":"user","isSidechain":false,"message":{"content":"segue"}}));
        assert_eq!(transcript_limit(&later,1791570000.0),None,"a conversa seguiu depois do aviso");
        let throttle = json!({"type":"assistant","isSidechain":false,"isApiErrorMessage":true,"error":"rate_limit",
            "message":{"content":[{"type":"text","text":"API Error: rate limited"}]}}).to_string();
        assert_eq!(transcript_limit(&throttle,1791570000.0),None,"aperto temporário não é a cota");
        let mut other:Value = serde_json::from_str(FIVE_HOUR.trim()).unwrap();
        other["apiError"] = json!("overloaded");
        assert_eq!(transcript_limit(&other.to_string(),1791570000.0),None,"a reserva da cota recusada é só do CLI sem apiError");
        assert_eq!(transcript_limit("{\"type\":\"assistant\",\"isSidechain\":false,tor",1791570000.0),None);
    }

    fn spec(account:Account) -> SessionSpec {
        SessionSpec { version:SPEC_VERSION,key:"k".into(),name:"s".into(),cwd:"/w".into(),provider:AgentProvider::Claude,
            surface:Surface::Headless,account,account_source:AccountSource::Default,model:None,effort:None,service_tier:None,
            permission_mode:None,subagent_model:None,customizations:None,jev:false,read_only:false,worktree:None,creator:None,
            created_at:1.0,conversation:None,name_auto:None,rename_suggestion:None,rename_dismissed:false }
    }
    fn catalog(paths:&[&str]) -> Value { Value::Array(paths.iter().map(|p|json!({"path":p,"label":p.trim_start_matches('/')})).collect()) }
    fn quota(path:&str,pct:f64) -> Value { json!({"provedor":"claude","id":format!("claude:{path}"),"label":path.trim_start_matches('/'),"estado":"lida","janelas":[{"pct":pct},{"pct":pct/2.0}]}) }
    fn claude(dir:&str) -> Account { Account::Claude { config_dir:dir.into() } }

    #[test]
    fn the_most_room_wins_and_the_spent_account_never_comes_back_in_its_window() {
        let spent = Spent::default();
        let cat = catalog(&["/a","/b","/c"]);
        // O cache ainda mostra a conta que acabou de bater o limite com folga: ela nunca é destino.
        let quotas = [quota("/a",10.0),quota("/b",40.0),quota("/c",20.0)];
        spent.mark("/a",2000.0);
        assert_eq!(decide(&spec(claude("/a")),"/a",&cat,&quotas,&spent,1000.0),Decision::Switch { path:"/c".into(),label:"c".into() });
        // Na conta nova o limite bate de novo: a primeira continua gasta, sobra a outra.
        spent.mark("/c",2000.0);
        assert_eq!(decide(&spec(claude("/c")),"/c",&cat,&quotas,&spent,1000.0),Decision::Switch { path:"/b".into(),label:"b".into() });
        spent.mark("/b",2000.0);
        assert_eq!(decide(&spec(claude("/b")),"/b",&cat,&quotas,&spent,1000.0),Decision::Stay("limit_switch_no_room"));
        // Passado o reset, as contas voltam a valer, e a de mais folga é a primeira.
        assert_eq!(decide(&spec(claude("/b")),"/b",&cat,&quotas,&spent,2001.0),Decision::Switch { path:"/a".into(),label:"a".into() });
    }

    #[test]
    fn a_spent_account_stays_spent_after_a_restart_until_its_reset() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("limit-spent.json");
        let before = Spent::at(file.clone());
        before.mark("/a",2000.0);
        before.mark("/b",500.0);
        let after = Spent::at(file.clone());
        assert!(after.is_spent("/a",1000.0),"o reinício não devolve a conta esgotada");
        assert!(!after.is_spent("/a",2001.0));
        assert!(!after.is_spent("/b",1000.0));
        std::fs::write(&file,"não é json").unwrap();
        assert!(!Spent::at(file).is_spent("/a",1000.0),"arquivo ilegível começa vazio");
    }

    #[test]
    fn full_unread_or_unknown_accounts_are_no_room() {
        let spent = Spent::default();
        let cat = catalog(&["/a","/b","/c"]);
        let unread = json!({"provedor":"claude","id":"claude:/c","label":"c","estado":"erro"});
        let quotas = [quota("/a",10.0),quota("/b",99.5),unread,quota("/z",0.0)];
        assert_eq!(decide(&spec(claude("/a")),"/a",&cat,&quotas,&spent,1000.0),Decision::Stay("limit_switch_no_room"));
    }

    #[test]
    fn the_same_limit_read_again_does_not_switch_twice() {
        let switcher = Switcher::default();
        let limit = UsageLimit { window:"five_hour".into(),resets_at:Some(1000.0) };
        assert!(switcher.first_time("s",&limit));
        assert!(!switcher.first_time("s",&limit),"o plugin e o ator podem avisar o mesmo fim de turno");
        assert!(switcher.first_time("s",&UsageLimit { window:"five_hour".into(),resets_at:Some(2000.0) }),"a conta nova bateu o dela");
        assert!(switcher.first_time("other",&limit));
    }

    #[tokio::test]
    async fn the_runtime_hands_the_stopped_session_to_the_switcher() {
        let runtime = crate::runtime::gateway::RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"s".into(),"i".into());
        let limit = UsageLimit { window:"seven_day".into(),resets_at:None };
        let make = |name:&str|LimitHit { name:name.into(),provider:"codex".into(),account:None,limit:limit.clone() };
        runtime.usage_limit(make("lost"));
        let (sink,mut hits) = tokio::sync::mpsc::unbounded_channel();
        runtime.set_limit_sink(sink);
        runtime.usage_limit(make("s1"));
        let hit = hits.try_recv().unwrap();
        assert_eq!((hit.name.as_str(),hit.provider.as_str(),hit.limit),("s1","codex",limit));
        assert!(hits.try_recv().is_err());
    }

    struct Fake {
        spec:Mutex<SessionSpec>,
        catalog:Value,
        quotas:Mutex<Result<Vec<Value>,String>>,
        /// Respostas da reabertura, na ordem; vazia = feita.
        reopens:Mutex<Vec<Reopen>>,
        log:Mutex<Vec<String>>,
        diary:Mutex<Vec<String>>,
        problems:Mutex<Vec<LimitProblem>>,
        /// A sessão fechou ou foi renomeada antes da troca.
        gone:std::sync::atomic::AtomicBool,
        /// A pessoa troca a conta pelo `/conta` durante a espera entre tentativas.
        on_pause:Mutex<Option<String>>,
        /// A vida nova nunca recebe o recado.
        undelivered:std::sync::atomic::AtomicBool,
        /// O `/input` do recado recusa.
        continue_fails:std::sync::atomic::AtomicBool,
        /// A tela do pane: com o menu do limite aberto, a reabertura recusa até ele fechar.
        screen:Mutex<String>,
    }

    impl Fake {
        fn new(dir:&str,quotas:Vec<Value>) -> Self {
            Fake { spec:Mutex::new(spec(claude(dir))),catalog:catalog(&["/a","/b","/c"]),quotas:Mutex::new(Ok(quotas)),
                reopens:Mutex::new(Vec::new()),log:Mutex::new(Vec::new()),diary:Mutex::new(Vec::new()),problems:Mutex::new(Vec::new()),gone:Default::default(),
                on_pause:Mutex::new(None),undelivered:Default::default(),continue_fails:Default::default(),screen:Mutex::new(String::new()) }
        }
        fn log(&self) -> Vec<String> { self.log.lock().unwrap().clone() }
        fn diary(&self) -> Vec<String> { self.diary.lock().unwrap().clone() }
    }

    impl Host for Fake {
        fn spec<'a>(&'a self,_:&'a str) -> BoxFuture<'a,Option<Result<SessionSpec,String>>> {
            Box::pin(async move { (!self.gone.load(std::sync::atomic::Ordering::Relaxed)).then(||Ok(self.spec.lock().unwrap().clone())) })
        }
        fn account_dir(&self,config_dir:Option<&str>) -> String { config_dir.unwrap_or("/default").to_owned() }
        fn catalog(&self) -> Result<Value,String> { Ok(self.catalog.clone()) }
        fn quotas(&self) -> BoxFuture<'_,Result<Vec<Value>,String>> { Box::pin(async move { self.quotas.lock().unwrap().clone() }) }
        fn reopen<'a>(&'a self,_:&'a str,_:&'a SessionSpec,path:&'a str) -> BoxFuture<'a,Reopen> {
            Box::pin(async move {
                self.log.lock().unwrap().push(format!("reopen {path}"));
                let mut reopens = self.reopens.lock().unwrap();
                let outcome = if reopens.is_empty() { Reopen::Done } else { reopens.remove(0) };
                if outcome == Reopen::Done { *self.spec.lock().unwrap() = spec(claude(path)); }
                outcome
            })
        }
        fn release<'a>(&'a self,_:&'a str) -> BoxFuture<'a,()> { Box::pin(async move { self.log.lock().unwrap().push("release".into()); }) }
        fn pause(&self) -> BoxFuture<'_,()> {
            Box::pin(async move {
                self.log.lock().unwrap().push("pause".into());
                if let Some(dir) = self.on_pause.lock().unwrap().take() { *self.spec.lock().unwrap() = spec(claude(&dir)); }
            })
        }
        fn show_problem<'a>(&'a self,_:&'a str,problem:LimitProblem) -> BoxFuture<'a,()> {
            Box::pin(async move { self.problems.lock().unwrap().push(problem); })
        }
        fn diary(&self,_:&str,code:&str,_:&'static str) { self.diary.lock().unwrap().push(code.to_owned()); }
        fn now(&self) -> f64 { 1000.0 }
        fn dismiss_limit_menu<'a>(&'a self,_:&'a str) -> BoxFuture<'a,Result<bool,String>> {
            Box::pin(async move {
                let mut screen = self.screen.lock().unwrap();
                if !limit_menu_open(&screen) { return Ok(false); }
                self.log.lock().unwrap().push("esc".into());
                screen.clear();
                // Fechado o menu, a sessão para de recusar.
                self.reopens.lock().unwrap().retain(|r|!matches!(r,Reopen::Busy(code) if code=="erro_sessao_esperando_resposta"));
                Ok(true)
            })
        }
        fn send_continue<'a>(&'a self,_:&'a str) -> BoxFuture<'a,Result<bool,String>> {
            Box::pin(async move {
                self.log.lock().unwrap().push("continue".into());
                if self.continue_fails.load(std::sync::atomic::Ordering::Relaxed) { return Err("erro_sessao_trabalhando".into()); }
                Ok(!self.undelivered.load(std::sync::atomic::Ordering::Relaxed))
            })
        }
    }

    fn hit(account:&str,reset:f64) -> LimitHit {
        LimitHit { name:"s".into(),provider:"claude".into(),account:Some(account.into()),limit:UsageLimit { window:"five_hour".into(),resets_at:Some(reset) } }
    }

    #[tokio::test]
    async fn the_stopped_session_reopens_in_the_roomiest_account_and_is_told_to_continue() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/b",60.0),quota("/c",20.0)]);
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.log(),vec!["reopen /c","release","continue"],"reabre, solta a fila e só então manda o recado");
        assert_eq!(fake.diary(),vec!["switched","limit_switch_continue_delivered"]);
    }

    /// "Recebeu o recado" só com a entrega confirmada; sem ela, o diário diz que está na fila da vida nova.
    #[tokio::test]
    async fn the_continue_note_counts_only_once_delivered() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/b",60.0),quota("/c",20.0)]);
        fake.undelivered.store(true,std::sync::atomic::Ordering::Relaxed);
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.diary(),vec!["switched","limit_switch_continue_pending"]);
    }

    /// O recado que não sai deixa a sessão parada no aviso de limite: a linha mostra isso, em vez de limpar.
    #[tokio::test]
    async fn a_continue_note_that_fails_shows_on_the_session_row() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        fake.continue_fails.store(true,std::sync::atomic::Ordering::Relaxed);
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(*fake.problems.lock().unwrap(),vec![LimitProblem::Failed { code:"limit_switch_continue_failed:erro_sessao_trabalhando".into() }]);
    }

    #[tokio::test]
    async fn the_new_account_hitting_a_limit_with_the_same_reset_switches_again() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/b",60.0),quota("/c",20.0)]);
        let switcher = Switcher::default();
        switcher.handle(&fake,hit("/a",2000.0)).await;
        // O reset é arredondado: a conta nova bate o limite dela com o mesmo `resetsAt`.
        switcher.handle(&fake,hit("/c",2000.0)).await;
        let reopens:Vec<String> = fake.log().into_iter().filter(|line|line.starts_with("reopen")).collect();
        assert_eq!(reopens,vec!["reopen /c","reopen /b"]);
    }

    #[tokio::test]
    async fn a_late_limit_from_the_account_the_session_already_left_is_dropped() {
        let fake = Fake::new("/b",vec![quota("/a",100.0),quota("/b",60.0),quota("/c",20.0)]);
        let switcher = Switcher::default();
        // O aviso veio da conta /a, mas a pessoa já trocou a sessão para /b pelo /conta.
        switcher.handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.log(),vec!["release"]);
        assert!(!switcher.spent.is_spent("/b",1000.0),"a conta atual não bateu limite nenhum");
    }

    /// O menu como o Claude Code 2.1.302 o desenha ao bater o limite (opções do binário).
    const LIMIT_MENU:&str = "  ⎿  You've hit your session limit · resets 2pm\n\n╭──────────────────────────────────────────────╮\n│ What do you want to do?                      │\n│                                              │\n│ ❯ 1. Stop and wait for limit to reset        │\n│   2. Wait here, then continue automatically when the limit resets │\n│   3. Upgrade your plan                       │\n╰──────────────────────────────────────────────╯\n";

    #[test]
    fn the_limit_menu_is_recognised_only_on_the_bottom_of_the_pane() {
        assert!(limit_menu_open(LIMIT_MENU));
        assert!(!limit_menu_open("❯ \n"));
        let scrolled = format!("{LIMIT_MENU}{}", "linha da conversa\n".repeat(30));
        assert!(!limit_menu_open(&scrolled),"o menu que subiu com a conversa já foi respondido");
    }

    /// A sessão parada no menu do limite recusa a reabertura com pergunta aberta: a troca fecha o menu com Esc
    /// (o Cancelar, nunca a opção de créditos) e reabre, sem esperar a pessoa. Cada recusa vai ao diário.
    #[tokio::test]
    async fn the_switch_closes_the_limit_menu_itself_and_reopens() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        *fake.screen.lock().unwrap() = LIMIT_MENU.into();
        fake.reopens.lock().unwrap().extend((0..BUSY_TRIES).map(|_|Reopen::Busy("erro_sessao_esperando_resposta".into())));
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.log(),vec!["reopen /c","esc","pause","reopen /c","release","continue"]);
        assert_eq!(fake.diary(),vec!["limit_switch_busy_erro_sessao_esperando_resposta","limit_switch_menu_dismissed","switched","limit_switch_continue_delivered"]);
    }

    #[tokio::test]
    async fn a_busy_session_is_tried_again_and_the_queue_is_let_go_only_at_the_end() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        fake.reopens.lock().unwrap().push(Reopen::Busy("erro_sessao_trabalhando".into()));
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.log(),vec!["reopen /c","pause","reopen /c","release","continue"]);
        assert_eq!(fake.diary(),vec!["limit_switch_busy_erro_sessao_trabalhando","switched","limit_switch_continue_delivered"]);
    }

    #[tokio::test]
    async fn a_switch_that_does_not_happen_sends_no_continue_note() {
        for outcome in [Reopen::Failed("erro_troca".into()),Reopen::NotRust] {
            let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
            fake.reopens.lock().unwrap().push(outcome);
            Switcher::default().handle(&fake,hit("/a",2000.0)).await;
            assert_eq!(fake.log(),vec!["reopen /c","release"]);
        }
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        fake.reopens.lock().unwrap().extend((0..BUSY_TRIES).map(|_|Reopen::Busy("erro_sessao_trabalhando".into())));
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.log().last().map(String::as_str),Some("release"));
        assert!(!fake.log().contains(&"continue".to_owned()));
    }

    /// Troca que falhou não marca o aviso como tratado: o mesmo aviso, relido, tenta de novo.
    #[tokio::test]
    async fn a_failed_switch_tries_again_with_the_same_notice() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        fake.reopens.lock().unwrap().push(Reopen::Failed("erro_troca".into()));
        let switcher = Switcher::default();
        switcher.handle(&fake,hit("/a",2000.0)).await;
        switcher.handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.log().iter().filter(|l|*l=="reopen /c").count(),2);
        assert_eq!(fake.diary().last().map(String::as_str),Some("limit_switch_continue_delivered"));
    }

    #[tokio::test]
    async fn a_session_busy_to_the_end_is_not_marked_handled_and_lets_the_queue_go() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        fake.reopens.lock().unwrap().extend((0..BUSY_TRIES).map(|_|Reopen::Busy("erro_sessao_esperando_resposta".into())));
        let switcher = Switcher::default();
        switcher.handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.log().last().map(String::as_str),Some("release"));
        let busy = "limit_switch_busy_erro_sessao_esperando_resposta";
        assert_eq!(fake.diary().iter().filter(|code|*code==busy).count(),BUSY_TRIES,"cada recusa vai ao diário");
        assert_eq!(fake.diary().last().map(String::as_str),Some("erro_sessao_esperando_resposta"));
        // O mesmo limite, quando a sessão voltar a parar, ainda troca.
        switcher.handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.diary().last().map(String::as_str),Some("limit_switch_continue_delivered"));
    }

    #[tokio::test]
    async fn every_ending_lets_the_queue_go() {
        let fake = Fake::new("/a",vec![quota("/a",100.0)]);
        let switcher = Switcher::default();
        switcher.handle(&fake,hit("/a",2000.0)).await;
        switcher.handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.log(),vec!["release","release"],"sem folga e o aviso repetido também soltam a fila");
    }

    #[tokio::test]
    async fn the_session_shows_why_it_stayed_stopped_and_a_switch_clears_it() {
        let problems = |fake:&Fake|std::mem::take(&mut *fake.problems.lock().unwrap());
        let fake = Fake::new("/a",vec![quota("/a",100.0)]);
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(problems(&fake),vec![LimitProblem::NoRoom { resets_at:Some(2000.0) }]);
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        fake.reopens.lock().unwrap().push(Reopen::Failed("erro_troca".into()));
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(problems(&fake),vec![LimitProblem::Failed { code:"erro_troca".into() }]);
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(problems(&fake),vec![LimitProblem::Clear]);
        let fake = Fake::new("/b",vec![quota("/c",20.0)]);
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert!(problems(&fake).is_empty(),"aviso velho não mexe no estado");
    }

    #[tokio::test]
    async fn a_session_gone_before_the_switch_goes_to_the_diary() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        fake.gone.store(true,std::sync::atomic::Ordering::Relaxed);
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.diary(),vec!["limit_switch_not_rust"]);
        fake.gone.store(false,std::sync::atomic::Ordering::Relaxed);
        fake.reopens.lock().unwrap().push(Reopen::NotRust);
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.diary(),vec!["limit_switch_not_rust","limit_switch_not_rust"]);
    }

    #[tokio::test]
    async fn an_unreadable_quota_is_a_failure_with_its_code_not_no_room() {
        let fake = Fake::new("/a",Vec::new());
        *fake.quotas.lock().unwrap() = Err("quota_read_failed".into());
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.log(),vec!["release"]);
        assert_eq!(fake.diary(),vec!["quota_read_failed"]);
    }

    /// SF5: sem o vigia para receber o aviso, a fila não fica segurada para sempre.
    #[tokio::test]
    async fn a_limit_the_switcher_never_receives_does_not_hold_the_queue() {
        let runtime = crate::runtime::gateway::RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(),"s".into(),"i".into());
        let (sink,hits) = tokio::sync::mpsc::unbounded_channel();
        runtime.set_limit_sink(sink);
        drop(hits);
        runtime.usage_limit(LimitHit { name:"s1".into(),provider:"claude".into(),account:None,limit:UsageLimit { window:"five_hour".into(),resets_at:None } });
        assert!(!runtime.limit_held("s1"));
    }

    /// RR1: a troca que falhou ou não achou folga não guarda o limite: o próximo turno parado tenta de novo.
    #[tokio::test]
    async fn a_failed_or_roomless_switch_is_tried_again_on_the_next_stop() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        fake.reopens.lock().unwrap().push(Reopen::Failed("erro_troca".into()));
        let switcher = Switcher::default();
        switcher.handle(&fake,hit("/a",2000.0)).await;
        switcher.handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.diary().last().map(String::as_str),Some("limit_switch_continue_delivered"),"{:?}",fake.diary());
        let fake = Fake::new("/a",vec![quota("/a",100.0)]);
        let switcher = Switcher::default();
        switcher.handle(&fake,hit("/a",2000.0)).await;
        *fake.quotas.lock().unwrap() = Ok(vec![quota("/a",100.0),quota("/b",10.0)]);
        switcher.handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.diary(),vec!["limit_switch_no_room","switched","limit_switch_continue_delivered"]);
    }

    /// RR2: a pessoa que trocou de conta durante as tentativas vence; a tentativa seguinte não desfaz a escolha.
    #[tokio::test]
    async fn an_account_chosen_while_the_switch_waits_is_not_undone() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        fake.reopens.lock().unwrap().push(Reopen::Busy("erro_sessao_trabalhando".into()));
        *fake.on_pause.lock().unwrap() = Some("/b".into());
        Switcher::default().handle(&fake,hit("/a",2000.0)).await;
        assert_eq!(fake.log(),vec!["reopen /c","pause","release"]);
    }

    /// A conta que a pessoa escolheu durante a espera bate o limite com o mesmo reset arredondado: troca.
    #[tokio::test]
    async fn the_account_chosen_while_waiting_switches_when_it_hits_the_same_reset() {
        let fake = Fake::new("/a",vec![quota("/a",100.0),quota("/c",20.0)]);
        fake.reopens.lock().unwrap().push(Reopen::Busy("erro_sessao_trabalhando".into()));
        *fake.on_pause.lock().unwrap() = Some("/b".into());
        let switcher = Switcher::default();
        switcher.handle(&fake,hit("/a",2000.0)).await;
        *fake.quotas.lock().unwrap() = Ok(vec![quota("/a",100.0),quota("/b",100.0),quota("/c",20.0)]);
        switcher.handle(&fake,hit("/b",2000.0)).await;
        assert_eq!(fake.diary().last().map(String::as_str),Some("limit_switch_continue_delivered"),"{:?}",fake.diary());
    }

    #[test]
    fn engine_and_other_providers_stay() {
        let spent = Spent::default();
        let (cat,quotas) = (catalog(&["/a","/b"]),[quota("/a",10.0),quota("/b",10.0)]);
        let engine = Account::Engine { name:"kimi".into(),account:None,config_dir:None,context_window:None,context_window_unknown:false };
        assert_eq!(decide(&spec(engine),"/a",&cat,&quotas,&spent,1000.0),Decision::Stay("limit_switch_engine"));
        let mut codex = spec(claude("/a"));
        codex.provider = AgentProvider::Codex;
        assert_eq!(decide(&codex,"/a",&cat,&quotas,&spent,1000.0),Decision::Stay("limit_switch_not_claude"));
    }
}

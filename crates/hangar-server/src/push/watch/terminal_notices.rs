//! Produtores dos avisos de Pi, omp e Kimi; estado e entrega têm fontes substituíveis na prova.
use futures_util::future::BoxFuture;
use hangar_api::session::SessionRow;
use std::time::Duration;
use crate::terminal_state::PaneAnalysis;

pub(super) fn owns(provider:&str)->bool {matches!(provider,"pi"|"omp"|"kimi")}

pub(super) trait Sources:Send+Sync {
    fn question(&self,row:&SessionRow)->Option<String>;
    fn pane<'a>(&'a self,row:&'a SessionRow)->BoxFuture<'a,Result<PaneAnalysis,&'static str>>;
    fn sleep(&self,duration:Duration)->BoxFuture<'_,()>;
    fn awaiting(&self,name:&str,body:Option<String>);
}

pub(super) async fn awaiting(source:&dyn Sources,row:SessionRow) {
    if !owns(&row.provider){return;}
    for attempt in 0..2 {
        if attempt>0 {source.sleep(super::AWAITING_RETRY).await;}
        let real=source.question(&row).is_some() || source.pane(&row).await.map_or(true,|pane|pane.state=="awaiting_input"||pane.overlay);
        if real {
            let body=match source.question(&row) {
                Some(question)=>Some(question),
                None=>source.pane(&row).await.ok().and_then(|pane|pane.question).filter(|text|!text.is_empty()),
            };
            source.awaiting(&row.name,body);return;
        }
    }
}

struct Captured {text:String,error:Option<String>}
async fn capture_text(program:&std::ffi::OsStr,name:&str,target:Option<String>,env:crate::accounts::environment::AccountEnvironment,budget:Duration)->Captured {
    let target=target.unwrap_or_else(||format!("={name}:"));
    let args=vec![program.to_string_lossy().into_owned(),"capture-pane".into(),"-p".into(),"-t".into(),target,"-S".into(),"-200".into()];
    match crate::command_capture::capture(args,env,budget).await {
        Ok(output)=>Captured {text:output.stdout,error:(!output.success).then(||"capture_returned_failure".into())},
        // O capturador Python devolve saída vazia ao converter falha de processo em retorno.
        Err(error)=>Captured {text:String::new(),error:Some(error)},
    }
}

impl Sources for super::Watch {
    fn question(&self,row:&SessionRow)->Option<String> {
        let sid=row.jsonl.as_deref().map(crate::locate::session_key)?;
        let dirs=self.st.list.state_dirs_blocking().map(|(dirs,_)|dirs.to_vec()).unwrap_or_default();
        crate::list::facts_files::open_question(Some(&sid),&dirs).map(|question|question.question)
    }
    fn pane<'a>(&'a self,row:&'a SessionRow)->BoxFuture<'a,Result<PaneAnalysis,&'static str>> {
        Box::pin(async move {
            let target=match self.st.list.agent_target(&row.name).await {
                Ok(target)=>target,
                Err(error)=>{self.st.diag.report("rust.push_target_failed",&row.name,error.code,"pane do agente indisponível; usa o alvo exato da sessão");None}
            };
            let captured=capture_text(&self.st.list.env().capture_program,&row.name,target,self.st.accounts.env.clone(),crate::state::capture::TIMEOUT).await;
            if let Some(code)=captured.error {self.st.diag.report("rust.push_capture_failed",&row.name,&code,"captura do aviso sem resposta válida do multiplexador");}
            Ok(crate::terminal_state::analyze(&captured.text))
        })
    }
    fn sleep(&self,duration:Duration)->BoxFuture<'_,()> {Box::pin(tokio::time::sleep(duration))}
    fn awaiting(&self,name:&str,body:Option<String>) {self.sender.awaiting(name,body);}
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json,Value};
    use std::sync::Mutex;
    struct Fake {scenario:String,sleeps:Mutex<Vec<f64>>,captures:Mutex<usize>,questions:Mutex<usize>,notices:Mutex<Vec<Value>>}
    impl Sources for Fake {
        fn question(&self,_:&SessionRow)->Option<String> {
            *self.questions.lock().unwrap()+=1;
            (self.scenario=="question"||self.scenario=="late_question"&&!self.sleeps.lock().unwrap().is_empty()).then(||"pergunta do sidecar".into())
        }
        fn pane<'a>(&'a self,_:&'a SessionRow)->BoxFuture<'a,Result<PaneAnalysis,&'static str>> {
            Box::pin(async move {
                *self.captures.lock().unwrap()+=1;
                if self.scenario=="capture_error" {return Err("fixture_capture_failed");}
                let menu=self.scenario=="menu"||self.scenario=="late_menu"&&!self.sleeps.lock().unwrap().is_empty();
                Ok(PaneAnalysis {state:if menu{"awaiting_input"}else{"idle"}.into(),question:menu.then(||"pergunta do pane".into()),overlay:self.scenario=="overlay",..Default::default()})
            })
        }
        fn sleep(&self,duration:Duration)->BoxFuture<'_,()> {Box::pin(async move {self.sleeps.lock().unwrap().push(duration.as_secs_f64());})}
        fn awaiting(&self,name:&str,body:Option<String>) {self.notices.lock().unwrap().push(json!([name,body]));}
    }
    #[tokio::test]
    async fn guest_push_first_question_refreshes_missing_snapshot() {
        for provider in ["pi", "omp", "kimi"] {
            let source = Fake {scenario:"question".into(),sleeps:Mutex::default(),captures:Mutex::default(),questions:Mutex::default(),notices:Mutex::default()};
            let calls = Mutex::new(Vec::new());
            let row = super::super::resolve_row(|fresh| {
                calls.lock().unwrap().push(fresh);
                std::future::ready(fresh.then(|| serde_json::from_value(json!({"name":"fixture","provider":provider,"jsonl":"fixture.jsonl"})).unwrap()))
            }, false).await;
            // Uma única transição: o marcador não muda enquanto a descoberta alcança a sessão.
            if let Some(row) = row {awaiting(&source, row).await;}
            assert_eq!(*source.notices.lock().unwrap(), vec![json!(["fixture","pergunta do sidecar"])], "GUE-082 {provider}");
            assert_eq!(*calls.lock().unwrap(), vec![false,true]);
        }
    }

    #[tokio::test]
    async fn guest_push_discovery_does_not_repeat_a_fresh_read() {
        for (fresh, present, expected) in [(false,true,vec![false]),(false,false,vec![false,true]),(true,false,vec![true])] {
            let calls = Mutex::new(Vec::new());
            let row = super::super::resolve_row(|refresh| {
                calls.lock().unwrap().push(refresh);
                std::future::ready(present.then(|| serde_json::from_value(json!({"name":"fixture","provider":"pi"})).unwrap()))
            }, fresh).await;
            assert_eq!(row.is_some(),present);
            assert_eq!(*calls.lock().unwrap(),expected);
        }
    }

}

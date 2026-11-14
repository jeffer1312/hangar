//! Solicitação, escolha e acompanhamento das ações do conector na página de Tickets.
use super::*;

#[derive(Clone,Deserialize)]
pub(super) struct Action {
    pub(super) id: String,
    pub(super) label: String,
    #[serde(default)]
    pub(super) description: String,
    fingerprint: String,
    pub(super) stage: Option<String>,
}

#[derive(Clone,Deserialize)]
pub(super) struct Receipt {
    pub(super) id: String,
    pub(super) status: String,
    pub(super) session: Option<String>,
    pub(super) message: Option<String>,
    #[serde(default="execute_default")]
    execute: bool,
}

fn execute_default() -> bool {true}

#[derive(Clone)]
pub(super) struct Pending {
    server: String,
    connector: String,
    ticket: String,
    stage: String,
    repeat: bool,
    action: Option<Action>,
    confirm_retry: bool,
    has_message: bool,
    execute: bool,
    start: bool,
}

impl Hangar {
    pub(in crate::app) fn cancel_ticket_action(&mut self) {
        if let Some(pending)=self.mission.action_pending.take() {
            self.mission.dispatching.remove(&pending.ticket);
            self.mission.dispatch_seq+=1;
        }
    }
    fn remember_ticket_receipt(&mut self,ticket: String,receipt: Receipt) {
        if receipt.execute {self.mission.execution_receipts.insert(ticket.clone(),receipt.clone());}
        self.mission.receipts.insert(ticket,receipt);
    }

    pub(super) fn request_ticket_move(&mut self,ticket: String,stage: String,window: &mut Window,cx: &mut Context<Self>) {
        if self.mission_busy() {return;}
        let (Some(server),Some(connector),Some(board))=(self.mission.server.clone(),self.mission.connector.clone(),self.mission_board()) else {return;};
        let Some(card)=board.tickets.iter().find(|card|card.id==ticket) else {return;};
        if self.ticket_view(card).stage.is_some_and(|current|current.id==stage) {return;}
        self.mission.dispatch_seq+=1;
        self.mission.action_pending=Some(Pending {server,connector,ticket,stage,repeat:false,action:None,confirm_retry:false,has_message:false,execute:false,start:false});
        self.continue_ticket_action(window,cx);
    }
    pub(super) fn load_ticket_receipt(&mut self,api: Api,connector: String,ticket: String,window: &mut Window,cx: &mut Context<Self>) {
        if self.mission.dispatching.contains(&ticket) {return;}
        let (server,detail_seq,seq)=(api.identity(),self.mission.detail_seq,self.mission.dispatch_seq);
        let (source,id)=(connector.clone(),ticket.clone());
        let client=api.clone();
        let request=async move {
            let value=client.server_read(&["mission","actions"],&[("connector",source.as_str()),("ticket_id",id.as_str())],15).await?;
            let attempt=serde_json::from_value::<Option<Receipt>>(value["attempt"].clone()).map_err(|_|Failure::local("invalid_response"))?;
            let execution=serde_json::from_value::<Option<Receipt>>(value["execution"].clone()).map_err(|_|Failure::local("invalid_response"))?;
            Ok::<_,Failure>((attempt,execution))
        };
        run(&self.runtime,request,window,cx,move |this: &mut Hangar,result,window,cx| {
            if this.mission.server.as_deref()!=Some(&server) || this.mission.connector.as_deref()!=Some(&connector)
                || this.mission.detail_seq!=detail_seq || this.mission.dispatch_seq!=seq {return;}
            let Ok((attempt,execution))=result else {return;};
            let active=attempt.as_ref().filter(|receipt|receipt.active()).or_else(||execution.as_ref().filter(|receipt|receipt.active())).map(|receipt|receipt.id.clone());
            if let Some(execution)=execution {this.mission.execution_receipts.insert(ticket.clone(),execution);}
            if let Some(receipt)=attempt {this.remember_ticket_receipt(ticket.clone(),receipt);}
            if let Some(id)=active {this.watch_ticket_receipt(api,connector,ticket,id,window,cx);}
            cx.notify();
        });
    }

    fn watch_ticket_receipt(&mut self,api: Api,connector: String,ticket: String,id: String,window: &mut Window,cx: &mut Context<Self>) {
        self.mission.dispatching.insert(ticket.clone());
        let (server,seq)=(api.identity(),self.mission.dispatch_seq);
        run(&self.runtime,poll_receipt(api,id),window,cx,move |this: &mut Hangar,result,window,cx| {
            if this.mission.server.as_deref()!=Some(&server) || this.mission.connector.as_deref()!=Some(&connector) || this.mission.dispatch_seq!=seq {return;}
            this.mission.dispatching.remove(&ticket);
            match result {
                Ok(receipt)=>{this.remember_ticket_receipt(ticket,receipt);this.reload_work(window,cx);}
                Err(error)=>eprintln!("consulta de tentativa do ticket falhou: {}",error.code.as_deref().unwrap_or("network")),
            }
            cx.notify();
        });
    }

    pub(super) fn request_ticket_action(&mut self,ticket: String,stage: String,repeat: bool,window: &mut Window,cx: &mut Context<Self>) {
        self.prepare_ticket_action(ticket,stage,repeat,false,window,cx);
    }

    pub(super) fn request_ticket_start(&mut self,ticket: String,stage: String,window: &mut Window,cx: &mut Context<Self>) {
        self.prepare_ticket_action(ticket,stage,true,true,window,cx);
    }

    fn prepare_ticket_action(&mut self,ticket: String,stage: String,repeat: bool,start: bool,window: &mut Window,cx: &mut Context<Self>) {
        if self.mission_busy() {return;}
        let (Some(api),Some(connector),Some(board))=(self.api.clone(),self.mission.connector.clone(),self.mission_board()) else {return;};
        let Some(config)=board.work.stages.iter().find(|s|s.id==stage).cloned() else {return;};
        let Some(card)=board.tickets.iter().find(|t|t.id==ticket) else {return;};
        if !repeat && self.ticket_view(card).stage.is_some_and(|current|current.id==stage) {return;}
        let supported=board.connectors.iter().find(|c|c.id==connector).is_some_and(|c|c.capabilities.contains_key("work_actions"));
        if self.mission.execution_receipts.get(&ticket).is_some_and(Receipt::active) {
            self.load_ticket_receipt(api,connector,ticket,window,cx);return;
        }
        let first=board.work.stages.first().is_some_and(|first|first.id==stage);
        let pending=Pending {server:api.identity(),connector:connector.clone(),ticket:ticket.clone(),stage,repeat,action:None,confirm_retry:false,has_message:false,execute:true,start};
        self.mission.dispatch_seq+=1;
        let seq=self.mission.dispatch_seq;
        self.mission.action_pending=Some(pending.clone());
        self.mission.dispatching.insert(ticket.clone());
        let request=async move {
            if !supported {
                let value=api.server_read(&["connectors",&connector,"tickets",&ticket],&[],30).await?;
                return Ok((Vec::<Action>::new(),first && value["start_prompt"].as_str().is_some_and(|p|!p.trim().is_empty())));
            }
            let value=api.server_read(&["connectors",&connector,"actions"],&[("ticket_id",ticket.as_str())],30).await?;
            serde_json::from_value::<Vec<Action>>(value["actions"].clone()).map(|actions|(actions,false)).map_err(|_|Failure::local("invalid_response"))
        };
        run(&self.runtime,request,window,cx,move |this: &mut Hangar,result,window,cx| {
            if !this.ticket_action_context(&pending,seq) {return;}
            this.mission.dispatching.remove(&pending.ticket);
            let (actions,legacy_message)=match result {
                Ok(actions)=>actions,
                Err(error)=>{this.cancel_ticket_action();window.push_notification(Notification::error(failure_text(&error)),cx);cx.notify();return;}
            };
            let configured=config.actions.as_ref().and_then(|map|map.get(&pending.connector));
            let available:Vec<_>=actions.into_iter().filter(|action|configured.map_or_else(||action.stage.as_deref()==Some(&config.id),|ids|ids.contains(&action.id))).collect();
            let mut pending=pending;
            pending.has_message=legacy_message || !available.is_empty() || config.prompt.as_deref().is_some_and(|p|!p.trim().is_empty());
            this.mission.action_pending=Some(pending);
            match available.len() {
                0=>this.continue_ticket_action(window,cx),
                1=>{this.mission.action_pending.as_mut().unwrap().action=available.into_iter().next();this.continue_ticket_action(window,cx);}
                _=>this.choose_ticket_action(available,seq,window,cx),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn ticket_action_context(&self,pending: &Pending,seq: u64) -> bool {
        self.mission.open && self.mission.dispatch_seq==seq && self.mission.server.as_deref()==Some(&pending.server)
            && self.mission.connector.as_deref()==Some(&pending.connector)
            && (!pending.execute || self.mission.selected.as_deref()==Some(&pending.ticket))
    }

    fn choose_ticket_action(&mut self,actions: Vec<Action>,seq: u64,window: &mut Window,cx: &mut Context<Self>) {
        let weak=cx.entity().downgrade();
        let chosen=std::rc::Rc::new(std::cell::Cell::new(false));
        window.open_dialog(cx,move |dialog,_,_| {
            let buttons=actions.iter().map(|action| {
                let action=action.clone();let weak=weak.clone();let chosen=chosen.clone();
                Button::new(SharedString::from(format!("tickets-action-{}",action.id))).outline().w_full().label(action.label.clone())
                    .on_click(move |_,window,cx| {
                        let _=weak.update(cx,|this,cx| {
                            if this.mission.dispatch_seq!=seq {return;}
                            chosen.set(true);
                            if let Some(pending)=this.mission.action_pending.as_mut() {pending.action=Some(action.clone());}
                            window.close_dialog(cx);cx.defer_in(window,|this,window,cx|this.continue_ticket_action(window,cx));
                        });
                    }).into_any_element()
            }).collect::<Vec<_>>();
            let explanations=actions.iter().filter(|a|!a.description.trim().is_empty()).map(|a| {
                div().text_sm().whitespace_normal().child(format!("{}: {}",a.label,a.description))
            });
            let weak=weak.clone();
            let close_chosen=chosen.clone();
            popup::dialog(dialog).w(px(560.)).title(tr("mission_action_choose")).overlay_closable(false)
                .child(div().flex().flex_col().gap(px(10.)).child(div().text_sm().text_color(theme::muted()).child(tr("mission_action_choose_hint")))
                    .children(buttons).children(explanations))
                .on_close(move |_,_,cx| {let _=weak.update(cx,|this,cx| {
                    if !close_chosen.get() && this.mission.dispatch_seq==seq {this.mission.action_pending=None;}
                    cx.notify();
                });})
        });
    }

    pub(super) fn continue_ticket_action(&mut self,window: &mut Window,cx: &mut Context<Self>) {
        let Some(pending)=self.mission.action_pending.clone() else {return;};
        let seq=self.mission.dispatch_seq;
        if !self.ticket_action_context(&pending,seq) {self.mission.action_pending=None;return;}
        if pending.execute && self.mission.execution_receipts.get(&pending.ticket).is_some_and(|r|r.status=="uncertain") && !pending.confirm_retry {
            let weak=cx.entity().downgrade();
            let accepted=std::rc::Rc::new(std::cell::Cell::new(false));
            window.open_dialog(cx,move |dialog,_,_| {
                let confirm=weak.clone();let cancel=weak.clone();let done=accepted.clone();let close=weak.clone();let close_accepted=accepted.clone();
                popup::dialog(dialog).title(tr("mission_action_uncertain_title")).child(div().text_sm().whitespace_normal().child(tr("mission_action_uncertain_hint")))
                    .footer(div().flex().gap(px(8.))
                        .child(Button::new("tickets-retry").primary().label(tr("mission_action_retry_checked"))
                            .on_click(move |_,window,cx| {let _=confirm.update(cx,|this,cx| {
                                if this.mission.dispatch_seq!=seq {return;}
                                done.set(true);
                                if let Some(pending)=this.mission.action_pending.as_mut() {pending.confirm_retry=true;}
                                window.close_dialog(cx);cx.defer_in(window,|this,window,cx|this.continue_ticket_action(window,cx));
                            });}))
                        .child(Button::new("tickets-retry-cancel").ghost().label(tr("cancel"))
                            .on_click(move |_,window,cx| {let _=cancel.update(cx,|this,cx| {this.mission.action_pending=None;window.close_dialog(cx);cx.notify();});})))
                    .on_close(move |_,_,cx| {if !close_accepted.get() {let _=close.update(cx,|this,cx|{this.mission.action_pending=None;cx.notify();});}})
            });
            return;
        }
        let alive=self.mission_board().and_then(|board|board.tickets.iter().find(|ticket|ticket.id==pending.ticket)
            .and_then(|ticket|self.ticket_session(ticket,board.item(&pending.connector,&pending.ticket))))
            .and_then(|linked|linked.session).is_some_and(|session|session.state!="dead");
        let repository=self.mission_board().and_then(|board|board.item(&pending.connector,&pending.ticket)).and_then(|item|item.repository.clone());
        if pending.has_message && !alive && repository.is_none() {
            self.open_ticket_launch(window,cx);
            return;
        }
        self.mission.action_pending=None;
        let Some(api)=self.api.clone().filter(|api|api.identity()==pending.server) else {return;};
        let mut raw=[0u8;16];
        if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(),&mut raw).is_err() {
            window.push_notification(Notification::error(tr("invalid_response")),cx);return;
        }
        let id:String=raw.iter().map(|b|format!("{b:02x}")).collect();
        let body=json!({"attempt":id,"connector":pending.connector,"ticket_id":pending.ticket,"stage":pending.stage,
            "action":pending.action.as_ref().map(|a|&a.id),"fingerprint":pending.action.as_ref().map(|a|&a.fingerprint),
            "open":repository.map(|cwd|json!({"cwd":cwd})),"repeat":pending.repeat,"confirm_retry":pending.confirm_retry,"execute":pending.execute,"start":pending.start});
        self.mission.dispatching.insert(pending.ticket.clone());
        self.mission.receipts.insert(pending.ticket.clone(),Receipt {id:id.clone(),status:"preparing".into(),session:None,message:None,execute:pending.execute});
        let request=async move {
            api.server_send(reqwest::Method::POST,&["mission","actions"],Some(body),30).await?;
            poll_receipt(api,id).await
        };
        run(&self.runtime,request,window,cx,move |this: &mut Hangar,result,window,cx| {
            if this.mission.server.as_deref()!=Some(&pending.server) || this.mission.dispatch_seq!=seq {return;}
            this.mission.dispatching.remove(&pending.ticket);
            match result {
                Ok(receipt)=>{
                    if matches!(receipt.status.as_str(),"sent"|"queued") && receipt.session.is_some() && this.mission.selected.as_deref()==Some(&pending.ticket) {
                        this.mission.conversation=true;
                    }
                    if matches!(receipt.status.as_str(),"failed"|"uncertain") {
                        let text=receipt.message.clone().unwrap_or_else(||tr("mission_action_uncertain_hint"));
                        window.push_notification(Notification::warning(text),cx);
                    }
                    this.remember_ticket_receipt(pending.ticket.clone(),receipt);
                }
                Err(error)=>{
                    if let Some(receipt)=this.mission.receipts.get_mut(&pending.ticket) {
                        let uncertain=error.status.is_none() || error.status.is_some_and(|status|status>=500)
                            || matches!(error.code.as_deref(),Some("ticket_action_uncertain"|"ticket_action_check_required"));
                        receipt.status=if uncertain {"uncertain"} else {"failed"}.into();receipt.message=Some(failure_text(&error));
                    }
                    if pending.execute && let Some(receipt)=this.mission.receipts.get(&pending.ticket).cloned() {
                        this.mission.execution_receipts.insert(pending.ticket.clone(),receipt);
                    }
                    window.push_notification(Notification::warning(failure_text(&error)),cx);
                }
            }
            this.reload_work(window,cx);cx.notify();
        });
        cx.notify();
    }

    pub(super) fn render_ticket_receipt(&self) -> Option<AnyElement> {
        let receipt=self.mission.receipts.get(self.mission.selected.as_ref()?)?;
        let text=match receipt.status.as_str() {
            "sent"=>tr("mission_action_sent"),"queued"=>tr("mission_action_queued"),"recorded"=>tr("mission_action_recorded"),
            "failed"=>receipt.message.clone().unwrap_or_else(||tr("mission_action_failed")),
            "uncertain"=>tr("mission_action_uncertain_hint"),_=>tr("mission_action_sending"),
        };
        Some(note_box(text,if matches!(receipt.status.as_str(),"failed"|"uncertain") {theme::warning()} else {theme::muted()}).into_any_element())
    }
}

impl Receipt {
    fn active(&self) -> bool {matches!(self.status.as_str(),"preparing"|"opening"|"sending")}
}

async fn poll_receipt(api: Api,id: String) -> Result<Receipt,Failure> {
    let deadline=Instant::now()+Duration::from_secs(120);
    loop {
        let value=api.server_read(&["mission","actions",&id],&[],15).await?;
        let receipt:Receipt=serde_json::from_value(value["attempt"].clone()).map_err(|_|Failure::local("invalid_response"))?;
        if !receipt.active() {return Ok(receipt);}
        if Instant::now()>=deadline {return Err(Failure::local("ticket_action_uncertain"));}
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

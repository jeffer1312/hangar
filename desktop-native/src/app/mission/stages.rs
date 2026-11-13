//! Configuração das etapas em modal, com identidade estável e um formulário por etapa.
use super::*;
use super::super::machines::enter_to_focused;

struct Draft {
    stage: Stage,
    name: Entity<InputState>,
    prompt: Entity<TextareaState>,
    statuses: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

pub(super) struct Editor {
    server: String,
    seq: u64,
    rows: Vec<Draft>,
    selected: Option<String>,
    connector: Option<String>,
    connector_name: String,
    has_actions: bool,
    catalog: Option<Result<Vec<actions::Action>,String>>,
    busy: bool,
    error: Option<String>,
}

#[derive(Clone)]
struct EditorHandle {view: WeakEntity<Hangar>,seq: u64}

impl EditorHandle {
    fn apply(&self,cx: &mut App,action: impl FnOnce(&mut Hangar,&mut Context<Hangar>)) {
        let _=self.view.update(cx,|this,cx| {
            let current=this.mission.stages_editor.as_ref().is_some_and(|editor|editor.seq==self.seq && !editor.busy
                && this.mission.server.as_deref()==Some(&editor.server));
            if current {action(this,cx);}
        });
    }
}

impl Draft {
    fn value(&self,cx: &App) -> Stage {
        Stage {name:self.name.read(cx).value().trim().to_owned(),prompt:Some(self.prompt.read(cx).value().to_string()),
            statuses:self.statuses.read(cx).value().lines().map(str::trim).filter(|s|!s.is_empty()).map(str::to_owned).collect(),..self.stage.clone()}
    }
}

impl Hangar {
    fn stage_draft(&self,stage: Stage,window: &mut Window,cx: &mut Context<Self>) -> Draft {
        let name=cx.new(|cx|InputState::new(window,cx).placeholder(tr("mission_stage_name")));
        name.update(cx,|input,cx|input.set_value(stage.name.clone(),window,cx));
        let prompt=cx.new(|cx|TextareaState::new(window,cx).auto_grow(4,10));
        prompt.update(cx,|input,cx|input.set_value(stage.prompt.clone().unwrap_or_default(),window,cx));
        let statuses=cx.new(|cx|TextareaState::new(window,cx).auto_grow(2,5));
        statuses.update(cx,|input,cx|input.set_value(stage.statuses.join("\n"),window,cx));
        let subscriptions=vec![cx.observe(&name,|_,_,cx|cx.notify()),cx.observe(&prompt,|_,_,cx|cx.notify()),cx.observe(&statuses,|_,_,cx|cx.notify())];
        Draft {stage,name,prompt,statuses,_subscriptions:subscriptions}
    }

    pub(super) fn edit_stages(&mut self,window: &mut Window,cx: &mut Context<Self>) {
        if self.mission.stages_editor.is_some() || self.mission_busy() {return;}
        let (Some(board),Some(api))=(self.mission_board(),self.api.clone()) else {return;};
        let stages=board.work.stages.clone();
        let connector=self.mission.connector.clone();
        let source=connector.as_ref().and_then(|id|board.connectors.iter().find(|source|&source.id==id));
        let connector_name=source.map(|source|source.name.clone()).unwrap_or_default();
        let has_actions=source.is_some_and(|source|source.capabilities.contains_key("work_actions"));
        let ticket=self.mission.selected.clone().or_else(||board.tickets.first().map(|ticket|ticket.id.clone()));
        let rows=stages.into_iter().map(|stage|self.stage_draft(stage,window,cx)).collect::<Vec<_>>();
        self.mission.stages_editor_seq+=1;
        let seq=self.mission.stages_editor_seq;
        let server=api.identity();
        let loading=has_actions && connector.is_some() && ticket.is_some();
        self.mission.stages_editor=Some(Editor {server:server.clone(),seq,selected:rows.first().map(|row|row.stage.id.clone()),rows,
            connector:connector.clone(),connector_name,has_actions,catalog:(!loading).then(||Ok(Vec::new())),busy:false,error:None});
        let weak=cx.entity().downgrade();
        window.open_dialog(cx,move |dialog,window,app| {
            let content=weak.upgrade().map(|view|view.read(app).render_stages_editor(window,app,weak.clone()));
            let busy=weak.upgrade().is_some_and(|view|view.read(app).mission.stages_editor.as_ref().is_some_and(|editor|editor.busy));
            let close=weak.clone();
            popup::dialog(dialog).w((window.viewport_size().width*0.94).min(px(1080.))).title(tr("mission_stages_title"))
                .keyboard(!busy).close_button(!busy).overlay_closable(false).on_ok(enter_to_focused).children(content)
                .on_close(move |_,_,app| {let _=close.update(app,|this,cx| {
                    if this.mission.stages_editor.as_ref().is_some_and(|editor|editor.seq==seq) {this.mission.stages_editor=None;}
                    cx.notify();
                });})
        });
        if let (true,Some(connector),Some(ticket))=(loading,connector,ticket) {
            let request=async move {
                let value=api.server_read(&["connectors",&connector,"actions"],&[("ticket_id",ticket.as_str())],30).await?;
                serde_json::from_value::<Vec<actions::Action>>(value["actions"].clone()).map_err(|_|Failure::local("invalid_response"))
            };
            run(&self.runtime,request,window,cx,move |this: &mut Hangar,result,_,cx| {
                if let Some(editor)=this.mission.stages_editor.as_mut().filter(|editor|editor.seq==seq && editor.server==server) {
                    editor.catalog=Some(result.map_err(|error|failure_text(&error)));cx.notify();
                }
            });
        }
        cx.notify();
    }

    fn add_ticket_stage(&mut self,window: &mut Window,cx: &mut Context<Self>) {
        let Some(editor)=self.mission.stages_editor.as_ref().filter(|editor|!editor.busy) else {return;};
        let mut bytes=[0u8;16];
        if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(),&mut bytes).is_err() {return;}
        let token=bytes.iter().map(|byte|format!("{byte:02x}")).collect::<String>();
        let id=format!("stage-{}",&token[..26]);
        let stage=Stage {id:id.clone(),name:tr("mission_stage_new").replace("{n}",&(editor.rows.len()+1).to_string()),gate:false,statuses:Vec::new(),prompt:None,actions:None};
        let draft=self.stage_draft(stage,window,cx);
        if let Some(editor)=self.mission.stages_editor.as_mut() {editor.rows.push(draft);editor.selected=Some(id);editor.error=None;}
        cx.notify();
    }

    fn select_ticket_stage(&mut self,id: String,cx: &mut Context<Self>) {
        if let Some(editor)=self.mission.stages_editor.as_mut().filter(|editor|!editor.busy) {editor.selected=Some(id);}
        cx.notify();
    }

    fn reorder_ticket_stage(&mut self,id: &str,direction: isize,cx: &mut Context<Self>) {
        let Some(editor)=self.mission.stages_editor.as_mut().filter(|editor|!editor.busy) else {return;};
        let Some(index)=editor.rows.iter().position(|row|row.stage.id==id) else {return;};
        let next=index as isize+direction;
        if next>=0 && (next as usize)<editor.rows.len() {editor.rows.swap(index,next as usize);}
        cx.notify();
    }

    fn remove_ticket_stage(&mut self,id: &str,cx: &mut Context<Self>) {
        if let Some(editor)=self.mission.stages_editor.as_mut().filter(|editor|!editor.busy) {
            editor.rows.retain(|row|row.stage.id!=id);
            if editor.selected.as_deref()==Some(id) {editor.selected=editor.rows.first().map(|row|row.stage.id.clone());}
        }
        cx.notify();
    }

    fn toggle_ticket_stage_action(&mut self,stage: &str,action: &str,on: bool,cx: &mut Context<Self>) {
        let Some(editor)=self.mission.stages_editor.as_mut().filter(|editor|!editor.busy) else {return;};
        let Some(connector)=editor.connector.clone() else {return;};
        let defaults=editor.catalog.as_ref().and_then(|catalog|catalog.as_ref().ok()).into_iter().flatten()
            .filter(|action|action.stage.as_deref()==Some(stage)).map(|action|action.id.clone()).collect::<Vec<_>>();
        let Some(row)=editor.rows.iter_mut().find(|row|row.stage.id==stage) else {return;};
        let map=row.stage.actions.get_or_insert_with(BTreeMap::new);
        let ids=map.entry(connector).or_insert(defaults);
        if on {if !ids.iter().any(|id|id==action) {ids.push(action.to_owned());}} else {ids.retain(|id|id!=action);}
        cx.notify();
    }

    fn save_ticket_stages(&mut self,window: &mut Window,cx: &mut Context<Self>) {
        let Some(editor)=self.mission.stages_editor.as_ref().filter(|editor|!editor.busy) else {return;};
        if self.mission.server.as_deref()!=Some(&editor.server) {return;}
        let stages=editor.rows.iter().map(|row|row.value(cx)).collect::<Vec<_>>();
        if stages.is_empty() || stages.iter().any(|stage|stage.name.is_empty()) {
            if let Some(editor)=self.mission.stages_editor.as_mut() {editor.error=Some(tr("mission_stages_empty_name"));}
            cx.notify();return;
        }
        let (server,seq)=(editor.server.clone(),editor.seq);
        let Some(api)=self.api_for(&server) else {return;};
        if let Some(editor)=self.mission.stages_editor.as_mut() {editor.busy=true;editor.error=None;}
        let request=async move {api.server_send(reqwest::Method::PUT,&["mission","stages"],Some(json!({"stages":stages})),30).await};
        run(&self.runtime,request,window,cx,move |this: &mut Hangar,result,window,cx| {
            if this.mission.stages_editor.as_ref().is_none_or(|editor|editor.seq!=seq || editor.server!=server) {return;}
            match result {
                Ok(value)=>{
                    this.mission.stages_editor=None;window.close_dialog(cx);
                    if this.mission.server.as_deref()==Some(&server) {
                        if let Ok(work)=serde_json::from_value::<Work>(value) {
                            if let Some(Ok(board))=this.mission.board.as_mut() {board.work=work;}
                        } else {this.reload_work(window,cx);}
                    }
                }
                Err(error)=>if let Some(editor)=this.mission.stages_editor.as_mut() {editor.busy=false;editor.error=Some(failure_text(&error));},
            }
            cx.notify();
        });
        cx.notify();
    }

    fn render_stages_editor(&self,window: &Window,cx: &App,owner: WeakEntity<Self>) -> AnyElement {
        let Some(editor)=self.mission.stages_editor.as_ref() else {return div().into_any_element();};
        let owner=EditorHandle {view:owner,seq:editor.seq};
        let busy=editor.busy;
        let height=(window.viewport_size().height-px(210.)).min(px(740.)).max(px(320.));
        let add=owner.clone();let save=owner.clone();let cancel=owner.clone();
        let mut left=div().w(px(250.)).flex_shrink_0().h_full().min_h_0().pr(px(16.)).border_r_1().border_color(theme::border()).flex().flex_col().gap(px(10.));
        let rows=editor.rows.iter().enumerate().map(|(index,row)| {
            let id=row.stage.id.clone();let pick=owner.clone();let up=owner.clone();let down=owner.clone();
            let up_id=id.clone();let down_id=id.clone();let selected=editor.selected.as_deref()==Some(&id);
            let name=row.name.read(cx).value().to_string();
            div().flex().items_center().gap(px(4.))
                .child(Button::new(SharedString::from(format!("tickets-stage-{id}"))).ghost().small().flex_1()
                    .label(format!("{} · {}",index+1,name)).selected(selected).disabled(busy)
                    .on_click(move |_,_,cx|pick.apply(cx,|this,cx|this.select_ticket_stage(id.clone(),cx))))
                .child(Button::new(SharedString::from(format!("tickets-stage-up-{up_id}"))).ghost().small().icon(IconName::ArrowUp)
                    .accessibility_label(tr("mission_stage_up")).disabled(busy || index==0)
                    .on_click(move |_,_,cx|up.apply(cx,|this,cx|this.reorder_ticket_stage(&up_id,-1,cx))))
                .child(Button::new(SharedString::from(format!("tickets-stage-down-{down_id}"))).ghost().small().icon(IconName::ArrowDown)
                    .accessibility_label(tr("mission_stage_down")).disabled(busy || index+1==editor.rows.len())
                    .on_click(move |_,_,cx|down.apply(cx,|this,cx|this.reorder_ticket_stage(&down_id,1,cx))))
        });
        left=left.child(div().id("tickets-stage-list").relative().flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(4.)).children(rows))
            .child(Button::new("tickets-stage-add").outline().small().w_full().icon(IconName::Plus).label(tr("mission_stage_add")).disabled(busy)
                .on_click(move |_,window,cx|add.apply(cx,|this,cx|this.add_ticket_stage(window,cx))));
        let selected=editor.selected.as_ref().and_then(|id|editor.rows.iter().find(|row|&row.stage.id==id));
        let right=match selected {
            None=>div().flex_1().p(px(20.)).child(empty_state(tr("mission_stages_none"))).into_any_element(),
            Some(row)=>{
                let gate=owner.clone();let stage=row.stage.id.clone();let remove=owner.clone();let remove_id=stage.clone();
                let label=|text: String|div().text_sm().font_weight(FontWeight::SEMIBOLD).child(text);
                let mut fields=div().id("tickets-stage-form").relative().flex_1().min_w_0().h_full().overflow_y_scroll().pl(px(20.)).pr(px(4.)).flex().flex_col().gap(px(18.))
                    .child(div().flex().flex_col().gap(px(6.)).child(label(tr("mission_stage_name")))
                        .child(Input::new(&row.name).aria_label(tr("mission_stage_name")).disabled(busy)))
                    .child(Checkbox::new("tickets-stage-gate").label(tr("mission_stage_gate")).checked(row.stage.gate).disabled(busy)
                        .on_click(move |on,_,cx|gate.apply(cx,|this,cx| {
                            if let Some(editor)=this.mission.stages_editor.as_mut() && let Some(row)=editor.rows.iter_mut().find(|row|row.stage.id==stage) {row.stage.gate=*on;}
                            cx.notify();
                        })))
                    .child(div().flex().flex_col().gap(px(6.)).child(label(tr("mission_stage_prompt")))
                        .child(Textarea::new(&row.prompt).aria_label(tr("mission_stage_prompt")).disabled(busy))
                        .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("mission_stage_prompt_hint"))))
                    .child(self.render_stage_actions(editor,row,cx,owner.clone()))
                    .child(div().flex().flex_col().gap(px(6.)).child(label(tr("mission_stage_statuses")))
                        .child(Textarea::new(&row.statuses).aria_label(tr("mission_stage_statuses")).disabled(busy))
                        .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("mission_stage_statuses_hint"))));
                fields=fields.child(div().pt(px(8.)).child(Button::new("tickets-stage-remove").ghost().small().icon(IconName::Trash).label(tr("mission_stage_remove")).disabled(busy)
                    .on_click(move |_,_,cx|remove.apply(cx,|this,cx|this.remove_ticket_stage(&remove_id,cx)))));
                fields.into_any_element()
            }
        };
        div().h(height).w_full().flex().flex_col().gap(px(14.))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("mission_stages_form_hint")))
            .child(div().flex_1().min_h_0().flex().child(left).child(right))
            .when_some(editor.error.clone(),|el,error|el.child(note_box(error,theme::warning())))
            .child(div().flex_shrink_0().pt(px(10.)).border_t_1().border_color(theme::border()).flex().justify_end().gap(px(8.))
                .child(Button::new("tickets-stages-cancel").ghost().small().label(tr("cancel")).disabled(busy).on_click(move |_,window,cx|cancel.apply(cx,|this,cx| {
                    this.mission.stages_editor=None;
                    window.close_dialog(cx);
                    cx.notify();
                })))
                .child(Button::new("tickets-stages-save").primary().small().label(tr("mission_stages_save")).loading(busy).disabled(busy || editor.rows.is_empty())
                    .on_click(move |_,window,cx|save.apply(cx,|this,cx|this.save_ticket_stages(window,cx)))))
            .into_any_element()
    }

    fn render_stage_actions(&self,editor: &Editor,row: &Draft,_cx: &App,owner: EditorHandle) -> AnyElement {
        let mut group=div().flex().flex_col().gap(px(6.))
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("mission_stage_actions")))
            .child(div().text_xs().text_color(theme::muted()).child(editor.connector_name.clone()));
        if !editor.has_actions {group=group.child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("mission_stage_no_actions")));}
        let catalog=match editor.catalog.as_ref() {
            None=>return group.child(loading_state()).into_any_element(),
            Some(Err(error))=>return group.child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(error.clone())).into_any_element(),
            Some(Ok(catalog))=>catalog,
        };
        let configured=editor.connector.as_ref().and_then(|connector|row.stage.actions.as_ref().and_then(|map|map.get(connector)));
        if catalog.is_empty() {group=group.child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("mission_stage_actions_empty")));}
        for missing in configured.into_iter().flatten().filter(|id|!catalog.iter().any(|action|&action.id==*id)) {
            let (stage,id,weak)=(row.stage.id.clone(),missing.clone(),owner.clone());
            group=group.child(Checkbox::new(SharedString::from(format!("tickets-stage-missing-{missing}")))
                .label(tr("mission_stage_action_unavailable").replace("{action}",missing)).checked(true).disabled(editor.busy)
                .on_click(move |on,_,cx|weak.apply(cx,|this,cx|this.toggle_ticket_stage_action(&stage,&id,*on,cx))));
        }
        for action in catalog {
            let on=configured.map_or_else(||action.stage.as_deref()==Some(&row.stage.id),|ids|ids.contains(&action.id));
            let (stage,id,weak)=(row.stage.id.clone(),action.id.clone(),owner.clone());
            group=group.child(div().py(px(5.)).flex().flex_col().gap(px(3.))
                .child(Checkbox::new(SharedString::from(format!("tickets-stage-action-{}",action.id))).label(action.label.clone()).checked(on).disabled(editor.busy)
                    .on_click(move |on,_,cx|weak.apply(cx,|this,cx|this.toggle_ticket_stage_action(&stage,&id,*on,cx))))
                .when(!action.description.is_empty(),|el|el.child(div().pl(px(24.)).text_xs().text_color(theme::muted()).whitespace_normal().child(action.description.clone()))));
        }
        group.into_any_element()
    }
}

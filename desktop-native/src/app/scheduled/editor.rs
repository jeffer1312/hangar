use super::*;
use chrono::{NaiveDate, NaiveTime, TimeZone};
use gpui_kit::component::{
    date_picker::{DatePicker, DatePickerState},
    time_field::{TimeField, TimeFieldState},
};

pub(in crate::app) struct Editor {
    pub(super) api: Api,
    runtime: Arc<Runtime>,
    pub(super) page: WeakEntity<TaskPage>,
    tx: async_channel::Sender<Envelope>,
    pub(super) connection: u64,
    pub(super) id: Option<String>,
    /// Tarefa nova: o mesmo pedido em cada nova tentativa, para uma criação sem resposta não virar duas tarefas.
    request_id: Option<String>,
    revision: u64,
    name: Entity<InputState>,
    prompt: Entity<TextareaState>,
    date: Entity<DatePickerState>,
    time: Entity<TimeFieldState>,
    interval: Entity<InputState>,
    unit: u64,
    kind: &'static str,
    weekdays: Vec<usize>,
    enabled: bool,
    pub(in crate::app) target: Value,
    timezone: String,
    sessions: Option<Result<Vec<Value>, String>>,
    pub(super) saving: bool,
    conflict: bool,
    pub(in crate::app) choosing: bool,
    error: Option<String>,
}

impl Editor {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        api: Api,
        runtime: Arc<Runtime>,
        page: WeakEntity<TaskPage>,
        tx: async_channel::Sender<Envelope>,
        connection: u64,
        task: Option<Task>,
        timezone: String,
        server_time: String,
        sessions: Option<Result<Vec<Value>, String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = |value: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).default_value(value))
        };
        let name = field(
            task.as_ref().map(|t| t.name.clone()).unwrap_or_default(),
            window,
            cx,
        );
        name.update(cx, |input, cx| {
            input.set_placeholder(t("scheduled_name_hint"), window, cx)
        });
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder(t("scheduled_prompt_hint"))
                .default_value(task.as_ref().map(|t| t.prompt.clone()).unwrap_or_default())
        });
        let schedule = task
            .as_ref()
            .map(|t| t.schedule.clone())
            .unwrap_or_else(|| json!({"kind":"weekly", "time":"09:00", "weekdays":[1,2,3,4,5]}));
        let kind = match schedule["kind"].as_str() {
            Some("once") => "once",
            Some("interval") => "interval",
            _ => "weekly",
        };
        let once = schedule["at"]
            .as_str()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok());
        let date = once
            .or_else(|| chrono::DateTime::parse_from_rfc3339(&server_time).ok())
            .map(|v| {
                if let Ok(zone) = timezone.parse::<chrono_tz::Tz>() {
                    v.with_timezone(&zone).format("%Y-%m-%d").to_string()
                } else {
                    v.format("%Y-%m-%d").to_string()
                }
            })
            .unwrap_or_default();
        let time = if let Some(once) = once {
            if let Ok(zone) = timezone.parse::<chrono_tz::Tz>() {
                once.with_timezone(&zone).format("%H:%M").to_string()
            } else {
                once.format("%H:%M").to_string()
            }
        } else {
            schedule["time"].as_str().unwrap_or("09:00").to_owned()
        };
        let weekdays = schedule["weekdays"]
            .as_array()
            .map(|days| {
                days.iter()
                    .filter_map(Value::as_u64)
                    .map(|day| day as usize)
                    .collect()
            })
            .unwrap_or_else(|| vec![1, 2, 3, 4, 5]);
        let date = cx.new(|cx| {
            let mut state = DatePickerState::new(window, cx).date_format("%d/%m/%Y");
            if let Ok(date) = NaiveDate::parse_from_str(&date, "%Y-%m-%d") {
                state.set_date(date, window, cx);
            }
            state
        });
        let time = cx.new(|cx| {
            let mut state = TimeFieldState::new(window, cx);
            state.set_time(
                NaiveTime::parse_from_str(&time, "%H:%M").unwrap_or(NaiveTime::MIN),
                window,
                cx,
            );
            state
        });
        Self {
            api,
            runtime,
            page,
            tx,
            connection,
            id: task.as_ref().map(|t| t.id.clone()),
            request_id: task.is_none().then(crate::app::accounts::idempotency_key),
            revision: task.as_ref().map_or(0, |t| t.revision),
            name,
            prompt,
            date,
            time,
            interval: field(
                schedule["minutes"].as_u64().unwrap_or(120).to_string(),
                window,
                cx,
            ),
            unit: 1,
            kind,
            weekdays,
            enabled: task.as_ref().is_none_or(|t| t.enabled),
            target: task
                .map(|t| t.target)
                .unwrap_or_else(|| json!({"kind":"new", "launch":{}, "worktree":false})),
            timezone,
            sessions,
            saving: false,
            conflict: false,
            choosing: false,
            error: None,
        }
    }

    fn body(&self, cx: &App) -> Result<Value, String> {
        let name = self.name.read(cx).value().trim().to_string();
        let prompt = self.prompt.read(cx).value().trim().to_string();
        let invalid = if name.is_empty() {
            Some("scheduled_invalid_name")
        } else if prompt.is_empty() {
            Some("scheduled_invalid_prompt")
        } else if self.kind == "weekly" && self.weekdays.is_empty() {
            Some("scheduled_invalid_days")
        } else if self.target["kind"] == "new"
            && self.target["launch"]["cwd"]
                .as_str()
                .is_none_or(str::is_empty)
        {
            Some("scheduled_invalid_folder")
        } else if self.target["kind"] == "session"
            && self.target["name"].as_str().is_none_or(str::is_empty)
        {
            Some("scheduled_select_session")
        } else {
            None
        };
        if let Some(invalid) = invalid {
            return Err(t(invalid));
        }
        let time = self.time.read(cx).time();
        let schedule = match self.kind {
            "interval" => {
                let minutes = self
                    .interval
                    .read(cx)
                    .value()
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n > 0)
                    .and_then(|n| n.checked_mul(self.unit))
                    .ok_or_else(|| t("scheduled_invalid_interval"))?;
                json!({"kind":"interval", "minutes":minutes})
            }
            "once" => {
                let date = self
                    .date
                    .read(cx)
                    .date_time()
                    .start()
                    .map(|date| date.date())
                    .ok_or_else(|| t("scheduled_invalid_date"))?;
                let zone = self
                    .timezone
                    .parse::<chrono_tz::Tz>()
                    .map_err(|_| t("scheduled_invalid_date"))?;
                // Horário inexistente ou repetido pelo horário de verão exige outra escolha, nunca um offset adivinhado.
                let at = zone
                    .from_local_datetime(&date.and_time(time))
                    .single()
                    .ok_or_else(|| t("scheduled_invalid_date"))?;
                // O servidor valida o futuro com o relógio dele; o relógio deste aparelho pode estar adiantado.
                json!({"kind":"once", "at":at.to_rfc3339()})
            }
            _ => {
                json!({"kind":"weekly", "time":time.format("%H:%M").to_string(), "weekdays":self.weekdays})
            }
        };
        let mut body = json!({"name":name, "prompt":prompt, "enabled":self.enabled,"schedule":schedule,"target":self.target});
        if self.id.is_some() {
            body["revision"] = json!(self.revision);
        } else if let Some(request_id) = &self.request_id {
            body["request_id"] = json!(request_id);
        }
        Ok(body)
    }

    pub(super) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.conflict {
            return;
        }
        let body = match self.body(cx) {
            Ok(body) => body,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.saving = true;
        self.error = None;
        let (api, id) = (self.api.clone(), self.id.clone());
        run(
            &self.runtime,
            async move {
                match id {
                    Some(id) => {
                        api.server_send(
                            reqwest::Method::PUT,
                            &["scheduled-tasks", &id],
                            Some(body),
                            30,
                        )
                        .await
                    }
                    None => {
                        api.server_send(reqwest::Method::POST, &["scheduled-tasks"], Some(body), 30)
                            .await
                    }
                }
            },
            window,
            cx,
            |this, result, window, cx| {
                this.saving = false;
                match result {
                    Ok(value) => match serde_json::from_value::<Task>(value) {
                        Ok(task) => {
                            // A identidade do destino devolvida pelo servidor prevalece sobre a seleção inicial.
                            this.target = task.target.clone();
                            let created = this.id.is_none();
                            let _ = this.page.update(cx, |page, cx| {
                                page.draft = None;
                                page.notice = Some(t(if created {
                                    "scheduled_created"
                                } else {
                                    "scheduled_saved"
                                }));
                                page.load(window, cx);
                            });
                            window.close_dialog(cx);
                        }
                        Err(_) => this.error = Some(tr("invalid_response")),
                    },
                    Err(error) => {
                        this.conflict = error.status == Some(409)
                            && error
                                .code
                                .as_deref()
                                .is_some_and(|code| code == "scheduled_revision_conflict");
                        this.error = Some(failure(&error));
                    }
                }
            },
        );
        cx.notify();
    }

    fn choose_launch(&mut self, cx: &mut Context<Self>) {
        if self.choosing {
            return;
        }
        self.choosing = true;
        let (api, tx, connection, editor) = (
            self.api.clone(),
            self.tx.clone(),
            self.connection,
            cx.entity().downgrade(),
        );
        self.runtime.spawn(async move {
            let _ = tx
                .send(Envelope {
                    connection,
                    selection: None,
                    payload: Payload::Scheduled(Reply::ChooseLaunch(api, editor)),
                })
                .await;
        });
    }

    fn discard(&self, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.page.clone();
        confirmation(
            window,
            cx,
            t("scheduled_discard_title"),
            vec![t("scheduled_discard_body")],
            "scheduled_keep_editing",
            "scheduled_discard",
            move |window, cx| {
                let _ = page.update(cx, |page, cx| {
                    page.draft = None;
                    page.load(window, cx);
                });
                window.close_dialog(cx);
            },
        );
    }

    fn input(&self, input: &Entity<InputState>, key: &'static str) -> AnyElement {
        field(
            key,
            Input::new(input)
                .disabled(self.saving)
                .aria_label(t(key))
                .into_any_element(),
        )
    }

    fn session_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let sessions = match &self.sessions {
            None => return muted(t("scheduled_loading_sessions")).into_any_element(),
            Some(Err(error)) => {
                return alert("scheduled-sessions-error", error.clone()).into_any_element();
            }
            Some(Ok(sessions)) => sessions.clone(),
        };
        let current = self.target["name"].as_str().unwrap_or_default().to_owned();
        let weak = cx.entity().downgrade();
        let empty = sessions.is_empty();
        field(
            "scheduled_session",
            Button::new("scheduled-session")
                .outline()
                .w_full()
                .disabled(self.saving || empty)
                .label(if empty {
                    t("scheduled_no_sessions")
                } else if current.is_empty() {
                    t("scheduled_select_session")
                } else {
                    current.clone()
                })
                .dropdown_caret(true)
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    let mut menu = sidebar::menu_style(menu).max_h(px(300.));
                    for session in &sessions {
                        let Some(name) = session["name"].as_str().map(str::to_owned) else {
                            continue;
                        };
                        let mut target = json!({"kind":"session","name":name});
                        if let Some(key) = session["session_key"].as_str() {
                            target["session_key"] = json!(key);
                        }
                        let weak = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(name == current)
                                .on_click(move |_, _, cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.target = target.clone();
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
                })
                .into_any_element(),
        )
    }
}

impl Render for Editor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let off = self.saving;
        let kinds = [
            ("once", "scheduled_once"),
            ("weekly", "scheduled_weekly"),
            ("interval", "scheduled_interval"),
        ];
        let choose_kind = div().flex().gap_1().children(kinds.map(|(kind, key)| {
            Button::new(key)
                .outline()
                .small()
                .flex_1()
                .selected(self.kind == kind)
                .label(t(key))
                .disabled(off)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.kind = kind;
                    cx.notify();
                }))
        }));
        let mut when = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(field("scheduled_when", choose_kind.into_any_element()));
        if self.kind == "weekly" {
            when = when.child(
                div()
                    .flex()
                    .gap_1()
                    .children([1, 2, 3, 4, 5, 6, 0].map(|day| {
                        Button::new(SharedString::from(format!("scheduled-day-{day}")))
                            .outline()
                            .small()
                            .flex_1()
                            .selected(self.weekdays.contains(&day))
                            .label(weekday(day))
                            .disabled(off)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.weekdays.contains(&day) {
                                    this.weekdays.retain(|d| *d != day);
                                } else {
                                    this.weekdays.push(day);
                                    this.weekdays.sort();
                                }
                                cx.notify();
                            }))
                    })),
            );
        }
        if self.kind == "interval" {
            when = when
                .child(self.input(&self.interval, "scheduled_every"))
                .child(
                    div().flex().gap_1().children(
                        [
                            (1, "scheduled_minutes"),
                            (60, "scheduled_hours"),
                            (1440, "scheduled_days"),
                        ]
                        .map(|(unit, key)| {
                            Button::new(key)
                                .ghost()
                                .small()
                                .label(t(key))
                                .selected(self.unit == unit)
                                .disabled(off)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.unit = unit;
                                    cx.notify();
                                }))
                        }),
                    ),
                );
        } else {
            when = when.child(
                div()
                    .flex()
                    .gap_3()
                    .when(self.kind == "once", |el| {
                        el.child(div().flex_1().child(field(
                            "scheduled_date",
                            DatePicker::new(&self.date).disabled(off).into_any_element(),
                        )))
                    })
                    .child(div().flex_1().child(field(
                        "scheduled_time",
                        TimeField::new(&self.time).disabled(off).into_any_element(),
                    ))),
            );
        }
        when = when.child(muted(tr_shared(
            "scheduled_timezone",
            &[("timezone", &self.timezone)],
        )));
        let new_target = self.target["kind"] == "new";
        let where_to = div().flex().gap_2().children(
            [
                (true, "scheduled_target_new"),
                (false, "scheduled_target_session"),
            ]
            .map(|(new, key)| {
                Button::new(key)
                    .outline()
                    .small()
                    .flex_1()
                    .label(t(key))
                    .selected(new_target == new)
                    .disabled(off)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if (this.target["kind"] == "new") != new {
                            this.target = json!({"kind":if new {"new"} else {"session"}});
                        }
                        cx.notify();
                    }))
            }),
        );
        let destination = if new_target {
            let launch = &self.target["launch"];
            let folder = launch["cwd"].as_str().unwrap_or_default();
            let summary = [
                launch["provider"].as_str().unwrap_or_default(),
                launch["model"].as_str().unwrap_or_default(),
            ]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
            div()
                .flex()
                .flex_col()
                .gap_2()
                .when(!folder.is_empty(), |el| {
                    el.child(div().text_sm().whitespace_normal().child(folder.to_owned()))
                        .child(muted(summary))
                })
                .when(self.target["worktree"] == true, |el| {
                    el.child(muted(t("scheduled_worktree"))).child(muted(
                        self.target["base"].as_str().unwrap_or_default().to_owned(),
                    ))
                })
                .child(
                    Button::new("scheduled-launch-options")
                        .outline()
                        .icon(IconName::FolderOpen)
                        .label(t(if folder.is_empty() {
                            "scheduled_choose_folder"
                        } else {
                            "scheduled_advanced"
                        }))
                        .disabled(off || self.choosing)
                        .on_click(cx.listener(|this, _, _, cx| this.choose_launch(cx))),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(self.session_picker(cx))
                .child(muted(t("scheduled_session_queue_hint")))
                .into_any_element()
        };
        let content = div()
            .id("scheduled-editor-content")
            .max_h(px(580.))
            .overflow_y_scroll()
            .relative()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.input(&self.name, "scheduled_name"))
            .child(field(
                "scheduled_prompt",
                Textarea::new(&self.prompt)
                    .disabled(off)
                    .aria_label(t("scheduled_prompt"))
                    .into_any_element(),
            ))
            .child(when)
            .child(
                div()
                    .pt_4()
                    .border_t_1()
                    .border_color(theme::border())
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(field("scheduled_where", where_to.into_any_element()))
                    .child(destination),
            )
            .child(
                Checkbox::new("scheduled-enabled")
                    .label(t("scheduled_enabled_on_save"))
                    .checked(self.enabled)
                    .disabled(off)
                    .on_click(cx.listener(|this, checked, _, cx| {
                        this.enabled = *checked;
                        cx.notify();
                    })),
            );
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(content)
            .when_some(self.error.clone(), |el, error| {
                el.child(alert("scheduled-form-error", error))
            })
            .child(
                div()
                    .pt_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("scheduled-cancel")
                            .ghost()
                            .label(t(if self.conflict {
                                "scheduled_retry"
                            } else {
                                "scheduled_cancel"
                            }))
                            .disabled(off)
                            .on_click(cx.listener(|this, _, window, cx| this.discard(window, cx))),
                    )
                    .child(
                        Button::new("scheduled-save")
                            .primary()
                            .label(t(if off {
                                "scheduled_saving"
                            } else if self.id.is_some() {
                                "scheduled_save"
                            } else {
                                "scheduled_create"
                            }))
                            .loading(off)
                            .disabled(off || self.conflict)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

fn field(key: &'static str, control: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(t(key)),
        )
        .child(control)
        .into_any_element()
}

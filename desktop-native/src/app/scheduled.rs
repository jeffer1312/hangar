//! Tarefas da máquina escolhida: lista e rascunhos permanecem presos à conexão onde nasceram.
mod editor;
mod filter;

use super::vault::{alert, muted};
use super::*;
use gpui_kit::component::switch::Switch;
use serde::{Deserialize, Serialize};

use editor::Editor;
use filter::Filter;

#[derive(Clone, Deserialize, Serialize)]
struct Task {
    id: String,
    revision: u64,
    name: String,
    prompt: String,
    enabled: bool,
    schedule: Value,
    target: Value,
    next_run_at: Option<String>,
    last_run_at: Option<String>,
    last_status: String,
    last_error: Option<String>,
    last_session: Option<String>,
}

#[derive(Deserialize)]
struct Snapshot {
    tasks: Vec<Task>,
    server_time: String,
    timezone: String,
}

#[derive(Default)]
pub(super) struct Scheduled {
    pages: HashMap<String, Entity<TaskPage>>,
    pub(super) chooser: Option<WeakEntity<Editor>>,
}

pub(super) enum Reply {
    ChooseLaunch(Api, WeakEntity<Editor>),
    Launch(Value),
}

struct TaskPage {
    api: Api,
    runtime: Arc<Runtime>,
    tx: async_channel::Sender<Envelope>,
    connection: u64,
    host: WeakEntity<Hangar>,
    snapshot: Option<Snapshot>,
    sessions: Option<Result<Vec<Value>, String>>,
    loading: bool,
    load_seq: u64,
    refresh: bool,
    error: Option<String>,
    notice: Option<String>,
    filter: Filter,
    busy: HashSet<String>,
    draft: Option<Entity<Editor>>,
}

impl Hangar {
    pub(super) fn scheduled_opened(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self
            .api
            .as_ref()
            .and_then(|api| self.scheduled.pages.get(&api.identity()))
        {
            page.update(cx, |page, cx| {
                page.refresh = true;
                cx.notify();
            });
        }
        cx.notify();
    }

    pub(super) fn render_scheduled(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(api) = self.api.clone() else {
            return div()
                .child(muted(t("scheduled_load_failed")))
                .into_any_element();
        };
        let key = api.identity();
        let (current_api, connection) = (api.clone(), self.connection);
        let page = if let Some(page) = self.scheduled.pages.get(&key) {
            page.clone()
        } else {
            let (runtime, tx, connection, host) = (
                self.runtime.clone(),
                self.tx.clone(),
                self.connection,
                cx.entity().downgrade(),
            );
            let page = cx.new(|cx| {
                let mut page = TaskPage {
                    api,
                    runtime,
                    tx,
                    connection,
                    host,
                    snapshot: None,
                    sessions: None,
                    loading: false,
                    load_seq: 0,
                    refresh: false,
                    error: None,
                    notice: None,
                    filter: Filter::All,
                    busy: HashSet::new(),
                    draft: None,
                };
                page.load(window, cx);
                page
            });
            self.scheduled.pages.insert(key, page.clone());
            page
        };
        page.update(cx, |page, cx| {
            // Outro token no mesmo endereço: o que foi lido com o anterior não vale, e a leitura recomeça.
            if !page.api.same_credential(&current_api) {
                (page.snapshot, page.sessions, page.error, page.notice) = (None, None, None, None);
                page.refresh = true;
            }
            page.api = current_api.clone();
            page.connection = connection;
            if let Some(draft) = &page.draft {
                draft.update(cx, |draft, _| {
                    draft.api = current_api.clone();
                    draft.connection = connection;
                });
            }
            if page.refresh {
                page.refresh = false;
                page.load(window, cx);
            }
        });
        div().child(page).into_any_element()
    }

    pub(super) fn receive_scheduled(
        &mut self,
        reply: Reply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match reply {
            Reply::ChooseLaunch(api, editor) => {
                self.scheduled.chooser = Some(editor);
                self.open_scheduled_launch(api, window, cx);
            }
            Reply::Launch(target) => {
                if let Some(editor) = self.scheduled.chooser.take() {
                    let _ = editor.update(cx, |editor, cx| {
                        editor.target = target;
                        editor.choosing = false;
                        cx.notify();
                    });
                    window.close_dialog(cx);
                }
            }
        }
        cx.notify();
    }
}

impl TaskPage {
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_seq += 1;
        let seq = self.load_seq;
        self.loading = true;
        self.error = None;
        let api = self.api.clone();
        run(
            &self.runtime,
            async move {
                Ok(tokio::join!(
                    api.server_read(&["scheduled-tasks"], &[], 20),
                    api.server_read(&["sessions"], &[], 20)
                ))
            },
            window,
            cx,
            move |this, result, _, _| {
                if seq != this.load_seq {
                    return;
                }
                let (tasks, sessions) = match result {
                    Ok(pair) => pair,
                    Err(error) => {
                        this.loading = false;
                        this.error = Some(failure(&error));
                        return;
                    }
                };
                this.loading = false;
                this.sessions =
                    Some(sessions.map_err(|e| failure(&e)).and_then(|v| {
                        serde_json::from_value(v).map_err(|_| tr("invalid_response"))
                    }));
                match tasks.and_then(|v| {
                    serde_json::from_value::<Snapshot>(v)
                        .map_err(|_| Failure::local(tr("invalid_response")))
                }) {
                    Ok(snapshot) => this.snapshot = Some(snapshot),
                    Err(error) => this.error = Some(failure(&error)),
                }
            },
        );
        cx.notify();
    }

    fn open_editor(&mut self, task: Option<Task>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            return;
        };
        // Fechar a folha mantém o rascunho. Ao pedir outra tarefa, a pessoa precisa escolher descartá-lo.
        if let Some(draft) = self.draft.clone() {
            if draft.read(cx).id != task.as_ref().map(|t| t.id.clone()) {
                let weak = cx.entity().downgrade();
                confirmation(
                    window,
                    cx,
                    t("scheduled_discard_title"),
                    vec![t("scheduled_discard_body")],
                    "scheduled_keep_editing",
                    "scheduled_discard",
                    move |window, cx| {
                        let _ = weak.update(cx, |page, _| {
                            page.draft = None;
                        });
                        let (weak, task) = (weak.clone(), task.clone());
                        window.on_next_frame(move |window, cx| {
                            let _ = weak.update(cx, |page, cx| page.open_editor(task, window, cx));
                        });
                    },
                );
                return;
            }
            self.show_editor(draft, window, cx);
            return;
        }
        let (api, runtime, page, tx, connection) = (
            self.api.clone(),
            self.runtime.clone(),
            cx.entity().downgrade(),
            self.tx.clone(),
            self.connection,
        );
        let (timezone, server_time) = (snapshot.timezone.clone(), snapshot.server_time.clone());
        let sessions = self.sessions.as_ref().cloned();
        let editor = cx.new(|cx| {
            Editor::new(
                api,
                runtime,
                page,
                tx,
                connection,
                task,
                timezone,
                server_time,
                sessions,
                window,
                cx,
            )
        });
        self.draft = Some(editor.clone());
        self.show_editor(editor, window, cx);
    }

    fn show_editor(&self, editor: Entity<Editor>, window: &mut Window, cx: &mut Context<Self>) {
        window.open_dialog(cx, move |d, _, cx| {
            let busy = editor.read(cx).saving;
            let submit = editor.clone();
            // Esc/fechar conserva o rascunho da máquina; Cancelar também avisa antes de descartar.
            popup::dialog(d)
                .title(t(if editor.read(cx).id.is_some() {
                    "scheduled_edit"
                } else {
                    "scheduled_new"
                }))
                .w(px(660.))
                .max_w_full()
                .overlay_closable(false)
                .keyboard(!busy)
                .close_button(!busy)
                .on_ok(move |_, window, cx| {
                    submit.update(cx, |editor, cx| editor.save(window, cx));
                    false
                })
                .child(editor.clone())
                .on_close({
                    let page = editor.read(cx).page.clone();
                    move |_, _, cx| {
                        let _ = page.update(cx, |page, cx| {
                            if page.draft.is_some() {
                                page.notice = Some(t("scheduled_draft_kept"));
                                cx.notify();
                            }
                        });
                    }
                })
        });
    }

    fn action(
        &mut self,
        task: Task,
        action: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.busy.insert(task.id.clone()) {
            return;
        }
        self.error = None;
        self.notice = None;
        let api = self.api.clone();
        let id = task.id.clone();
        run(
            &self.runtime,
            async move {
                match action {
                    "run" => {
                        api.server_send(
                            reqwest::Method::POST,
                            &["scheduled-tasks", &id, "run"],
                            None,
                            30,
                        )
                        .await
                    }
                    "delete" => {
                        api.server_send_query(
                            reqwest::Method::DELETE,
                            &["scheduled-tasks", &id],
                            &[("revision", &task.revision.to_string())],
                            None,
                            20,
                        )
                        .await
                    }
                    _ => {
                        api.server_send(
                            reqwest::Method::PATCH,
                            &["scheduled-tasks", &id, "enabled"],
                            Some(json!({"enabled":!task.enabled,"revision":task.revision})),
                            20,
                        )
                        .await
                    }
                }
            },
            window,
            cx,
            move |this, result, window, cx| {
                this.busy.remove(&task.id);
                match result {
                    Ok(_) => {
                        this.notice = Some(t(match action {
                            "run" => "scheduled_run_started",
                            "delete" => "scheduled_deleted",
                            _ => "scheduled_saved",
                        }));
                        this.load(window, cx);
                    }
                    Err(error) => {
                        this.error = Some(if action == "run" && error.uncertain {
                            t("scheduled_status_uncertain")
                        } else {
                            failure(&error)
                        })
                    }
                }
            },
        );
        cx.notify();
    }

    fn confirm_delete(&self, task: Task, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        confirmation(
            window,
            cx,
            t("scheduled_delete_title"),
            vec![task.name.clone(), t("scheduled_delete_body")],
            "scheduled_keep",
            "scheduled_confirm_delete",
            move |window, cx| {
                let _ = weak.update(cx, |page, cx| {
                    page.action(task.clone(), "delete", window, cx)
                });
            },
        );
    }

    fn row(&self, task: &Task, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy.contains(&task.id);
        let toggle_task = task.clone();
        let edit = task.clone();
        let run_task = task.clone();
        let delete_task = task.clone();
        let schedule = schedule_label(
            &task.schedule,
            self.snapshot
                .as_ref()
                .map(|s| s.timezone.as_str())
                .unwrap_or("UTC"),
        );
        let next = task
            .next_run_at
            .as_deref()
            .map(|v| {
                tr_shared(
                    "scheduled_next",
                    &[(
                        "time",
                        &display_time(
                            v,
                            self.snapshot
                                .as_ref()
                                .map(|s| s.timezone.as_str())
                                .unwrap_or("UTC"),
                        ),
                    )],
                )
            })
            .unwrap_or_else(|| {
                t(if task.enabled {
                    "scheduled_not_scheduled"
                } else {
                    "scheduled_status_paused"
                })
            });
        let status = t(&format!("scheduled_status_{}", task.last_status));
        let last = task
            .last_run_at
            .as_deref()
            .map(|v| {
                tr_shared(
                    "scheduled_last",
                    &[(
                        "time",
                        &display_time(
                            v,
                            self.snapshot
                                .as_ref()
                                .map(|s| s.timezone.as_str())
                                .unwrap_or("UTC"),
                        ),
                    )],
                )
            })
            .unwrap_or_else(|| t("scheduled_never"));
        let weak = cx.entity().downgrade();
        let menu = Button::new(SharedString::from(format!("scheduled-actions-{}", task.id)))
            .ghost()
            .small()
            .icon(IconName::Ellipsis)
            .accessibility_label(t("scheduled_actions"))
            .disabled(busy)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let (edit, run_task, delete_task) =
                    (edit.clone(), run_task.clone(), delete_task.clone());
                let (edit_weak, run_weak, delete_weak) = (weak.clone(), weak.clone(), weak.clone());
                sidebar::menu_style(menu)
                    .item(
                        PopupMenuItem::new(t("scheduled_edit"))
                            .icon(IconName::Pencil)
                            .on_click(move |_, window, cx| {
                                let _ = edit_weak.update(cx, |page, cx| {
                                    page.open_editor(Some(edit.clone()), window, cx)
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new(t("scheduled_run_now"))
                            .icon(IconName::Play)
                            .on_click(move |_, window, cx| {
                                let _ = run_weak.update(cx, |page, cx| {
                                    page.action(run_task.clone(), "run", window, cx)
                                });
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new(t("scheduled_delete"))
                            .icon(IconName::Trash)
                            .on_click(move |_, window, cx| {
                                let _ = delete_weak.update(cx, |page, cx| {
                                    page.confirm_delete(delete_task.clone(), window, cx)
                                });
                            }),
                    )
            });
        let text = row_text(
            task.name.clone(),
            task.prompt.clone(),
            schedule,
            next,
            if task.last_run_at.is_some() {
                format!("{status} · {last}")
            } else {
                status
            },
            task.enabled,
        )
        .children(task.last_error.clone().map(|error| {
            div()
                .text_xs()
                .text_color(theme::danger())
                .whitespace_normal()
                .child(error)
        }));
        let destination = if task.target["kind"] == "session" {
            task.target["name"].as_str().unwrap_or_default().to_owned()
        } else {
            [
                task.target["launch"]["cwd"].as_str().unwrap_or_default(),
                task.target["launch"]["provider"]
                    .as_str()
                    .unwrap_or_default(),
            ]
            .join(" · ")
        };
        let last_session = task.last_session.clone();
        let api = self.api.clone();
        let host = self.host.clone();
        let open = last_session.map(|name| {
            Button::new(SharedString::from(format!("scheduled-open-{}", task.id)))
                .ghost()
                .small()
                .label(t("scheduled_last_session"))
                .self_start()
                .on_click(move |_, window, cx| {
                    let (api, name, host) = (api.clone(), name.clone(), host.clone());
                    let _ = host.update(cx, |this, cx| {
                        if let Some(session) = this
                            .sessions_of(&api.identity())
                            .iter()
                            .find(|s| s.name == name)
                            .cloned()
                        {
                            this.close_settings(window, cx);
                            this.select_on(&api.identity(), session, window, cx);
                        } else {
                            window.push_notification(
                                Notification::warning(t("scheduled_unavailable_session")),
                                cx,
                            );
                        }
                    });
                })
        });
        row_frame(SharedString::from(format!("scheduled-row-{}", task.id)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(text)
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme::muted())
                            .truncate()
                            .child(destination),
                    )
                    .children(open),
            )
            .child(
                Switch::new(SharedString::from(format!("scheduled-toggle-{}", task.id)))
                    .checked(task.enabled)
                    .disabled(busy)
                    .accessibility_label(t(if task.enabled {
                        "scheduled_pause"
                    } else {
                        "scheduled_activate"
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.action(toggle_task.clone(), "toggle", window, cx)
                    })),
            )
            .child(menu)
            .into_any_element()
    }
}

impl Render for TaskPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut page = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t("scheduled_title")),
                    )
                    .child(
                        Button::new("scheduled-new")
                            .primary()
                            .icon(IconName::Plus)
                            .label(t("scheduled_new"))
                            .disabled(self.snapshot.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_editor(None, window, cx)
                            })),
                    ),
            )
            .child(muted(t("scheduled_intro")))
            .child(
                div().flex().justify_end().child(
                    Button::new("scheduled-refresh")
                        .ghost()
                        .small()
                        .icon(IconName::RefreshCw)
                        .label(t("scheduled_refresh"))
                        .disabled(self.loading)
                        .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                ),
            );
        if let Some(snapshot) = &self.snapshot {
            page = page.child(muted(tr_shared(
                "scheduled_timezone",
                &[("timezone", &snapshot.timezone)],
            )));
        }
        let filters = [
            (Filter::All, "scheduled_all"),
            (Filter::Active, "scheduled_active"),
            (Filter::Paused, "scheduled_paused"),
        ];
        page = page
            .child(
                div()
                    .flex()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .pb_2()
                    .children(filters.map(|(filter, key)| {
                        Button::new(key)
                            .ghost()
                            .small()
                            .selected(self.filter == filter)
                            .label(t(key))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.filter = filter;
                                cx.notify();
                            }))
                    })),
            )
            .when_some(self.notice.clone(), |el, notice| {
                el.child(
                    div()
                        .id("scheduled-notice")
                        .role(Role::Status)
                        .text_sm()
                        .text_color(theme::accent_text())
                        .child(notice),
                )
            })
            .when_some(self.error.clone(), |el, error| {
                el.child(alert("scheduled-error", error)).child(
                    Button::new("scheduled-retry")
                        .outline()
                        .small()
                        .label(t("scheduled_retry"))
                        .on_click(cx.listener(|this, _, window, cx| this.load(window, cx))),
                )
            });
        if self.loading {
            page = page.child(
                div()
                    .id("scheduled-loading")
                    .role(Role::Status)
                    .child(muted(t("scheduled_loading"))),
            );
        }
        if let Some(snapshot) = &self.snapshot {
            let tasks: Vec<_> = snapshot
                .tasks
                .iter()
                .filter(|t| self.filter.includes(t.enabled))
                .cloned()
                .collect();
            if tasks.is_empty() {
                page = page.child(
                    div()
                        .py(px(46.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_2()
                        .child(chrome::small_icon(IconName::Clock, 28., theme::faint()))
                        .child(div().text_base().child(t("scheduled_empty")))
                        .child(muted(t("scheduled_empty_hint"))),
                );
            } else {
                page = page.child(div().children(tasks.iter().map(|task| self.row(task, cx))));
            }
        }
        page.child(
            div()
                .pt_4()
                .border_t_1()
                .border_color(theme::border())
                .child(muted(t("scheduled_runs_closed"))),
        )
    }
}

pub(crate) fn row_frame(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .py(px(22.))
        .flex()
        .items_start()
        .gap(px(14.))
        .border_b_1()
        .border_color(theme::border())
}

pub(crate) fn row_text(
    name: String,
    prompt: String,
    schedule: String,
    next: String,
    last: String,
    enabled: bool,
) -> Div {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(name),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme::muted())
                .whitespace_normal()
                .child(prompt),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .text_xs()
                .child(chrome::small_icon(IconName::Clock, 13., theme::faint()))
                .child(schedule)
                .child(
                    div()
                        .text_color(if enabled {
                            theme::accent_text()
                        } else {
                            theme::muted()
                        })
                        .child(next),
                ),
        )
        .child(div().text_xs().text_color(theme::muted()).child(last))
}

fn t(key: &str) -> String {
    tr_shared(key, &[])
}
fn failure(error: &Failure) -> String {
    if error.status == Some(409)
        && error
            .code
            .as_deref()
            .is_some_and(|code| code == "scheduled_revision_conflict")
    {
        t("scheduled_revision_conflict")
    } else {
        tr_shared(
            "scheduled_operation_failed",
            &[("error", &Hangar::fetch_failure(error))],
        )
    }
}
fn display_time(value: &str, timezone: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| {
            if let Ok(zone) = timezone.parse::<chrono_tz::Tz>() {
                date.with_timezone(&zone)
                    .format("%d/%m/%Y %H:%M")
                    .to_string()
            } else {
                date.format("%d/%m/%Y %H:%M %:z").to_string()
            }
        })
        .unwrap_or_else(|| value.to_owned())
}
fn schedule_label(schedule: &Value, timezone: &str) -> String {
    match schedule["kind"].as_str() {
        Some("once") => tr_shared(
            "scheduled_schedule_once",
            &[(
                "time",
                &display_time(schedule["at"].as_str().unwrap_or_default(), timezone),
            )],
        ),
        Some("interval") => tr_shared(
            "scheduled_schedule_interval",
            &[("minutes", &schedule["minutes"].to_string())],
        ),
        _ => {
            let days: Vec<_> = schedule["weekdays"]
                .as_array()
                .map(|days| {
                    days.iter()
                        .filter_map(Value::as_u64)
                        .map(|day| weekday(day as usize))
                        .collect()
                })
                .unwrap_or_default();
            tr_shared(
                "scheduled_schedule_weekly",
                &[
                    ("days", &days.join(", ")),
                    ("time", schedule["time"].as_str().unwrap_or_default()),
                ],
            )
        }
    }
}
fn weekday(day: usize) -> String {
    t([
        "scheduled_weekday_sun",
        "scheduled_weekday_mon",
        "scheduled_weekday_tue",
        "scheduled_weekday_wed",
        "scheduled_weekday_thu",
        "scheduled_weekday_fri",
        "scheduled_weekday_sat",
    ][day.min(6)])
}

fn confirmation(
    window: &mut Window,
    cx: &mut App,
    title: String,
    lines: Vec<String>,
    cancel: &'static str,
    accept: &'static str,
    approve: impl Fn(&mut Window, &mut App) + 'static,
) {
    let approve = std::rc::Rc::new(approve);
    window.open_dialog(cx, move |d, _, _| {
        let (enter, click) = (approve.clone(), approve.clone());
        popup::dialog(d)
            .title(title.clone())
            .children(
                lines
                    .iter()
                    .cloned()
                    .map(|line| div().text_sm().whitespace_normal().child(line)),
            )
            .overlay_closable(false)
            .on_ok(move |_, window, cx| {
                window.close_dialog(cx);
                enter(window, cx);
                false
            })
            .footer(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("scheduled-confirm-cancel")
                            .outline()
                            .label(t(cancel))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("scheduled-confirm-accept")
                            .primary()
                            .label(t(accept))
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                click(window, cx);
                            }),
                    ),
            )
    });
}

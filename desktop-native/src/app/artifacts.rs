//! Catálogo passivo da conversa: só títulos e URLs reais, abertos no navegador do sistema.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArtifactOwner { key: SessionKey, lifecycle: Option<String>, provider: String }

#[derive(Clone, Debug, serde::Deserialize)]
pub(super) struct ArtifactItem { id: String, url: String, title: String, #[serde(default)] version: Option<String> }

impl ArtifactItem {
    fn external_url(&self) -> Option<String> {
        if self.url.chars().any(char::is_control) { return None; }
        let url = url::Url::parse(&self.url).ok()?;
        if url.scheme() != "https" || url.host_str() != Some("claude.ai") || url.port_or_known_default() != Some(443)
            || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() { return None; }
        let parts = url.path_segments()?.collect::<Vec<_>>();
        let id = match parts.as_slice() { ["artifact", id] | ["code", "artifact", id] => *id, _ => return None };
        if id != self.id || id.is_empty() || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')) { return None; }
        Some(url.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ArtifactRequest { owner: ArtifactOwner, generation: u64 }

#[derive(Default)]
pub(super) struct Artifacts {
    owner: Option<ArtifactOwner>, generation: u64, pub open: bool,
    loading: bool, items: Option<Vec<ArtifactItem>>, error: Option<String>,
}

impl Artifacts {
    pub fn items(&self) -> Option<&[ArtifactItem]> { self.items.as_deref() }
    pub fn set_owner(&mut self, owner: Option<ArtifactOwner>) {
        if self.owner != owner { self.owner = owner; self.invalidate(); self.open = false; }
    }
    pub fn invalidate(&mut self) { self.generation += 1; self.loading = false; self.items = None; self.error = None; }
    pub fn begin(&mut self) -> Option<ArtifactRequest> {
        let owner = self.owner.clone()?;
        self.generation += 1;
        self.loading = true;
        self.error = None;
        Some(ArtifactRequest { owner, generation: self.generation })
    }
    pub fn accept(&mut self, request: &ArtifactRequest, result: Result<Vec<ArtifactItem>, String>) -> bool {
        if self.owner.as_ref() != Some(&request.owner) || self.generation != request.generation { return false; }
        self.loading = false;
        match result {
            Ok(items) => {
                let mut unique: Vec<ArtifactItem> = Vec::new();
                for mut item in items {
                    let Some(url) = item.external_url() else { continue; };
                    item.url = url;
                    if let Some(previous) = unique.iter_mut().find(|previous| previous.id == item.id) { *previous = item; }
                    else { unique.push(item); }
                }
                self.items = Some(unique);
            }
            Err(error) => { self.items = None; self.error = Some(error); },
        }
        true
    }
}

async fn fetch_artifacts(api: Api, request: ArtifactRequest, connection: u64) -> Envelope {
    #[derive(serde::Deserialize)]
    struct Listing { items: Vec<ArtifactItem> }
    let result = api.read(&request.owner.key.name, &["artifacts"], &[], 15).await
        .and_then(|value| serde_json::from_value::<Listing>(value).map(|body| body.items).map_err(|_| Failure::local("invalid_response")));
    // Dono e geração protegem o catálogo; reselecionar a mesma aba só muda a seleção do chat.
    Envelope { connection, selection: None, payload: Payload::Artifacts(request, result) }
}

impl Hangar {
    fn artifact_owner(&self) -> Option<ArtifactOwner> {
        let session = self.selected.as_ref().filter(|session| session.readable() && session.provider == "claude")?;
        if self.open_read_only() || self.session_server().and_then(|server| self.server_entry(&servers::norm(&server))).is_some_and(|entry| entry.invite) { return None; }
        Some(ArtifactOwner { key: self.selected_key()?, lifecycle: session.lifecycle_id.clone(), provider: session.provider.clone() })
    }

    pub(super) fn sync_artifacts_owner(&mut self, cx: &mut Context<Self>) {
        let owner = self.artifact_owner();
        if self.artifacts.owner != owner {
            self.artifacts.set_owner(owner);
            self.refresh_artifacts(cx);
        }
    }

    pub(super) fn reset_artifacts(&mut self, cx: &mut Context<Self>) {
        self.artifacts.invalidate();
        self.refresh_artifacts(cx);
    }

    fn refresh_artifacts(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.session_api() else { return };
        let Some(request) = self.artifacts.begin() else { return };
        let (tx, connection) = (self.tx.clone(), self.connection);
        self.runtime.spawn(async move {
            let _ = tx.send(fetch_artifacts(api, request, connection).await).await;
        });
        self.redraw(panes::Area::Bottom, cx);
        cx.notify();
    }

    pub(super) fn receive_artifacts(&mut self, request: ArtifactRequest, result: Result<Vec<ArtifactItem>, Failure>, cx: &mut Context<Self>) {
        if self.artifacts.accept(&request, result.map_err(|error| Self::fetch_failure(&error))) {
            self.redraw(panes::Area::Bottom, cx);
            cx.notify();
        }
    }

    pub(super) fn render_artifacts_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let owner = self.artifact_owner()?;
        let count = if self.artifacts.owner.as_ref() == Some(&owner) {
            self.artifacts.items().map(|items| items.len().to_string()).unwrap_or_else(|| "…".into())
        } else { "…".into() };
        let label = tr("artifacts_count").replace("{n}", &count);
        // Conversa nova: "Artefatos 0" em 13 px, o número na cor do texto.
        let beta = crate::appearance::conversation_beta();
        let button = Button::new("artifacts-toggle").ghost().xsmall().h(px(22.)).px(px(6.));
        let button = if beta {
            button.accessibility_label(label).child(div().flex().items_center().gap(px(6.)).text_size(px(13.)).text_color(theme::muted())
                .child(chrome::small_icon(IconName::ExternalLink, 14., theme::muted())).child(tr("beta_artifacts"))
                .child(div().text_color(theme::text()).child(count)))
        } else { button.icon(IconName::ExternalLink).label(label) };
        Some(popup::anchor(div().relative(), "artifacts-toggle")
            .child(button.tooltip(tr("artifacts_open")).selected(self.artifacts.open)
                .on_click(cx.listener(|this, _, _, cx| {
                    let opening = !this.artifacts.open;
                    this.close_popups();
                    this.artifacts.open = opening;
                    if opening { this.refresh_artifacts(cx); }
                    cx.notify();
                }))).into_any_element())
    }

    pub(super) fn render_artifacts_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.artifacts;
        div().id("artifacts-panel").w(px(380.).min(window.viewport_size().width - px(32.))).flex().flex_col().gap_2().p_3()
            .child(div().flex().items_center().gap_2().text_sm().font_weight(FontWeight::SEMIBOLD)
                .child(tr("artifacts_title")).child(div().flex_1())
                .child(chrome::icon_button("artifacts-refresh", IconName::RefreshCw, tr("artifacts_refresh"), cx).disabled(state.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_artifacts(cx)))))
            .child(div().text_xs().text_color(theme::faint()).child(tr("artifacts_external")))
            .when(state.loading, |el| el.child(div().id("artifacts-loading").py_3().text_sm().text_color(theme::muted()).child(tr("artifacts_loading"))))
            .when_some(state.error.as_ref(), |el, error| el.child(div().id("artifacts-error").role(Role::Alert).flex().flex_col().gap_2().py_2()
                .child(div().text_sm().text_color(theme::danger()).child(error.clone()))
                .child(Button::new("artifacts-retry").ghost().small().label(tr("retry"))
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_artifacts(cx))))))
            .when(!state.loading && state.error.is_none() && state.items().is_some_and(|items| items.is_empty()), |el|
                el.child(div().id("artifacts-empty").py_4().text_sm().text_color(theme::muted()).child(tr("artifacts_empty"))))
            .when(!state.loading, |el| el.child(div().id("artifacts-list").max_h(px(320.)).overflow_y_scroll().flex().flex_col().gap_1()
                .children(state.items().into_iter().flatten().enumerate().map(|(ix, item)| {
                    let url = item.url.clone();
                    let title = if item.title.trim().is_empty() { tr("artifacts_untitled") } else { item.title.clone() };
                    popup::row(("artifact-item", ix), false).accessibility_label(title.clone()).tooltip(url.clone())
                        .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                            .child(div().truncate().child(title))
                            .when_some(item.version.as_ref(), |el, version| el.child(div().text_xs().text_color(theme::faint())
                                .child(tr("artifacts_version").replace("{version}", version)))))
                        .child(chrome::small_icon(IconName::ExternalLink, 13., theme::faint()))
                        .on_click(cx.listener(move |this, event, window, cx| {
                            open_web_link(&url.clone().into(), event, window, cx);
                            this.artifacts.open = false;
                            cx.notify();
                        }))
                }))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{ArtifactOwner, Artifacts};
    use crate::delivery::SessionKey;

    #[tokio::test]
    async fn delayed_catalog_response_finishes_loading_after_reselecting_the_same_owner() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            loop {
                let mut line = String::new();
                assert!(stream.read_line(&mut line).await.unwrap() > 0);
                if line == "\r\n" { break; }
            }
            let body = r#"{"items":[{"id":"same-owner","url":"https://claude.ai/artifact/same-owner","title":"Resposta tardia preservada","version":"fixture-v1"}]}"#;
            let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let owner = ArtifactOwner { key: SessionKey { server: "fixture".into(), name: "same".into(), jsonl: "one".into() }, lifecycle: Some("life-1".into()), provider: "claude".into() };
        let mut catalog = Artifacts::default();
        catalog.set_owner(Some(owner.clone()));
        let request = catalog.begin().unwrap();
        let api = crate::api::Api::new(&format!("http://{address}"), "fixture-only").unwrap();
        let reply = super::fetch_artifacts(api, request, 11).await;
        server.await.unwrap();
        catalog.set_owner(Some(owner));
        assert!(!super::super::delivery_is_current(reply.connection, reply.selection, 12, 8));
        if super::super::delivery_is_current(reply.connection, reply.selection, 11, 8) {
            let super::super::Payload::Artifacts(request, result) = reply.payload else { panic!("tipo de resposta incorreto") };
            assert!(catalog.accept(&request, result.map_err(|error| error.detail)));
        }
        assert!(!catalog.loading, "reselecionar o mesmo dono não pode descartar a resposta e prender o carregamento");
        assert_eq!(catalog.items().unwrap()[0].title, "Resposta tardia preservada");
    }

    #[test]
    fn catalog_deduplicates_updates_and_only_keeps_real_artifact_destinations() {
        use super::ArtifactItem;
        let mut catalog = Artifacts::default();
        catalog.set_owner(Some(ArtifactOwner { key: SessionKey { server: "fixture".into(), name: "one".into(), jsonl: "one".into() }, lifecycle: None, provider: "claude".into() }));
        let request = catalog.begin().unwrap();
        let item = |id: &str, url: &str, version: &str| ArtifactItem { id: id.into(), url: url.into(), title: "fixture".into(), version: Some(version.into()) };
        let mut items = vec![item("abc", "https://claude.ai/artifact/abc", "1"), item("abc", "https://claude.ai/code/artifact/abc", "2")];
        items.extend([
            "https://claude.ai.evil.test/artifact/evil", "javascript:alert(1)", "https://claude.ai/chat/evil",
            "https://user:pass@claude.ai/artifact/evil", "http://claude.ai/artifact/evil", "https://claude.ai:444/artifact/evil",
            "https://claude.ai/artifact/evil?token=unsafe", "https://claude.ai/artifact/evil#fragment",
        ].into_iter().map(|url| item("evil", url, "1")));
        assert!(catalog.accept(&request, Ok(items)));
        let visible = catalog.items().unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "abc");
        assert_eq!(visible[0].version.as_deref(), Some("2"));
        assert_eq!(visible[0].url, "https://claude.ai/code/artifact/abc");
    }

    #[test]
    fn artifact_response_before_reset_cannot_restore_old_items_under_same_name() {
        let owner = ArtifactOwner { key: SessionKey { server: "fixture".into(), name: "same".into(), jsonl: "one".into() }, lifecycle: Some("life-1".into()), provider: "claude".into() };
        let mut catalog = Artifacts::default();
        catalog.set_owner(Some(owner));
        let before = catalog.begin().unwrap();
        catalog.invalidate();
        let after = catalog.begin().unwrap();
        assert!(!catalog.accept(&before, Ok(vec![])));
        assert!(catalog.accept(&after, Ok(vec![])));
    }
}

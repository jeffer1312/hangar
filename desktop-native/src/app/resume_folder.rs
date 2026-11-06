//! A pasta da conversa sumiu: navega pelas pastas da máquina dela e diz onde retomar.
use super::*;
use super::create::{Root, Scan, choice, crumbs, scan_of};
use super::device::Remote;

/// Só esta recusa abre o seletor: as outras não se resolvem trocando de pasta.
pub(super) const FOLDER_GONE: &str = "erro_cwd_inexistente";

pub(super) enum ResumeFolderEvent { Chosen(String), Closed }

pub(super) struct ResumeFolder {
    pub(super) focus: FocusHandle,
    api: Api,
    runtime: Arc<Runtime>,
    message: String,
    roots: Remote<Vec<Root>>,
    root: Option<Root>,
    dir: String,
    scan: Remote<Scan>,
}

impl EventEmitter<ResumeFolderEvent> for ResumeFolder {}

impl ResumeFolder {
    pub(super) fn new(api: Api, runtime: Arc<Runtime>, message: String, cx: &mut Context<Self>) -> Self {
        let mut this = Self { focus: cx.focus_handle(), api, runtime, message, roots: Remote::default(), root: None, dir: String::new(), scan: Remote::default() };
        this.load_roots(cx);
        this
    }

    fn load_roots(&mut self, cx: &mut Context<Self>) {
        let seq = self.roots.start();
        let api = self.api.clone();
        let task = self.runtime.spawn(async move {
            api.server_read(&["fs", "roots"], &[], 15).await.map_err(|e| Hangar::fetch_failure(&e))
                .and_then(|v| serde_json::from_value::<Vec<Root>>(v).map_err(|_| tr("invalid_response")))
        });
        cx.spawn(async move |this, cx| {
            let roots = task.await.unwrap_or_else(|_| Err(tr("search_interrupted")));
            let first = roots.as_ref().ok().and_then(|list| list.first().cloned());
            let _ = this.update(cx, |this, cx| {
                if !this.roots.finish(seq, roots) { return; }
                if let Some(root) = first { this.pick_root(root, cx); }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn pick_root(&mut self, root: Root, cx: &mut Context<Self>) {
        let path = root.path.clone();
        self.root = Some(root);
        self.open_dir(path, cx);
    }

    fn open_dir(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(root) = self.root.as_ref().map(|r| r.path.clone()) else { return };
        self.dir = path.clone();
        let seq = self.scan.start();
        let api = self.api.clone();
        let task = self.runtime.spawn(async move {
            let query: Vec<(&str, &str)> = if path == root { vec![("root", root.as_str())] } else { vec![("root", root.as_str()), ("path", path.as_str())] };
            scan_of(api.server_read(&["fs", "scan"], &query, 15).await)
        });
        cx.spawn(async move |this, cx| {
            let scan = task.await.unwrap_or_else(|_| Err(tr("search_interrupted")));
            let _ = this.update(cx, |this, cx| if this.scan.finish(seq, scan) { cx.notify(); });
        }).detach();
        cx.notify();
    }
}

impl Render for ResumeFolder {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = |text: String| div().text_sm().text_color(theme::muted()).whitespace_normal().child(text);
        let loading = |id: &'static str| div().id(id).role(Role::Status).text_sm().text_color(theme::muted()).child(tr_shared("comum_carregando", &[]));
        let alert = |id: &'static str, text: String| div().id(id).role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(text);
        let retry = |id: &'static str| Button::new(id).outline().small().label(tr_shared("lista_tentar_novamente", &[]));
        let title = tr_shared("retomar_escolher_pasta", &[]);
        let frame = div().id("resume-folder").track_focus(&self.focus).role(Role::Group).aria_label(title.clone()).w_full().flex().flex_col().gap_2().p_3().rounded(px(12.))
            .border_1().border_color(theme::border()).bg(theme::popup_fill(theme::raised())).shadow(theme::popover_shadow())
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(alert("resume-folder-gone", self.message.clone()));
        let close = Button::new("resume-folder-close").ghost().small().label(tr_shared("comum_cancelar", &[]))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(ResumeFolderEvent::Closed)));
        let roots = match self.roots.value.as_ref() {
            _ if self.roots.loading => return frame.child(loading("resume-folder-roots-loading")).child(div().child(close)),
            None => return frame.child(div().child(close)),
            Some(Err(error)) => return frame.child(alert("resume-folder-roots-error", format!("{} {error}", tr_shared("arquivo_carregar_raizes_erro", &[]))))
                .child(div().flex().gap_2().child(retry("resume-folder-roots-retry").on_click(cx.listener(|this, _, _, cx| this.load_roots(cx)))).child(close)),
            Some(Ok(list)) if list.is_empty() => return frame.child(muted(tr_shared("arquivo_sem_raizes", &[]))).child(div().child(close)),
            Some(Ok(list)) => list,
        };
        let Some(root) = self.root.as_ref() else { return frame.child(div().child(close)) };
        let chips = div().id("resume-folder-roots").role(Role::Group).aria_label(tr_shared("arquivo_raizes_aria", &[])).flex().flex_wrap().gap(px(6.))
            .children(roots.iter().map(|r| {
                let pick = r.clone();
                choice(SharedString::from(format!("resume-folder-root-{}", r.path)), r.path == root.path, cx).small().rounded_full()
                    .label(r.name.clone()).tooltip(r.path.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_root(pick.clone(), cx)))
            }));
        let trail = div().id("resume-folder-crumbs").aria_label(tr_shared("arquivo_caminho_aria", &[])).flex().flex_wrap().items_center().gap(px(2.))
            .children(crumbs(root, &self.dir).into_iter().enumerate().map(|(n, (text, path))| div().flex().items_center().gap(px(2.))
                .when(n > 0, |el| el.child(div().text_color(theme::faint()).text_size(px(12.)).child("/")))
                .child(Button::new(SharedString::from(format!("resume-folder-crumb-{path}"))).ghost().xsmall().label(text)
                    .on_click(cx.listener(move |this, _, _, cx| this.open_dir(path.clone(), cx))))));
        let rows = match self.scan.value.as_ref() {
            _ if self.scan.loading => loading("resume-folder-scan-loading").into_any_element(),
            None => div().into_any_element(),
            Some(Err(error)) => {
                let dir = self.dir.clone();
                div().flex().flex_col().gap_2().child(alert("resume-folder-scan-error", error.clone()))
                    .child(div().child(retry("resume-folder-scan-retry").on_click(cx.listener(move |this, _, _, cx| this.open_dir(dir.clone(), cx)))))
                    .into_any_element()
            }
            Some(Ok(scan)) if scan.error.is_some() => muted(scan.error.clone().unwrap_or_default()).into_any_element(),
            Some(Ok(scan)) if scan.entries.is_empty() => muted(tr_shared("arquivo_sem_subpastas", &[])).into_any_element(),
            Some(Ok(scan)) => div().id("resume-folder-list").max_h(px(240.)).overflow_y_scroll().flex().flex_col().gap(px(2.))
                .children(scan.entries.iter().map(|entry| {
                    let path = entry.path.clone();
                    Button::new(SharedString::from(format!("resume-folder-open-{}", entry.path))).ghost().small().justify_start()
                        .icon(IconName::Folder).label(entry.name.clone()).tooltip(entry.path.clone())
                        .on_click(cx.listener(move |this, _, _, cx| this.open_dir(path.clone(), cx)))
                })).into_any_element(),
        };
        let dir = self.dir.clone();
        // Durante a leitura `dir` já é a pasta nova e `scan` ainda é a anterior.
        let readable = !self.scan.loading && self.scan.ok().is_some_and(|scan| scan.error.is_none());
        frame.child(chips).child(trail).child(rows)
            .child(div().flex().items_center().gap_2()
                .child(Button::new("resume-folder-use").primary().small().icon(IconName::FolderOpen).label(tr_shared("arquivo_usar_pasta", &[]))
                    .disabled(!readable).on_click(cx.listener(move |_, _, _, cx| cx.emit(ResumeFolderEvent::Chosen(dir.clone())))))
                .child(close))
    }
}

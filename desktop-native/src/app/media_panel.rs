//! Aba "Mídia" do painel da direita: tudo que o agente citou e tudo que a pessoa mandou nesta conversa, também de
//! sessão de outra máquina (pelo servidor dela). Mídia em grade por dia, que abre o visor; arquivos com o tamanho antes
//! de baixar; áudio com o player do app. Os anexos vêm do cofre da sessão e os tamanhos citados, do `files/resolver`.
use super::*;
use super::gallery::{self, Filter, Item, Kind};
use super::panes::Area;
use crate::api::dto::UploadFile;

/// Quanto dura o realce da mensagem do "Mostrar na conversa".
pub(super) const FLASH: Duration = Duration::from_millis(1800);
/// Miniaturas por página da grade: cada uma é um pedido ao servidor da sessão.
const PAGE: usize = 48;
const PAD: f32 = 14.;
const GAP: f32 = 4.;
const COLUMNS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Section { #[default] Media, Files, Audio }

impl Section {
    const ALL: [Section; 3] = [Section::Media, Section::Files, Section::Audio];

    fn holds(self, item: &Item) -> bool {
        match self { Section::Media => item.visual(), Section::Files => item.kind == Kind::File, Section::Audio => item.kind == Kind::Audio }
    }

    fn label(self) -> &'static str {
        match self { Section::Media => "media_section_media", Section::Files => "media_section_files", Section::Audio => "media_section_audio" }
    }
}

/// Pedido de "Mostrar na conversa": o evento, e a linha e a hora depois de achada.
pub(super) struct Flash { pub(super) key: SessionKey, pub(super) event: String, pub(super) row: Option<String>, pub(super) at: Option<Instant> }

type Stamp = (Option<SessionKey>, usize, Option<String>, u64);

#[derive(Default)]
pub(super) struct MediaPanel {
    section: Section,
    filter: Filter,
    /// Sessão do que está abaixo: outra sessão recomeça a leitura, a aba e o filtro ficam.
    pub(super) owner: Option<SessionKey>,
    /// Anexos do cofre da sessão; `None` até a primeira leitura.
    pub(super) uploads: Option<Result<Vec<UploadFile>, String>>,
    /// Mensagens da pessoa na última leitura dos anexos: anexo novo só chega com mensagem nova (ou ditado, que vira uma).
    uploads_for: Option<usize>,
    listing: bool,
    /// Caminhos citados já perguntados: o achado, ou nada (não existe, ou a pergunta falhou).
    pub(super) cited: HashMap<String, Option<gallery::Found>>,
    asking: bool,
    /// Muda quando o que se sabe muda: a lista guardada é refeita.
    pub(super) generation: u64,
    pub(super) items: Option<(Stamp, std::rc::Rc<Vec<Item>>)>,
    /// Miniaturas à mostra na grade.
    shown: usize,
    /// Sessão cujo histórico inteiro já foi pedido para a aba.
    loaded_all: Option<SessionKey>,
    pub(super) flash: Option<Flash>,
}

impl MediaPanel {
    fn reset(&mut self, owner: SessionKey) {
        let (section, filter, generation, flash) = (self.section, self.filter, self.generation + 1, self.flash.take());
        *self = MediaPanel { section, filter, generation, flash, owner: Some(owner), shown: PAGE, ..Default::default() };
    }
}

impl Hangar {
    /// Lê o que a aba precisa da sessão aberta: o histórico inteiro uma vez, os anexos do cofre quando chega mensagem
    /// da pessoa, e o tamanho dos caminhos citados ainda não perguntados.
    fn media_sync(&mut self, cx: &mut Context<Self>) {
        let (Some(key), Some(api)) = (self.selected_key(), self.session_api()) else { return };
        if self.media_panel.owner.as_ref() != Some(&key) { self.media_panel.reset(key.clone()); }
        if self.history_limit != 0 && self.media_panel.loaded_all.as_ref() != Some(&key) {
            self.media_panel.loaded_all = Some(key.clone());
            self.load_all(cx);
        }
        let users = self.chat.events.iter().filter(|event| event.kind == "user_msg").count();
        if !self.media_panel.listing && self.media_panel.uploads_for != Some(users) {
            (self.media_panel.uploads_for, self.media_panel.listing) = (Some(users), true);
            let (uploads, name, list_api, owner) = (self.uploads_for(&key), key.name.clone(), api.clone(), key.clone());
            let job = self.runtime.spawn(async move { uploads.list(&list_api, &name).await });
            cx.spawn(async move |this, cx| {
                let result = job.await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
                let _ = this.update(cx, |this, cx| {
                    if this.media_panel.owner.as_ref() != Some(&owner) { return; }
                    this.media_panel.listing = false;
                    this.media_panel.uploads = Some(result.map_err(|error| Self::fetch_failure(&error)));
                    this.media_panel.generation += 1;
                    this.redraw(Area::Side, cx);
                    cx.notify();
                });
            }).detach();
        }
        if self.media_panel.asking { return; }
        let items = self.media_items();
        let pending: Vec<String> = items.iter().filter_map(|item| match &item.source {
            Source::Cited(path) if !self.media_panel.cited.contains_key(path) => Some(path.clone()),
            _ => None,
        }).collect();
        if pending.is_empty() { return; }
        for path in &pending { self.media_panel.cited.insert(path.clone(), None); }
        self.media_panel.asking = true;
        let (name, owner) = (key.name.clone(), key.clone());
        let job = self.runtime.spawn(async move {
            let mut found = Vec::new();
            // ponytail: lotes de 50, como a conferência dos chips; o servidor procura fora da pasta só 30 por pedido.
            for batch in pending.chunks(50) {
                let answer = api.act(&name, &["files", "resolver"], Some(json!({"caminhos": batch})), false, 30).await
                    .and_then(|value| value.get("ok").filter(|ok| ok.is_object()).cloned().ok_or_else(|| Failure::local("invalid_response")));
                match answer {
                    Ok(ok) => found.extend(batch.iter().map(|path| (path.clone(), ok.get(path).and_then(gallery::found_entry)))),
                    // Sem tamanho a linha só não o mostra: vai ao log, não à tela.
                    Err(error) => eprintln!("tamanho de {} arquivos citados falhou: {}", batch.len(), Self::fetch_failure(&error)),
                }
            }
            found
        });
        cx.spawn(async move |this, cx| {
            let found = job.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.media_panel.owner.as_ref() != Some(&owner) { return; }
                this.media_panel.asking = false;
                this.media_panel.cited.extend(found);
                this.media_panel.generation += 1;
                this.redraw(Area::Side, cx);
                cx.notify();
            });
        }).detach();
    }

    pub(super) fn render_media_panel(&mut self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        self.media_sync(cx);
        let Some(key) = self.selected_key() else { return div().into_any_element() };
        let items = self.media_items();
        let (section, filter) = (self.media_panel.section, self.media_panel.filter);
        let kept: Vec<usize> = (0..items.len()).filter(|&i| filter.keeps(items[i].origin)).collect();
        let count = |s: Section| kept.iter().filter(|&&i| s.holds(&items[i])).count();
        let selected = Section::ALL.iter().position(|s| *s == section).unwrap_or(0);
        let tabs = TabBar::new("media-sections").segmented().small().selected_index(selected)
            .children(Section::ALL.map(|s| Tab::new().label(tr(s.label()).replace("{n}", &count(s).to_string()))))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.media_panel.section = Section::ALL[*ix];
                this.redraw(Area::Side, cx);
            }));
        let chips = div().flex().flex_wrap().gap(px(6.)).children(Filter::ALL.iter().enumerate().map(|(i, &chip)| {
            let label = match chip { Filter::All => "media_filter_all", Filter::Agent => "media_filter_agent", Filter::User => "media_filter_user" };
            Button::new(SharedString::from(format!("media-filter-{i}"))).outline().xsmall().rounded_full().selected(chip == filter).label(tr(label))
                .on_click(cx.listener(move |this, _, _, cx| {
                    (this.media_panel.filter, this.media_panel.shown) = (chip, PAGE);
                    this.redraw(Area::Side, cx);
                }))
        }));
        let note = |text: String, color: Hsla| div().py_3().text_sm().text_color(color).whitespace_normal().child(text);
        let list: Vec<usize> = kept.iter().copied().filter(|&i| section.holds(&items[i])).collect();
        // Só o estado da leitura, sem segurar o painel emprestado: a grade abaixo pede miniaturas.
        let listing = self.media_panel.uploads.as_ref().map(|result| result.as_ref().map(|_| ()).map_err(Clone::clone));
        let body = match listing {
            None => div().id("media-loading").role(Role::Status).child(note(tr("media_panel_loading"), theme::muted())).into_any_element(),
            Some(Err(reason)) => div().id("media-failed").role(Role::Alert).flex().flex_col().gap_2()
                .child(note(tr("media_panel_failed").replace("{reason}", &reason), theme::warning()))
                .child(div().flex().child(Button::new("media-retry").ghost().xsmall().label(tr("retry"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        (this.media_panel.uploads, this.media_panel.uploads_for) = (None, None);
                        this.redraw(Area::Side, cx);
                    }))))
                .into_any_element(),
            Some(Ok(())) if list.is_empty() => {
                let empty = if filter != Filter::All && items.iter().any(|item| section.holds(item)) { "media_empty_filter" } else {
                    match section { Section::Media => "media_empty_media", Section::Files => "media_empty_files", Section::Audio => "media_empty_audio" }
                };
                note(tr(empty), theme::muted()).into_any_element()
            }
            Some(Ok(())) => match section {
                Section::Media => self.render_media_grid(&key, &items, &list, width, cx),
                Section::Files => div().flex().flex_col().children(list.iter().map(|&i| self.render_media_file(&key, i, &items[i], cx))).into_any_element(),
                Section::Audio => div().flex().flex_col().children(list.iter().map(|&i| self.render_media_audio(&key, i, &items[i], cx))).into_any_element(),
            },
        };
        div().size_full().flex().flex_col()
            .child(div().flex_shrink_0().px(px(PAD)).pt(px(12.)).child(tabs))
            .child(div().flex_shrink_0().px(px(PAD)).pt(px(10.)).pb(px(4.)).child(chips))
            .child(div().id("media-scroll").flex_1().min_h_0().overflow_y_scroll().px(px(PAD)).pb(px(PAD)).child(body))
            .into_any_element()
    }

    /// Grade de 4 por linha, do dia mais recente para o mais antigo; a página seguinte só pelo botão.
    fn render_media_grid(&mut self, key: &SessionKey, items: &[Item], list: &[usize], width: f32, cx: &mut Context<Self>) -> AnyElement {
        let side = ((width - PAD * 2. - GAP * (COLUMNS as f32 - 1.)) / COLUMNS as f32).floor().max(32.);
        let newest: Vec<usize> = list.iter().rev().copied().collect();
        let shown = self.media_panel.shown.max(PAGE);
        let days: Vec<(usize, Option<chrono::NaiveDate>)> = newest.iter().take(shown).map(|&i| (i, gallery::day_of(items[i].ts))).collect();
        let today = chrono::Local::now().date_naive();
        let mut grid = div().flex().flex_col();
        for (day, members) in gallery::by_day(&days) {
            let title = day.map_or_else(|| tr("media_group_undated"), |day| gallery::day_text(gallery::day_name(day, today), true));
            grid = grid.child(div().mt(px(12.)).mb(px(6.)).text_size(px(11.)).text_color(theme::faint()).child(title));
            for row in members.chunks(COLUMNS) {
                let cells: Vec<AnyElement> = row.iter().map(|&i| self.render_media_cell(key, i, &items[i], side, cx)).collect();
                grid = grid.child(div().flex().gap(px(GAP)).mb(px(GAP)).children(cells));
            }
        }
        let rest = newest.len().saturating_sub(shown);
        grid.when(rest > 0, |el| el.child(div().pt_2().flex().justify_center()
                .child(Button::new("media-more").ghost().small().label(tr("media_more").replace("{n}", &rest.to_string()))
                    .on_click(cx.listener(|this, _, _, cx| { this.media_panel.shown = this.media_panel.shown.max(PAGE) + PAGE; this.redraw(Area::Side, cx); })))))
            .into_any_element()
    }

    fn render_media_cell(&mut self, key: &SessionKey, i: usize, item: &Item, side: f32, cx: &mut Context<Self>) -> AnyElement {
        let thumb = item.thumb().cloned();
        if let Some(thumb) = &thumb { self.ensure_media(thumb, false); }
        let picture = thumb.as_ref().and_then(|thumb| match self.media.get(&(key.clone(), thumb.clone())) {
            Some(MediaState::Image(picture)) => Some(picture.clone()),
            _ => None,
        });
        let video = item.kind == Kind::Video;
        let (open_key, source) = (key.clone(), item.source.clone());
        let label = format!("{} · {}", item.name, gallery::origin_text(item.origin, false).unwrap_or_default());
        div().id(SharedString::from(format!("media-cell-{i}"))).relative().size(px(side)).flex_shrink_0().rounded(px(6.)).overflow_hidden()
            .bg(theme::inset()).cursor_pointer().focusable().tab_stop(true).role(Role::Button).aria_label(label)
            .border_1().border_color(transparent_black()).focus_visible(|el| el.border_color(theme::accent_focus()))
            .on_click(cx.listener(move |this, _, window, cx| this.open_gallery(open_key.clone(), source.clone(), vec![source.clone()], 0, window, cx)))
            .map(|el| match picture {
                Some(picture) => el.child(img(picture).size_full().object_fit(ObjectFit::Cover)),
                // Vídeo sem quadro: o nome no lugar da capa.
                None if video => el.child(div().size_full().p_1().flex().items_end().child(div().w_full().truncate().text_size(px(9.))
                    .text_color(theme::faint()).child(item.name.clone()))),
                None => el,
            })
            .when(video, |el| el.child(div().absolute().inset_0().flex().items_center().justify_center()
                .child(div().size(px(22.)).rounded_full().bg(theme::scrim()).flex().items_center().justify_center()
                    .child(chrome::small_icon(IconName::Play, 11., theme::text())))))
            .when_some(gallery::origin_text(item.origin, true), |el, who| el.child(div().absolute().top(px(4.)).left(px(4.))
                .px(px(5.)).rounded(px(4.)).bg(theme::scrim()).text_size(px(9.)).text_color(theme::text()).child(who)))
            .into_any_element()
    }

    /// "tamanho · pasta · quem", sem o que não se sabe.
    fn media_detail(item: &Item, middle: Option<String>) -> String {
        [item.size.map(human_size), middle, gallery::origin_text(item.origin, true)].into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }

    fn render_media_file(&self, key: &SessionKey, i: usize, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let web = activity::web;
        let state = self.file_cards.get(&(key.clone(), item.source.clone())).cloned();
        let folder = match &item.source { Source::Upload(_) => Some(tr("media_uploads_folder")), _ => item.folder.as_deref().map(abbreviate_home) };
        let (detail, color) = match &state {
            Some(FileCard::Busy) => (Self::media_detail(item, Some(web("file_card_downloading"))), theme::muted()),
            Some(FileCard::Saved(_)) => (Self::media_detail(item, Some(web("file_card_saved").to_lowercase())), theme::success_text()),
            Some(FileCard::Missing) => (web("file_card_missing"), theme::warning_text()),
            Some(FileCard::Failed(error)) => (format!("{} · {error}", web("file_card_failed")), theme::warning_text()),
            Some(FileCard::Empty) => (web("file_card_empty"), theme::warning_text()),
            _ => (Self::media_detail(item, folder), theme::faint()),
        };
        let (badge, tint) = gallery::type_badge(&item.name);
        let (source, name) = (item.source.clone(), item.name.clone());
        let save = { let (source, name) = (source.clone(), name.clone());
            move |this: &mut Self, cx: &mut Context<Self>| this.keep_file(source.clone(), name.clone(), false, cx) };
        let action = match &state {
            Some(FileCard::Saved(path)) => {
                let (path, openable) = (path.clone(), composer::openable(&composer::safe_name(&name)));
                div().flex().items_center().gap_1()
                    .when(openable, |el| el.child(Button::new(SharedString::from(format!("media-file-open-{i}"))).ghost().xsmall()
                        .icon(IconName::ExternalLink).label(tr("open"))
                        .on_click(cx.listener(move |this, _, _, cx| this.keep_file(source.clone(), name.clone(), true, cx)))))
                    .child(Button::new(SharedString::from(format!("media-file-reveal-{i}"))).ghost().xsmall().icon(IconName::FolderOpen)
                        .tooltip(tr("file_card_show_in_folder")).accessibility_label(tr("file_card_show_in_folder"))
                        .on_click(move |_, _, cx| cx.reveal_path(&path)))
                    .into_any_element()
            }
            Some(FileCard::Missing | FileCard::Failed(_)) => Button::new(SharedString::from(format!("media-file-retry-{i}"))).ghost().xsmall()
                .icon(IconName::RefreshCw).label(web("file_card_retry"))
                .on_click(cx.listener(move |this, _, _, cx| save(this, cx))).into_any_element(),
            busy => Button::new(SharedString::from(format!("media-file-save-{i}"))).outline().xsmall().icon(IconName::Download)
                .label(web("file_card_download")).loading(matches!(busy, Some(FileCard::Busy))).disabled(matches!(busy, Some(FileCard::Busy)))
                .on_click(cx.listener(move |this, _, _, cx| save(this, cx))).into_any_element(),
        };
        div().id(SharedString::from(format!("media-file-{i}"))).py(px(9.)).px(px(6.)).rounded(px(8.)).flex().items_center().gap(px(10.))
            .hover(|el| el.bg(theme::hover()))
            .child(div().size(px(36.)).flex_shrink_0().rounded(px(8.)).flex().items_center().justify_center()
                .map(|el| match tint { Some(tint) => el.bg(Hsla::from(rgb(tint))).text_color(theme::on_press()), None => el.bg(theme::raised()).text_color(theme::muted()) })
                .text_size(px(10.)).font_weight(FontWeight::BOLD).child(badge))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().truncate().text_size(px(13.)).text_color(theme::text()).child(item.name.clone()))
                .child(div().mt(px(2.)).truncate().text_size(px(11.)).text_color(color).child(detail)))
            .child(div().flex_shrink_0().child(action))
            .into_any_element()
    }

    fn render_media_audio(&self, key: &SessionKey, i: usize, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let audio_key = format!("media:{}:{:?}", key.name, item.source);
        let state = self.audio_state(&audio_key);
        let duration = state.duration.map(crate::audio::clock);
        let detail = match &state.error {
            Some(error) => tr("audio_failed").replace("{error}", error),
            None => Self::media_detail(item, duration),
        };
        let label = tr(if state.playing { "audio_pause" } else { "audio_play" });
        let (play_source, play_name, play_key) = (item.source.clone(), item.name.clone(), audio_key.clone());
        let play = Button::new(SharedString::from(format!("media-audio-play-{i}"))).ghost().small().rounded_full()
            .icon(if state.playing { IconName::Pause } else { IconName::Play }).loading(state.loading)
            .tooltip(label.clone()).accessibility_label(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                let (Some(api), Some(key)) = (this.session_api(), this.selected_key()) else { return };
                let (uploads, source) = (this.uploads_for(&key), play_source.clone());
                this.toggle_audio(play_key.clone(), &play_name, async move {
                    uploads.fetch(&api, &key.name, &source).await.map_err(|error| Self::fetch_failure(&error))
                }, cx);
            }));
        let again = match (&item.source, item.origin) {
            (Source::Upload(file), Some(gallery::Origin::User)) => {
                let file = file.clone();
                Some(Button::new(SharedString::from(format!("media-audio-again-{i}"))).ghost().xsmall().label(tr("dictation_again"))
                    .on_click(cx.listener(move |this, _, _, cx| this.dictate_recent(file.clone(), cx))))
            }
            _ => None,
        };
        let (save_source, save_name) = (item.source.clone(), item.name.clone());
        div().id(SharedString::from(format!("media-audio-{i}"))).py(px(9.)).px(px(6.)).rounded(px(8.)).flex().items_center().gap(px(10.))
            .hover(|el| el.bg(theme::hover()))
            .child(play)
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().truncate().text_size(px(13.)).text_color(theme::text()).child(item.name.clone()))
                .child(div().mt(px(2.)).truncate().text_size(px(11.)).text_color(if state.error.is_some() { theme::warning_text() } else { theme::faint() })
                    .child(detail)))
            .children(again)
            .child(Button::new(SharedString::from(format!("media-audio-save-{i}"))).ghost().xsmall().icon(IconName::Download)
                .tooltip(activity::web("file_card_download")).accessibility_label(activity::web("file_card_download"))
                .on_click(cx.listener(move |this, _, _, cx| this.keep_file(save_source.clone(), save_name.clone(), false, cx))))
            .into_any_element()
    }
}

/// `/home/<usuário>/x` vira `~/x`, como a pasta da aba Arquivos.
fn abbreviate_home(path: &str) -> String {
    match std::env::home_dir().map(|home| home.to_string_lossy().into_owned()) {
        Some(home) if path == home => "~".into(),
        Some(home) if path.starts_with(&format!("{home}/")) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}

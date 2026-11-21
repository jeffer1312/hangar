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
/// Teto de miniaturas na grade ao mesmo tempo: as que estão à vista nunca saem do cache das prévias, então a grade
/// passada do teto anda por troca, não por acréscimo.
const MAX_SHOWN: usize = PAGE * 5;
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

/// Seleção, revisão, eventos, o id do último deles (em hash), a versão dos eventos e a geração do painel.
pub(super) type Stamp = (u64, u64, usize, u64, u64, u64);

/// A sessão e a revisão do histórico dela: reabrir a mesma sessão traz a primeira página de novo, e ela é outra dona.
pub(super) type Owner = (SessionKey, u64);

#[derive(Default)]
pub(super) struct MediaPanel {
    section: Section,
    filter: Filter,
    /// Dono do que está abaixo: outro recomeça a leitura, a aba e o filtro ficam.
    pub(super) owner: Option<Owner>,
    /// Anexos do cofre da sessão; `None` até a primeira leitura.
    pub(super) uploads: Option<Result<Vec<UploadFile>, String>>,
    /// Mensagens da pessoa na última leitura dos anexos: anexo novo só chega com mensagem nova (ou ditado, que vira uma).
    uploads_for: Option<usize>,
    /// Pedido de anexos em voo: só a resposta dele o encerra.
    listing: Option<u64>,
    /// Caminhos citados já perguntados: o achado, ou nada (não existe, ou a pergunta falhou).
    pub(super) cited: HashMap<String, Option<gallery::Found>>,
    /// Pergunta de tamanhos em voo, como `listing`.
    asking: Option<u64>,
    /// Último número de pedido dado. Não volta a zero com a troca de dono: resposta velha nunca casa com pedido novo.
    tickets: u64,
    /// Muda quando o que se sabe muda: a lista guardada é refeita.
    pub(super) generation: u64,
    pub(super) items: Option<(Stamp, std::rc::Rc<Vec<Item>>)>,
    /// Janela da grade: quantas das mais novas pular e quantas mostrar.
    start: usize,
    shown: usize,
    /// Dono cujo histórico inteiro já foi pedido para a aba.
    loaded_all: Option<Owner>,
    /// O histórico inteiro falhou: a lista pode estar incompleta, e só o Tentar de novo pede outra vez.
    full_failed: Option<String>,
    pub(super) flash: Option<Flash>,
}

impl MediaPanel {
    fn reset(&mut self, owner: Option<Owner>) {
        let (section, filter, generation, tickets, flash) = (self.section, self.filter, self.generation + 1, self.tickets, self.flash.take());
        *self = MediaPanel { section, filter, generation, tickets, flash, owner, shown: PAGE, ..Default::default() };
    }

    /// Outra sessão (ou a mesma reaberta) na tela: nada desta fica, nem o pedido do histórico inteiro.
    pub(super) fn release(&mut self) { self.reset(None); }

    /// Passa a ser de `owner`; devolve se mudou.
    fn adopt(&mut self, owner: &Owner) -> bool {
        if self.owner.as_ref() == Some(owner) { return false; }
        self.reset(Some(owner.clone()));
        true
    }

    fn ticket(&mut self) -> u64 { self.tickets += 1; self.tickets }

    /// Encerra o pedido `ticket` em `slot` se ele ainda é o em voo; senão a resposta é de outro dono e é descartada.
    fn settle(slot: &mut Option<u64>, ticket: u64) -> bool {
        if *slot != Some(ticket) { return false; }
        *slot = None;
        true
    }

    fn wants_full(&self, owner: &Owner) -> bool { self.loaded_all.as_ref() != Some(owner) && self.full_failed.is_none() }

    /// O histórico inteiro de `owner` falhou: a aba mostra e pode pedir de novo. Falha de outro pedido (a busca) não é dela.
    pub(super) fn full_history_failed(&mut self, owner: Option<Owner>, reason: String) {
        if owner.is_none() || self.loaded_all != owner { return; }
        (self.loaded_all, self.full_failed) = (None, Some(reason));
        self.generation += 1;
    }

    /// "Mostrar mais": a grade cresce uma página até o teto; no teto, troca a página mais nova por uma mais antiga.
    fn more(start: usize, shown: usize) -> (usize, usize) {
        let shown = shown.clamp(PAGE, MAX_SHOWN);
        if shown + PAGE <= MAX_SHOWN { (start, shown + PAGE) } else { (start + PAGE, MAX_SHOWN) }
    }
}

impl Hangar {
    /// A primeira página da conversa já chegou e nada do histórico está em voo.
    fn history_settled(&self) -> bool {
        self.history_installed && !self.loading && self.history_task.as_ref().is_none_or(|task| task.is_finished())
    }

    /// Lê o que a aba precisa da sessão aberta: o histórico inteiro uma vez, os anexos do cofre quando chega mensagem
    /// da pessoa, e o tamanho dos caminhos citados ainda não perguntados.
    fn media_sync(&mut self, cx: &mut Context<Self>) {
        let (Some(key), Some(api)) = (self.selected_key(), self.session_api()) else { return };
        let owner = (key.clone(), self.revision);
        self.media_panel.adopt(&owner);
        // Só depois da primeira página: pedir na abertura cancelaria a página e juntaria o transcript inteiro na thread
        // da tela antes de a conversa aparecer.
        if self.history_limit != 0 && self.history_settled() && self.media_panel.wants_full(&owner) {
            self.media_panel.loaded_all = Some(owner.clone());
            self.load_all(cx);
        }
        let users = self.chat.events.iter().filter(|event| event.kind == "user_msg").count();
        if self.media_panel.listing.is_none() && self.media_panel.uploads_for != Some(users) {
            let ticket = self.media_panel.ticket();
            (self.media_panel.uploads_for, self.media_panel.listing) = (Some(users), Some(ticket));
            let (uploads, name, list_api) = (self.uploads_for(&key), key.name.clone(), api.clone());
            let job = self.runtime.spawn(async move { uploads.list(&list_api, &name).await });
            cx.spawn(async move |this, cx| {
                let result = job.await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
                let _ = this.update(cx, |this, cx| {
                    if !MediaPanel::settle(&mut this.media_panel.listing, ticket) { return; }
                    this.media_panel.uploads = Some(result.map_err(|error| Self::fetch_failure(&error)));
                    this.media_panel.generation += 1;
                    this.redraw(Area::Side, cx);
                    cx.notify();
                });
            }).detach();
        }
        if self.media_panel.asking.is_some() { return; }
        let items = self.media_items();
        let pending: Vec<String> = items.iter().filter_map(|item| match &item.source {
            Source::Cited(path) if !self.media_panel.cited.contains_key(path) => Some(path.clone()),
            _ => None,
        }).collect();
        if pending.is_empty() { return; }
        for path in &pending { self.media_panel.cited.insert(path.clone(), None); }
        let ticket = self.media_panel.ticket();
        self.media_panel.asking = Some(ticket);
        let name = key.name.clone();
        let retry = pending.clone();
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
            let found = job.await.map_err(|error| crate::log_line(&format!("mídia: a tarefa dos tamanhos citados caiu: {error}")));
            let _ = this.update(cx, |this, cx| {
                if !MediaPanel::settle(&mut this.media_panel.asking, ticket) { return; }
                match found {
                    Ok(found) => this.media_panel.cited.extend(found),
                    // Sem resposta nenhuma, os caminhos saem da marcação e o próximo desenho pergunta de novo.
                    Err(()) => for path in &retry { this.media_panel.cited.remove(path); },
                }
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
        let tabs = media_section_tabs(Section::ALL.map(count), selected)
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.media_panel.section = Section::ALL[*ix];
                this.redraw(Area::Side, cx);
            }));
        let chips = media_filter_row().children(Filter::ALL.iter().enumerate().map(|(i, &chip)| {
            media_filter_chip(i, chip == filter)
                .on_click(cx.listener(move |this, _, _, cx| {
                    (this.media_panel.filter, this.media_panel.start, this.media_panel.shown) = (chip, 0, PAGE);
                    this.redraw(Area::Side, cx);
                }))
        }));
        let note = |text: String, color: Hsla| div().py_3().text_sm().text_color(color).whitespace_normal().child(text);
        let list: Vec<usize> = kept.iter().copied().filter(|&i| section.holds(&items[i])).collect();
        // Só o estado da leitura, sem segurar o painel emprestado: a grade abaixo pede miniaturas.
        let listing = self.media_panel.uploads.as_ref().map(|result| result.as_ref().map(|_| ()).map_err(Clone::clone));
        // O começo da conversa ainda vem: o que já se achou fica à mostra, e vazio só depois de tudo lido.
        let reading = history_pending(self.history_limit, self.history_settled(), self.media_panel.full_failed.is_some());
        let still_reading = (reading && matches!(listing, Some(Ok(()))) && !list.is_empty())
            .then(|| div().id("media-history-loading").role(Role::Status).child(note(tr("media_panel_loading"), theme::muted())));
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
            Some(Ok(())) if list.is_empty() && reading => div().id("media-loading").role(Role::Status)
                .child(note(tr("media_panel_loading"), theme::muted())).into_any_element(),
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
        // O que já veio continua à mostra; o aviso diz que pode faltar o começo da conversa.
        let partial = self.media_panel.full_failed.clone().map(|reason| div().id("media-history-failed").role(Role::Alert).flex().flex_col().gap_2()
            .child(note(tr("media_history_failed").replace("{reason}", &reason), theme::warning()))
            .child(div().flex().child(Button::new("media-history-retry").ghost().xsmall().label(tr("retry"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.media_panel.full_failed = None;
                    this.redraw(Area::Side, cx);
                })))));
        div().size_full().flex().flex_col()
            .child(div().flex_shrink_0().px(px(PAD)).pt(px(12.)).child(tabs))
            .child(div().flex_shrink_0().px(px(PAD)).pt(px(10.)).pb(px(4.)).child(chips))
            .child(div().id("media-scroll").flex_1().min_h_0().overflow_y_scroll().px(px(PAD)).pb(px(PAD)).children(partial).children(still_reading).child(body))
            .into_any_element()
    }

    /// Grade de 4 por linha, do dia mais recente para o mais antigo; a página seguinte só pelo botão.
    fn render_media_grid(&mut self, key: &SessionKey, items: &[Item], list: &[usize], width: f32, cx: &mut Context<Self>) -> AnyElement {
        let side = media_cell_side(width - PAD * 2.);
        let newest: Vec<usize> = list.iter().rev().copied().collect();
        let shown = self.media_panel.shown.clamp(PAGE, MAX_SHOWN);
        let start = self.media_panel.start.min(newest.len().saturating_sub(1));
        let days: Vec<(usize, Option<chrono::NaiveDate>)> = newest.iter().skip(start).take(shown).map(|&i| (i, gallery::day_of(items[i].ts))).collect();
        let today = chrono::Local::now().date_naive();
        let mut grid = div().flex().flex_col().when(start > 0, |el| el.child(div().pb_2().flex().justify_center()
            .child(Button::new("media-newer").ghost().small().label(tr("media_newer").replace("{n}", &start.to_string()))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.media_panel.start = this.media_panel.start.saturating_sub(PAGE);
                    this.redraw(Area::Side, cx);
                })))));
        for (day, members) in gallery::by_day(&days) {
            let title = day.map_or_else(|| tr("media_group_undated"), |day| gallery::day_text(gallery::day_name(day, today), true));
            grid = grid.child(media_day_title(title));
            for row in members.chunks(COLUMNS) {
                // Laço, não `map`: cada célula pede a miniatura dela.
                let mut cells: Vec<AnyElement> = Vec::with_capacity(row.len());
                for &i in row { cells.push(self.render_media_cell(key, i, &items[i], side, cx)); }
                grid = grid.child(media_grid_row(cells));
            }
        }
        let rest = newest.len().saturating_sub(start + shown);
        grid.when(rest > 0, |el| el.child(div().pt_2().flex().justify_center()
                .child(Button::new("media-more").ghost().small().label(tr("media_more").replace("{n}", &rest.to_string()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        (this.media_panel.start, this.media_panel.shown) = MediaPanel::more(this.media_panel.start, this.media_panel.shown);
                        this.redraw(Area::Side, cx);
                    })))))
            .into_any_element()
    }

    fn render_media_cell(&mut self, key: &SessionKey, i: usize, item: &Item, side: f32, cx: &mut Context<Self>) -> AnyElement {
        let thumb = item.thumb().cloned();
        if let Some(thumb) = &thumb { self.ensure_media(thumb, false); }
        let (picture, failed) = match thumb.as_ref().and_then(|thumb| self.media.get(&(key.clone(), thumb.clone()))) {
            Some(MediaState::Image(picture)) => (Some(picture.clone()), None),
            Some(MediaState::Failed(reason)) => (None, Some(reason.clone())),
            _ => (None, None),
        };
        let (open_key, source, retry) = (key.clone(), item.source.clone(), thumb.clone());
        let label = match &failed {
            Some(_) => format!("{} · {}", item.name, activity::web("file_card_retry")),
            None => format!("{} · {}", item.name, gallery::origin_text(item.origin, false).unwrap_or_default()),
        };
        let retrying = failed.is_some();
        let is_video = item.kind == Kind::Video;
        media_cell(SharedString::from(format!("media-cell-{i}")), side, picture, item.kind == Kind::Video, item.name.clone(),
            gallery::origin_text(item.origin, true))
            // A falha fica à vista e o clique busca de novo, em vez de abrir o visor sem imagem.
            .when_some(failed, |el, reason| el.child(div().absolute().inset_0().p_1().overflow_hidden().bg(theme::inset())
                .flex().flex_col().justify_end().gap_1().text_size(px(10.))
                .child(div().text_color(theme::warning()).child(reason))
                .child(div().text_color(theme::accent()).child(activity::web("file_card_retry")))))
            .cursor_pointer().focusable().tab_stop(true).role(Role::Button).aria_label(label)
            .focus_visible(|el| el.border_color(theme::accent_focus()))
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(thumb) = retry.clone().filter(|_| retrying) {
                    this.retry_media(open_key.clone(), thumb, cx);
                    this.redraw(Area::Side, cx);
                    // Só a capa falhou: o vídeo em si continua abrindo.
                    if !is_video { return; }
                }
                this.open_gallery(open_key.clone(), source.clone(), vec![source.clone()], 0, window, cx);
            }))
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

// Partes da aba que a animação da novidade (`crate::news`) também desenha.

/// Lado da miniatura para a grade de 4 por linha ocupar `inner` (a largura sem o respiro dos lados).
pub(crate) fn media_cell_side(inner: f32) -> f32 { ((inner - GAP * (COLUMNS as f32 - 1.)) / COLUMNS as f32).floor().max(32.) }

/// Mídia, Arquivos e Áudio, cada uma com a contagem.
pub(crate) fn media_section_tabs(counts: [usize; 3], selected: usize) -> TabBar {
    TabBar::new("media-sections").segmented().small().selected_index(selected)
        .children(Section::ALL.iter().zip(counts).map(|(s, n)| Tab::new().label(tr(s.label()).replace("{n}", &n.to_string()))))
}

pub(crate) fn media_filter_row() -> Div { div().flex().flex_wrap().gap(px(6.)) }

/// Chip `i` de Todos, Do agente e Seus.
pub(crate) fn media_filter_chip(i: usize, on: bool) -> Button {
    let label = match Filter::ALL[i] { Filter::All => "media_filter_all", Filter::Agent => "media_filter_agent", Filter::User => "media_filter_user" };
    Button::new(SharedString::from(format!("media-filter-{i}"))).outline().xsmall().rounded_full().selected(on).label(tr(label))
}

pub(crate) fn media_day_title(title: String) -> Div { div().mt(px(12.)).mb(px(6.)).text_size(px(11.)).text_color(theme::faint()).child(title) }

pub(crate) fn media_grid_row(cells: Vec<AnyElement>) -> Div { div().flex().gap(px(GAP)).mb(px(GAP)).children(cells) }

/// Miniatura da grade: a imagem (ou a capa do vídeo), o ▶ do vídeo e o selo de quem mandou.
pub(crate) fn media_cell(id: SharedString, side: f32, picture: Option<Arc<RenderImage>>, video: bool, name: String, who: Option<String>) -> Stateful<Div> {
    div().id(id).relative().size(px(side)).flex_shrink_0().rounded(px(6.)).overflow_hidden()
        .bg(theme::inset()).border_1().border_color(transparent_black())
        .map(|el| match picture {
            Some(picture) => el.child(img(picture).size_full().object_fit(ObjectFit::Cover)),
            // Vídeo sem quadro: o nome no lugar da capa.
            None if video => el.child(div().size_full().p_1().flex().items_end().child(div().w_full().truncate().text_size(px(9.))
                .text_color(theme::faint()).child(name))),
            None => el,
        })
        .when(video, |el| el.child(div().absolute().inset_0().flex().items_center().justify_center().child(super::viewer::play_badge(22., 11.))))
        .when_some(who, |el, who| el.child(div().absolute().top(px(4.)).left(px(4.))
            .px(px(5.)).rounded(px(4.)).bg(theme::scrim()).text_size(px(9.)).text_color(theme::text()).child(who)))
}

/// A aba ainda não leu a conversa inteira: `limit` 0 é o histórico todo pedido, e só vale depois de chegar. A falha
/// dele não é leitura em curso: a aba mostra o que tem e o aviso.
fn history_pending(limit: usize, settled: bool, failed: bool) -> bool { !failed && !(limit == 0 && settled) }

/// `/home/<usuário>/x` vira `~/x`, como a pasta da aba Arquivos.
fn abbreviate_home(path: &str) -> String {
    match std::env::home_dir().map(|home| home.to_string_lossy().into_owned()) {
        Some(home) if path == home => "~".into(),
        Some(home) if path.starts_with(&format!("{home}/")) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` da gpui colide com o atributo padrão.
    use super::{MAX_SHOWN, MediaPanel, Owner, PAGE, history_pending};
    use crate::delivery::SessionKey;

    fn owner(name: &str, revision: u64) -> Owner {
        (SessionKey { server: "http://m/".into(), name: name.into(), jsonl: format!("/{name}.jsonl") }, revision)
    }

    #[test]
    fn coming_back_to_a_session_asks_the_whole_history_again() {
        let mut panel = MediaPanel::default();
        let a = owner("a", 1);
        assert!(panel.adopt(&a) && panel.wants_full(&a));
        panel.loaded_all = Some(a.clone());
        assert!(!panel.wants_full(&a), "já pedido para este dono");
        // A → B → A: cada abertura traz a primeira página de novo, numa revisão nova.
        panel.release();
        assert!(panel.adopt(&owner("b", 2)));
        let back = owner("a", 3);
        assert!(panel.adopt(&back) && panel.wants_full(&back), "a volta à A pede o histórico inteiro outra vez");
        assert!(!panel.adopt(&back), "o mesmo dono não recomeça a leitura");
    }

    #[test]
    fn failed_whole_history_is_shown_and_waits_for_retry() {
        let mut panel = MediaPanel::default();
        let a = owner("a", 1);
        panel.adopt(&a);
        panel.loaded_all = Some(a.clone());
        // Falha da busca, com outro dono, não é da aba.
        panel.full_history_failed(Some(owner("b", 1)), "x".into());
        assert_eq!((panel.loaded_all.as_ref(), panel.full_failed.as_ref()), (Some(&a), None));
        panel.full_history_failed(Some(a.clone()), "sem rede".into());
        assert_eq!((panel.loaded_all.as_ref(), panel.full_failed.as_deref()), (None, Some("sem rede")));
        assert!(!panel.wants_full(&a), "sem pedir de novo sozinho a cada quadro");
        panel.full_failed = None;
        assert!(panel.wants_full(&a), "o Tentar de novo libera outro pedido");
    }

    #[test]
    fn stale_answers_from_the_previous_owner_do_not_end_the_new_request() {
        let mut panel = MediaPanel::default();
        panel.adopt(&owner("a", 1));
        let old = panel.ticket();
        panel.listing = Some(old);
        panel.release();
        panel.adopt(&owner("a", 2));
        let new = panel.ticket();
        panel.listing = Some(new);
        assert_ne!(old, new, "o número segue crescendo depois da troca");
        assert!(!MediaPanel::settle(&mut panel.listing, old), "a resposta velha é descartada");
        assert_eq!(panel.listing, Some(new), "e o pedido novo segue em voo");
        assert!(MediaPanel::settle(&mut panel.listing, new) && panel.listing.is_none());
    }

    #[test]
    fn grid_grows_to_the_cap_then_pages_by_replacement() {
        let mut window = (0, PAGE);
        while window.1 < MAX_SHOWN { window = MediaPanel::more(window.0, window.1); }
        assert_eq!(window, (0, MAX_SHOWN));
        assert_eq!(MediaPanel::more(window.0, window.1), (PAGE, MAX_SHOWN), "no teto troca em vez de crescer");
    }

    #[test]
    fn empty_waits_for_the_whole_history_unless_it_failed() {
        assert!(history_pending(400, true, false), "só a primeira página chegou");
        assert!(history_pending(0, false, false), "o histórico inteiro está em voo");
        assert!(!history_pending(0, true, false), "tudo lido: vazio é vazio");
        assert!(!history_pending(400, true, true), "a falha mostra o aviso, não carregando para sempre");
    }
}

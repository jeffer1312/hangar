//! A mídia e os arquivos da conversa inteira, numa lista só e na ordem da conversa: o que o agente citou (resposta,
//! Read, SendUserFile) e o que a pessoa mandou, mais os anexos do cofre que nenhuma mensagem mostra (o áudio do
//! ditado). O visor anda por ela e a aba "Mídia" do painel a mostra por tipo e por quem mandou.
use super::*;
use crate::api::dto::UploadFile;
use chrono::{Datelike, Local, NaiveDate, TimeZone, Timelike};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Origin { Agent, User }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind { Image, Video, Audio, File }

/// Filtro por quem mandou, os chips do painel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Filter { #[default] All, Agent, User }

impl Filter {
    pub(super) const ALL: [Filter; 3] = [Filter::All, Filter::Agent, Filter::User];

    pub(super) fn keeps(self, origin: Option<Origin>) -> bool {
        match self { Filter::All => true, Filter::Agent => origin == Some(Origin::Agent), Filter::User => origin == Some(Origin::User) }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Item {
    pub(super) source: Source,
    pub(super) name: String,
    pub(super) kind: Kind,
    /// `None`: aberto fora da conversa (anexo no campo, diagrama, árvore de arquivos).
    pub(super) origin: Option<Origin>,
    /// Evento onde apareceu primeiro: o "Mostrar na conversa" rola até ele.
    pub(super) event: Option<String>,
    pub(super) ts: Option<f64>,
    pub(super) size: Option<u64>,
    /// Pasta mostrada na linha do arquivo.
    pub(super) folder: Option<String>,
    /// Quadro que o servidor tirou do vídeo enviado: a capa dele.
    pub(super) poster: Option<Source>,
}

impl Item {
    /// Fora da conversa: só a origem, com o nome que o visor sempre deu a ela.
    pub(super) fn bare(source: Source, name: String) -> Self {
        let kind = if crate::audio::is_video(&name) { Kind::Video } else { Kind::Image };
        Item { source, name, kind, origin: None, event: None, ts: None, size: None, folder: None, poster: None }
    }

    pub(super) fn visual(&self) -> bool { matches!(self.kind, Kind::Image | Kind::Video) }

    /// O que a miniatura mostra: a imagem, ou a capa do vídeo quando há.
    pub(super) fn thumb(&self) -> Option<&Source> {
        match self.kind { Kind::Image => Some(&self.source), Kind::Video => self.poster.as_ref(), _ => None }
    }
}

fn kind_of(name: &str, image: bool) -> Kind {
    if image { Kind::Image }
    else if crate::audio::is_video(name) { Kind::Video }
    else if crate::audio::is_audio(name) { Kind::Audio }
    else { Kind::File }
}

/// `<nome>-q<N>.jpg` de `<nome>.<vídeo>`: os quadros que o servidor grava ao lado do vídeo enviado.
fn frame_of(video: &str, name: &str) -> bool {
    let Some((stem, _)) = video.rsplit_once('.') else { return false };
    name.strip_prefix(stem).and_then(|rest| rest.strip_prefix("-q")).and_then(|rest| rest.strip_suffix(".jpg"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A fala que o servidor tirou do vídeo enviado (`<nome>-audio.m4a`): já está na mensagem, como texto.
fn speech_of(video: &str, name: &str) -> bool {
    video.rsplit_once('.').is_some_and(|(stem, _)| name.strip_prefix(stem) == Some("-audio.m4a"))
}

fn folder_of(path: &str) -> Option<String> {
    path.rsplit_once(['/', '\\']).map(|(folder, _)| folder.to_owned()).filter(|folder| !folder.is_empty())
}

/// Tudo da conversa, na ordem em que apareceu; o que aparece de novo fica no primeiro lugar.
pub(super) fn collect(events: &[ChatEvent]) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut seen: HashSet<Source> = HashSet::new();
    for event in events {
        let (refs, origin) = match event.kind.as_str() {
            "user_msg" => (super::attachment_refs(event), Origin::User),
            "assistant_msg" => (super::attachment_refs(event), Origin::Agent),
            "tool_use" => {
                let mut refs = super::tool_file_refs(event);
                // Código e texto que o agente leu são trabalho dele, não arquivo entregue: do Read só entra a mídia.
                if event.tool_name.as_deref().is_some_and(|name| name.eq_ignore_ascii_case("read")) {
                    refs.retain(|(_, name, image)| kind_of(name, *image) != Kind::File);
                }
                (refs, Origin::Agent)
            }
            _ => continue,
        };
        let videos: Vec<String> = refs.iter().filter(|(source, name, _)| matches!(source, Source::Upload(_)) && crate::audio::is_video(name))
            .map(|(_, name, _)| name.clone()).collect();
        for (source, name, image) in refs {
            // O quadro do vídeo enviado na mesma mensagem é a capa dele, não uma foto a mais.
            if let Source::Upload(file) = &source && let Some(video) = videos.iter().find(|video| frame_of(video, file)) {
                let owner = Source::Upload(video.clone());
                if let Some(item) = items.iter_mut().find(|item| item.source == owner) && item.poster.is_none() { item.poster = Some(source); }
                continue;
            }
            if !seen.insert(source.clone()) { continue; }
            let folder = match &source { Source::Cited(path) | Source::Workspace(path) => folder_of(path), _ => None };
            items.push(Item { kind: kind_of(&name, image), name, origin: Some(origin), event: Some(event.id.clone()), ts: event.ts,
                size: None, folder, poster: None, source });
        }
    }
    items
}

/// Tamanhos dos anexos e os anexos do cofre que nenhuma mensagem mostra, no lugar da data deles. Quadro e fala de
/// vídeo ficam de fora: são o vídeo.
pub(super) fn with_uploads(mut items: Vec<Item>, uploads: &[UploadFile]) -> Vec<Item> {
    let by_name: HashMap<&str, &UploadFile> = uploads.iter().map(|file| (file.filename.as_str(), file)).collect();
    let mut known: HashSet<String> = HashSet::new();
    for item in &mut items {
        if let Source::Upload(name) = &item.source {
            if let Some(file) = by_name.get(name.as_str()) { item.size = Some(file.size); }
            known.insert(name.clone());
        }
        if let Some(Source::Upload(poster)) = &item.poster { known.insert(poster.clone()); }
    }
    let videos: Vec<&str> = uploads.iter().map(|file| file.filename.as_str()).filter(|name| crate::audio::is_video(name)).collect();
    let mut extra: Vec<&UploadFile> = uploads.iter()
        .filter(|file| !known.contains(&file.filename))
        .filter(|file| !videos.iter().any(|video| frame_of(video, &file.filename) || speech_of(video, &file.filename)))
        .collect();
    extra.sort_by(|a, b| a.mtime.total_cmp(&b.mtime));
    for file in extra {
        let name = file.filename.clone();
        let poster = crate::audio::is_video(&name).then(|| uploads.iter().map(|f| &f.filename).filter(|f| frame_of(&name, f)).min().cloned())
            .flatten().map(Source::Upload);
        let item = Item { source: Source::Upload(name.clone()), kind: kind_of(&name, composer::previewable(&name)), name,
            origin: Some(Origin::User), event: None, ts: Some(file.mtime), size: Some(file.size), folder: None, poster };
        let at = items.iter().position(|other| other.ts.is_some_and(|ts| ts > file.mtime)).unwrap_or(items.len());
        items.insert(at, item);
    }
    items
}

/// Achado do `files/resolver`: o caminho real e, de servidor novo, o tamanho.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Found { pub(super) real: String, pub(super) size: Option<u64> }

pub(super) fn with_found(mut items: Vec<Item>, found: &HashMap<String, Option<Found>>) -> Vec<Item> {
    for item in &mut items {
        let Source::Cited(path) = &item.source else { continue };
        let Some(Some(hit)) = found.get(path) else { continue };
        if item.size.is_none() { item.size = hit.size; }
        item.folder = folder_of(&hit.real).or(item.folder.take());
    }
    items
}

/// Um achado da resposta do `files/resolver`; tamanho ausente é servidor antigo.
pub(super) fn found_entry(entry: &Value) -> Option<Found> {
    Some(Found { real: entry.get("real")?.as_str()?.to_owned(), size: entry.get("size").and_then(Value::as_u64) })
}

/// Dia de um instante no fuso desta máquina.
pub(super) fn day_of(ts: Option<f64>) -> Option<NaiveDate> {
    let ts = ts.filter(|ts| ts.is_finite() && *ts > 0.)?;
    Local.timestamp_opt(ts as i64, 0).single().map(|at| at.date_naive())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DayName { Today, Yesterday, Date { day: u32, month: u32, year: Option<i32> } }

/// Hoje, ontem ou a data; o ano só quando não é o deste ano.
pub(super) fn day_name(day: NaiveDate, today: NaiveDate) -> DayName {
    if day == today { return DayName::Today; }
    if today.pred_opt() == Some(day) { return DayName::Yesterday; }
    DayName::Date { day: day.day(), month: day.month(), year: (day.year() != today.year()).then_some(day.year()) }
}

/// `capital`: o título do grupo no painel ("Hoje"); minúsculo no meio da linha ("hoje 11:42").
pub(super) fn day_text(name: DayName, capital: bool) -> String {
    match name {
        DayName::Today => tr(if capital { "media_group_today" } else { "media_day_today" }),
        DayName::Yesterday => tr(if capital { "media_group_yesterday" } else { "media_day_yesterday" }),
        DayName::Date { day, month, year: None } => tr("message_date").replace("{d}", &format!("{day:02}")).replace("{m}", &format!("{month:02}")),
        DayName::Date { day, month, year: Some(year) } => tr("media_date_year").replace("{d}", &format!("{day:02}"))
            .replace("{m}", &format!("{month:02}")).replace("{y}", &year.to_string()),
    }
}

/// "hoje 11:42", "ontem 22:05", "12/11 09:30".
pub(super) fn when(ts: Option<f64>) -> Option<String> {
    let ts = ts.filter(|ts| ts.is_finite() && *ts > 0.)?;
    let at = Local.timestamp_opt(ts as i64, 0).single()?;
    let day = day_text(day_name(at.date_naive(), Local::now().date_naive()), false);
    Some(format!("{day} {:02}:{:02}", at.hour(), at.minute()))
}

/// Grupos seguidos do mesmo dia, na ordem dada: `days[i]` é o dia do item `i`.
pub(super) fn by_day(days: &[(usize, Option<NaiveDate>)]) -> Vec<(Option<NaiveDate>, Vec<usize>)> {
    let mut groups: Vec<(Option<NaiveDate>, Vec<usize>)> = Vec::new();
    for &(item, day) in days {
        match groups.last_mut() {
            Some((last, members)) if *last == day => members.push(item),
            _ => groups.push((day, vec![item])),
        }
    }
    groups
}

pub(super) fn origin_text(origin: Option<Origin>, short: bool) -> Option<String> {
    Some(tr(match (origin?, short) {
        (Origin::Agent, true) => "media_origin_agent_short",
        (Origin::User, true) => "media_origin_user_short",
        (Origin::Agent, false) => "media_origin_agent",
        (Origin::User, false) => "media_origin_user",
    }))
}

/// Selo do tipo na linha do arquivo: a sigla e a cor dela. Sem cor do tema para tipo de arquivo: a paleta é só daqui.
pub(super) fn type_badge(name: &str) -> (String, Option<u32>) {
    let ext = composer::extension(name);
    let (label, color) = match ext.as_str() {
        "pdf" => ("PDF", Some(0xe5484d)),
        "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" => ("ZIP", Some(0x8e6cf0)),
        "xls" | "xlsx" | "ods" => ("XLS", Some(0x30a46c)),
        "csv" | "tsv" => ("CSV", Some(0x30a46c)),
        "doc" | "docx" | "odt" | "rtf" => ("DOC", Some(0x3e63dd)),
        "ppt" | "pptx" | "odp" => ("PPT", Some(0xf76b15)),
        "html" | "htm" => ("HTML", Some(0xf76b15)),
        _ => ("", None),
    };
    if !label.is_empty() { return (label.to_owned(), color); }
    (ext.to_uppercase().chars().take(4).collect(), None)
}

impl Hangar {
    /// A lista da conversa aberta com o que o painel já sabe (tamanhos, anexos do cofre), refeita só quando a
    /// conversa ou esse conhecimento mudam: o desenho do painel e do visor não relê as mensagens a cada quadro.
    pub(super) fn media_items(&mut self) -> std::rc::Rc<Vec<Item>> {
        // Sem alocar: isto roda a cada quadro do painel e do visor. A sessão muda com `selection` ou `revision`.
        let last = self.chat.events.last().map_or(0, |event| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::hash::DefaultHasher::new();
            event.id.hash(&mut hasher);
            hasher.finish()
        });
        let stamp = (self.selection, self.revision, self.chat.events.len(), last, self.media_panel.generation);
        if let Some((cached, items)) = &self.media_panel.items && *cached == stamp { return items.clone(); }
        let key = self.selected_key();
        let mut items = collect(&self.chat.events);
        if self.media_panel.owner.as_ref().is_some_and(|(owner, revision)| key.as_ref() == Some(owner) && *revision == self.revision) {
            if let Some(Ok(uploads)) = &self.media_panel.uploads { items = with_uploads(items, uploads); }
            items = with_found(items, &self.media_panel.cited);
        }
        let items = std::rc::Rc::new(items);
        self.media_panel.items = Some((stamp, items.clone()));
        items
    }

    /// Abre o visor na mídia da conversa inteira, a partir de `source`; fora dela (prévia de outra aba, mensagem que a
    /// lista não tem), só as imagens da mensagem, como antes.
    pub(super) fn open_gallery(&mut self, key: SessionKey, source: Source, fallback: Vec<Source>, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_key().as_ref() == Some(&key) {
            let items: Vec<Item> = self.media_items().iter().filter(|item| item.visual()).cloned().collect();
            // O quadro do vídeo enviado abre o próprio vídeo.
            if let Some(at) = items.iter().position(|item| item.source == source || item.poster.as_ref() == Some(&source)) {
                return self.open_viewer(key, items, at, true, window, cx);
            }
        }
        self.open_image(key, fallback, index, window, cx);
    }

    /// Fecha o visor na conversa: rola até a mensagem do item e a realça por um instante.
    pub(super) fn show_in_conversation(&mut self, key: SessionKey, event: String, cx: &mut Context<Self>) {
        if self.selected_key().as_ref() != Some(&key) { return; }
        self.media_panel.flash = Some(super::media_panel::Flash { key, event, row: None, at: None });
        self.redraw(super::panes::Area::Conversation, cx);
        cx.notify();
    }

    /// Passo do "Mostrar na conversa" a cada desenho da conversa: acha a linha, rola até ela e segura o realce.
    pub(super) fn paint_media_flash(&mut self, cx: &mut Context<Self>) {
        let Some(flash) = self.media_panel.flash.as_ref() else { return };
        if self.selected_key().as_ref() != Some(&flash.key) { self.media_panel.flash = None; return; }
        if flash.row.is_some() { return; }
        let event = flash.event.clone();
        let row = self.chat.events.iter().position(|e| e.id == event)
            .and_then(|ix| self.conversation.items.iter().position(|item| super::find::covers(item, ix)));
        let Some(row) = row else {
            // Ainda chegando com o histórico inteiro: o fim da carga redesenha a conversa.
            if !self.loading { self.media_panel.flash = None; }
            return;
        };
        let id = self.row_ids.get(row).cloned();
        if let Some(flash) = self.media_panel.flash.as_mut() { (flash.row, flash.at) = (id, Some(Instant::now())); }
        self.jump_to_row(row, cx);
    }

    /// Opacidade do realce da linha `id`, enquanto ele dura.
    pub(super) fn media_flash_alpha(&mut self, id: &str) -> Option<f32> {
        let flash = self.media_panel.flash.as_ref()?;
        if flash.row.as_deref() != Some(id) { return None; }
        let t = flash.at?.elapsed().as_secs_f32() / super::media_panel::FLASH.as_secs_f32();
        if t >= 1. { self.media_panel.flash = None; return None; }
        Some(0.22 * (1. - t))
    }
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` da gpui colide com o atributo padrão.
    use super::{DayName, Filter, Found, Kind, Origin, by_day, collect, day_name, found_entry, frame_of, type_badge, with_found, with_uploads};
    use crate::api::{Source, dto::{ChatEvent, UploadFile}};
    use chrono::NaiveDate;
    use std::collections::HashMap;

    fn event(kind: &str, id: &str, text: &str, ts: f64) -> ChatEvent {
        ChatEvent { kind: kind.into(), id: id.into(), text: Some(text.into()), ts: Some(ts), ..Default::default() }
    }

    #[test]
    fn gallery_walks_the_whole_conversation_in_order_and_folds_video_frames() {
        let sent = ChatEvent { kind: "tool_use".into(), id: "t1".into(), tool_name: Some("SendUserFile".into()), ts: Some(30.),
            tool_input: serde_json::json!({"files": ["/tmp/a/demo.mp4"]}).as_object().cloned(), ..Default::default() };
        let events = vec![
            event("assistant_msg", "a1", "Pronto: /tmp/a/um.png e /tmp/a/rel.pdf", 10.),
            event("user_msg", "u1", "olha — 📎arquivo:/srv/up/1-a.mp4 📎imagem:/srv/up/1-a-q1.jpg 📎imagem:/srv/up/1-a-q2.jpg", 20.),
            sent,
            // A mesma imagem citada de novo não entra duas vezes.
            event("assistant_msg", "a2", "De novo /tmp/a/um.png e /tmp/a/dois.png", 40.),
        ];
        let items = collect(&events);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["um.png", "rel.pdf", "1-a.mp4", "demo.mp4", "dois.png"]);
        assert_eq!(items.iter().map(|i| i.kind).collect::<Vec<_>>(), [Kind::Image, Kind::File, Kind::Video, Kind::Video, Kind::Image]);
        assert_eq!(items[2].poster, Some(Source::Upload("1-a-q1.jpg".into())), "o primeiro quadro vira a capa");
        assert_eq!(items[2].origin, Some(Origin::User));
        assert_eq!((items[3].origin, items[3].event.as_deref()), (Some(Origin::Agent), Some("t1")));
        assert_eq!(items[0].event.as_deref(), Some("a1"), "fica o primeiro lugar em que apareceu");
        assert_eq!(items[1].folder.as_deref(), Some("/tmp/a"));
        let visual: Vec<&str> = items.iter().filter(|i| i.visual()).map(|i| i.name.as_str()).collect();
        assert_eq!(visual, ["um.png", "1-a.mp4", "demo.mp4", "dois.png"], "o visor passa por imagem e vídeo, de mensagens diferentes");
    }

    #[test]
    fn read_brings_only_media_and_sent_files_still_count() {
        let tool = |id: &str, name: &str, input: serde_json::Value| ChatEvent { kind: "tool_use".into(), id: id.into(),
            tool_name: Some(name.into()), ts: Some(1.), tool_input: input.as_object().cloned(), ..Default::default() };
        let events = vec![
            tool("r1", "Read", serde_json::json!({"file_path": "/repo/src/main.rs"})),
            tool("r2", "Read", serde_json::json!({"file_path": "/repo/docs/guia.pdf"})),
            tool("r3", "Read", serde_json::json!({"file_path": "/tmp/tela.png"})),
            tool("r4", "Read", serde_json::json!({"file_path": "/tmp/fala.mp3"})),
            tool("s1", "SendUserFile", serde_json::json!({"files": ["/tmp/out/relatorio.pdf"]})),
        ];
        let items = collect(&events);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["tela.png", "fala.mp3", "relatorio.pdf"], "o que o Read leu de código e documento fica fora dos Arquivos");
        assert_eq!(items.iter().map(|i| i.kind).collect::<Vec<_>>(), [Kind::Image, Kind::Audio, Kind::File]);
    }

    #[test]
    fn uploads_add_sizes_and_the_unshown_files_in_their_place() {
        let items = collect(&[event("user_msg", "u1", "📎imagem:/srv/up/5-b.png", 50.), event("assistant_msg", "a1", "Feito: /tmp/x.png", 90.)]);
        let file = |name: &str, size: u64, mtime: f64| UploadFile { filename: name.into(), size, mtime };
        let uploads = [file("5-b.png", 700, 49.), file("7-dit.m4a", 30, 60.), file("8-v.mp4", 900, 95.), file("8-v-q1.jpg", 9, 95.),
            file("8-v-audio.m4a", 9, 95.)];
        let merged = with_uploads(items, &uploads);
        let names: Vec<&str> = merged.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["5-b.png", "7-dit.m4a", "x.png", "8-v.mp4"], "o ditado entra no lugar da data; quadro e fala do vídeo não");
        assert_eq!(merged[0].size, Some(700));
        assert_eq!((merged[1].kind, merged[1].origin, merged[1].event.as_ref()), (Kind::Audio, Some(Origin::User), None));
        assert_eq!(merged[3].poster, Some(Source::Upload("8-v-q1.jpg".into())));
    }

    #[test]
    fn resolver_size_reaches_the_cited_file_only_when_the_server_sends_it() {
        let items = collect(&[event("assistant_msg", "a1", "Relatório em ~/out/r.pdf e /tmp/s.zip", 1.)]);
        let found = HashMap::from([
            ("~/out/r.pdf".to_owned(), found_entry(&serde_json::json!({"relativo": null, "real": "/home/u/out/r.pdf", "size": 1800, "mtime": 5}))),
            ("/tmp/s.zip".to_owned(), found_entry(&serde_json::json!({"relativo": null, "real": "/tmp/s.zip"}))),
        ]);
        assert_eq!(found["/tmp/s.zip"], Some(Found { real: "/tmp/s.zip".into(), size: None }));
        let items = with_found(items, &found);
        assert_eq!((items[0].size, items[0].folder.as_deref()), (Some(1800), Some("/home/u/out")));
        assert_eq!(items[1].size, None, "servidor antigo não manda o tamanho: nada aparece");
    }

    #[test]
    fn filter_keeps_by_who_sent() {
        assert!(Filter::All.keeps(None) && Filter::All.keeps(Some(Origin::User)));
        assert!(Filter::Agent.keeps(Some(Origin::Agent)) && !Filter::Agent.keeps(Some(Origin::User)));
        assert!(Filter::User.keeps(Some(Origin::User)) && !Filter::User.keeps(Some(Origin::Agent)) && !Filter::User.keeps(None));
    }

    #[test]
    fn days_group_in_runs_and_name_today_yesterday_and_dates() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        let today = d(2026, 11, 21);
        assert_eq!(day_name(today, today), DayName::Today);
        assert_eq!(day_name(d(2026, 11, 20), today), DayName::Yesterday);
        assert_eq!(day_name(d(2026, 11, 2), today), DayName::Date { day: 2, month: 11, year: None });
        assert_eq!(day_name(d(2025, 12, 31), today), DayName::Date { day: 31, month: 12, year: Some(2025) });
        let days = [(3, Some(today)), (2, Some(today)), (1, Some(d(2026, 11, 20))), (0, None)];
        assert_eq!(by_day(&days), vec![(Some(today), vec![3, 2]), (Some(d(2026, 11, 20)), vec![1]), (None, vec![0])]);
    }

    #[test]
    fn video_frames_and_type_badges() {
        assert!(frame_of("1-a.mp4", "1-a-q1.jpg") && frame_of("1-a.mov", "1-a-q12.jpg"));
        assert!(!frame_of("1-a.mp4", "1-a-q.jpg") && !frame_of("1-a.mp4", "1-ab-q1.jpg") && !frame_of("1-a.mp4", "1-a-q1.png"));
        assert_eq!(type_badge("r.PDF"), ("PDF".into(), Some(0xe5484d)));
        assert_eq!(type_badge("planilha.xlsx").0, "XLS");
        assert_eq!(type_badge("notas.markdown"), ("MARK".into(), None));
    }
}

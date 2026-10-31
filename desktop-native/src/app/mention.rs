use super::*;
use std::ops::Range;

type Snapshot = (u64, u64, String, Range<usize>);

#[derive(Default)]
pub(super) struct Mention {
    snapshot: Option<Snapshot>,
    range: Option<Range<usize>>,
    serial: u64,
    task: Option<JoinHandle<()>>,
    result: Option<Result<Vec<String>, Failure>>,
    pick: usize,
}

impl Mention {
    pub fn close(&mut self) {
        self.serial += 1;
        self.range = None;
        self.result = None;
        if let Some(task) = self.task.take() { task.abort(); }
    }
}

impl Drop for Mention {
    fn drop(&mut self) { self.close(); }
}

#[derive(serde::Deserialize)]
struct Search { hits: Vec<Hit> }
#[derive(serde::Deserialize)]
struct Hit { path: String }

impl Hangar {
    fn mention_snapshot(&self, cx: &App) -> Snapshot {
        let input = self.composer.read(cx);
        (self.connection, self.selection, input.value().to_string(), input.selected_range())
    }

    pub(super) fn refresh_mention(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.mention_snapshot(cx);
        if self.mention.snapshot.as_ref() == Some(&snapshot) { return; }
        self.mention.close();
        self.mention.snapshot = Some(snapshot.clone());
        self.mention.pick = 0;
        self.redraw(panes::Area::Bottom, cx);
        if !snapshot.3.is_empty() { return; }
        let Some((range, query)) = composer::mention_query(&snapshot.2, snapshot.3.end) else { return; };
        let (Some(api), Some(session)) = (self.session_api(), self.selected.as_ref()) else { return; };
        self.mention.range = Some(range);
        if query.is_empty() { return; }
        let (query, name) = (query.to_owned(), session.name.clone());
        let (connection, selection, seq, tx) = (self.connection, self.selection, self.mention.serial, self.tx.clone());
        self.mention.task = Some(self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            let result = api.read(&name, &["files", "search"], &[("q", &query), ("mode", "names")], 15).await
                .and_then(|value| serde_json::from_value::<Search>(value).map_err(|_| Failure::local("invalid_response")))
                .map(|search| search.hits.into_iter().take(8).map(|hit| hit.path).collect());
            let _ = tx.send(Envelope { connection, selection: Some(selection), payload: Payload::Mentions(seq, result) }).await;
        }));
    }

    pub(super) fn mention_is_open(&self, cx: &App) -> bool {
        self.mention.range.is_some() && self.mention.snapshot.as_ref() == Some(&self.mention_snapshot(cx))
    }

    pub(super) fn receive_mentions(&mut self, seq: u64, result: Result<Vec<String>, Failure>, cx: &mut Context<Self>) {
        if seq != self.mention.serial || !self.mention_is_open(cx) { return; }
        self.mention.result = Some(result);
        self.redraw(panes::Area::Bottom, cx);
    }

    pub(super) fn move_mention(&mut self, step: isize, cx: &mut Context<Self>) -> bool {
        if !self.mention_is_open(cx) { return false; }
        if let Some(Ok(paths)) = &self.mention.result {
            if !paths.is_empty() { self.mention.pick = (self.mention.pick as isize + step).rem_euclid(paths.len() as isize) as usize; }
        }
        self.redraw(panes::Area::Bottom, cx);
        true
    }

    pub(super) fn accept_mention(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.mention_is_open(cx) { return false; }
        let path = self.mention.result.as_ref().and_then(|r| r.as_ref().ok()).and_then(|paths| paths.get(self.mention.pick)).cloned();
        if let Some(path) = path { self.pick_mention(path, window, cx); }
        true
    }

    fn pick_mention(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.mention_is_open(cx) { return; }
        let range = self.mention.range.clone().unwrap();
        self.replace_composer(range, format!("@{path}"), window, cx);
        self.mention.snapshot = Some(self.mention_snapshot(cx));
        self.mention.close();
        self.redraw(panes::Area::Bottom, cx);
        self.tour_event(crate::tour::TourEvent::ContextAdded, cx);
    }

    pub(super) fn render_mentions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.mention_is_open(cx) { return None; }
        let note = match &self.mention.result {
            None if self.mention.range.as_ref().is_some_and(|range| range.len() == 1) => Some(tr("mention_hint")),
            None => Some(tr("mention_loading")),
            Some(Err(error)) => Some(Self::fetch_failure(error)),
            Some(Ok(paths)) if paths.is_empty() => Some(tr("mention_empty")),
            _ => None,
        };
        let mut panel = div().flex().flex_col().rounded_md().bg(theme::raised()).p_1();
        if let Some(note) = note {
            panel = panel.child(div().px_3().py_2().text_sm().text_color(theme::muted()).child(note));
        } else if let Some(Ok(paths)) = &self.mention.result {
            for (ix, path) in paths.iter().enumerate() {
                let picked = path.clone();
                panel = panel.child(Button::new(SharedString::from(format!("mention-{path}"))).ghost().small().w_full()
                    .selected(ix == self.mention.pick).tab_stop(false).tooltip(path.clone())
                    .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).child(format!("@{path}")))
                    .on_click(cx.listener(move |this, _, window, cx| this.pick_mention(picked.clone(), window, cx))));
            }
        }
        Some(panel.into_any_element())
    }
}

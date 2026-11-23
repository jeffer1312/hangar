//! Contas puras da revisão do trabalho: as mesmas de `packages/core/src/workReview.ts`, que o nativo não importa.
use crate::api::dto::{WorkReview, WorkReviewChoice, WorkReviewDecision, WorkReviewDraftPick, WorkReviewState, WorkReviewSteps, WorkReviewSummary};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

/// O que a pessoa escolheu numa decisão; `reason` é o texto da caixa, mesmo quando a escolha não pede motivo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Picked { pub(super) choice: String, pub(super) reason: String }

pub(super) type Choices = HashMap<String, Picked>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone { Accent, Warning, Success }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StepStatus { Done, Current, Pending }

/// `tone` nulo = etapa por fazer; `pulse` só na etapa atual de uma revisão rodando.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Step { pub(super) status: StepStatus, pub(super) tone: Option<Tone>, pub(super) pulse: bool }

/// Escolha que existe e, quando pede motivo, com motivo escrito.
pub(super) fn decided(decision: &WorkReviewDecision, picked: Option<&Picked>) -> bool {
    let Some(picked) = picked else { return false };
    decision.choices.iter().find(|c| c.id == picked.choice).is_some_and(|c| !c.requires_reason || !picked.reason.trim().is_empty())
}

/// Decisões sem escolha válida ou com motivo exigido em branco.
pub(super) fn pending_count(review: &WorkReview, choices: &Choices) -> usize {
    review.decisions.iter().filter(|d| !decided(d, choices.get(&d.id))).count()
}

/// O servidor recebe todas as pendentes de uma vez, e só enquanto a revisão espera.
pub(super) fn ready_to_send(review: &WorkReview, choices: &Choices) -> bool {
    review.state == WorkReviewState::Waiting && !review.decisions.is_empty() && pending_count(review, choices) == 0
}

pub(super) fn initial_reason(choice: &WorkReviewChoice) -> String { choice.default_reason.clone().unwrap_or_default() }

/// O teto do servidor (`MAX_REASON` em `hangar-connectors`), em caracteres: acima dele o envio volta recusado.
pub(super) const MAX_REASON: usize = 2000;

/// O motivo cortado no teto; `None` quando já cabe.
pub(super) fn capped_reason(text: &str) -> Option<String> {
    text.char_indices().nth(MAX_REASON).map(|(at, _)| text[..at].to_owned())
}

pub(super) fn state_tone(state: WorkReviewState) -> Tone {
    match state {
        WorkReviewState::Running => Tone::Accent,
        WorkReviewState::Waiting => Tone::Warning,
        WorkReviewState::Applying | WorkReviewState::Done => Tone::Success,
    }
}

pub(super) fn step_status(steps: &WorkReviewSteps, state: WorkReviewState, index: usize) -> Step {
    let current = steps.current as usize;
    // Concluída fica toda verde mesmo que o conector não avance o `current` até o fim.
    if state == WorkReviewState::Done || index < current { return Step { status: StepStatus::Done, tone: Some(Tone::Success), pulse: false }; }
    if index == current { return Step { status: StepStatus::Current, tone: Some(state_tone(state)), pulse: state == WorkReviewState::Running }; }
    Step { status: StepStatus::Pending, tone: None, pulse: false }
}

/// Corpo do `POST …/work-review/decisions`; `None` enquanto falta decidir.
pub(super) fn decisions_payload(review: &WorkReview, choices: &Choices) -> Option<Value> {
    if !ready_to_send(review, choices) { return None; }
    let decisions: Vec<Value> = review.decisions.iter().map(|d| {
        let picked = &choices[&d.id];
        let required = d.choices.iter().find(|c| c.id == picked.choice).is_some_and(|c| c.requires_reason);
        if required { json!({"id": d.id, "choice": picked.choice, "reason": picked.reason.trim()}) } else { json!({"id": d.id, "choice": picked.choice}) }
    }).collect();
    Some(json!({"review_id": review.id, "decisions": decisions}))
}

/// Escolhas que ainda valem depois de reler: mesma rodada, decisão e escolha ainda presentes.
pub(super) fn carry(previous: &Choices, review: &WorkReview) -> Choices {
    previous.iter().filter(|(id, picked)| review.decisions.iter().any(|d| d.id == **id && d.choices.iter().any(|c| c.id == picked.choice)))
        .map(|(id, picked)| (id.clone(), picked.clone())).collect()
}

/// Onde o painel abre: na primeira decisão que falta; nada faltando, na primeira.
pub(super) fn first_pending(review: &WorkReview, choices: &Choices) -> usize {
    review.decisions.iter().position(|d| !decided(d, choices.get(&d.id))).unwrap_or(0)
}

/// O que ficou guardado no servidor por baixo do que a pessoa já escolheu aqui.
pub(super) fn with_saved(local: &Choices, saved: &HashMap<String, WorkReviewDraftPick>) -> Choices {
    let mut merged: Choices = saved.iter().map(|(id, p)| (id.clone(), Picked { choice: p.choice.clone(), reason: p.reason.clone() })).collect();
    merged.extend(local.iter().map(|(id, p)| (id.clone(), p.clone())));
    merged
}

/// Corpo do `PUT …/work-review/draft`: todas as escolhas da rodada, com o texto da caixa.
pub(super) fn draft_payload(review_id: &str, choices: &Choices) -> Value {
    let choices: serde_json::Map<String, Value> = choices.iter()
        .map(|(id, p)| (id.clone(), json!({"choice": p.choice, "reason": p.reason}))).collect();
    json!({"review_id": review_id, "choices": choices})
}

/// A releitura só vale quando a rodada, o estado ou a hora mudaram; o selo sozinho não pede outra leitura.
pub(super) fn moved(before: Option<&WorkReviewSummary>, now: Option<&WorkReviewSummary>) -> bool {
    match (before, now) {
        (Some(a), Some(b)) => (&a.review_id, a.state, &a.updated_at) != (&b.review_id, b.state, &b.updated_at),
        (None, None) => false,
        _ => true,
    }
}

/// Retângulo em px do quadro: x, y, largura, altura.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rect { pub(super) x: f32, pub(super) y: f32, pub(super) w: f32, pub(super) h: f32 }

/// Onde a imagem fica dentro do quadro e a moldura do trecho (`None` sem `box`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Framing { pub(super) image: Rect, pub(super) mark: Option<Rect> }

/// O `enquadrar` do desenho: o trecho cabe no quadro com folga (160 × 120 px do asset), zoom até 3×, centrado; a moldura
/// sai 3 px para fora do trecho. Sem trecho, a imagem inteira cabe centrada.
pub(super) fn frame(view: (f32, f32), image: (u32, u32), area: Option<[u32; 4]>) -> Framing {
    let (w, h) = view;
    let (iw, ih) = (image.0.max(1) as f32, image.1.max(1) as f32);
    let Some([x, y, bw, bh]) = area.map(|a| a.map(|n| n as f32)) else {
        let z = (w / iw).min(h / ih);
        return Framing { image: Rect { x: (w - iw * z) / 2., y: (h - ih * z) / 2., w: iw * z, h: ih * z }, mark: None };
    };
    let z = (w / (bw + 160.)).min(h / (bh + 120.)).min(3.);
    let (left, top) = ((x + bw / 2.) * z - w / 2., (y + bh / 2.) * z - h / 2.);
    Framing {
        image: Rect { x: -left, y: -top, w: iw * z, h: ih * z },
        mark: Some(Rect { x: x * z - left - 3., y: y * z - top - 3., w: bw * z + 6., h: bh * z + 6. }),
    }
}

/// Guarda o evento `work_reviews` da máquina `server`: `None` quando o corpo não se lê, senão se mudou. Linha torta sai
/// sozinha, como no web.
pub(in crate::app) fn store(map: &mut HashMap<String, HashMap<String, WorkReviewSummary>>, server: &str, data: &Value) -> Option<bool> {
    let rows = data.get("reviews")?.as_object()?;
    let list: HashMap<String, WorkReviewSummary> = rows.iter()
        .filter_map(|(name, row)| Some((name.clone(), WorkReviewSummary::deserialize(row).ok()?))).collect();
    if map.get(server) == Some(&list) { return Some(false); }
    map.insert(server.to_owned(), list);
    Some(true)
}

/// Quem pediu a leitura: decide se a falha dela vai à tela.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Read {
    /// Abrir a sessão: a pessoa não pediu a revisão, só abriu a conversa.
    Open,
    /// O evento da lista disse que a revisão andou.
    Background,
    /// Depois de um gesto da pessoa (enviar as decisões).
    Requested,
}

/// A falha só aparece quando a pessoa agiu ou quando a lista já disse que a sessão tem revisão: sem isso, toda sessão
/// com branch e sem ticket mostraria a falha da busca pela branch.
pub(super) fn shows_failure(read: Read, known: bool) -> bool {
    match read { Read::Requested => true, Read::Open => known, Read::Background => false }
}

/// Como terminou a última leitura da revisão da sessão aberta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Nenhuma leitura terminou para esta sessão ainda.
    Pending,
    /// Sem vínculo, sem a capacidade ou sem revisão: não há o que mostrar.
    Absent,
    Present,
    /// O erro mora na aba.
    Failed,
}

/// A aba aparece quando a lista diz que há revisão ou a leitura trouxe algo; a leitura vazia esconde mesmo com o resumo
/// na lista, senão a aba abriria em branco. Releitura em voo não volta a mostrar: ela guarda o último resultado.
pub(super) fn tab_shown(listed: bool, read: Outcome) -> bool {
    match read { Outcome::Pending => listed, Outcome::Absent => false, Outcome::Present | Outcome::Failed => true }
}

/// Texto da faixa de chamada: o do conector, o de "prontas para enviar" ou o do Hangar para N decisões esperando.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BandText { Ready, Waiting(usize), Connector(String) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Band { pub(super) value: String, pub(super) text: Option<BandText>, pub(super) detail: Option<String> }

/// Faixa de chamada, como no Expo: aparece com `headline` ou esperando decisões; esperando, o número é o que ainda falta
/// decidir aqui, e zerado o texto vira "prontas para enviar". O texto do conector foi escrito para o número dele: só
/// vale quando é o mesmo que falta aqui.
pub(super) fn band(review: &WorkReview, pending: usize) -> Option<Band> {
    let headline = review.headline.as_ref();
    let waiting = review.state == WorkReviewState::Waiting && !review.decisions.is_empty();
    if !waiting { return headline.map(|h| Band { value: h.value.clone(), text: Some(BandText::Connector(h.text.clone())), detail: h.detail.clone() }); }
    let text = Some(match headline {
        _ if pending == 0 => BandText::Ready,
        Some(h) if h.value == pending.to_string() => BandText::Connector(h.text.clone()),
        _ => BandText::Waiting(pending),
    });
    Some(Band { value: pending.to_string(), text, detail: headline.and_then(|h| h.detail.clone()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::dto::WorkReviewTone;

    fn review(state: &str, decisions: Value) -> WorkReview {
        serde_json::from_value(json!({"id": "r1", "title": "T", "updated_at": "2026-11-18T10:00:00Z", "state": state,
            "decisions": decisions, "links": []})).unwrap()
    }

    fn two() -> WorkReview {
        review("waiting", json!([
            {"id": "47", "title": "Janela", "choices": [{"id": "igualar", "label": "Igualar"},
                {"id": "manter", "label": "Manter", "requires_reason": true, "default_reason": "padrão do kit"}]},
            {"id": "86", "title": "Título", "choices": [{"id": "igualar", "label": "Igualar"}]},
        ]))
    }

    fn pick(choice: &str, reason: &str) -> Picked { Picked { choice: choice.into(), reason: reason.into() } }

    #[test]
    fn pending_counts_missing_unknown_and_blank_required_reason() {
        let r = two();
        assert_eq!(pending_count(&r, &Choices::new()), 2);
        let mut c = Choices::from([("47".to_owned(), pick("manter", "   "))]);
        assert_eq!(pending_count(&r, &c), 2, "motivo em branco conta como pendente");
        c.insert("47".into(), pick("manter", "dado sem fonte"));
        c.insert("86".into(), pick("sumiu", ""));
        assert_eq!(pending_count(&r, &c), 1, "escolha inexistente conta como pendente");
        c.insert("86".into(), pick("igualar", ""));
        assert_eq!(pending_count(&r, &c), 0);
    }

    #[test]
    fn ready_only_while_waiting_with_nothing_pending() {
        let c = Choices::from([("47".to_owned(), pick("igualar", "")), ("86".to_owned(), pick("igualar", ""))]);
        assert!(ready_to_send(&two(), &c));
        assert!(!ready_to_send(&two(), &Choices::new()));
        let mut done = two();
        done.state = WorkReviewState::Applying;
        assert!(!ready_to_send(&done, &c));
        assert!(!ready_to_send(&review("waiting", json!([])), &Choices::new()), "sem decisão não há o que enviar");
    }

    #[test]
    fn payload_carries_trimmed_reason_only_where_required() {
        let c = Choices::from([("47".to_owned(), pick("manter", "  padrão do kit ")), ("86".to_owned(), pick("igualar", "sobra"))]);
        assert_eq!(decisions_payload(&two(), &c), Some(json!({"review_id": "r1", "decisions": [
            {"id": "47", "choice": "manter", "reason": "padrão do kit"}, {"id": "86", "choice": "igualar"}]})));
        assert_eq!(decisions_payload(&two(), &Choices::new()), None);
    }

    #[test]
    fn initial_reason_is_the_connector_default() {
        let r = two();
        assert_eq!(initial_reason(&r.decisions[0].choices[1]), "padrão do kit");
        assert_eq!(initial_reason(&r.decisions[0].choices[0]), "");
    }

    #[test]
    fn reason_is_capped_in_characters_at_the_server_limit() {
        assert_eq!(capped_reason(&"é".repeat(MAX_REASON)), None, "no teto, conta caractere e não byte");
        assert_eq!(capped_reason(&format!("{}xyz", "é".repeat(MAX_REASON))), Some("é".repeat(MAX_REASON)));
    }

    #[test]
    fn tone_follows_the_state() {
        assert_eq!(state_tone(WorkReviewState::Running), Tone::Accent);
        assert_eq!(state_tone(WorkReviewState::Waiting), Tone::Warning);
        assert_eq!(state_tone(WorkReviewState::Applying), Tone::Success);
        assert_eq!(state_tone(WorkReviewState::Done), Tone::Success);
    }

    #[test]
    fn steps_done_current_and_pending() {
        let steps = WorkReviewSteps { labels: (0..6).map(|n| n.to_string()).collect(), current: 4 };
        let done = Step { status: StepStatus::Done, tone: Some(Tone::Success), pulse: false };
        let pending = Step { status: StepStatus::Pending, tone: None, pulse: false };
        assert_eq!(step_status(&steps, WorkReviewState::Waiting, 3), done);
        assert_eq!(step_status(&steps, WorkReviewState::Waiting, 4), Step { status: StepStatus::Current, tone: Some(Tone::Warning), pulse: false });
        assert_eq!(step_status(&steps, WorkReviewState::Waiting, 5), pending);
        let running = WorkReviewSteps { current: 0, ..steps.clone() };
        assert_eq!(step_status(&running, WorkReviewState::Running, 0), Step { status: StepStatus::Current, tone: Some(Tone::Accent), pulse: true });
        assert_eq!(step_status(&running, WorkReviewState::Done, 5), done, "concluída fica toda verde");
        let all = WorkReviewSteps { current: 6, ..steps };
        assert_eq!(step_status(&all, WorkReviewState::Applying, 5), done, "current igual ao total = todas feitas");
    }

    #[test]
    fn carry_keeps_only_what_the_new_reading_still_has() {
        let c = Choices::from([("47".to_owned(), pick("manter", "x")), ("86".to_owned(), pick("igualar", "")), ("99".to_owned(), pick("igualar", ""))]);
        let mut r = two();
        r.decisions[1].choices[0].id = "outra".into();
        assert_eq!(carry(&c, &r), Choices::from([("47".to_owned(), pick("manter", "x"))]));
    }

    #[test]
    fn opens_at_the_first_decision_still_missing() {
        assert_eq!(first_pending(&two(), &Choices::new()), 0);
        assert_eq!(first_pending(&two(), &Choices::from([("47".to_owned(), pick("igualar", ""))])), 1);
        assert_eq!(first_pending(&two(), &Choices::from([("47".to_owned(), pick("manter", " "))])), 0, "motivo em branco ainda falta");
        let all = Choices::from([("47".to_owned(), pick("igualar", "")), ("86".to_owned(), pick("igualar", ""))]);
        assert_eq!(first_pending(&two(), &all), 0, "tudo decidido abre na primeira");
    }

    #[test]
    fn what_was_picked_here_wins_over_the_saved() {
        let saved = HashMap::from([("47".to_owned(), WorkReviewDraftPick { choice: "manter".into(), reason: "do servidor".into() }),
            ("86".to_owned(), WorkReviewDraftPick { choice: "igualar".into(), reason: String::new() })]);
        let local = Choices::from([("47".to_owned(), pick("igualar", ""))]);
        assert_eq!(with_saved(&local, &saved), Choices::from([("47".to_owned(), pick("igualar", "")), ("86".to_owned(), pick("igualar", ""))]));
    }

    #[test]
    fn draft_payload_keeps_every_pick_with_its_reason() {
        let c = Choices::from([("47".to_owned(), pick("manter", "regra")), ("86".to_owned(), pick("igualar", ""))]);
        assert_eq!(draft_payload("r1", &c), json!({"review_id": "r1", "choices": {
            "47": {"choice": "manter", "reason": "regra"}, "86": {"choice": "igualar", "reason": ""}}}));
    }

    #[test]
    fn moved_ignores_the_badge() {
        let a = WorkReviewSummary { review_id: "r1".into(), state: WorkReviewState::Waiting, badge: None, updated_at: "t1".into() };
        let badge = WorkReviewSummary { badge: Some(crate::api::dto::WorkReviewBadge { text: "x".into(), tone: WorkReviewTone::Info }), ..a.clone() };
        assert!(!moved(Some(&a), Some(&badge)));
        assert!(moved(Some(&a), Some(&WorkReviewSummary { updated_at: "t2".into(), ..a.clone() })));
        assert!(moved(Some(&a), Some(&WorkReviewSummary { review_id: "r2".into(), ..a.clone() })));
        assert!(moved(Some(&a), Some(&WorkReviewSummary { state: WorkReviewState::Applying, ..a.clone() })));
        assert!(moved(None, Some(&a)));
        assert!(moved(Some(&a), None));
        assert!(!moved(None, None));
    }

    #[test]
    fn frame_centers_the_box_with_margin() {
        let f = frame((300., 200.), (1000, 1000), Some([100, 100, 140, 80]));
        assert_eq!(f.image, Rect { x: -20., y: -40., w: 1000., h: 1000. });
        assert_eq!(f.mark, Some(Rect { x: 77., y: 57., w: 146., h: 86. }));
    }

    #[test]
    fn frame_zoom_stops_at_three() {
        let f = frame((900., 600.), (1600, 900), Some([800, 450, 10, 10]));
        assert_eq!(f.image.w, 4800.);
        assert_eq!(f.mark.map(|m| (m.w, m.h)), Some((36., 36.)));
    }

    #[test]
    fn frame_without_box_fits_the_whole_image() {
        let f = frame((300., 150.), (1600, 900), None);
        assert!((f.image.w - 266.667).abs() < 0.01 && f.image.h == 150. && (f.image.x - 16.667).abs() < 0.01 && f.image.y == 0.);
        assert_eq!(f.mark, None);
    }

    #[test]
    fn failure_shows_only_when_asked_or_known() {
        assert!(!shows_failure(Read::Open, false), "sessão sem revisão conhecida não mostra a falha da busca");
        assert!(shows_failure(Read::Open, true));
        assert!(!shows_failure(Read::Background, true));
        assert!(!shows_failure(Read::Background, false));
        assert!(shows_failure(Read::Requested, false), "falha depois do envio sempre aparece");
        assert!(shows_failure(Read::Requested, true));
    }

    #[test]
    fn tab_hides_after_an_empty_read_even_when_listed() {
        assert!(tab_shown(true, Outcome::Pending), "a lista disse que há revisão: a aba aparece enquanto lê");
        assert!(!tab_shown(false, Outcome::Pending));
        assert!(!tab_shown(true, Outcome::Absent), "leitura vazia esconde a aba mesmo com o resumo na lista");
        assert!(!tab_shown(false, Outcome::Absent));
        assert!(tab_shown(false, Outcome::Present));
        assert!(tab_shown(false, Outcome::Failed), "o erro do envio mora na aba");
    }

    #[test]
    fn band_while_waiting_needs_no_headline() {
        let r = two();
        assert_eq!(band(&r, 2), Some(Band { value: "2".into(), text: Some(BandText::Waiting(2)), detail: None }));
        assert_eq!(band(&r, 0), Some(Band { value: "0".into(), text: Some(BandText::Ready), detail: None }));
        let mut with = two();
        with.headline = Some(crate::api::dto::WorkReviewHeadline { value: "9".into(), text: "esperam".into(), detail: Some("d".into()) });
        let mut done = with.clone();
        done.state = WorkReviewState::Done;
        done.decisions.clear();
        assert_eq!(band(&done, 0), Some(Band { value: "9".into(), text: Some(BandText::Connector("esperam".into())), detail: Some("d".into()) }));
        let mut bare = two();
        bare.state = WorkReviewState::Applying;
        bare.decisions.clear();
        assert_eq!(band(&bare, 0), None, "fora da espera, sem headline não há faixa");
    }

    #[test]
    fn band_uses_the_connector_text_only_for_its_own_count() {
        let mut r = two();
        r.headline = Some(crate::api::dto::WorkReviewHeadline { value: "3".into(), text: "3 diferenças".into(), detail: Some("d".into()) });
        let at = |pending| band(&r, pending).and_then(|b| b.text);
        assert_eq!(at(3), Some(BandText::Connector("3 diferenças".into())));
        assert_eq!(at(2), Some(BandText::Waiting(2)), "o texto do conector falaria de 3");
        assert_eq!(at(1), Some(BandText::Waiting(1)));
        assert_eq!(at(0), Some(BandText::Ready));
        assert_eq!(band(&r, 1).map(|b| (b.value, b.detail)), Some(("1".into(), Some("d".into()))));
    }

    #[test]
    fn store_replaces_the_machine_list_and_drops_bad_rows() {
        let mut map = HashMap::new();
        let data = json!({"reviews": {"a": {"review_id": "r1", "state": "waiting", "badge": {"text": "mock: 2", "tone": "warning"}, "updated_at": "t"},
            "b": {"review_id": "r2", "state": "nonsense", "updated_at": "t"}}});
        assert_eq!(store(&mut map, "m1", &data), Some(true));
        assert_eq!(map["m1"].keys().collect::<Vec<_>>(), vec!["a"]);
        assert_eq!(store(&mut map, "m1", &data), Some(false));
        assert_eq!(store(&mut map, "m1", &json!({"x": 1})), None);
        assert_eq!(store(&mut map, "m1", &json!({"reviews": {}})), Some(true));
        assert!(map["m1"].is_empty());
    }
}

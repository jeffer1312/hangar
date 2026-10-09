mod mods_support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use hangar_server::mods::state::*;
use mods_support::NoLink;
use serde_json::{Value, json};

/// Um `plugin_ui` com o painel `painel` e o botão `abrir` do mod `vitrine`: é dele que a janela do clique
/// toma a cópia.
fn with_button() -> Value {
    json!({"above": null, "panes": [{"id": "painel", "title": "Painel", "placement": "dock", "columns": 58,
        "tree": {"type": "Button", "props": {"key": "abrir", "label": "Abrir"}, "press": {"plugin": "vitrine", "handle": 1}}}],
        "shown_id": "painel", "columns": 110, "source": "surface"})
}

fn mods() -> Mods {
    let mods = Mods::default();
    mods.attach("s", 1, Arc::new(NoLink));
    mods
}

fn replayed(mods: &Mods, event: &str) -> Vec<Value> {
    mods.replay("s").into_iter().filter(|(name, _)| *name == event).map(|(_, data)| serde_json::from_str(&data).unwrap()).collect()
}

#[test]
fn attach_owns_and_forget_checks_generation() {
    let mods = mods();
    assert!(mods.owns("s") && !mods.owns("outra"));
    mods.forget("s", 2);
    assert!(mods.owns("s"), "geração diferente não esquece a sessão de agora");
    mods.forget("s", 1);
    assert!(!mods.owns("s") && mods.link("s").is_none());
}

#[test]
fn publish_keeps_the_latest_and_skips_repeats() {
    let mods = mods();
    let band = json!({"above": {"type": "Text"}, "panes": [], "shown_id": null, "columns": 110, "source": "surface"});
    assert!(mods.publish_ui("s", 1, band.clone()));
    assert!(!mods.publish_ui("s", 1, band.clone()), "igual ao último não sai de novo");
    assert!(!mods.publish_ui("s", 0, json!({"above": null})), "geração velha não publica");
    assert_eq!(replayed(&mods, "plugin_ui"), vec![band]);
}

#[test]
fn toasts_follow_the_python_limits() {
    let mods = mods();
    mods.toast("s", 1, &"m".repeat(80), &"x".repeat(2500), 10);
    mods.toast("s", 1, "vitrine", "padrão", 0);
    mods.toast("s", 1, "vitrine", "longo", 10_000_000);
    mods.toast("s", 1, "vitrine", "   ", 4000);
    let toasts = replayed(&mods, "plugin_toast");
    assert_eq!(toasts.len(), 3, "texto vazio não vira aviso");
    assert_eq!(toasts[0]["text"].as_str().unwrap().chars().count(), 2000);
    assert_eq!(toasts[0]["plugin"].as_str().unwrap().chars().count(), 64);
    assert!(toasts[0]["timeoutMs"].as_u64().unwrap() <= 1000 && toasts[0]["timeoutMs"].as_u64().unwrap() > 900,
        "10 ms sobe ao piso de 1000");
    assert!(toasts[1]["timeoutMs"].as_u64().unwrap() <= 4000 && toasts[1]["timeoutMs"].as_u64().unwrap() > 3000);
    assert!(toasts[2]["timeoutMs"].as_u64().unwrap() <= 300_000 && toasts[2]["timeoutMs"].as_u64().unwrap() > 299_000);
    assert!(toasts.iter().all(|toast| toast["id"].as_str().unwrap().starts_with("rs-")));
    for index in 0..25 { mods.toast("s", 1, "vitrine", &format!("n-{index}"), 9000); }
    let kept = replayed(&mods, "plugin_toast");
    assert_eq!(kept.len(), 20);
    assert_eq!(kept[0]["text"], "n-5", "o mais antigo sai primeiro");
}

#[tokio::test]
async fn click_effects_belong_to_the_open_click() {
    let mods = mods();
    mods.publish_ui("s", 1, with_button());
    let attempt = mods.begin_click("s", "painel", "vitrine", "abrir");
    assert_eq!(mods.match_click("s", "painel", None, "outra"), None);
    assert_eq!(mods.match_click("s", "painel", Some("outro-mod"), "abrir"), None, "a mesma `key` de outro mod não é o clique");
    assert_eq!(mods.match_click("s", "painel", Some("vitrine"), "abrir").as_deref(), Some(attempt.as_str()));
    assert_eq!(mods.match_click("s", "painel", None, "abrir"), None, "o press só casa uma vez");
    // O plugin do Hangar carregado antes de o press levar o mod casa pelo lugar e pela `key`.
    let attempt = mods.begin_click("s", "painel", "vitrine", "abrir");
    assert_eq!(mods.match_click("s", "painel", None, "abrir").as_deref(), Some(attempt.as_str()));
    assert_eq!(mods.match_click("s", "painel", None, "abrir"), None, "o press só casa uma vez");
    assert!(!mods.opened("s", "outra-tentativa", "https://example.com"));
    assert!(mods.opened("s", &attempt, "https://example.com"));
    mods.copied("s", 1, "vitrine", "texto");
    assert_eq!(mods.finish_click("s", &attempt, Duration::from_millis(300)).await,
        (Some("texto".to_owned()), Some("https://example.com".to_owned())));
    assert!(replayed(&mods, "plugin_toast").is_empty(), "cópia de clique em aberto não vira aviso");
}

#[tokio::test]
async fn copy_from_another_mod_is_a_toast() {
    // A janela de 1,5 s é do mod dono do botão (A11): um relógio de outro mod que copia no meio não vai
    // para o aparelho de quem clicou.
    let mods = mods();
    mods.publish_ui("s", 1, with_button());
    let attempt = mods.begin_click("s", "painel", "vitrine", "abrir");
    mods.copied("s", 1, "outro-mod", "texto de outro");
    assert_eq!(mods.finish_click("s", &attempt, Duration::from_millis(10)).await, (None, None));
    let toasts = replayed(&mods, "plugin_toast");
    assert_eq!((toasts[0]["text"].as_str(), toasts[0]["plugin"].as_str()), (Some("texto de outro"), Some("outro-mod")));
}

#[test]
fn copy_without_click_becomes_a_toast() {
    let mods = mods();
    mods.copied("s", 1, "vitrine", "Texto copiado pela vitrine (V44)");
    let toasts = replayed(&mods, "plugin_toast");
    assert_eq!((toasts[0]["text"].as_str(), toasts[0]["plugin"].as_str()), (Some("Texto copiado pela vitrine (V44)"), Some("vitrine")));
}

#[tokio::test]
async fn finish_waits_for_a_late_effect_only_when_the_plugin_matched() {
    let mods = mods();
    let attempt = mods.begin_click("s", "painel", "vitrine", "abrir");
    let started = Instant::now();
    assert_eq!(mods.finish_click("s", &attempt, Duration::from_millis(300)).await, (None, None));
    assert!(started.elapsed() < Duration::from_millis(100), "sem o plugin no press não há efeito a esperar");

    let attempt = mods.begin_click("s", "painel", "vitrine", "abrir");
    mods.match_click("s", "painel", None, "abrir").unwrap();
    let late = { let mods = mods.clone(); let attempt = attempt.clone();
        tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(100)).await; mods.opened("s", &attempt, "https://example.com") }) };
    assert_eq!(mods.finish_click("s", &attempt, Duration::from_millis(300)).await.1.as_deref(), Some("https://example.com"));
    assert!(late.await.unwrap());
}

#[tokio::test]
async fn finish_gives_up_when_the_matched_press_never_produces_an_effect() {
    let mods = mods();
    let attempt = mods.begin_click("s", "painel", "vitrine", "abrir");
    mods.match_click("s", "painel", None, "abrir").unwrap();
    let started = Instant::now();
    assert_eq!(mods.finish_click("s", &attempt, Duration::from_millis(100)).await, (None, None));
    assert!(started.elapsed() >= Duration::from_millis(100), "com o press casado espera até o prazo");
}

#[test]
fn attach_keeps_the_live_toasts() {
    let mods = mods();
    mods.toast("s", 1, "vitrine", "fica", 9000);
    mods.attach("s", 2, Arc::new(NoLink));
    let toasts = replayed(&mods, "plugin_toast");
    assert_eq!(toasts.len(), 1);
    assert_eq!(toasts[0]["text"], "fica");
}

#[test]
fn bridge_finds_the_session_by_the_key_of_the_process() {
    // O token com chave diz o processo: renomeada sem relançar, a sessão segue achada; relançada (processo
    // novo), a chave antiga não acha mais nada. A conferência do HMAC é da rota.
    let mods = Mods::default();
    mods.attach_process("a", "p1:10:x", 1, Arc::new(NoLink));
    assert_eq!(mods.bridge_session("a", "p1.mac").as_deref(), Some("a"));
    mods.forget("a", 1);
    mods.attach_process("c", "p1:10:x", 2, Arc::new(NoLink));
    assert_eq!(mods.bridge_session("a", "p1.mac").as_deref(), Some("c"));
    mods.forget("c", 2);
    mods.attach_process("c", "p2:11:x", 3, Arc::new(NoLink));
    assert_eq!((mods.bridge_session("c", "p2.mac").as_deref(), mods.bridge_session("a", "p1.mac")), (Some("c"), None));
    // Token só do nome (processo lançado antes da chave): a sessão com esse nome agora.
    assert_eq!((mods.bridge_session("c", "hex").as_deref(), mods.bridge_session("a", "hex")), (Some("c"), None));
}

#[test]
fn a_new_session_with_an_old_name_inherits_nothing_and_shares_no_bridge() {
    let mods = Mods::default();
    mods.attach_process("a", "p1", 1, Arc::new(NoLink));
    mods.publish_ui("a", 1, with_button());
    mods.toast("a", 1, "vitrine", "da antiga", 60_000);
    mods.begin_click("a", "painel", "vitrine", "abrir");
    // A sessão é renomeada para `b` (mesmo processo) e outra nasce com o nome `a`.
    mods.forget("a", 1);
    mods.attach_process("b", "p1", 2, Arc::new(NoLink));
    mods.attach_process("a", "p2", 3, Arc::new(NoLink));
    let replay = mods.replay("a");
    assert!(replay.is_empty(), "nem faixa nem aviso da antiga: {replay:?}");
    assert_eq!(mods.match_click("a", "painel", None, "abrir"), None, "nem o clique em aberto");
    // A vida da antiga não publica na nova, mesmo com o mesmo nome.
    assert!(!mods.publish_ui("a", 1, json!({"above": {"type": "Text"}})));
    mods.toast("a", 1, "vitrine", "atrasado", 4000);
    assert!(mods.replay("a").is_empty());
    // Os dois processos vivos nasceram como `a`, cada um com a própria chave no token.
    assert_eq!((mods.bridge_session("a", "p1.mac").as_deref(), mods.bridge_session("a", "p2.mac").as_deref()), (Some("b"), Some("a")));
}

#[test]
fn another_process_replacing_a_live_session_takes_nothing_from_it() {
    // Um `close` preso deixa a sessão no `Mods`; outra sessão aberta com o mesmo nome a substitui.
    let mods = Mods::default();
    mods.attach_process("x", "p1", 1, Arc::new(NoLink));
    mods.toast("x", 1, "vitrine", "da antiga", 60_000);
    mods.attach_process("x", "p2", 2, Arc::new(NoLink));
    assert!(mods.replay("x").is_empty(), "outro processo não herda os avisos");
    assert!(!mods.publish_ui("x", 1, json!({"above": null})), "o ator velho não publica na sessão nova");
    // Reaberta no mesmo processo, os avisos vivos ficam.
    mods.toast("x", 2, "vitrine", "da nova", 60_000);
    mods.attach_process("x", "p2", 3, Arc::new(NoLink));
    assert_eq!(mods.replay("x").len(), 1);
}

/// Sem terminal, o que o plugin manda pela ponte (`caps` e o estado do painel) entra na vista da superfície: na
/// próxima publicação dela e já na última, republicada quando o `extra` muda.
#[test]
fn surface_view_carries_the_plugin_extra() {
    let mods = mods();
    let view = json!({"above": null, "panes": [{"id": "hangar-btw", "tree": {"type": "Box"}}, {"id": "outro", "tree": null}],
        "shown_id": "hangar-btw", "columns": 80, "source": "surface"});
    assert!(mods.publish_ui("s", 1, view.clone()));
    assert!(replayed(&mods, "plugin_ui").last().unwrap().get("caps").is_none(), "sem extra, a vista de sempre");
    let data = std::collections::BTreeMap::from([("hangar-btw".to_owned(), json!({"current": 0, "entries": []}))]);
    mods.surface_extra("s", SurfaceExtra { caps: vec!["btw".into()], data });
    let last = replayed(&mods, "plugin_ui").last().cloned().unwrap();
    assert_eq!(last["caps"], json!(["btw"]));
    assert_eq!(last["panes"][0]["data"]["current"], 0);
    assert!(last["panes"][1].get("data").is_none());
    // A próxima vista da superfície continua com o extra.
    let mut next = view;
    next["shown_id"] = json!("outro");
    assert!(mods.publish_ui("s", 1, next));
    assert_eq!(replayed(&mods, "plugin_ui").last().unwrap()["caps"], json!(["btw"]));
}

/// O ator morreu: o que o plugin anunciou some junto, e o app para de oferecer o `/btw`.
#[test]
fn clear_drops_the_plugin_extra() {
    let mods = mods();
    mods.publish_ui("s", 1, json!({"above": null, "panes": [], "shown_id": null, "columns": 80, "source": "surface"}));
    mods.surface_extra("s", SurfaceExtra { caps: vec!["btw".into()], data: Default::default() });
    assert_eq!(replayed(&mods, "plugin_ui").last().unwrap()["caps"], json!(["btw"]));
    mods.clear_ui("s", 1);
    assert!(replayed(&mods, "plugin_ui").last().unwrap().get("caps").is_none());
}

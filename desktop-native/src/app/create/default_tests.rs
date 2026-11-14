use super::*;
use core::prelude::v1::test;
use ::gpui::{TestAppContext, VisualTestContext};

fn dialog(cx: &mut TestAppContext) -> (Entity<NewSession>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let cx = cx.add_empty_window();
    let dialog = cx.update(|window, cx| cx.new(|cx| NewSession::new(Link {
        api: Api::new("http://127.0.0.1:1", "fixture-owner").unwrap(),
        runtime: Arc::new(Runtime::new().unwrap()), tx: async_channel::unbounded().0,
        connection: 1, servers: Vec::new(), servers_rev: 0,
    }, None, window, cx)));
    (dialog, cx)
}

fn choice(model: &str) -> (String, String, String) {
    (model.into(), "high".into(), "bypassPermissions".into())
}

#[::gpui::test]
fn stale_save_reply_does_not_release_a_newer_save_or_replace_its_default(cx: &mut TestAppContext) {
    let (view, cx) = dialog(cx);
    cx.update(|window, cx| view.update(cx, |dialog, cx| {
        dialog.default_save_seq = 2;
        dialog.default_saving = true;
        dialog.saved_default = Some(choice("current"));
        let scope = dialog.models.seq;
        dialog.receive(CreateReply::HarnessDefaultSaved(1, scope, Ok(Some(choice("stale")))), window, cx);
        assert!(dialog.default_saving);
        assert_eq!(dialog.saved_default, Some(choice("current")));
        dialog.receive(CreateReply::HarnessDefaultSaved(2, scope, Ok(Some(choice("new")))), window, cx);
        assert!(!dialog.default_saving);
        assert_eq!(dialog.saved_default, Some(choice("new")));
    }));
}

#[::gpui::test]
fn changing_scope_keeps_the_save_pending_and_ignores_its_old_default(cx: &mut TestAppContext) {
    let (view, cx) = dialog(cx);
    cx.update(|window, cx| view.update(cx, |dialog, cx| {
        let old_scope = dialog.models.seq;
        dialog.default_save_seq = 1;
        dialog.default_saving = true;
        dialog.load_models(window, cx);
        assert!(dialog.default_saving);
        let current_scope = dialog.models.seq;
        dialog.receive_models(current_scope, Ok(json!({"models":[]})), Default::default(), Ok(Some(choice("current"))), window, cx);
        dialog.receive_models(old_scope, Ok(json!({"models":[]})), Default::default(), Ok(Some(choice("stale"))), window, cx);
        assert!(dialog.default_saving);
        assert_eq!(dialog.saved_default, Some(choice("current")));
        dialog.receive(CreateReply::HarnessDefaultSaved(1, old_scope, Ok(Some(choice("stale")))), window, cx);
        assert!(!dialog.default_saving);
        assert_eq!(dialog.saved_default, Some(choice("current")));
    }));
}

#[::gpui::test]
fn failed_save_stays_visible_after_changing_scope_without_replacing_the_current_default(cx: &mut TestAppContext) {
    let (view, cx) = dialog(cx);
    cx.update(|window, cx| view.update(cx, |dialog, cx| {
        let old_scope = dialog.models.seq;
        dialog.models.start();
        dialog.default_save_seq = 1;
        dialog.default_saving = true;
        dialog.saved_default = Some(choice("current"));
        dialog.receive(CreateReply::HarnessDefaultSaved(1, old_scope, Err(Failure::local("fixture-error"))), window, cx);
        assert!(!dialog.default_saving);
        assert_eq!(dialog.saved_default, Some(choice("current")));
        assert!(dialog.error.as_ref().is_some_and(|error| error.contains("fixture-error")));
    }));
}

#[::gpui::test]
fn changing_server_invalidates_the_old_save_reply(cx: &mut TestAppContext) {
    let (view, cx) = dialog(cx);
    cx.update(|window, cx| view.update(cx, |dialog, cx| {
        let old_scope = dialog.models.seq;
        dialog.default_save_seq = 1;
        dialog.default_saving = true;
        dialog.servers.set(Ok(vec![ServerChoice { key: "other".into(), label: "Outra máquina".into(),
            address: "http://127.0.0.1:2".into(), token: "fixture-owner".into(), offline: false }]));
        dialog.pick_machine("other".into(), window, cx);
        assert!(!dialog.default_saving);
        assert!(dialog.default_save_seq > 1);
        dialog.receive_models(dialog.models.seq, Ok(json!({"models":[]})), Default::default(), Ok(Some(choice("other"))), window, cx);
        dialog.receive(CreateReply::HarnessDefaultSaved(1, old_scope, Ok(Some(choice("stale")))), window, cx);
        assert!(!dialog.default_saving);
        assert_eq!(dialog.saved_default, Some(choice("other")));
    }));
}

#[::gpui::test]
fn home_mode_pill_applies_to_this_conversation_only(cx: &mut TestAppContext) {
    let (view, cx) = dialog(cx);
    cx.update(|window, cx| view.update(cx, |dialog, cx| {
        dialog.provider = "claude";
        dialog.jev.loading = true;
        dialog.choose_mode(false, window, cx);
        assert!(dialog.headless && !dialog.headless_touched, "a escolha antes da leitura do padrão não vale");

        dialog.jev.loading = false;
        dialog.headless_owner = Some(false);
        assert!(dialog.headless_inherited());
        assert_eq!(dialog.open_body("s", "/p", "", cx)["headless"], Value::Null, "sem escolha, quem decide é o servidor");

        dialog.headless_owner = Some(true);
        dialog.choose_mode(false, window, cx);
        assert!(!dialog.headless_saving, "a pílula nunca grava o padrão do servidor");
        assert_eq!(dialog.open_body("s", "/p", "", cx)["headless"], json!(false));
    }));
}

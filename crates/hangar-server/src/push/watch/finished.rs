//! Fim de trabalho dos provedores de terminal, com a série consumida antes da política de envio.
use futures_util::future::BoxFuture;
use hangar_api::session::SessionRow;
use serde_json::Value;
use super::terminal_notices::Sources;

pub(super) trait FinishSources: Sources {
    fn marker(&self, sid: &str) -> Option<(String, f64)>;
    fn same_series(&self, sid: &str, started: f64) -> bool;
    fn consume(&self, sid: &str, started: f64) -> bool;
    fn row<'a>(&'a self, sid: &'a str) -> BoxFuture<'a, Result<Option<SessionRow>, String>>;
    fn kimi_working(&self, row: &SessionRow) -> bool;
    fn settings(&self) -> Result<(Value, Value), String>;
    fn notify_finished(&self, name: &str);
    fn failure(&self, sid: &str, code: &str);
}

pub(super) fn permitted(enabled: &Value, threshold: &Value, elapsed: f64) -> Result<bool, &'static str> {
    if !crate::config_sync::apply::truthy(enabled) { return Ok(false); }
    let minimum = match threshold {
        Value::Bool(value) => f64::from(u8::from(*value)),
        Value::Number(value) => value.as_f64().unwrap_or_else(|| if value.to_string().starts_with('-') {f64::NEG_INFINITY} else {f64::INFINITY}),
        Value::String(_) if crate::orchestration::codec::contains_nonfinite(threshold) => {
            match crate::orchestration::codec::dumps(threshold, false).as_str() {
                "NaN" => f64::NAN,
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                _ => return Err("push_finish_threshold_invalid"),
            }
        }
        _ => return Err("push_finish_threshold_invalid"),
    };
    Ok(!(elapsed < minimum))
}

pub(super) async fn confirm_and_finish(source: &dyn FinishSources, sid: &str, started: f64, mut idle_ts: f64) {
    let initial = match source.row(sid).await {
        Ok(row) => row,
        Err(code) => { source.failure(sid, &code); return; }
    };
    if initial.is_some_and(|row| row.provider == "kimi") {
        let mut waited = false;
        // O Stop pode repetir idle sem transição; só o wire confirma o fim real do Kimi.
        loop {
            if !source.same_series(sid, started) { return; }
            let Some((state, timestamp)) = source.marker(sid) else { return; };
            if state != "idle" { return; }
            let row = match source.row(sid).await {
                Ok(row) => row,
                Err(code) => { source.failure(sid, &code); return; }
            };
            if row.as_ref().is_some_and(|row| source.kimi_working(row)) {
                source.sleep(std::time::Duration::from_secs(5)).await;
                waited = true;
                continue;
            }
            if waited { idle_ts = timestamp; }
            break;
        }
    }
    finished(source, sid, started, idle_ts).await;
}

pub(super) async fn finished(source: &dyn FinishSources, sid: &str, started: f64, idle_ts: f64) {
    source.sleep(super::FINISH_SETTLE).await;
    settled(source, sid, started, idle_ts).await;
}

pub(super) async fn settled(source: &dyn FinishSources, sid: &str, started: f64, idle_ts: f64) {
    let Some((state, timestamp)) = source.marker(sid) else { return; };
    if state != "idle" || timestamp != idle_ts || !source.same_series(sid, started) { return; }
    if let Err(code) = source.row(sid).await { source.failure(sid, &code); return; }
    if !source.consume(sid, started) { return; }
    let allowed = source.settings().and_then(|(enabled, minimum)| {
        let result = permitted(&enabled, &minimum, timestamp - started).map_err(str::to_owned);
        crate::orchestration::codec::discard(enabled);
        crate::orchestration::codec::discard(minimum);
        result
    });
    match allowed {
        Ok(true) => match source.row(sid).await {
            Ok(Some(row)) => source.notify_finished(&row.name),
            Ok(None) => {},
            Err(code) => source.failure(sid, &code),
        },
        Ok(false) => {},
        Err(code) => source.failure(sid, &code),
    }
}

impl FinishSources for super::Watch {
    fn marker(&self, sid: &str) -> Option<(String, f64)> { self.state_of(sid).map(|marker| (marker.state, marker.ts)) }
    fn same_series(&self, sid: &str, started: f64) -> bool { self.memory.lock().unwrap_or_else(|e| e.into_inner()).started.get(sid) == Some(&started) }
    fn consume(&self, sid: &str, started: f64) -> bool {
        {
            let mut memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
            if memory.started.get(sid) != Some(&started) { return false; }
            memory.started.remove(sid);
        }
        self.persist();
        true
    }
    fn row<'a>(&'a self, sid: &'a str) -> BoxFuture<'a, Result<Option<SessionRow>, String>> {
        Box::pin(async move {
            let produced = self.st.list.refresh().await.map_err(|_| "push_list_unavailable".to_owned())?;
            Ok(produced.rows.iter().find(|row| row.jsonl.as_deref().is_some_and(|path| crate::locate::session_key(path) == sid)).cloned())
        })
    }
    fn kimi_working(&self, row: &SessionRow) -> bool {
        let dirs = self.st.list.state_dirs_blocking().map(|(dirs, _)| dirs.to_vec()).unwrap_or_default();
        crate::provider_models::kimi_working(&dirs, row.jsonl.as_deref())
    }
    fn settings(&self) -> Result<(Value, Value), String> {
        Ok((self.config.get("notify_finished"), self.config.get("finish_min_seconds")))
    }
    fn notify_finished(&self, name: &str) { self.sender.notify(name, super::Notice::Finished(None)); }
    fn failure(&self, sid: &str, code: &str) {
        let (code, reason) = if code == "push_list_unavailable" {
            ("push_list_unavailable", "lista indisponível impediu o aviso de trabalho concluído")
        } else { ("push_finish_config_invalid", "configuração inválida impediu o aviso de trabalho concluído") };
        self.st.diag.report("rust.push_finish_failed", sid, code, reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use serde_json::json;
    struct Fake { provider: String, scenario: String, reads:Mutex<usize>, failed_read:usize, failures:Mutex<Vec<String>>, rechecks:usize, advance_marker:bool, series: Mutex<std::collections::BTreeMap<String,f64>>, sleeps: Mutex<Vec<f64>>, notices: Mutex<Vec<Value>> }
    impl Sources for Fake {
        fn question(&self, _: &SessionRow) -> Option<String> { None }
        fn pane<'a>(&'a self, _: &'a SessionRow) -> BoxFuture<'a,Result<crate::terminal_state::PaneAnalysis,&'static str>> { Box::pin(async { unreachable!() }) }
        fn sleep(&self, duration: std::time::Duration) -> BoxFuture<'_,()> { Box::pin(async move { self.sleeps.lock().unwrap().push(duration.as_secs_f64()); }) }
        fn awaiting(&self, _: &str, _: Option<String>) { unreachable!() }
    }
    impl FinishSources for Fake {
        fn marker(&self, _: &str) -> Option<(String,f64)> { Some((if self.scenario=="working" {"working"} else {"idle"}.into(), if self.scenario=="short" {1010.0} else if self.scenario=="new_idle" {1101.0} else {1100.0 + if self.advance_marker {self.sleeps.lock().unwrap().iter().filter(|duration|**duration==5.0).count() as f64} else {0.0}})) }
        fn same_series(&self, sid: &str, started:f64) -> bool {self.series.lock().unwrap().get(sid)==Some(&started)}
        fn consume(&self,sid:&str,started:f64)->bool {let mut series=self.series.lock().unwrap();if series.get(sid)!=Some(&started){return false;}series.remove(sid);true}
        fn row<'a>(&'a self,_:&'a str)->BoxFuture<'a,Result<Option<SessionRow>,String>> {Box::pin(async move {
            let read={let mut reads=self.reads.lock().unwrap();*reads+=1;*reads};
            if read==self.failed_read {return Err("push_list_unavailable".into());}
            if self.scenario=="new_series_during_list" {self.series.lock().unwrap().insert("sid".into(),999.0);}
            if self.scenario=="gone_before_send"&&read>1 || self.scenario=="appeared_before_send"&&read==1 {return Ok(None);}
            Ok(Some(serde_json::from_value(json!({"name":"session","provider":self.provider,"jsonl":"fixture.jsonl"})).unwrap()))
        })}
        fn kimi_working(&self,_:&SessionRow)->bool {self.sleeps.lock().unwrap().iter().filter(|duration|**duration==5.0).count()<self.rechecks}
        fn settings(&self)->Result<(Value,Value),String> {Ok((json!(self.scenario!="disabled"),json!(45)))}
        fn notify_finished(&self,name:&str){self.notices.lock().unwrap().push(json!([name]));}
        fn failure(&self,_:&str,code:&str){self.failures.lock().unwrap().push(code.into());}
    }
    #[tokio::test]
    async fn guest_push_watch_finished_discovers_terminal_after_settle() {
        for provider in ["pi", "omp", "kimi"] {
            let home = tempfile::tempdir().unwrap();
            let mut state = crate::routes::AppState::new(crate::config::Config {
                listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:1".parse().unwrap(),
                internal_secret: "fixture-secret".into(), auth_token: "fixture-owner".into(),
                log_path: None, trusted: crate::auth::TrustedHosts::parse("127.0.0.1"),
            });
            state.accounts.env = crate::accounts::environment::AccountEnvironment::from_map([
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
            ].into());
            let state = std::sync::Arc::new(state);
            let source = std::sync::Arc::new(Fake {provider:provider.into(),scenario:"appeared_before_send".into(),reads:Mutex::default(),failed_read:0,failures:Mutex::default(),rechecks:0,advance_marker:false,series:Mutex::new([("sid".into(),1000.0)].into()),sleeps:Mutex::default(),notices:Mutex::default()});
            let watch = std::sync::Arc::new(super::super::Watch {
                finish_source: Some(source.clone()), sender: super::super::Sender::new(state.clone(), Default::default()),
                config: crate::config_sync::runtime::RuntimeConfig::from_state(&state).unwrap(),
                frozen: super::super::settings::Frozen::load(&state.settings_snapshot().unwrap()).unwrap(),
                st: state, hooks: Mutex::default(),
                memory: Mutex::new(super::super::Memory {started:[("sid".into(),1000.0)].into(),..Default::default()}),
                memory_path: home.path().join("push.json"),
            });
            watch.finished("sid".into(),1000.0,1100.0).await;
            assert_eq!(*source.notices.lock().unwrap(), vec![serde_json::json!(["session"])], "GUE-082 {provider}");
            assert!(source.series.lock().unwrap().is_empty());
            assert_eq!(*source.sleeps.lock().unwrap(), vec![20.0]);
        }
    }

    #[tokio::test]
    async fn guest_push_finished_matches_original_producer() {
        let fixture:Value=serde_json::from_str(include_str!("../../../../../backend/tests/fixtures/contract/guests/push.json")).unwrap();
        for row in fixture["finished"].as_array().unwrap() {
            let scenario=row["scenario"].as_str().unwrap();
            let source=Fake {provider:row["provider"].as_str().unwrap().into(),scenario:scenario.into(),reads:Mutex::default(),failed_read:0,failures:Mutex::default(),rechecks:0,advance_marker:false,series:Mutex::new([("sid".into(),if scenario=="new_series" {999.0}else{1000.0})].into()),sleeps:Mutex::default(),notices:Mutex::default()};
            finished(&source,"sid",1000.0,if scenario=="short"{1010.0}else{1100.0}).await;
            assert_eq!(json!(*source.notices.lock().unwrap()),row["notices"],"GUE-082 {row}");
            assert_eq!(json!(*source.series.lock().unwrap()),row["series"],"GUE-082 {row}");
            assert_eq!(json!(*source.sleeps.lock().unwrap()),row["sleeps"],"GUE-082 {row}");
        }
    }
    #[tokio::test]
    async fn guest_push_kimi_finish_waits_without_another_transition() {
        let fixture:Value=serde_json::from_str(include_str!("../../../../../backend/tests/fixtures/contract/guests/push-kimi-finish.json")).unwrap();
        for row in fixture["rows"].as_array().unwrap() {
            let source=Fake {provider:"kimi".into(),scenario:"long".into(),reads:Mutex::default(),failed_read:0,failures:Mutex::default(),rechecks:row["rechecks"].as_u64().unwrap() as usize,advance_marker:row["advance_marker"].as_bool().unwrap(),series:Mutex::new([("sid".into(),1000.0)].into()),sleeps:Mutex::default(),notices:Mutex::default()};
            confirm_and_finish(&source,"sid",1000.0,1100.0).await;
            assert_eq!(json!(*source.notices.lock().unwrap()),row["notices"],"GUE-082 {row}");
            assert_eq!(json!(*source.series.lock().unwrap()),row["series"],"GUE-082 {row}");
            assert_eq!(json!(*source.sleeps.lock().unwrap()),row["sleeps"],"GUE-082 {row}");
        }
    }

    #[tokio::test]
    async fn guest_push_finished_rechecks_series_and_destination() {
        let fixture:Value=serde_json::from_str(include_str!("../../../../../backend/tests/fixtures/contract/guests/push-finish-races.json")).unwrap();
        for row in fixture["rows"].as_array().unwrap() {
            let source=Fake {provider:"pi".into(),scenario:row["scenario"].as_str().unwrap().into(),reads:Mutex::default(),failed_read:0,failures:Mutex::default(),rechecks:0,advance_marker:false,series:Mutex::new([("sid".into(),1000.0)].into()),sleeps:Mutex::default(),notices:Mutex::default()};
            finished(&source,"sid",1000.0,1100.0).await;
            assert_eq!(json!(*source.notices.lock().unwrap()),row["notices"],"GUE-082 {row}");
            assert_eq!(json!(*source.series.lock().unwrap()),row["series"],"GUE-082 {row}");
            assert_eq!(json!(*source.sleeps.lock().unwrap()),row["sleeps"],"GUE-082 {row}");
            assert_eq!(json!(*source.reads.lock().unwrap()),row["list_reads"],"GUE-082 {row}");
        }
    }

    #[tokio::test]
    async fn guest_push_finished_preserves_series_when_discovery_fails() {
        let fixture:Value=serde_json::from_str(include_str!("../../../../../backend/tests/fixtures/contract/guests/push-finish-errors.json")).unwrap();
        for row in fixture["rows"].as_array().unwrap() {
            let source=Fake {provider:"pi".into(),scenario:"stable".into(),reads:Mutex::default(),failed_read:row["failed_read"].as_u64().unwrap() as usize,failures:Mutex::default(),rechecks:0,advance_marker:false,series:Mutex::new([("sid".into(),1000.0)].into()),sleeps:Mutex::default(),notices:Mutex::default()};
            finished(&source,"sid",1000.0,1100.0).await;
            assert_eq!(json!(*source.notices.lock().unwrap()),row["notices"],"GUE-082 {row}");
            assert_eq!(json!(*source.series.lock().unwrap()),row["series"],"GUE-082 {row}");
            assert_eq!(json!(*source.reads.lock().unwrap()),row["list_reads"],"GUE-082 {row}");
            assert_eq!(*source.failures.lock().unwrap(),vec!["push_list_unavailable"]);
        }
    }

    proptest::proptest! {
        #[test]
        fn guest_push_disabled_never_sends_for_external_threshold(raw in ".{0,2048}", elapsed in proptest::prelude::any::<f64>()) {
            let threshold = crate::orchestration::codec::loads(&raw).unwrap_or(Value::String(raw));
            proptest::prop_assert_eq!(permitted(&Value::Bool(false), &threshold, elapsed), Ok(false));
            crate::orchestration::codec::discard(threshold);
        }
    }

    #[test]
    fn guest_push_finished_settings_match_python() {
        let fixture = crate::orchestration::codec::loads(include_str!("../../../../../backend/tests/fixtures/contract/guests/push-settings.json")).unwrap();
        for row in fixture["rows"].as_array().unwrap() {
            let enabled = if row["key"] == "notify_finished" { &row["value"] } else { &Value::Bool(true) };
            let threshold = if row["key"] == "finish_min_seconds" { &row["value"] } else { &serde_json::json!(45) };
            let result = permitted(enabled, threshold, 100.0);
            assert_eq!(result.is_err(), !row["error"].is_null(), "GUE-082 {row}");
            assert_eq!(result.unwrap_or(false), !row["notices"].as_array().unwrap().is_empty(), "GUE-082 {row}");
        }
    }
}

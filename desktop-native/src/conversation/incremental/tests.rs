use super::Incremental;
use crate::{api::dto::ChatEvent, appearance::ThinkingTools, conversation::{self, Item, View}};
use serde_json::{Value, json};
use std::collections::HashSet;

fn event(value: Value) -> ChatEvent { serde_json::from_value(value).unwrap() }

#[test]
fn codex_history_stays_in_place_without_running_pins_until_native_state_arrives() {
    let events = vec![
        event(json!({"id":"spawn","kind":"tool_use","tool_use_id":"launch","tool_name":"spawn_agent","tool_input":{"task_name":"review"}})),
        event(json!({"id":"created","kind":"tool_result","tool_use_id":"launch","result":"{\"task_name\":\"/root/review\"}"})),
    ];
    let mut fold = Incremental::default();
    fold.require_agent_states(true);
    for _ in 0..2 {
        fold.update(&events, 0, View::default());
        assert_eq!(fold.running_agents().count(), 0);
        assert_eq!(fold.activity_badge(), 0);
        assert!(fold.pinned.is_empty());
        assert!(fold.items.iter().any(|item| matches!(item, Item::Tool(tool) if tool.call == 0)));
    }
    fold.set_agent_states([("/root/review".into(), true, None)]);
    fold.update(&events, events.len(), View::default());
    assert_eq!(fold.running_agents().count(), 1);
}

#[test]
fn codex_known_uuid_does_not_take_the_state_of_another_child_at_the_same_path() {
    let events = vec![
        event(json!({"id":"spawn","kind":"tool_use","tool_use_id":"launch","tool_name":"spawn_agent","tool_input":{"task_name":"review"}})),
        event(json!({"id":"created","kind":"tool_result","tool_use_id":"launch","result":"{\"agent_id\":\"old-child\",\"task_name\":\"/root/review\"}"})),
    ];
    let mut fold = Incremental::default();
    fold.set_agent_states([("/root/review".into(), false, Some(1010.))]);
    fold.update(&events, 0, View::default());
    assert_eq!(fold.running_agents().count(), 1);
    fold.set_agent_states([("old-child".into(), false, Some(1010.))]);
    fold.update(&events, events.len(), View::default());
    assert_eq!(fold.running_agents().count(), 0);
}

#[test]
fn codex_native_identity_applies_to_new_and_rebuilt_history_without_timestamps() {
    for (identity, result) in [
        ("thread-uuid", "{\"agent_id\":\"thread-uuid\"}"),
        ("/root/review", "{\"task_name\":\"/root/review\"}"),
    ] {
        let events = vec![
            event(json!({"id":"spawn","kind":"tool_use","tool_use_id":"launch","tool_name":"spawn_agent",
                "tool_input":{"task_name":"review","agent_type":"worker"}})),
            event(json!({"id":"created","kind":"tool_result","tool_use_id":"launch","result":result})),
        ];
        let mut fold = Incremental::default();
        fold.set_agent_states([(identity.into(), false, Some(1010.0))]);
        for _ in 0..2 {
            fold.update(&events, 0, View::default());
            assert_eq!(fold.running_agents().count(), 0);
            assert_eq!(fold.activity_badge(), 0);
            assert!(!fold.pinned.contains(&0));
            assert!(fold.items.iter().any(|item| matches!(item, Item::Tool(tool) if tool.call == 0)));
        }
        fold.set_agent_states([(identity.into(), true, None)]);
        fold.update(&events, events.len(), View::default());
        assert_eq!(fold.running_agents().count(), 1);
        assert!(fold.pinned.contains(&0));
        fold.set_agent_states([(identity.into(), false, Some(1020.0))]);
        fold.update(&events, events.len(), View::default());
        assert_eq!(fold.running_agents().count(), 0);
        assert!(!fold.pinned.contains(&0));
    }
}

#[test]
fn codex_spawn_refused_without_error_flag_does_not_leave_a_running_pin() {
    let events = vec![
        event(json!({"id":"spawn","kind":"tool_use","tool_use_id":"launch","tool_name":"spawn_agent",
            "tool_input":{"task_name":"review","agent_type":"worker"},"ts":1000.0})),
        event(json!({"id":"refused","kind":"tool_result","tool_use_id":"launch",
            "result":"collab spawn failed: agent thread limit reached","is_error":false,"ts":1001.0})),
    ];
    let mut fold = Incremental::default();
    for n in 1..=events.len() { fold.update(&events[..n], n - 1, View::default()); }
    assert_eq!(fold.activity, conversation::fold_activity(&events));
    assert_eq!(fold.running_agents().count(), 0);
    assert_eq!(fold.activity_badge(), 0);
    assert!(!fold.pinned.contains(&0));
    assert_eq!(fold.activity.agents[0].finished_at, Some(1001.0));
}

#[test]
fn codex_native_state_moves_a_completed_pin_back_to_its_body_and_reopens_it() {
    let events = vec![
        event(json!({"id":"spawn","kind":"tool_use","tool_use_id":"launch","tool_name":"spawn_agent",
            "tool_input":{"task_name":"review","agent_type":"worker","message":"gAAAA-ciphertext"},"ts":1000.0})),
        event(json!({"id":"created","kind":"tool_result","tool_use_id":"launch","result":"{\"task_name\":\"/root/review\"}","ts":1001.0})),
        event(json!({"id":"parent","kind":"assistant_msg","text":"Continuo trabalhando.","ts":1002.0})),
    ];
    let mut fold = Incremental::default();
    for n in 1..=events.len() { fold.update(&events[..n], n - 1, View::default()); }
    assert_eq!(fold.activity, conversation::fold_activity(&events));
    assert_eq!(fold.running_agents().count(), 1);
    assert_eq!(fold.activity.agents[0].agent_path.as_deref(), Some("/root/review"));
    assert_eq!(fold.activity.agents[0].agent_id, None);
    assert_eq!(fold.activity.agents[0].prompt, None);
    assert!(fold.pinned.contains(&0));

    fold.set_agent_states([("launch".into(), false, Some(1010.0))]);
    let delta = fold.update(&events, events.len(), View::default());
    assert!(delta.activity_changed);
    assert_eq!(fold.running_agents().count(), 0);
    assert_eq!(fold.activity_badge(), 0);
    assert!(!fold.pinned.contains(&0));
    assert!(fold.items.iter().any(|item| matches!(item, Item::Tool(tool) if tool.call == 0)));
    assert_eq!(fold.activity.agents[0].elapsed(1100.0).map(|duration| duration.as_secs()), Some(10));

    fold.set_agent_states([("launch".into(), true, None)]);
    fold.update(&events, events.len(), View::default());
    assert_eq!(fold.running_agents().count(), 1);
    assert_eq!(fold.activity_badge(), 1);
    assert!(fold.pinned.contains(&0));
    assert!(!fold.items.iter().any(|item| matches!(item, Item::Tool(tool) if tool.call == 0)));
}

#[test]
fn review_notices_and_empty_replies_leave_no_rows_in_history_or_live_updates() {
    let mut events = vec![
        event(json!({"id":"notice","kind":"user_msg","text":"[painel: revisão do PR] Evento de acompanhamento"})),
        event(json!({"id":"empty","kind":"assistant_msg","text":" \n "})),
        event(json!({"id":"missing","kind":"assistant_msg"})),
        event(json!({"id":"user","kind":"user_msg","text":"Como está a revisão do PR?"})),
        event(json!({"id":"panel","kind":"user_msg","text":"[painel: grupo de trabalho] Grupo alterado"})),
        event(json!({"id":"peer","kind":"user_msg","text":"[de: revisão do PR] Recado"})),
        event(json!({"id":"answer","kind":"assistant_msg","text":"Correção enviada."})),
    ];
    let expected = vec![Item::Event(3), Item::Event(4), Item::Event(5), Item::Event(6)];
    let mut fold = Incremental::default();
    for n in 1..=events.len() {
        fold.update(&events[..n], n - 1, View::default());
        assert_eq!(fold.items, conversation::build(&events[..n], View::default(), &HashSet::new()));
    }
    assert_eq!(fold.items, expected);
    events[1].text = Some("Resposta recebida depois.".into());
    fold.update(&events, 1, View::default());
    assert_eq!(fold.items, vec![Item::Event(1), Item::Event(3), Item::Event(4), Item::Event(5), Item::Event(6)]);
}

#[test]
fn agent_duration_tracks_execution_and_freezes_at_the_real_finish() {
    let start = event(json!({"id":"agent","kind":"tool_use","tool_use_id":"a","tool_name":"Agent","ts":1000.0}));
    let launch = event(json!({"id":"launch","kind":"tool_result","tool_use_id":"a","bg_agent_id":"worker","ts":1002.0}));
    let finish = event(json!({"id":"finish","kind":"tool_result","tool_use_id":"task:worker","ts":1192.0}));
    let foreground = event(json!({"id":"foreground","kind":"tool_result","tool_use_id":"a","ts":1075.0}));
    let untimed = event(json!({"id":"untimed","kind":"tool_result","tool_use_id":"task:worker"}));
    let untimed_foreground = event(json!({"id":"untimed-fg","kind":"tool_result","tool_use_id":"a"}));
    let duplicate = event(json!({"id":"duplicate","kind":"tool_result","tool_use_id":"task:worker","ts":1250.0}));
    let notification = event(json!({"id":"note","kind":"user_msg","text":"<task-notification><task-id>worker</task-id></task-notification>","ts":1192.0}));
    for (events, running, seconds) in [
        (vec![start.clone()], true, 300),
        (vec![start.clone(), launch.clone()], true, 300),
        (vec![start.clone(), foreground.clone()], false, 75),
        (vec![start.clone(), untimed_foreground, foreground], false, 75),
        (vec![start.clone(), launch.clone(), untimed.clone(), finish.clone(), duplicate], false, 192),
        (vec![start.clone(), untimed, finish.clone(), launch.clone()], false, 192),
        (vec![start.clone(), launch.clone(), finish.clone()], false, 192),
        (vec![start.clone(), finish, launch.clone()], false, 192),
        (vec![start, launch, notification], false, 192),
    ] {
        let mut fold = Incremental::default();
        for n in 1..=events.len() { fold.update(&events[..n], n - 1, View::default()); }
        for activity in [&fold.activity, &conversation::fold_activity(&events)] {
            let run = &activity.agents[0];
            assert_eq!(run.running, running);
            assert_eq!(run.elapsed(1300.0).map(|d| d.as_secs()), Some(seconds));
            if !running { assert_eq!(run.elapsed(1600.0).map(|d| d.as_secs()), Some(seconds)); }
        }
    }
}

#[test]
fn ordinary_results_do_not_rebuild_historical_activity() {
    let mut fold = super::ActivityFold::default();
    for i in 0..200 {
        assert!(fold.push(i * 2, &event(json!({"id":format!("call-{i}"),"kind":"tool_use","tool_use_id":format!("agent-{i}"),"tool_name":"Agent","tool_input":{"prompt":"Histórico extenso".repeat(100)}}))));
        assert!(fold.push(i * 2 + 1, &event(json!({"id":format!("done-{i}"),"kind":"tool_result","tool_use_id":format!("agent-{i}"),"result":"Concluído"}))));
    }
    let before = fold.snapshot();
    assert!(!fold.push(400, &event(json!({"id":"ordinary","kind":"tool_result","tool_use_id":"read","result":"Arquivo"}))));
    assert!(!fold.push(401, &event(json!({"id":"unknown","kind":"tool_result","tool_use_id":"task:future","result":"Concluído"}))));
    assert_eq!(fold.snapshot(), before);
    // O vínculo sem mudança visual ainda precisa ser lembrado para uma chamada posterior.
    fold.push(402, &event(json!({"id":"later","kind":"tool_use","tool_use_id":"read","tool_name":"Agent"})));
    assert!(!fold.snapshot().agents.last().unwrap().running);
}

#[test]
fn structured_launch_id_keeps_the_agent_running_without_the_launch_text() {
    let events = [
        event(json!({"id":"agent","kind":"tool_use","tool_use_id":"a","tool_name":"Agent","tool_input":{"prompt":"Ler"}})),
        event(json!({"id":"launch","kind":"tool_result","tool_use_id":"a","result":"Agente iniciado","bg_agent_id":"worker"})),
        event(json!({"id":"end","kind":"tool_result","tool_use_id":"task:worker","result":"Concluído"})),
    ];
    let mut fold = super::ActivityFold::default();
    for (i, ev) in events.iter().enumerate().take(2) { fold.push(i, ev); }
    assert!(fold.snapshot().agents[0].running);
    assert_eq!(fold.snapshot(), conversation::fold_activity(&events[..2]));
    fold.push(2, &events[2]);
    assert!(!fold.snapshot().agents[0].running);
    assert_eq!(fold.snapshot(), conversation::fold_activity(&events));
}

#[test]
fn background_agent_stopped_by_the_user_ends_by_its_description() {
    let events = [
        event(json!({"id":"agent","kind":"tool_use","tool_use_id":"a","tool_name":"Agent","tool_input":{"prompt":"Ler","description":"Validar a tela","run_in_background":true}})),
        event(json!({"id":"launch","kind":"tool_result","tool_use_id":"a","result":"Async agent launched successfully.\nagentId: ab12 (use it)"})),
        event(json!({"id":"stop","kind":"tool_result","tool_use_id":"task:stopped:Validar a tela","result":"task-notification","ts":9.0})),
    ];
    let mut fold = super::ActivityFold::default();
    for (i, ev) in events.iter().enumerate() { fold.push(i, ev); }
    assert!(!fold.snapshot().agents[0].running);
    assert_eq!(fold.snapshot(), conversation::fold_activity(&events));
}

#[test]
fn waiting_notice_keeps_the_agent_running_as_waiting_until_its_end() {
    let events = [
        event(json!({"id":"agent","kind":"tool_use","tool_use_id":"a","tool_name":"Agent","tool_input":{"prompt":"Ler","run_in_background":true}})),
        event(json!({"id":"launch","kind":"tool_result","tool_use_id":"a","result":"Async agent launched successfully.\nagentId: ab12 (use it)"})),
        event(json!({"id":"wait","kind":"tool_result","tool_use_id":"task:waiting:ab12","result":"task-notification","ts":5.0})),
        event(json!({"id":"end","kind":"tool_result","tool_use_id":"task:ab12","result":"task-notification","ts":9.0})),
    ];
    let mut fold = super::ActivityFold::default();
    for (i, ev) in events.iter().enumerate().take(3) { fold.push(i, ev); }
    let run = &fold.snapshot().agents[0];
    assert_eq!((run.running, run.waiting), (true, true));
    assert_eq!(fold.snapshot(), conversation::fold_activity(&events[..3]));
    fold.push(3, &events[3]);
    let run = &fold.snapshot().agents[0];
    assert_eq!((run.running, run.waiting), (false, false));
    assert_eq!(fold.snapshot(), conversation::fold_activity(&events));
}

#[test]
fn teammate_runs_until_idle_and_keeps_its_real_agent_id() {
    let events = [
        event(json!({"id":"agent","kind":"tool_use","tool_use_id":"a","tool_name":"Agent","tool_input":{"prompt":"Ler","name":"x"}})),
        event(json!({"id":"spawn","kind":"tool_result","tool_use_id":"a","result":"Spawned successfully.\nagent_id: ax-b72\nname: x","bg_agent_id":"teammate:x"})),
        event(json!({"id":"idle","kind":"tool_result","tool_use_id":"task:teammate:x","result":"task-notification"})),
    ];
    let mut fold = super::ActivityFold::default();
    for (i, ev) in events.iter().enumerate().take(2) { fold.push(i, ev); }
    let run = &fold.snapshot().agents[0];
    assert_eq!((run.running, run.agent_id.as_deref()), (true, Some("ax-b72")));
    assert_eq!(fold.snapshot(), conversation::fold_activity(&events[..2]));
    fold.push(2, &events[2]);
    assert!(!fold.snapshot().agents[0].running);
    assert_eq!(fold.snapshot(), conversation::fold_activity(&events));
}

#[test]
fn teammate_woken_by_send_message_runs_again_and_ignores_the_late_idle() {
    let events = [
        event(json!({"id":"agent","kind":"tool_use","tool_use_id":"a","tool_name":"Agent","tool_input":{"prompt":"Ler","name":"ana"}})),
        event(json!({"id":"spawn","kind":"tool_result","tool_use_id":"a","result":"Spawned successfully.","bg_agent_id":"teammate:ana"})),
        event(json!({"id":"send","kind":"tool_use","tool_use_id":"s","tool_name":"SendMessage","tool_input":{"to":"ana"},"ts":10.0})),
        event(json!({"id":"idle1","kind":"tool_result","tool_use_id":"task:teammate:ana","result":"task-notification","ts":5.0})),
        event(json!({"id":"idle2","kind":"tool_result","tool_use_id":"task:teammate:ana","result":"task-notification","ts":20.0})),
    ];
    let mut fold = super::ActivityFold::default();
    for (i, ev) in events.iter().enumerate().take(4) { fold.push(i, ev); }
    assert!(fold.snapshot().agents[0].running);
    assert_eq!(fold.snapshot(), conversation::fold_activity(&events[..4]));
    fold.push(4, &events[4]);
    assert!(!fold.snapshot().agents[0].running);
    assert_eq!(fold.snapshot(), conversation::fold_activity(&events));
}

fn views() -> Vec<View> {
    [ThinkingTools::None, ThinkingTools::Search, ThinkingTools::All].into_iter().flat_map(|thinking| {
        [false, true].into_iter().flat_map(move |tasks| {
            [(false, false), (true, false), (false, true)].into_iter().map(move |(merge_thinking, every_run_groups)| {
                View { thinking, tasks, merge_thinking, every_run_groups }
            })
        })
    }).collect()
}

fn check(state: &Incremental, events: &[ChatEvent], view: View) {
    let activity = conversation::fold_activity(events);
    let pinned: HashSet<_> = activity.running_agents().map(|agent| agent.call).collect();
    assert_eq!(state.activity, activity, "atividade com {} eventos", events.len());
    assert_eq!(state.pinned, pinned);
    assert_eq!(state.running_agents().map(|agent| agent.call).collect::<Vec<_>>(), activity.running_agents().map(|agent| agent.call).collect::<Vec<_>>());
    assert_eq!(state.activity_badge(), activity.badge());
    assert_eq!(state.items, conversation::build(events, view, &pinned), "linhas com {} eventos e {view:?}", events.len());
    let (paired, orphans) = conversation::pair_results(events);
    assert_eq!(state.paired, paired);
    assert_eq!(state.orphans, orphans);
}

fn row_content(items: &[Item], events: &[ChatEvent]) -> Vec<Vec<ChatEvent>> {
    let (paired, _) = conversation::pair_results(events);
    items.iter().map(|item| {
        let calls: Vec<_> = match item {
            Item::Event(i) | Item::Orphan(i) => vec![*i],
            Item::Tool(tool) => vec![tool.call],
            Item::Group { tools, .. } => tools.iter().map(|tool| tool.call).collect(),
            Item::Thinking { parts, .. } => parts.clone(),
            Item::Tasks { .. } => Vec::new(),
        };
        calls.into_iter().flat_map(|i| std::iter::once(events[i].clone())
            .chain(paired.get(&i).map(|&result| events[result].clone()))).collect()
    }).collect()
}

fn exercise(sequence: Vec<ChatEvent>) {
    for view in views() {
        let mut state = Incremental::default();
        let mut events = Vec::new();
        for event in &sequence {
            let old = state.items.clone();
            let old_content = row_content(&old, &events);
            let stable = events.len();
            events.push(event.clone());
            let delta = state.update(&events, stable, view);
            check(&state, &events, view);
            assert_eq!(delta.previous_len, old.len());
            assert_eq!(&old[..delta.from], &state.items[..delta.from], "prefixo declarado intacto mudou");
            assert_eq!(&old_content[..delta.from], &row_content(&state.items, &events)[..delta.from], "conteúdo fora da invalidação mudou");
        }
    }
}

#[test]
fn append_matches_full_oracle_for_every_view_and_group_boundary() {
    exercise(vec![
        event(json!({"id":"u","kind":"user_msg","text":"Começar"})),
        event(json!({"id":"t","kind":"thinking","text":"Analisar"})),
        event(json!({"id":"s","kind":"tool_use","tool_use_id":" search ","tool_name":"WebSearch"})),
        event(json!({"id":"sr","kind":"tool_result","tool_use_id":"search","result":"Encontrado"})),
        event(json!({"id":"a","kind":"assistant_msg","text":""})),
        event(json!({"id":"r1","kind":"tool_use","tool_use_id":"read","tool_name":"Read"})),
        event(json!({"id":"r2","kind":"tool_use","tool_use_id":"read","tool_name":"Read"})),
        event(json!({"id":"r3","kind":"tool_use","tool_use_id":"third","tool_name":"Bash"})),
        event(json!({"id":"out1","kind":"tool_result","tool_use_id":"read","result":"Primeiro"})),
        event(json!({"id":"out2","kind":"tool_result","tool_use_id":"read","result":"Segundo"})),
        event(json!({"id":"done","kind":"assistant_msg","text":"Fim"})),
        event(json!({"id":"late","kind":"tool_result","tool_use_id":"third","result":"Tardio"})),
    ]);
}

#[test]
fn published_page_stays_out_of_groups_like_the_full_build() {
    exercise(vec![
        event(json!({"id":"t","kind":"thinking","text":"Montar"})),
        event(json!({"id":"r1","kind":"tool_use","tool_use_id":"r1","tool_name":"Read"})),
        event(json!({"id":"p","kind":"tool_use","tool_use_id":"p","tool_name":"mcp__hangar__html_render"})),
        event(json!({"id":"pr","kind":"tool_result","tool_use_id":"p","result":"{\"hangar_page\":{\"id\":\"a\",\"title\":\"T\"}}"})),
        event(json!({"id":"r2","kind":"tool_use","tool_use_id":"r2","tool_name":"Read"})),
    ]);
}

#[test]
fn orphan_never_binds_forward_and_duplicate_row_ids_stay_deduplicated() {
    exercise(vec![
        event(json!({"id":"orphan","kind":"tool_result","tool_use_id":"future","result":"Antes"})),
        event(json!({"id":"first","kind":"tool_use","tool_use_id":"future","tool_name":"Read"})),
        event(json!({"id":"first","kind":"tool_use","tool_use_id":"future","tool_name":"Read"})),
        event(json!({"id":"g-first","kind":"assistant_msg","text":"Colisão"})),
        event(json!({"id":"last","kind":"tool_result","tool_use_id":"future","result":"Depois"})),
        event(json!({"id":"none","kind":"tool_use","tool_use_id":"  ","tool_name":"Read"})),
        event(json!({"id":"none-r","kind":"tool_result","tool_use_id":" ","result":"Sem vínculo"})),
    ]);
}

#[test]
fn task_result_retroactively_resolves_prior_update_and_moves_task_row() {
    exercise(vec![
        event(json!({"id":"create","kind":"tool_use","tool_use_id":"c","tool_name":"TaskCreate","tool_input":{"subject":"Verificar"}})),
        event(json!({"id":"update","kind":"tool_use","tool_use_id":"u","tool_name":"TaskUpdate","tool_input":{"taskId":"2","status":"completed"}})),
        event(json!({"id":"message","kind":"assistant_msg","text":"Entre tarefas"})),
        event(json!({"id":"created","kind":"tool_result","tool_use_id":"c","result":"Task #2 created successfully"})),
        event(json!({"id":"create2","kind":"tool_use","tool_use_id":"c2","tool_name":"TaskCreate","tool_input":{"subject":"Continuar"}})),
        event(json!({"id":"delete","kind":"tool_use","tool_use_id":"d","tool_name":"TaskUpdate","tool_input":{"taskId":"2","status":"deleted"}})),
    ]);
}

#[test]
fn background_activity_preserves_early_finish_reused_ids_and_whole_tasks() {
    exercise(vec![
        event(json!({"id":"early","kind":"tool_result","tool_use_id":"task:worker","result":"Concluído"})),
        event(json!({"id":"agent","kind":"tool_use","tool_use_id":"a","tool_name":"Agent","tool_input":{"description":"Analisar","prompt":"Ler"}})),
        event(json!({"id":"middle","kind":"assistant_msg","text":"Continua"})),
        event(json!({"id":"launch","kind":"tool_result","tool_use_id":"a","result":"Async agent launched. agentId: worker"})),
        event(json!({"id":"agent2","kind":"tool_use","tool_use_id":"a","tool_name":"AgentSwarm","tool_input":{"items":[1,2]}})),
        event(json!({"id":"bash","kind":"tool_use","tool_use_id":"b","tool_name":"Bash","tool_input":{"command":"cargo test","run_in_background":true}})),
        event(json!({"id":"launched","kind":"tool_result","tool_use_id":"b","result":"Command running in background with ID: job"})),
        event(json!({"id":"notify","kind":"user_msg","text":"<task-notification><task-id>job</task-id></task-notification>"})),
        event(json!({"id":"todo","kind":"tool_use","tool_name":"TodoWrite","tool_input":{"todos":[{"content":"Uma","status":"in_progress"}]}})),
        event(json!({"id":"task","kind":"tool_use","tool_name":"TaskCreate","tool_input":{"subject":"Outra"}})),
        event(json!({"id":"stop","kind":"tool_use","tool_name":"TaskStop","tool_input":{"task_id":1}})),
        event(json!({"id":"plan","kind":"tool_use","tool_name":"update_plan","tool_input":{"plan":[{"step":"Nova","status":"completed"}]}})),
    ]);
}

#[test]
fn chat_reconciliation_merge_and_reset_keep_indices_and_derived_content() {
    for view in views() {
        let mut chat = crate::chat::Chat::default();
        let mut state = Incremental::default();
        for value in [
            json!({"id":"queued-1","kind":"user_msg","text":"Executar"}),
            json!({"id":"call","kind":"tool_use","tool_use_id":"r","tool_name":"Read"}),
            json!({"id":"real","kind":"user_msg","text":"Executar"}),
            json!({"id":"result","kind":"tool_result","tool_use_id":"r","result":"antes"}),
            json!({"id":"result","kind":"tool_result","tool_use_id":"r","result":"outro"}),
            json!({"id":"queued-1","kind":"user_msg","queued_confirmed":true}),
        ] {
            chat.apply(event(value));
            let stable = chat.take_unsynced();
            state.update(&chat.events, stable, view);
            check(&state, &chat.events, view);
        }
        chat.merge_history(vec![
            event(json!({"id":"older","kind":"thinking","text":"Anterior"})),
            event(json!({"id":"call","kind":"tool_use","tool_use_id":"r","tool_name":"Read"})),
        ]);
        let stable = chat.take_unsynced();
        assert!(state.update(&chat.events, stable, view).full);
        check(&state, &chat.events, view);
        chat = crate::chat::Chat::default();
        chat.apply(event(json!({"id":"different","kind":"user_msg","text":"Outra sessão"})));
        let stable = chat.take_unsynced();
        assert!(state.update(&chat.events, stable, view).full);
        check(&state, &chat.events, view);
    }
}

#[test]
fn late_result_invalidates_thinking_content_even_when_item_is_equal() {
    let view = View { thinking: ThinkingTools::All, ..View::default() };
    let mut events = vec![
        event(json!({"id":"before","kind":"user_msg","text":"Começar"})),
        event(json!({"id":"think","kind":"thinking","text":"Raciocínio"})),
        event(json!({"id":"call","kind":"tool_use","tool_use_id":"r","tool_name":"Read"})),
        event(json!({"id":"end","kind":"assistant_msg","text":"Fim"})),
    ];
    let mut state = Incremental::default();
    state.update(&events, 0, view);
    let old = state.items.clone();
    events.push(event(json!({"id":"late","kind":"tool_result","tool_use_id":"r","result":"Resultado tardio"})));
    let delta = state.update(&events, 4, view);
    check(&state, &events, view);
    assert_eq!(old, state.items);
    assert_eq!(delta.from, 1);
    assert_eq!(state.paired.get(&2), Some(&4));
}

#[test]
fn reset_prepend_same_length_replacement_and_view_change_rebuild_safely() {
    let mut state = Incremental::default();
    let view = View::default();
    let mut events = vec![event(json!({"id":"one","kind":"assistant_msg","text":"antes"}))];
    state.update(&events, 0, view);
    events[0].text = Some("outro".into());
    let delta = state.update(&events, 0, view);
    assert!(delta.full);
    assert_eq!(delta.from, 0);
    check(&state, &events, view);
    events.insert(0, event(json!({"id":"older","kind":"user_msg","text":"Histórico"})));
    assert!(state.update(&events, 0, view).full);
    check(&state, &events, view);
    events.remove(0);
    state.update(&events, 0, view);
    check(&state, &events, view);
    let changed = View { every_run_groups: true, ..view };
    assert!(state.update(&events, events.len(), changed).full);
    check(&state, &events, changed);
    events.clear();
    state.update(&events, 0, changed);
    check(&state, &events, changed);
}

#[test]
fn activity_and_tasks_only_rebuild_their_dependent_suffix() {
    for view in views() {
        let mut events: Vec<_> = (0..20).map(|i| event(json!({"id":format!("prefix-{i}"),"kind":"assistant_msg","text":"Preservar"}))).collect();
        let mut state = Incremental::default();
        state.update(&events, 0, view);
        for value in [
            json!({"id":"agent","kind":"tool_use","tool_use_id":"a","tool_name":"Agent"}),
            json!({"id":"middle","kind":"assistant_msg","text":"Enquanto trabalha"}),
            json!({"id":"finished","kind":"tool_result","tool_use_id":"a","result":"Fim"}),
            json!({"id":"create","kind":"tool_use","tool_use_id":"c","tool_name":"TaskCreate","tool_input":{"subject":"Uma tarefa"}}),
            json!({"id":"created","kind":"tool_result","tool_use_id":"c","result":"Task #9 created successfully"}),
            json!({"id":"after","kind":"assistant_msg","text":"Depois da tarefa"}),
            json!({"id":"update","kind":"tool_use","tool_name":"TaskUpdate","tool_input":{"taskId":"9","status":"completed"}}),
        ] {
            let stable = events.len();
            events.push(event(value));
            let delta = state.update(&events, stable, view);
            check(&state, &events, view);
            assert!(delta.from >= 20, "prefixo independente foi invalidado: {delta:?}");
        }
    }
}

#[test]
fn deterministic_bursts_match_oracle_after_every_batch() {
    let mut sequence = Vec::new();
    let mut seed = 22006368_u64;
    for i in 0..320 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let key = format!("tool-{}", (seed >> 32) % 13);
        let mut value = match (seed >> 16) % 12 {
            0 => json!({"kind":"thinking","text":"Analisar os dados"}),
            1 => json!({"kind":"tool_use","tool_use_id":key,"tool_name":"Read"}),
            2 => json!({"kind":"tool_use","tool_use_id":key,"tool_name":"WebSearch"}),
            3 => json!({"kind":"tool_result","tool_use_id":key,"result":"Resultado"}),
            4 => json!({"kind":"tool_use","tool_use_id":key,"tool_name":"Agent"}),
            5 => json!({"kind":"tool_result","tool_use_id":key,"result":"Async agent launched. agentId: worker"}),
            6 => json!({"kind":"tool_result","tool_use_id":"task:worker","result":"Fim"}),
            7 => json!({"kind":"tool_use","tool_use_id":key,"tool_name":"TaskCreate","tool_input":{"subject":"Conferir"}}),
            8 => json!({"kind":"tool_use","tool_use_id":key,"tool_name":"TaskUpdate","tool_input":{"taskId":"2","status":"completed"}}),
            9 => json!({"kind":"tool_result","tool_use_id":key,"result":"Task #2 created successfully"}),
            10 => json!({"kind":"assistant_msg","text":""}),
            _ => json!({"kind":"user_msg","text":"Continuar"}),
        };
        value["id"] = json!(format!("event-{i}"));
        sequence.push(event(value));
    }
    for batch in [1, 3, 17] {
        for view in views() {
            let mut state = Incremental::default();
            let mut events = Vec::new();
            for chunk in sequence.chunks(batch) {
                let old = state.items.clone();
                let stable = events.len();
                events.extend_from_slice(chunk);
                let delta = state.update(&events, stable, view);
                check(&state, &events, view);
                assert_eq!(&old[..delta.from], &state.items[..delta.from]);
            }
        }
    }
}

#[test]
fn common_append_and_identical_replay_keep_prefix_rows() {
    let view = View::default();
    let mut events: Vec<_> = (0..1000).map(|i| event(json!({"id":format!("e-{i}"),"kind":"assistant_msg","text":"Texto"}))).collect();
    let mut state = Incremental::default();
    state.update(&events, 0, view);
    let delta = state.update(&events, events.len(), view);
    assert!(!delta.full);
    assert_eq!(delta.from, 1000);
    events.push(event(json!({"id":"new","kind":"assistant_msg","text":"Novo"})));
    let delta = state.update(&events, 1000, view);
    assert!(!delta.full);
    assert_eq!(delta.from, 1000);
    assert_eq!(state.items[0], Item::Event(0));
    check(&state, &events, view);
}

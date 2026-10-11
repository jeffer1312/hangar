//! Ferramentas de sessão e o Jev: o nome falado vira sessão pela lista fresca e a ação roda numa tarefa.
use super::{Controller, Ctx, Done, Forwarded, Key, Want, watch};
use crate::voice::call::CallId;
use crate::voice::machines::{HERE, Machines};
use crate::voice::organizer::{OpenRequest, SEND_DUPLICATE, SWITCH_REFUSED, confirmed_switch_target, mentions_session, session_context,
    switch_requested, tool_reply};
use crate::voice::rules::{JEV_SWITCH_ECHO, Listed, SessionMatch, TALKED_WINDOW, can_leave, can_pair, conversation_pairs, jev_action_choice,
    jev_switch_choice, looks_like_path, machine_aliases, match_qualified, match_session, narrow_by_context, offer_summary, pick_folder,
    question_text, send_reply, sessions_text, unique_name, Delivery};
use crate::voice::{SendVerdict, computer, jev, log, observe};
use hangar_api::session::SessionRow;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};
use std::{collections::HashSet, time::Instant};

/// O que o Jev recebeu, para agir sobre a mesma lista quando a resposta chegar.
pub(super) struct JevAsked {
    turn: String, speech: String, recent: String, sessions: Vec<Key>, actions: Vec<String>, action_labels: Vec<String>, started: Instant,
}

type Rows = [(String, SessionRow)];

fn folder_name(path: &str) -> &str { path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or_default() }

impl Controller {
    /// A lista fresca de todas as máquinas, lida fora do laço; a ferramenta continua em `on_rows`.
    pub(super) fn lookup(&self, want: Want) {
        let ctx = self.ctx();
        tokio::spawn(async move {
            let (rows, unreachable) = watch::read_rows(&ctx.machines, &ctx.backoff).await;
            let _ = ctx.done.send(Done::Rows { rows, unreachable, want });
        });
    }

    pub(super) fn on_rows(&mut self, rows: Vec<(String, SessionRow)>, unreachable: Vec<String>, want: Want) {
        match want {
            Want::List(call) => self.list(call, &rows, &unreachable),
            Want::Switch { call, name, said, recent } => self.switch(call, &rows, &unreachable, &name, &said, &recent),
            Want::Close { call, name, confirmed, turn } => self.close(call, &rows, &unreachable, &name, confirmed, &turn),
            Want::Pair(call, a, b) => self.pair(call, &rows, &unreachable, &a, &b),
            Want::Unpair(call, name) => self.unpair(call, &rows, &unreachable, &name),
            Want::Follow(call, name, on) => self.follow(call, &rows, &unreachable, &name, on),
            Want::Send { call, request, name, turn } => match self.resolve("send_to_session", &rows, &unreachable, &name) {
                Ok(i) => self.send_to(call, (rows[i].0.clone(), rows[i].1.name.clone()), request, turn),
                Err(text) => self.voice.reply(call, tool_reply(text, false)),
            },
            Want::Plan { name, text } => match self.resolve("finish_plan", &rows, &unreachable, &name) {
                Ok(i) => self.send_plan_to((rows[i].0.clone(), rows[i].1.name.clone()), text),
                Err(why) => self.voice.session_answer(format!("O plano não foi enviado nem perdido; continua guardado. {why} \
                    Para reenviar, diga para qual sessão mandar e chame finish_plan de novo com session.")),
            },
        }
        self.rows = rows;
    }

    /// Nome falado (com ou sem a máquina) → índice em `rows`. Com `narrow`, várias se desempatam pelo contexto e pela raiz da
    /// família; `Some(true)` tira a sessão da tela, porque trocar é ir para outra. Enviar, agrupar e fechar não desempatam.
    fn resolve_match(&self, rows: &Rows, spoken: &str, narrow: Option<bool>) -> SessionMatch {
        let names: Vec<&str> = rows.iter().map(|(_, s)| s.name.as_str()).collect();
        let machines: Vec<Vec<String>> = rows.iter().map(|(m, _)| machine_aliases(m, &self.machines.label(m))).collect();
        let on_active: Vec<bool> = rows.iter().map(|(m, _)| m == HERE).collect();
        match match_qualified(spoken, &names, &machines, &on_active) {
            SessionMatch::Many(found) if let Some(for_switch) = narrow => {
                let now = Instant::now();
                let open = self.screen_key();
                let all: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
                let context: Vec<bool> = found.iter().map(|&i| {
                    let entry = (rows[i].0.clone(), rows[i].1.name.clone());
                    if open.as_ref() == Some(&entry) { return !for_switch; }
                    self.followed.contains(&entry)
                        || self.talked.get(&entry).is_some_and(|at| now.saturating_duration_since(*at) < TALKED_WINDOW)
                        || mentions_session(&self.heard_recent, &entry.1, &all)
                }).collect();
                let subset: Vec<&str> = found.iter().map(|&i| names[i]).collect();
                narrow_by_context(found, &subset, &context)
            }
            other => other,
        }
    }

    fn ambiguous(&self, rows: &Rows, found: &[usize]) -> String {
        let list: Vec<String> = found.iter().take(5).map(|&i| format!("{} em {}", rows[i].1.name, self.machines.label(&rows[i].0))).collect();
        format!("Mais de uma sessão combina: {}. Pergunte ao usuário qual e chame de novo com o nome e a máquina (ex.: '{}').",
            list.join(", "), list.first().cloned().unwrap_or_default())
    }

    /// Máquina sem resposta não é "sessão inexistente": o organizador precisa saber que faltam as de lá.
    fn not_found(unreachable: &[String]) -> String {
        if unreachable.is_empty() { return "Não achei sessão com esse nome (e máquina, se foi dita).".into(); }
        format!("Não achei sessão com esse nome; a(s) máquina(s) {} não responderam e as sessões delas não foram vistas. Nada foi feito.",
            unreachable.join(", "))
    }

    /// Ambíguo ou ausente volta como texto para o organizador; nunca um palpite.
    fn resolve(&self, tool: &str, rows: &Rows, unreachable: &[String], spoken: &str) -> Result<usize, String> {
        let narrow = matches!(tool, "follow_session" | "unfollow_session").then_some(false);
        match self.resolve_match(rows, spoken, narrow) {
            SessionMatch::One(i) => { log(format!("{tool} one")); Ok(i) }
            SessionMatch::Many(found) => { log(format!("{tool} many({})", found.len())); Err(self.ambiguous(rows, &found)) }
            SessionMatch::None => { log(format!("{tool} none unreachable={}", unreachable.len())); Err(Self::not_found(unreachable)) }
        }
    }

    fn list(&mut self, call: CallId, rows: &Rows, unreachable: &[String]) {
        let open = self.screen_key();
        let multi = rows.iter().any(|(m, _)| m != HERE) || !unreachable.is_empty();
        let listed: Vec<Listed> = rows.iter().map(|(m, s)| {
            let entry = (m.clone(), s.name.clone());
            Listed { on_screen: open.as_ref() == Some(&entry), followed: self.followed.contains(&entry),
                machine: multi.then(|| self.machines.label(m)),
                provider: if s.provider.is_empty() { "claude".into() } else { s.provider.clone() },
                folder: s.cwd.as_deref().map(folder_name).unwrap_or_default().to_owned(),
                name: s.name.clone(), state: s.state.clone() }
        }).collect();
        log(format!("list_sessions count={} unreachable={}", listed.len(), unreachable.len()));
        self.voice.reply(call, tool_reply(sessions_text(&listed, unreachable), true));
    }

    /// O aparelho abre a sessão: `server` vazio é este servidor; de um peer, o endereço dele no `peers.json`.
    fn switch_args(&self, key: &Key) -> Value {
        let base_url = (key.0 != HERE).then(|| self.machines.peers.base_url(&key.0)).flatten();
        json!({"server": key.0, "base_url": base_url, "name": key.1})
    }

    /// `said`: a fala do turno; sem pedido de troca para essa sessão, nada muda (o pedido segue para a sessão da tela).
    fn switch(&mut self, call: CallId, rows: &Rows, unreachable: &[String], spoken: &str, said: &str, recent: &str) {
        let now = Instant::now();
        let key = match self.resolve_match(rows, spoken, Some(true)) {
            SessionMatch::One(i) => { log("switch_session one"); (rows[i].0.clone(), rows[i].1.name.clone()) }
            SessionMatch::Many(found) => {
                log(format!("switch_session many({})", found.len()));
                // Pedido de troca de verdade, só que ambíguo: a resposta à pergunta "qual?" vale sem repetir o verbo.
                if found.iter().any(|&i| switch_requested(said, recent, &rows[i].1.name)) {
                    self.switch_offer.offer(found.iter().map(|&i| (rows[i].0.clone(), rows[i].1.name.clone())).collect(), now);
                }
                self.voice.reply(call, tool_reply(self.ambiguous(rows, &found), false));
                return;
            }
            SessionMatch::None => {
                log("switch_session none");
                self.voice.reply(call, tool_reply(Self::not_found(unreachable), false));
                return;
            }
        };
        if self.screen_key().as_ref() == Some(&key) {
            let by_jev = self.jev_switched.as_ref()
                .is_some_and(|(m, n, at)| *m == key.0 && *n == key.1 && now.saturating_duration_since(*at) < JEV_SWITCH_ECHO);
            log(format!("switch_session result=already jev={by_jev}"));
            let text = if by_jev { "O Hangar já trocou para essa sessão a pedido do usuário e já avisou por voz. Termine sem escrever nada." }
                else { "Já estou nessa sessão." };
            self.voice.reply(call, tool_reply(text, true));
        } else if !switch_requested(said, recent, &key.1) && !self.switch_offer.take(&key.0, &key.1, now) {
            log("switch_session refused not asked");
            self.voice.reply(call, tool_reply(format!("{SWITCH_REFUSED}."), false));
        } else if !self.can("switch_session") {
            log("switch_session refused: device lacks it");
            self.voice.reply(call, tool_reply("Este aparelho não troca de sessão pela voz.", false));
        } else {
            let args = self.switch_args(&key);
            self.send_tool("switch_session", args, Forwarded::Switch(call, key));
        }
    }

    pub(super) fn switched(&mut self, call: CallId, key: Key, ok: bool, text: String) {
        if !ok {
            log("switch_session result=device-refused");
            let text = if text.trim().is_empty() { "A máquina dessa sessão não está conectada.".to_owned() } else { text };
            self.voice.reply(call, tool_reply(text, false));
            return;
        }
        let name = key.1.clone();
        let offer = offer_summary(&mut self.talked, key, Instant::now());
        log(format!("switch_session result=switched offer={offer}"));
        self.voice.reply(call, tool_reply(if offer {
            format!("Sessão {name} aberta; a troca já foi anunciada, não repita. Pergunte numa frase curta se ele quer um resumo \
                do que a sessão fez por último; só se ele disser que sim, leia com read_session e resuma em até três frases.")
        } else {
            format!("Sessão {name} aberta; a troca já foi anunciada, não repita. Você falou dela há pouco: não ofereça resumo.")
        }, true));
    }

    /// Fechar em duas chamadas (`ConfirmGate`): a primeira só arma e pede a confirmação.
    fn close(&mut self, call: CallId, rows: &Rows, unreachable: &[String], spoken: &str, confirmed: bool, turn: &str) {
        let i = match self.resolve("close_session", rows, unreachable, spoken) {
            Ok(i) => i,
            Err(text) => { self.voice.reply(call, tool_reply(text, false)); return; }
        };
        let (machine, session) = &rows[i];
        // Orquestrador e par de fora não têm "Fechar" no menu.
        if session.provider == "orq" || session.guest_kind.as_deref() == Some("pair") {
            self.voice.reply(call, tool_reply("Essa sessão não pode ser fechada por aqui.", false));
            return;
        }
        if self.closing {
            self.voice.reply(call, tool_reply("Já há um fechamento em andamento; espere o resultado.", false));
            return;
        }
        let key = (machine.clone(), session.name.clone());
        if !self.close_gate.check(key.clone(), confirmed, turn, Instant::now()) {
            log("close_session armed");
            self.voice.reply(call, tool_reply(format!("Nada foi fechado. Confirme com o usuário: fechar a sessão {} em {}? \
                Só depois de um sim explícito, chame close_session de novo com confirmed true.", key.1, self.machines.label(&key.0)), true));
            return;
        }
        log("close_session sent");
        self.closing = true;
        let ctx = self.ctx();
        tokio::spawn(async move {
            let result = ctx.machines.close(&key.0, &key.1).await;
            let _ = ctx.done.send(Done::Closed { call, key, result });
        });
    }

    /// Mesma chamada do agrupar da tela; só na mesma máquina, com as recusas dela.
    fn pair(&mut self, call: CallId, rows: &Rows, unreachable: &[String], a: &str, b: &str) {
        let (first, second) = match (self.resolve("pair_sessions", rows, unreachable, a), self.resolve("pair_sessions", rows, unreachable, b)) {
            (Ok(first), Ok(second)) => (&rows[first], &rows[second]),
            (Err(text), _) | (_, Err(text)) => { self.voice.reply(call, tool_reply(text, false)); return; }
        };
        if let Err(refusal) = can_pair(&first.1, &second.1, first.0 == second.0) { self.voice.reply(call, tool_reply(refusal, false)); return; }
        log("pair_sessions sent");
        let (machine, origin, target) = (first.0.clone(), first.1.name.clone(), second.1.name.clone());
        let ctx = self.ctx();
        tokio::spawn(async move {
            let result = ctx.machines.pair(&machine, &target, &origin).await;
            grouped(&ctx, call, "pair_sessions", "Falhou o agrupamento.", result);
        });
    }

    fn unpair(&mut self, call: CallId, rows: &Rows, unreachable: &[String], spoken: &str) {
        let i = match self.resolve("unpair_session", rows, unreachable, spoken) {
            Ok(i) => i,
            Err(text) => { self.voice.reply(call, tool_reply(text, false)); return; }
        };
        let (machine, session) = &rows[i];
        if !can_leave(session) { self.voice.reply(call, tool_reply(format!("A sessão {} não está em grupo.", session.name), false)); return; }
        log("unpair_session sent");
        let (machine, name) = (machine.clone(), session.name.clone());
        let ctx = self.ctx();
        tokio::spawn(async move {
            let result = ctx.machines.unpair(&machine, &name).await;
            grouped(&ctx, call, "unpair_session", "Falhou a saída do grupo.", result);
        });
    }

    /// Acompanhar ou parar: a vigia fala cada resposta da sessão, não só depois de um pedido da voz.
    fn follow(&mut self, call: CallId, rows: &Rows, unreachable: &[String], spoken: &str, on: bool) {
        let i = match self.resolve(if on { "follow_session" } else { "unfollow_session" }, rows, unreachable, spoken) {
            Ok(i) => i,
            Err(text) => { self.voice.reply(call, tool_reply(text, false)); return; }
        };
        let entry = (rows[i].0.clone(), rows[i].1.name.clone());
        let name = entry.1.clone();
        let text = if on {
            self.followed.insert(entry);
            format!("Acompanhando a sessão {name}: cada resposta dela chega para você falar.")
        } else if self.followed.remove(&entry) {
            format!("Parei de acompanhar a sessão {name}.")
        } else {
            format!("A sessão {name} não estava sendo acompanhada.")
        };
        log(format!("follow on={on} count={}", self.followed.len()));
        self.voice.reply(call, tool_reply(text, true));
        self.dirty = true;
    }

    /// Destino nomeado vale pela sessão escolhida, não pela tela; sem nome, a da tela neste instante.
    pub(super) fn send(&mut self, call: CallId, request: String, session: Option<String>, turn: String) {
        match session {
            Some(name) => self.lookup(Want::Send { call, request, name, turn }),
            None => match self.screen_key() {
                Some(key) => self.send_to(call, key, request, turn),
                None => self.voice.reply(call, tool_reply("Nenhuma sessão aberta na tela; nada foi enviado.", false)),
            },
        }
    }

    fn send_to(&mut self, call: CallId, key: Key, request: String, turn: String) {
        // Nomes diferentes podem chegar à mesma sessão ("hcc" e "hcc-rust-plano"): a trava final é pela sessão.
        let fresh = match &mut self.sent_turn {
            Some((sent, keys)) if *sent == turn => keys.insert(key.clone()),
            slot => { *slot = Some((turn.clone(), HashSet::from([key.clone()]))); true }
        };
        if !fresh {
            log("send refused duplicate session");
            self.voice.reply(call, tool_reply(SEND_DUPLICATE, false));
            return;
        }
        // Vigiada antes do envio: um turno curto pode acabar antes de a resposta do POST voltar.
        self.watched.insert(key.clone());
        let ctx = self.ctx();
        tokio::spawn(async move {
            let result = ctx.machines.send(&key.0, &key.1, &request).await;
            log(format!("send result {}", match &result {
                Ok(d) if d.delivered => "sent", Ok(_) => "queued", Err(e) if e.certain => "refused", Err(_) => "uncertain" }));
            ctx.voice.reply(call, send_reply(&key.1, &result.clone().map_err(|e| e.text)));
            let _ = ctx.done.send(Done::Sent { key, turn, error: result.err() });
        });
    }

    /// Pergunta curta do organizador à sessão da tela; a resposta volta pela vigia (`ask_intercept`).
    pub(super) fn ask(&mut self, question: String) {
        if self.pending_question.is_some() { self.voice.session_answer("Já há uma pergunta aguardando a sessão.".into()); return; }
        let Some(key) = self.screen_key() else {
            self.voice.session_answer("Nenhuma sessão aberta na tela; a pergunta não foi enviada.".into());
            return;
        };
        let text = question_text(&question);
        log(format!("ask_session sent bytes={}", text.len()));
        self.pending_question = Some((key.clone(), Instant::now()));
        let ctx = self.ctx();
        tokio::spawn(async move {
            if ctx.machines.send(&key.0, &key.1, &text).await.is_err() { let _ = ctx.done.send(Done::AskFailed(key)); }
        });
    }

    /// O plano vai para a sessão confirmada, esteja ou não na tela, e a tela não muda. A da nascença usa a máquina de então;
    /// outro nome é resolvido como no envio nomeado (ambíguo ou ausente não vira palpite).
    pub(super) fn send_plan(&mut self, session: String, text: String) {
        match self.plan_key.clone().filter(|(_, n)| *n == session) {
            Some(key) => self.send_plan_to(key, text),
            None => { log("plan target by name"); self.lookup(Want::Plan { name: session, text }); }
        }
    }

    fn send_plan_to(&mut self, key: Key, text: String) {
        // Quem manda trabalho acompanha as respostas, como no envio direto.
        if self.followed.insert(key.clone()) { self.dirty = true; }
        self.watched.insert(key.clone());
        let ctx = self.ctx();
        tokio::spawn(async move {
            match ctx.machines.send(&key.0, &key.1, &text).await {
                Ok(_) => {
                    log("plan delivered");
                    ctx.voice.plan_delivered();
                    let _ = ctx.done.send(Done::PlanDelivered);
                }
                Err(error) => {
                    ctx.voice.session_answer("O plano não chegou à sessão.".into());
                    let _ = ctx.done.send(Done::Failed { code: "send_plan", text: error.text });
                }
            }
        });
    }

    pub(super) fn read_session(&mut self, call: CallId) {
        let Some(key) = self.screen_key() else {
            self.voice.reply(call, tool_reply(session_context("", &[]), true));
            return;
        };
        self.talked.insert(key.clone(), Instant::now());
        let ctx = self.ctx();
        tokio::spawn(async move {
            match ctx.machines.history(&key.0, &key.1, 60).await {
                Ok(events) => ctx.voice.reply(call, tool_reply(session_context(&key.1, &conversation_pairs(&events)), true)),
                Err(error) => ctx.fail(call, "read_session", format!("Não consegui ler a sessão {}: {error}", key.1)),
            }
        });
    }

    /// Duas leituras com a amostra no meio, fora do laço.
    pub(super) fn observe(&mut self, call: CallId, request: observe::Request) {
        let voice = self.voice.clone();
        tokio::spawn(async move {
            let reply = match tokio::task::spawn_blocking(move || observe::observe(&request)).await {
                Ok(text) => tool_reply(text, true),
                Err(_) => tool_reply("A leitura dos processos falhou.", false),
            };
            log("observe done");
            voice.reply(call, reply);
        });
    }

    /// O HCC roda fora do laço e responde quando acaba; um objetivo por vez.
    pub(super) fn computer(&mut self, call: CallId, objective: String) {
        if self.computer.as_ref().is_some_and(|task| !task.is_finished()) {
            self.voice.reply(call, tool_reply("Já há um objetivo no computador em andamento; espere o resultado.", false));
            return;
        }
        log("computer start");
        let ctx = self.ctx();
        self.computer = Some(tokio::spawn(async move {
            let result = match tokio::task::spawn_blocking(computer::launch).await {
                Ok(Ok(launch)) => computer::run_objective(launch, &objective).await,
                Ok(Err(text)) => Err(text),
                Err(_) => Err("Não consegui preparar o hangar-computer-control.".into()),
            };
            match result {
                // "parou: …" é resultado do HCC, não falha do app: volta como está, sem sucesso.
                Ok(text) => {
                    let done = !text.trim_start().starts_with("parou");
                    log(format!("computer done completed={done}"));
                    ctx.voice.reply(call, tool_reply(text, done));
                }
                Err(text) => ctx.fail(call, "computer", text),
            }
        }));
    }

    /// O mesmo `POST /api/sessions` da tela de criação, com o que ela traria sem toque; o pedido ditado sai logo que a
    /// sessão nasce, e a resposta ao organizador espera a entrega e a troca no aparelho.
    pub(super) fn open(&mut self, call: CallId, request: OpenRequest) {
        log(format!("open_session start provider={} request={}", request.provider, request.request.is_some()));
        let ctx = self.ctx();
        tokio::spawn(async move {
            let machine = match pick_machine(&ctx.machines, request.server.as_deref()) {
                Ok(machine) => machine,
                Err(text) => { ctx.voice.reply(call, tool_reply(text, false)); return; }
            };
            let work = request.request.clone();
            let created = create(&ctx.machines, &machine, &request).await;
            let (created, sent) = match (created, work) {
                (Ok(name), Some(work)) => {
                    let delivery = ctx.machines.send(&machine, &name, &work).await.map_err(|e| e.text);
                    (Ok(name), Some(delivery))
                }
                (Err(text), Some(_)) => (Err(format!("{text} O pedido não foi enviado.")), None),
                (created, None) => (created, None),
            };
            match created {
                Ok(name) => { let _ = ctx.done.send(Done::Opened { call, machine, name, sent }); }
                Err(text) => { log("open_session failed"); ctx.fail(call, "open_session", text); }
            }
        });
    }

    pub(super) fn opened(&mut self, call: CallId, machine: String, name: String, sent: Option<Result<Delivery, String>>) {
        log(format!("open_session created request={}", sent.is_some()));
        let key = (machine, name.clone());
        if sent.is_some() { self.watched.insert(key.clone()); }
        if self.can("switch_session") {
            let args = self.switch_args(&key);
            self.send_tool("switch_session", args, Forwarded::Opened { call, name, sent });
            return;
        }
        let note = match &sent {
            None => String::new(),
            Some(Ok(d)) if d.ok => " O pedido foi enviado a ela; aguarde o resultado real.".into(),
            Some(_) => " O pedido não chegou a ela; não reenvie, peça para o usuário conferir o chat.".into(),
        };
        let ok = sent.as_ref().is_none_or(|r| r.as_ref().is_ok_and(|d| d.ok));
        self.voice.reply(call, tool_reply(format!("Sessão {name} criada; este aparelho não a abre na tela pela voz.{note}"), ok));
    }

    /// Pergunta ao Jev o que a fala pede, com as sessões e as ações de tela de agora; roda em paralelo ao organizador.
    pub(super) fn jev_ask(&mut self, turn: String, speech: String, recent: String) {
        let Some(config) = self.jev.clone() else { return };
        let open = self.screen_key();
        // Todas as máquinas: a pessoa diz só o nome, e a sessão pode estar em outro servidor.
        let labels: Vec<String> = self.rows.iter()
            .map(|(m, s)| jev::session_label(&s.name, &self.machines.label(m), open.as_ref().is_some_and(|(om, on)| om == m && *on == s.name)))
            .collect();
        let sessions: Vec<Key> = self.rows.iter().map(|(m, s)| (m.clone(), s.name.clone())).collect();
        let catalog: Vec<(String, String, String)> = if self.can("hangar_action") {
            self.actions.iter().filter_map(|a| {
                let text = |k: &str| a[k].as_str().unwrap_or_default().to_owned();
                Some((a["id"].as_str().filter(|id| !id.is_empty())?.to_owned(), text("label"), text("description")))
            }).collect()
        } else { Vec::new() };
        let action_texts: Vec<String> = catalog.iter().map(|(_, label, description)| format!("{label}: {description}")).collect();
        let (state, questions) = jev::questions(&speech, &recent, open.as_ref().map(|(_, n)| n.as_str()), &labels, &action_texts);
        // Só contagens: o texto da fala nunca vai ao diário.
        log(format!("jev asked words={} recent_lines={} sessions={} actions={}", speech.split_whitespace().count(), recent.lines().count(),
            sessions.len(), catalog.len()));
        let asked = JevAsked { turn, speech, recent, sessions, actions: catalog.iter().map(|(id, ..)| id.clone()).collect(),
            action_labels: catalog.into_iter().map(|(_, label, _)| label).collect(), started: Instant::now() };
        let done = self.done.clone();
        tokio::spawn(async move {
            let result = jev::ask(&config, &state, &questions).await.map(|picks| jev::decision(&picks));
            let _ = done.send(Done::Jev(asked, result));
        });
    }

    /// Só certeza alta age. Troca e ação de tela vão ao aparelho; quando ele confirma, a voz fala e o organizador recebe a
    /// nota no mesmo turno. O veredito de envio vai à trava da chamada.
    pub(super) fn on_jev(&mut self, asked: JevAsked, result: Result<jev::Decision, String>) {
        let ms = asked.started.elapsed().as_millis();
        let decision = match result {
            Ok(decision) => decision,
            Err(error) => { log(format!("jev failed ms={ms} error={error}")); return; }
        };
        let intent = decision.intent.clone();
        log(format!("jev intent={:?} p={:.2} session_p={:.2} action_p={:.2} ms={ms}", intent.as_ref().map(|(i, _)| i),
            intent.as_ref().map_or(0., |(_, p)| *p), decision.session.map_or(0., |(_, p)| p), decision.action.map_or(0., |(_, p)| p)));
        let sure = |what: jev::Intent| intent.as_ref().is_some_and(|(i, p)| *i == what && *p >= jev::SURE);
        let verdict = if sure(jev::Intent::Send) { SendVerdict::Send }
            else if sure(jev::Intent::Talk) || sure(jev::Intent::Hold) || sure(jev::Intent::Noise) { SendVerdict::Block }
            else { SendVerdict::Unsure };
        // 'Volta pra lá' não tem nome: com a intenção de troca firme, o destino é o que a voz acabou de oferecer.
        let names: Vec<&str> = asked.sessions.iter().map(|(_, n)| n.as_str()).collect();
        let choice = match jev_switch_choice(&decision) {
            Err("no-session" | "low-session") if let Some(at) = confirmed_switch_target(&asked.recent, &names) => Ok(at),
            other => other,
        };
        match choice {
            Ok(at) => match asked.sessions.get(at).cloned() {
                None => log("jev skip reason=no-session"),
                Some(key) => {
                    let reason = if self.screen_key().as_ref() == Some(&key) { Some("on-screen") }
                        else if !switch_requested(&asked.speech, &asked.recent, &key.1) { Some("not-asked") }
                        else if !self.rows.iter().any(|(m, s)| *m == key.0 && s.name == key.1) { Some("gone") }
                        else if !self.can("switch_session") { Some("no-device") }
                        else { None };
                    match reason {
                        Some(reason) => log(format!("jev skip reason={reason}")),
                        None => {
                            let machine = self.machines.label(&key.0);
                            let args = self.switch_args(&key);
                            self.send_tool("switch_session", args, Forwarded::JevSwitch { turn: asked.turn, verdict, key, machine });
                            return;
                        }
                    }
                }
            },
            Err("not-switch") => match jev_action_choice(&decision) {
                Ok(at) => match (asked.actions.get(at), asked.action_labels.get(at)) {
                    (Some(id), Some(label)) => {
                        self.send_tool("hangar_action", json!({"id": id, "arg": null}),
                            Forwarded::JevAction { turn: asked.turn, verdict, label: label.clone() });
                        return;
                    }
                    _ => log("jev skip reason=screen-gone"),
                },
                Err(reason) if reason != "not-screen" => log(format!("jev skip reason=screen-{reason}")),
                Err(_) => {}
            },
            Err(reason) => log(format!("jev skip reason={reason}")),
        }
        self.voice.jev_verdict(asked.turn, verdict, None, None);
    }

    pub(super) fn jev_switched(&mut self, turn: String, verdict: SendVerdict, key: Key, machine: String, ok: bool) {
        if !ok {
            log("jev act=switch refused by device");
            self.voice.jev_verdict(turn, verdict, None, None);
            return;
        }
        let name = key.1.clone();
        self.jev_switched = Some((key.0.clone(), key.1.clone(), Instant::now()));
        let offer = offer_summary(&mut self.talked, key, Instant::now());
        log(format!("jev act=switched offer={offer}"));
        let (speech, note) = if offer {
            (format!("Pronto, abri a {name}. Quer um resumo do que ela fez por último?"),
                format!("O Hangar já trocou a tela para a sessão {name} (máquina {machine}) a pedido do usuário, e a voz já \
                    perguntou se ele quer um resumo. Não chame switch_session e termine sem escrever nada; se ele disser \
                    que sim, leia com read_session e resuma em até três frases."))
        } else {
            (format!("Pronto, abri a {name}."),
                format!("O Hangar já trocou a tela para a sessão {name} (máquina {machine}) a pedido do usuário e já confirmou \
                    por voz. Não chame switch_session e termine sem escrever nada."))
        };
        self.voice.jev_verdict(turn, verdict, Some(speech), Some(note));
    }

    pub(super) fn jev_acted(&mut self, turn: String, verdict: SendVerdict, label: String, ok: bool, text: String) {
        log(format!("jev act=screen ok={ok}"));
        let (speech, note) = if ok {
            (Some(format!("Pronto. {text}")), format!("O Hangar já fez na tela, a pedido do usuário: {label}. {text} Já confirmou \
                por voz. Não repita a ação; se faltar um passo (uma seção, um botão), complete com as ferramentas de tela; senão \
                termine sem escrever nada."))
        } else {
            (None, format!("O Hangar tentou na tela, a pedido do usuário, {label}, mas não deu: {text}"))
        };
        self.voice.jev_verdict(turn, verdict, speech, Some(note));
    }
}

/// Resposta de agrupar/sair: aviso de quem não foi avisado vai junto; falha sem motivo usa a frase do web.
fn grouped(ctx: &Ctx, call: CallId, code: &'static str, fallback: &str, result: Result<crate::voice::rules::PairResult, String>) {
    match result {
        Ok(result) => {
            log(format!("group done warning={}", result.warning.is_some()));
            let text = result.warning.map_or_else(|| "Feito.".to_owned(), |w| format!("Feito, mas com aviso: {w}"));
            ctx.voice.reply(call, tool_reply(text, true));
        }
        Err(text) => {
            log(format!("{code} failed"));
            ctx.fail(call, code, if text.trim().is_empty() { fallback.to_owned() } else { text });
        }
    }
}

/// Máquina falada → chave; sem máquina, este servidor.
fn pick_machine(machines: &Machines, spoken: Option<&str>) -> Result<String, String> {
    let Some(spoken) = spoken else { return Ok(HERE.to_owned()) };
    let keys: Vec<String> = std::iter::once(HERE.to_owned()).chain(machines.peers.enabled_ids()).collect();
    let labels: Vec<String> = keys.iter().map(|k| machines.label(k)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    match match_session(spoken, &refs, &[]) {
        SessionMatch::One(i) => Ok(keys[i].clone()),
        SessionMatch::Many(found) => Err(format!("Mais de uma máquina combina: {}. Peça para o usuário dizer qual.",
            found.iter().map(|&i| refs[i]).collect::<Vec<_>>().join(", "))),
        SessionMatch::None => Err(format!("Não achei máquina com esse nome. Máquinas: {}.", refs.join(", "))),
    }
}

/// Nome falado → pasta, entre as raízes da máquina e as subpastas delas.
async fn folder(machines: &Machines, machine: &str, spoken: &str) -> Result<String, String> {
    if looks_like_path(spoken) { return Ok(spoken.to_owned()); }
    let roots = machines.call(machine, reqwest::Method::GET, "/api/fs/roots", None).await?;
    let pair = |v: &Value| Some((v["name"].as_str()?.to_owned(), v["path"].as_str()?.to_owned()));
    let roots: Vec<(String, String)> = roots.as_array().ok_or("Resposta inválida das pastas.")?.iter().filter_map(pair).collect();
    let mut folders = roots.clone();
    let mut unread = 0;
    for (_, root) in &roots {
        let path = format!("/api/fs/scan?root={}", utf8_percent_encode(root, NON_ALPHANUMERIC));
        match machines.call(machine, reqwest::Method::GET, &path, None).await {
            Ok(scan) if scan["error"].is_null() => folders.extend(scan["entries"].as_array().into_iter().flatten().filter_map(pair)),
            _ => unread += 1,
        }
    }
    pick_folder(spoken, &folders, unread)
}

/// Cria a sessão; devolve o nome com que ela nasceu.
// ponytail: modelo e esforço ficam com o padrão do servidor, que é o que o catálogo dá sem escolha lembrada (o servidor
// não guarda o último modelo do aparelho).
async fn create(machines: &Machines, machine: &str, request: &OpenRequest) -> Result<String, String> {
    let cwd = folder(machines, machine, &request.folder).await?;
    let name = match &request.name {
        Some(name) => name.clone(),
        None => {
            let list = machines.call(machine, reqwest::Method::GET, "/api/sessions", None).await?;
            let taken: HashSet<String> = list.as_array().into_iter().flatten().filter_map(|s| s["name"].as_str().map(str::to_owned)).collect();
            unique_name(folder_name(&cwd), &taken)
        }
    };
    let mut body = json!({"name": name, "cwd": cwd, "provider": request.provider, "model": null, "effort": null});
    if request.provider == "claude" { body["permission_mode"] = json!("bypassPermissions"); }
    let created = machines.call(machine, reqwest::Method::POST, "/api/sessions", Some(body)).await?;
    Ok(created["name"].as_str().filter(|n| !n.is_empty()).map_or(name, str::to_owned))
}

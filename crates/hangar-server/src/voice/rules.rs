//! Regras puras da voz: nome falado → sessão, textos das ferramentas de sessão e as respostas que o organizador ouve.
use super::organizer::{family_root, name_tokens, squash, tool_reply, word_fits};
use hangar_api::session::SessionRow;
use serde_json::Value;
use std::{collections::{HashMap, HashSet}, time::Duration};

/// Ferramentas que dependem da tela do aparelho dono da chamada.
pub const SCREEN_TOOLS: [&str; 5] = ["switch_session", "hangar_actions", "hangar_action", "read_screen", "click_screen"];

/// Resposta de `POST /api/sessions/{n}/input`.
#[derive(Clone, Debug, Default, serde::Deserialize, PartialEq)]
pub struct Delivery { pub ok: bool, #[serde(default)] pub delivered: bool }

/// Resposta de `POST`/`DELETE /api/sessions/{n}/pair`: aviso de quem não foi avisado.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PairResult { pub warning: Option<String> }

impl PairResult {
    pub fn from_value(value: &serde_json::Value) -> Self {
        let warning = value.get("warning").filter(|w| !w.is_null()).map(|w| match w {
            serde_json::Value::String(text) => text.clone(),
            _ => w.get("msg").and_then(serde_json::Value::as_str).map(str::to_owned).unwrap_or_else(|| w.to_string()),
        });
        Self { warning }
    }
}

/// Quanto tempo o `switch_session` do organizador para a sessão que o Jev acabou de abrir conta como eco da mesma fala.
pub const JEV_SWITCH_ECHO: Duration = Duration::from_secs(20);

/// Quanto tempo depois de falar de uma sessão a troca por voz não oferece resumo dela de novo.
pub const TALKED_WINDOW: Duration = Duration::from_secs(15 * 60);

/// Troca por voz oferece resumo? Só se a voz não falou dessa sessão há pouco; oferecer conta como ter falado.
pub fn offer_summary(talked: &mut HashMap<(String, String), std::time::Instant>, session: (String, String), now: std::time::Instant) -> bool {
    let recent = talked.get(&session).is_some_and(|at| now.saturating_duration_since(*at) < TALKED_WINDOW);
    if !recent { talked.insert(session, now); }
    !recent
}

/// Troca que o Jev pode fazer sozinho: a sessão escolhida, ou o motivo de não agir (vai ao diário como código).
pub fn jev_switch_choice(decision: &super::jev::Decision) -> Result<usize, &'static str> {
    use super::jev::{Intent, SESSION_SURE, SWITCH_INTENT};
    match &decision.intent {
        Some((Intent::Switch, p)) if *p >= SWITCH_INTENT => {}
        Some((Intent::Switch, _)) => return Err("low-intent"),
        _ => return Err("not-switch"),
    }
    match decision.session { Some((at, p)) if p >= SESSION_SURE => Ok(at), Some(_) => Err("low-session"), None => Err("no-session") }
}

/// Ação de tela que o Jev pode fazer sozinho: a ação escolhida, ou o motivo de não agir.
pub fn jev_action_choice(decision: &super::jev::Decision) -> Result<usize, &'static str> {
    use super::jev::{ACTION_SURE, Intent};
    match &decision.intent {
        Some((Intent::Screen, p)) if *p >= ACTION_SURE => {}
        Some((Intent::Screen, _)) => return Err("low-intent"),
        _ => return Err("not-screen"),
    }
    match decision.action { Some((at, p)) if p >= ACTION_SURE => Ok(at), Some(_) => Err("low-action"), None => Err("no-action") }
}

/// Uma linha do `list_sessions`.
pub struct Listed { pub name: String, pub machine: Option<String>, pub provider: String, pub state: String,
    pub folder: String, pub on_screen: bool, pub followed: bool }

pub fn sessions_text(rows: &[Listed], unreachable: &[String]) -> String {
    let mut lines: Vec<String> = rows.iter().map(|r| {
        let mut line = format!("- {}", r.name);
        if let Some(machine) = &r.machine { line.push_str(&format!(" (máquina {machine})")); }
        line.push_str(&format!(": {}, {}", r.provider, if r.state.is_empty() { "?" } else { &r.state }));
        if !r.folder.is_empty() { line.push_str(&format!(", pasta {}", r.folder)); }
        if r.on_screen { line.push_str(", na tela"); }
        if r.followed { line.push_str(", acompanhada"); }
        line
    }).collect();
    if lines.is_empty() { lines.push("Nenhuma sessão aberta.".into()); }
    // Lista que falhou não é "sem sessões": o organizador precisa saber que faltam as de lá.
    lines.extend(unreachable.iter().map(|m| format!("Máquina {m} sem resposta; as sessões dela não estão aqui.")));
    lines.join("\n")
}

/// Caminho já dito como caminho (Unix, `~`, Windows): vai direto ao backend, sem procurar pelo nome.
pub fn looks_like_path(text: &str) -> bool {
    text.starts_with(['/', '~', '\\']) || matches!(text.get(1..3), Some(":\\" | ":/"))
}

/// Nome falado → pasta, entre as raízes e as subpastas delas (`(nome, caminho)`, a lista da tela de criação).
pub fn pick_folder(spoken: &str, folders: &[(String, String)], unread: usize) -> Result<String, String> {
    let mut seen = HashSet::new();
    let folders: Vec<&(String, String)> = folders.iter().filter(|(_, path)| seen.insert(path.as_str())).collect();
    let names: Vec<&str> = folders.iter().map(|(name, _)| name.as_str()).collect();
    match match_session(spoken, &names, &[]) {
        SessionMatch::One(i) => Ok(folders[i].1.clone()),
        SessionMatch::Many(found) => Err(format!("Mais de uma pasta combina: {}. Peça para o usuário dizer qual.",
            found.iter().take(5).map(|&i| folders[i].1.as_str()).collect::<Vec<_>>().join(", "))),
        SessionMatch::None if unread > 0 => Err(format!("Não achei pasta com esse nome; {unread} raiz(es) não puderam ser lidas.")),
        SessionMatch::None => Err("Não achei pasta com esse nome. Peça o nome exato ou o caminho.".into()),
    }
}

#[derive(Debug, PartialEq)]
pub enum SessionMatch { One(usize), Many(Vec<usize>), None }

/// Nome falado → sessão. Igual vence parcial; o mesmo nome em duas máquinas fica com o da ativa (`on_active`).
pub fn match_session(query: &str, names: &[&str], on_active: &[bool]) -> SessionMatch {
    let original = query;
    let query = squash(query);
    if query.is_empty() { return SessionMatch::None; }
    let squashed: Vec<String> = names.iter().map(|n| squash(n)).collect();
    let pick = |hits: Vec<usize>| match hits.as_slice() { [] => SessionMatch::None, [one] => SessionMatch::One(*one), _ => SessionMatch::Many(hits) };
    let exact: Vec<usize> = (0..names.len()).filter(|&i| squashed[i] == query).collect();
    if !exact.is_empty() {
        let mine: Vec<usize> = exact.iter().copied().filter(|&i| on_active.get(i) == Some(&true)).collect();
        return if mine.len() == 1 { SessionMatch::One(mine[0]) } else { pick(exact) };
    }
    let partial: Vec<usize> = (0..names.len()).filter(|&i| squashed[i].contains(&query)).collect();
    if !partial.is_empty() { return pick(partial); }
    // Palavras soltas em qualquer ordem: "plano do rust" casa com grupos-rust-plano; "do", "da", "a" não contam.
    let words: Vec<String> = original.split(|c: char| !c.is_alphanumeric()).map(squash).filter(|w| w.chars().count() > 2).collect();
    if words.len() >= 2 {
        let all: Vec<usize> = (0..names.len()).filter(|&i| words.iter().all(|w| squashed[i].contains(w.as_str()))).collect();
        if !all.is_empty() { return pick(all); }
    }
    // Aproximado, palavra por pedaço do nome: "setting ux" casa com jev-settings-ux, "seting" com settings.
    let spoken: Vec<String> = original.split(|c: char| !c.is_alphanumeric()).map(squash)
        .filter(|w| !w.is_empty() && !NAME_FILLERS.contains(&w.as_str())).collect();
    if spoken.is_empty() { return SessionMatch::None; }
    pick((0..names.len()).filter(|&i| { let tokens = name_tokens(names[i]); spoken.iter().all(|w| tokens.iter().any(|t| word_fits(w, t))) }).collect())
}

/// Palavras que acompanham o nome falado sem fazer parte dele.
const NAME_FILLERS: [&str; 12] = ["a", "o", "as", "os", "da", "do", "de", "pra", "pro", "para", "sessao", "secao"];

/// Várias sessões casam com o nome falado: uma só no contexto (citada na conversa, acompanhada, falada há pouco, a da
/// tela quando não é troca) decide; senão a raiz da família (hcc-rust-plano diante dos executores dela); senão continua
/// ambíguo e quem chama pergunta, sem escolher.
pub fn narrow_by_context(found: Vec<usize>, names: &[&str], context: &[bool]) -> SessionMatch {
    let hit: Vec<usize> = (0..found.len()).filter(|&i| context[i]).collect();
    if let [one] = hit[..] { return SessionMatch::One(found[one]); }
    // Contexto dividido entre várias: a raiz só vale entre elas.
    let pool: Vec<usize> = if hit.is_empty() { (0..found.len()).collect() } else { hit };
    let pool_names: Vec<&str> = pool.iter().map(|&i| names[i]).collect();
    match family_root(&pool_names) { Some(at) => SessionMatch::One(found[pool[at]]), None => SessionMatch::Many(found) }
}

/// Nomes pelos quais a máquina pode ser dita: o rótulo dela e o host do endereço ("jefferson-felizardo" de
/// `https://jefferson-felizardo.tailnet.ts.net`), sem acento, pontuação e caixa.
pub fn machine_aliases(key: &str, label: &str) -> Vec<String> {
    let host = key.rsplit("://").next().unwrap_or(key).split(['/', ':']).next().unwrap_or_default();
    let first = host.split('.').next().unwrap_or_default();
    let mut aliases: Vec<String> = [label, host, first].into_iter().map(squash).filter(|a| a.chars().count() >= 3).collect();
    aliases.dedup();
    aliases
}

/// Palavras que ligam o nome da sessão à máquina na fala ("hangar lá no PC X", "hangar na máquina X").
const MACHINE_LINKS: [&str; 15] = ["la", "no", "na", "em", "do", "da", "de", "pc", "maquina", "computador", "servidor", "sessao", "secao", "aquela", "aquele"];

/// Nome falado → sessão, aceitando a máquina junto: "X::nome", "nome (máquina X)", "nome lá no PC X". Máquina citada
/// restringe às sessões dela; sem máquina, é o `match_session` de sempre. Ambíguo continua ambíguo: quem chama pergunta.
pub fn match_qualified(spoken: &str, names: &[&str], machines: &[Vec<String>], on_active: &[bool]) -> SessionMatch {
    let (machine_text, name_text) = match spoken.split_once("::") {
        Some((machine, name)) => (Some(squash(machine)), name.to_owned()),
        None => (None, spoken.to_owned()),
    };
    let said = squash(spoken);
    let named = |aliases: &Vec<String>| aliases.iter().any(|alias| match &machine_text {
        Some(m) => !m.is_empty() && (alias.contains(m.as_str()) || m.contains(alias.as_str())),
        None => said.contains(alias.as_str()),
    });
    let on_machine: Vec<usize> = (0..names.len()).filter(|&i| machines.get(i).is_some_and(named)).collect();
    if on_machine.is_empty() {
        return if machine_text.is_some() { SessionMatch::None } else { match_session(spoken, names, on_active) };
    }
    // O que sobra da fala sem as palavras da máquina e as de ligação é o nome da sessão.
    let machine_words: Vec<String> = on_machine.iter().flat_map(|&i| machines[i].iter().cloned()).collect();
    let rest: Vec<String> = name_text.split(|c: char| !c.is_alphanumeric()).map(squash)
        .filter(|w| !w.is_empty() && !MACHINE_LINKS.contains(&w.as_str()) && !machine_words.iter().any(|m| m == w || (w.chars().count() >= 4 && m.contains(w.as_str()))))
        .collect();
    let subset: Vec<&str> = on_machine.iter().map(|&i| names[i]).collect();
    let active: Vec<bool> = on_machine.iter().map(|&i| on_active.get(i).copied().unwrap_or(false)).collect();
    match match_session(&rest.join(" "), &subset, &active) {
        SessionMatch::One(at) => SessionMatch::One(on_machine[at]),
        SessionMatch::Many(found) => SessionMatch::Many(found.into_iter().map(|at| on_machine[at]).collect()),
        SessionMatch::None => SessionMatch::None,
    }
}

/// Pergunta "qual das duas?" depois de um pedido de troca: o "isso"/"a do PC" seguinte não repete o verbo de trocar,
/// e vale só para uma das sessões oferecidas, por um minuto.
#[derive(Default)]
pub struct SwitchOffer { offered: Vec<(String, String)>, at: Option<std::time::Instant> }

const SWITCH_OFFER_WINDOW: Duration = Duration::from_secs(60);

impl SwitchOffer {
    pub fn offer(&mut self, candidates: Vec<(String, String)>, now: std::time::Instant) { (self.offered, self.at) = (candidates, Some(now)); }
    /// `true` consome a oferta.
    pub fn take(&mut self, key: &str, name: &str, now: std::time::Instant) -> bool {
        let fresh = self.at.is_some_and(|at| now.saturating_duration_since(at) < SWITCH_OFFER_WINDOW);
        let ok = fresh && self.offered.iter().any(|(k, n)| k == key && n == name);
        if ok { *self = Self::default(); }
        ok
    }
}

pub fn conversation_pairs(events: &[hangar_api::chat::ChatEvent]) -> Vec<(String, String)> {
    events.iter().filter(|e| matches!(e.kind.as_str(), "user_msg" | "assistant_msg") && e.text.as_deref().is_some_and(|t| !t.trim().is_empty()))
        .map(|e| (e.kind.as_str().to_owned(), e.text.clone().unwrap_or_default())).collect()
}

/// `(id, kind, text)` → `(id da última resposta, respostas depois da última fala do usuário)`.
pub fn last_reply(events: &[(String, String, String)]) -> Option<(String, String)> {
    let start = events.iter().rposition(|(_, kind, _)| kind == "user_msg").map(|i| i + 1).unwrap_or(0);
    let replies: Vec<&(String, String, String)> = events[start..].iter().filter(|(_, k, _)| k == "assistant_msg").collect();
    let last = replies.last()?;
    Some((last.0.clone(), replies.iter().map(|(_, _, t)| t.as_str()).collect::<Vec<_>>().join("\n\n")))
}

/// Quanto a pergunta do organizador espera pela sessão antes de desistir.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(600);
pub const ASK_MARK: &str = "[Pergunta da conversa de voz]";

/// A marca no começo é o que acha a pergunta no histórico.
pub fn question_text(q: &str) -> String {
    format!("{ASK_MARK} {q}\nResponda curto; é para o planejamento, não execute nada.")
}

pub fn question_expired(since: std::time::Instant, now: std::time::Instant) -> bool { now.saturating_duration_since(since) >= ASK_TIMEOUT }

pub fn question_marked(events: &[(String, String, String)]) -> bool {
    events.iter().any(|(_, kind, text)| kind == "user_msg" && text.starts_with(ASK_MARK))
}

/// `(id, kind, text)` → `(id da última resposta, respostas)` dadas depois da última pergunta marcada e antes de outro pedido.
pub fn question_answer(events: &[(String, String, String)]) -> Option<(String, String)> {
    let start = events.iter().rposition(|(_, kind, text)| kind == "user_msg" && text.starts_with(ASK_MARK))? + 1;
    let end = events[start..].iter().position(|(_, kind, _)| kind == "user_msg").map_or(events.len(), |i| start + i);
    let replies: Vec<&(String, String, String)> = events[start..end].iter().filter(|(_, k, _)| k == "assistant_msg").collect();
    let last = replies.last()?;
    Some((last.0.clone(), replies.iter().map(|(_, _, t)| t.as_str()).collect::<Vec<_>>().join("\n\n")))
}

/// Quanto o fim do turno espera pela última `assistant_msg` antes de falar o que já tem.
pub const REPLY_WAIT: Duration = Duration::from_millis(1500);

/// O que o resultado diz quando a sessão parou esperando o usuário: a pergunta, ou o fim da última resposta.
pub fn waiting_text(questions: &[&str], reply: Option<&str>) -> String {
    let asked = questions.iter().map(|q| q.trim()).filter(|q| !q.is_empty()).collect::<Vec<_>>().join(" ");
    let detail = if !asked.is_empty() { asked } else {
        let reply = reply.map(str::trim).unwrap_or_default();
        reply.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect()
    };
    if detail.is_empty() { "A sessão está esperando uma resposta ou aprovação no chat.".to_owned() }
    else { format!("A sessão está esperando sua resposta: {detail}") }
}

pub fn went_idle(was_working: bool, state: &str) -> bool { was_working && state != "working" }

pub fn send_reply(session: &str, result: &Result<Delivery, String>) -> Value {
    match result {
        Ok(d) if d.ok && d.delivered => tool_reply(format!("status sent: pedido entregue à sessão {session}. Aguarde o resultado real."), true),
        Ok(d) if d.ok => tool_reply(format!("status queued: a sessão {session} está ocupada; o pedido entrou na fila."), true),
        _ => tool_reply(format!("status failed: não foi possível confirmar a entrega à sessão {session}. Não reenvie; peça para o usuário conferir o chat."), false),
    }
}

/// Resposta do `open_session`: `shown` = abriu na tela; `sent` = entrega do pedido ditado junto. `Err` vira falha visível.
pub fn opened_reply(name: &str, shown: bool, sent: Option<&Result<Delivery, String>>) -> Result<String, String> {
    let failed = |result: &Result<Delivery, String>| match result {
        Ok(d) if d.ok => None,
        Ok(_) => Some("a sessão recusou o pedido".to_owned()),
        Err(error) => Some(error.clone()),
    };
    match (shown, sent) {
        (true, None) => Ok(format!("Sessão {name} criada e aberta na tela; a troca já foi anunciada, não repita.")),
        (false, None) => Err(format!("A sessão {name} foi criada, mas a máquina dela não está conectada para abri-la.")),
        (true, Some(result)) => match (failed(result), result) {
            (None, Ok(d)) if d.delivered => Ok(format!("Sessão {name} aberta e pedido enviado. A troca já foi anunciada, não repita; aguarde o resultado real.")),
            (None, _) => Ok(format!("Sessão {name} aberta e pedido enviado; ele entrou na fila da sessão. A troca já foi anunciada, não repita.")),
            (Some(why), _) => Err(format!("Sessão {name} aberta, mas o pedido não chegou a ela: {why}. Não reenvie; peça para o usuário conferir o chat.")),
        },
        (false, Some(result)) => Err(match failed(result) {
            None => format!("Sessão {name} criada e pedido enviado, mas a máquina dela não está conectada para abri-la na tela."),
            Some(why) => format!("Sessão {name} criada, mas não abriu na tela e o pedido não chegou a ela: {why}."),
        }),
    }
}

pub fn triples(events: &[hangar_api::chat::ChatEvent]) -> Vec<(String, String, String)> {
    events.iter().filter(|e| matches!(e.kind.as_str(), "user_msg" | "assistant_msg"))
        .map(|e| (e.id.clone(), e.kind.as_str().to_owned(), e.text.clone().unwrap_or_default())).collect()
}

fn remote_peer(s: &SessionRow) -> bool { s.pair_peers.iter().flatten().any(|p| p.contains("::")) }

/// `canPair` do web. `same_machine`: o grupo é resolvido pelo backend de uma máquina só.
pub fn can_pair(origin: &SessionRow, target: &SessionRow, same_machine: bool) -> Result<(), &'static str> {
    if !same_machine { return Err("Sessões de servidores diferentes não se agrupam por aqui."); }
    if origin.name == target.name { return Err("Não dá pra soltar uma sessão sobre ela mesma."); }
    // O grupo da orquestração é montado pelo orq; agrupar a linha dele à mão desfaz a execução.
    if origin.provider == "orq" || target.provider == "orq" { return Err("A linha do orquestrador não entra nem sai de grupo à mão."); }
    if origin.state == "dead" || target.state == "dead" { return Err("Sessão encerrada — não dá pra agrupar."); }
    if origin.pair_gid.is_some() && origin.pair_gid == target.pair_gid { return Err("Essas sessões já estão no mesmo grupo."); }
    if remote_peer(origin) || remote_peer(target) { return Err("Uma das sessões já está pareada 1:1 com outro servidor."); }
    Ok(())
}

/// `canLeave` do web: os pares também contam, para o par de outro servidor que venha sem `pair_gid`.
pub fn can_leave(s: &SessionRow) -> bool {
    s.provider != "orq" && (s.pair_gid.is_some() || s.pair_peers.as_ref().is_some_and(|p| !p.is_empty()))
}

/// Nome de sessão nova a partir da pasta, como a tela de criação: só ASCII, `-` no resto, sufixo `-2`… se já existe.
// ponytail: acento comum vira a letra base pelo `squash`; o resto vira `-`.
pub fn unique_name(folder: &str, taken: &HashSet<String>) -> String {
    let base = folder.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or_default();
    let clean: String = base.trim().chars().map(|c| match c {
        c if c.is_ascii_alphanumeric() || c == '_' || c == '-' => c,
        c => super::organizer::squash(&c.to_string()).chars().next().filter(char::is_ascii_alphanumeric).unwrap_or('-'),
    }).collect::<String>().trim_matches('-').to_owned();
    let clean = if clean.is_empty() { "sessao".to_owned() } else { clean };
    if !taken.contains(&clean) { return clean; }
    (2..).map(|n| format!("{clean}-{n}")).find(|name| !taken.contains(name)).unwrap_or(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, kind: &str, text: &str) -> (String, String, String) { (id.into(), kind.into(), text.into()) }

    const HCC: [&str; 7] = ["hcc-rust-plano", "hcc-rust-plano-rev14", "hcc-rust-plano-rev17", "hcc-rust-plano-t14", "hcc-rust-plano-t17",
        "jev-settings-ux", "jev-config-ux"];

    #[test]
    fn partial_and_near_names_resolve() {
        let none = [false; 7];
        assert_eq!(match_session("settings UX", &HCC, &none), SessionMatch::One(5));
        assert_eq!(match_session("setting ux", &HCC, &none), SessionMatch::One(5), "transcrição sem o s");
        assert_eq!(match_session("settins", &HCC, &none), SessionMatch::One(5));
        assert_eq!(match_session("config", &HCC, &none), SessionMatch::One(6));
        assert!(matches!(match_session("ux", &HCC, &none), SessionMatch::Many(found) if found == vec![5, 6]), "as duas têm ux: pergunta");
        assert_eq!(match_session("banana", &HCC, &none), SessionMatch::None);
    }

    /// "HCC" casou com cinco sessões e a troca parou; a raiz da família é a pretendida.
    #[test]
    fn prefix_family_picks_its_root_unless_context_says_otherwise() {
        let none = [false; 7];
        let SessionMatch::Many(found) = match_session("HCC", &HCC, &none) else { panic!("cinco sessões casam") };
        assert_eq!(found.len(), 5);
        let subset: Vec<&str> = found.iter().map(|&i| HCC[i]).collect();
        assert_eq!(narrow_by_context(found.clone(), &subset, &[false; 5]), SessionMatch::One(0), "sem contexto: a raiz");
        assert_eq!(narrow_by_context(found.clone(), &subset, &[false, false, false, true, false]), SessionMatch::One(3),
            "a executora t14 citada na conversa vence a raiz");
        assert!(matches!(narrow_by_context(found.clone(), &subset, &[false, true, false, true, false]), SessionMatch::Many(_)),
            "duas no contexto, nenhuma raiz delas: pergunta");
        assert_eq!(narrow_by_context(found, &subset, &[true, false, false, true, false]), SessionMatch::One(0),
            "duas no contexto, uma é raiz da outra: a raiz");
    }

    #[test]
    fn real_ambiguity_still_asks() {
        let names = ["hangar", "hangar", "voz-entendimento"];
        let SessionMatch::Many(found) = match_session("hangar", &names, &[false, false, false]) else { panic!("mesmo nome em duas máquinas") };
        let subset: Vec<&str> = found.iter().map(|&i| names[i]).collect();
        assert!(matches!(narrow_by_context(found.clone(), &subset, &[false, false]), SessionMatch::Many(_)), "sem contexto: pergunta");
        assert_eq!(narrow_by_context(found, &subset, &[false, true]), SessionMatch::One(1), "a que estava na conversa");
        let jev = ["jev-settings-ux", "jev-config-ux"];
        let SessionMatch::Many(found) = match_session("jev", &jev, &[false, false]) else { panic!("as duas são jev") };
        assert!(matches!(narrow_by_context(found, &jev, &[false, false]), SessionMatch::Many(_)), "irmãs, sem raiz: pergunta");
    }

    #[test]
    fn open_with_request_answers_only_after_delivery() {
        let delivered = Ok(Delivery { ok: true, delivered: true });
        let queued = Ok(Delivery { ok: true, delivered: false });
        let failed: Result<Delivery, String> = Err("x".into());
        assert!(opened_reply("s", true, None).unwrap().contains("criada e aberta"), "sem request, como antes");
        assert!(opened_reply("s", false, None).is_err());
        assert!(opened_reply("s", true, Some(&delivered)).unwrap().starts_with("Sessão s aberta e pedido enviado"));
        assert!(opened_reply("s", true, Some(&queued)).unwrap().contains("fila"));
        let both = opened_reply("s", true, Some(&failed)).unwrap_err();
        assert!(both.contains("aberta") && both.contains("não chegou"), "diz as duas coisas");
        assert!(opened_reply("s", true, Some(&Ok(Delivery { ok: false, delivered: false }))).is_err());
        assert!(opened_reply("s", false, Some(&delivered)).unwrap_err().contains("pedido enviado"));
    }

    #[test]
    fn session_match_prefers_exact_and_ignores_separators() {
        let names = ["hangar", "hangar-5", "shop-web", "Shop_Api"];
        let active = [true; 4];
        assert_eq!(match_session("hangar", &names, &active), SessionMatch::One(0), "igual vence parcial");
        assert_eq!(match_session("shop web", &names, &active), SessionMatch::One(2));
        assert_eq!(match_session("SHOP-API", &names, &active), SessionMatch::One(3), "caixa e traço não contam");
        assert_eq!(match_session("hangar 5", &names, &active), SessionMatch::One(1));
        assert_eq!(match_session("shop", &names, &active), SessionMatch::Many(vec![2, 3]));
        assert_eq!(match_session("cloudflare", &names, &active), SessionMatch::None);
        assert_eq!(match_session(" - ", &names, &active), SessionMatch::None, "consulta vazia não casa tudo");
        let names = ["grupos-rust-plano", "rust-parte5-claude", "gpt-sol"];
        let active = [true; 3];
        assert_eq!(match_session("grupos", &names, &active), SessionMatch::One(0), "pedaço do nome");
        assert_eq!(match_session("rust grupos", &names, &active), SessionMatch::One(0), "palavras em qualquer ordem");
        assert_eq!(match_session("plano do rust", &names, &active), SessionMatch::One(0), "palavra curta não conta");
        assert_eq!(match_session("plano do claude", &names, &active), SessionMatch::None, "palavra que não está no nome não casa");
        assert_eq!(match_session("rust", &names, &active), SessionMatch::Many(vec![0, 1]));
    }

    #[test]
    fn folder_pick_prefers_exact_and_lists_ambiguity() {
        let f = |n: &str, p: &str| (n.to_owned(), p.to_owned());
        let folders = [f("Projetos", "/h/Projetos"), f("hangar", "/h/Projetos/hangar"), f("hangar-5", "/h/Projetos/hangar-5"),
            f("hangar", "/h/Projetos/hangar"), f("shop-web", "/h/Projetos/shop-web"), f("shop-api", "/h/Work/shop-api")];
        assert_eq!(pick_folder("hangar", &folders, 0), Ok("/h/Projetos/hangar".into()), "igual vence parcial; caminho repetido conta uma vez");
        assert_eq!(pick_folder("shop web", &folders, 0), Ok("/h/Projetos/shop-web".into()));
        let many = pick_folder("shop", &folders, 0).unwrap_err();
        assert!(many.contains("/h/Projetos/shop-web") && many.contains("/h/Work/shop-api"));
        assert!(pick_folder("loja", &folders, 2).unwrap_err().contains("2 raiz"), "raiz não lida aparece");
        let twins = [f("api", "/a/api"), f("api", "/b/api")];
        assert!(pick_folder("api", &twins, 0).is_err(), "mesmo nome em duas raízes não é palpite");
    }

    #[test]
    fn paths_skip_the_folder_search() {
        for path in ["/home/x/p", "~/p", "C:\\Users\\x", "D:/w", "\\\\nas\\share"] { assert!(looks_like_path(path), "{path}"); }
        for name in ["hangar", "minha loja", "a:b"] { assert!(!looks_like_path(name), "{name}"); }
    }

    #[test]
    fn sessions_text_marks_screen_machine_and_unreachable() {
        let row = |name: &str, machine: Option<&str>, on_screen: bool| Listed { name: name.into(), machine: machine.map(str::to_owned),
            provider: "codex".into(), state: "working".into(), folder: "hangar".into(), on_screen, followed: !on_screen };
        let text = sessions_text(&[row("a", None, true), row("b", Some("casa"), false)], &["vps".into()]);
        assert_eq!(text, "- a: codex, working, pasta hangar, na tela\n- b (máquina casa): codex, working, pasta hangar, acompanhada\n\
            Máquina vps sem resposta; as sessões dela não estão aqui.");
        assert_eq!(sessions_text(&[], &[]), "Nenhuma sessão aberta.");
    }

    /// Duas `hangar`: a do Notebook (máquina ativa) e a do PC `jefferson-felizardo`.
    fn two_hangars() -> ([&'static str; 3], Vec<Vec<String>>, [bool; 3]) {
        let notebook = machine_aliases("http://127.0.0.1:8765", "Notebook");
        let pc = machine_aliases("https://jefferson-felizardo.tailcac351.ts.net", "jefferson-felizardo");
        (["hangar", "hangar", "pm18920-api"], vec![notebook.clone(), pc, notebook], [true, false, true])
    }

    #[test]
    fn same_session_name_on_two_machines_resolves_by_machine() {
        let (names, machines, active) = two_hangars();
        let pick = |spoken: &str| match_qualified(spoken, &names, &machines, &active);
        assert_eq!(pick("hangar"), SessionMatch::One(0), "sem máquina: a da máquina ativa, como antes");
        assert_eq!(pick("Hangar lá no PC Jefferson-Felizardo"), SessionMatch::One(1));
        assert_eq!(pick("jefferson-felizardo::hangar"), SessionMatch::One(1));
        assert_eq!(pick("hangar (máquina jefferson-felizardo)"), SessionMatch::One(1));
        assert_eq!(pick("hangar em jefferson-felizardo"), SessionMatch::One(1), "o formato que a pergunta de ambiguidade mostra");
        assert_eq!(pick("hangar no notebook"), SessionMatch::One(0));
        assert_eq!(pick("vps::hangar"), SessionMatch::None, "máquina dita e inexistente não cai noutra");
        assert_eq!(pick("pm18920 no PC jefferson-felizardo"), SessionMatch::None, "a sessão não está naquela máquina");
        // Sem preferência pela ativa, o nome sozinho continua ambíguo e quem chama pergunta.
        assert_eq!(match_qualified("hangar", &names, &machines, &[false, false, false]), SessionMatch::Many(vec![0, 1]));
    }

    #[test]
    fn machine_aliases_take_label_and_host() {
        assert_eq!(machine_aliases("https://jefferson-felizardo.tailcac351.ts.net", "PC"),
            vec!["jeffersonfelizardotailcac351tsnet".to_owned(), "jeffersonfelizardo".to_owned()], "rótulo curto demais fica de fora");
    }

    #[test]
    fn switch_offer_takes_the_yes_for_an_offered_session_only() {
        let now = std::time::Instant::now();
        let mut offer = SwitchOffer::default();
        assert!(!offer.take("pc", "hangar", now), "sem pergunta, nada vale");
        offer.offer(vec![("notebook".into(), "hangar".into()), ("pc".into(), "hangar".into())], now);
        assert!(!offer.take("pc", "outra", now), "só as oferecidas");
        assert!(offer.take("pc", "hangar", now + Duration::from_secs(5)), "o 'isso' depois da pergunta");
        assert!(!offer.take("notebook", "hangar", now + Duration::from_secs(6)), "usada, acabou");
        offer.offer(vec![("pc".into(), "hangar".into())], now);
        assert!(!offer.take("pc", "hangar", now + SWITCH_OFFER_WINDOW), "fora da janela não vale");
    }

    #[test]
    fn switch_offers_a_summary_unless_the_voice_talked_about_it_recently() {
        let (mut talked, now) = (HashMap::new(), std::time::Instant::now());
        let pc = ("delphi-02".to_owned(), "PWorkStation".to_owned());
        assert!(offer_summary(&mut talked, pc.clone(), now), "primeira vez: pergunta");
        assert!(!offer_summary(&mut talked, pc.clone(), now + Duration::from_secs(60)), "vai e volta: não pergunta de novo");
        assert!(offer_summary(&mut talked, ("notebook".into(), "voz".into()), now), "outra sessão: pergunta");
        assert!(offer_summary(&mut talked, pc, now + TALKED_WINDOW), "passado o tempo, pergunta de novo");
    }

    #[test]
    fn jev_acts_only_with_a_sure_target_and_says_why_not() {
        use crate::voice::jev::{Decision, Intent};
        let d = |intent: Intent, ip: f64, session: Option<(usize, f64)>, action: Option<(usize, f64)>| Decision { intent: Some((intent, ip)), session, action };
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 0.74, Some((14, 0.98)), None)), Ok(14), "'volta pra voz': destino certo basta com troca provável");
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 0.91, Some((18, 0.94)), None)), Ok(18), "PWorkStation em outra máquina");
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 0.89, Some((14, 0.58)), None)), Err("low-session"));
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 0.98, Some((14, 0.89)), None)), Ok(14), "'vai pro voz' com destino 0,89 agora troca");
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 1.0, Some((14, 0.63)), None)), Err("low-session"), "abaixo de 0,65 segue pelo organizador");
        assert_eq!(jev_switch_choice(&d(Intent::Switch, 0.64, Some((14, 0.99)), None)), Err("low-intent"));
        assert_eq!(jev_switch_choice(&d(Intent::Talk, 0.9, Some((1, 0.99)), None)), Err("not-switch"));
        assert_eq!(jev_switch_choice(&Decision::default()), Err("not-switch"), "sem resposta");
        assert_eq!(jev_action_choice(&d(Intent::Screen, 1.0, None, Some((10, 0.95)))), Ok(10));
        assert_eq!(jev_action_choice(&d(Intent::Screen, 0.95, None, Some((10, 0.6)))), Err("low-action"));
        assert_eq!(jev_action_choice(&d(Intent::Screen, 0.6, None, Some((10, 0.99)))), Err("low-intent"), "ação de tela exige as duas certezas");
        assert_eq!(jev_action_choice(&d(Intent::Screen, 0.75, None, Some((10, 0.72)))), Ok(10));
    }

    #[test]
    fn session_match_folds_accents() {
        assert_eq!(match_session("sao", &["são-x", "outra"], &[true, true]), SessionMatch::One(0));
        assert_eq!(match_session("AÇÃO", &["acao"], &[true]), SessionMatch::One(0));
    }

    #[test]
    fn session_match_same_name_prefers_active_machine() {
        let names = ["api", "api", "api-docs"];
        assert_eq!(match_session("api", &names, &[false, true, true]), SessionMatch::One(1));
        assert_eq!(match_session("api", &names, &[false, false, true]), SessionMatch::Many(vec![0, 1]));
        assert_eq!(match_session("api", &names, &[true, true, true]), SessionMatch::Many(vec![0, 1]));
    }

    #[test]
    fn question_text_is_marked_and_bounded() {
        let text = question_text("Qual banco vocês usam?");
        assert!(text.starts_with("[Pergunta da conversa de voz] Qual banco vocês usam?"));
        assert!(text.contains("não execute"));
    }

    #[test]
    fn ask_session_times_out() {
        let now = std::time::Instant::now();
        assert!(question_expired(now - ASK_TIMEOUT - Duration::from_secs(1), now));
        assert!(!question_expired(now, now));
    }

    #[test]
    fn question_answer_follows_the_marked_message() {
        let q = question_text("Qual banco?");
        let events = vec![ev("1", "user_msg", "outro pedido"), ev("2", "assistant_msg", "feito"), ev("3", "user_msg", &q)];
        assert_eq!(question_answer(&events), None, "ainda sem resposta; o turno anterior não conta");
        assert!(question_marked(&events), "marca sem resposta é espera, não outro pedido");
        assert!(!question_marked(&events[..2]));
        let mut events = events;
        events.extend([ev("4", "assistant_msg", "vou ler"), ev("5", "assistant_msg", "Postgres"), ev("6", "user_msg", "depois")]);
        assert_eq!(question_answer(&events), Some(("5".into(), "vou ler\n\nPostgres".into())));
    }

    #[test]
    fn last_reply_is_after_last_user_message() {
        let events = vec![ev("1", "user_msg", "a"), ev("2", "assistant_msg", "velha"), ev("3", "user_msg", "b"),
            ev("4", "assistant_msg", "nova 1"), ev("5", "assistant_msg", "nova 2")];
        assert_eq!(last_reply(&events), Some(("5".into(), "nova 1\n\nnova 2".into())));
        assert_eq!(last_reply(&[ev("1", "user_msg", "só pergunta")]), None);
    }

    #[test]
    fn equal_texts_with_different_ids_are_both_spoken() {
        // Dedup é por id: "Pronto." duas vezes em turnos diferentes fala duas vezes.
        let first = last_reply(&[ev("1", "user_msg", "a"), ev("2", "assistant_msg", "Pronto.")]).unwrap();
        let second = last_reply(&[ev("3", "user_msg", "b"), ev("4", "assistant_msg", "Pronto.")]).unwrap();
        assert_ne!(first.0, second.0);
    }

    #[test]
    fn send_reply_names_session_and_status() {
        let ok = send_reply("demo-session", &Ok(Delivery { ok: true, delivered: true }));
        let text = ok["contentItems"][0]["text"].as_str().unwrap();
        assert!(text.contains("demo-session") && text.contains("sent"));
        let queued = send_reply("demo-session", &Ok(Delivery { ok: true, delivered: false }));
        assert!(queued["contentItems"][0]["text"].as_str().unwrap().contains("queued"));
    }

    #[test]
    fn final_reply_after_intermediate_text_is_a_new_id() {
        // O passo do meio já falado não cobre a resposta final: ela tem outro id e ainda é falada.
        let middle = last_reply(&[ev("1", "user_msg", "a"), ev("2", "assistant_msg", "Vou ler o arquivo…")]).unwrap();
        let end = last_reply(&[ev("1", "user_msg", "a"), ev("2", "assistant_msg", "Vou ler o arquivo…"), ev("3", "assistant_msg", "Pronto, corrigi.")]).unwrap();
        assert_ne!(middle.0, end.0);
        assert!(end.1.ends_with("Pronto, corrigi."));
    }

    #[test]
    fn waiting_text_prefers_question_then_reply_tail() {
        assert_eq!(waiting_text(&["Qual branch?"], Some("texto")), "A sessão está esperando sua resposta: Qual branch?");
        assert_eq!(waiting_text(&[" "], Some("Posso apagar o arquivo?")), "A sessão está esperando sua resposta: Posso apagar o arquivo?");
        let long = format!("{}fim", "x".repeat(900));
        let text = waiting_text(&[], Some(&long));
        assert!(text.ends_with("fim") && text.chars().count() < 660);
        assert!(waiting_text(&[], None).contains("no chat"));
    }

    #[test]
    fn went_idle_only_from_working() {
        assert!(went_idle(true, "idle"));
        assert!(went_idle(true, "awaiting_input"));
        assert!(!went_idle(false, "idle"));
        assert!(!went_idle(true, "working"));
    }

    #[test]
    fn new_names_and_group_refusals() {
        let taken: HashSet<String> = ["minha-loja".to_owned()].into();
        assert_eq!(unique_name("/home/x/Minha Lojá/", &HashSet::new()), "Minha-Loja");
        assert_eq!(unique_name("C:\\code\\minha-loja", &taken), "minha-loja-2");
        assert_eq!(unique_name("///", &HashSet::new()), "sessao");
        let row = |name: &str, provider: &str, gid: Option<&str>| serde_json::from_value::<SessionRow>(
            serde_json::json!({"name": name, "provider": provider, "pair_gid": gid})).unwrap();
        assert!(can_pair(&row("a", "claude", None), &row("b", "codex", None), true).is_ok());
        assert!(can_pair(&row("a", "claude", None), &row("b", "codex", None), false).is_err());
        assert!(can_pair(&row("a", "claude", Some("g")), &row("b", "codex", Some("g")), true).is_err());
        assert!(can_pair(&row("a", "orq", None), &row("b", "codex", None), true).is_err());
        assert!(can_leave(&row("a", "claude", Some("g"))));
        assert!(!can_leave(&row("a", "claude", None)));
        assert!(!can_leave(&row("a", "orq", Some("g"))));
    }
}

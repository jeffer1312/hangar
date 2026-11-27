//! O registro único de toda continuação automática da sessão, de qualquer origem (queda passageira,
//! reboot, limite de uso, login recusado, o botão da falta de memória). Os tetos valem juntos: é o que
//! segura o laço em que o mesmo "continue" ia à sessão várias vezes em segundos. Fica em disco para
//! que reiniciar o servidor não zere os tetos.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::classify::StopKind;
use crate::scheduler::StopHit;

/// Continuações seguidas sem uma resposta real do modelo entre elas.
pub const CHAIN_MAX: usize = 2;
/// Duas continuações mais afastadas que isso não formam corrente.
pub const CHAIN_RESET_S: f64 = 1800.0;
/// Continuações por sessão na janela de uma hora.
pub const HOURLY_MAX: usize = 3;
pub const HOURLY_WINDOW_S: f64 = 3600.0;
/// Provedor fora do ar não melhora com insistência: uma por corrente.
pub const SERVER_5XX_MAX: usize = 1;
/// Por quanto tempo uma chave já continuada segue barrada. Parada com mais de 2 h não continua, então
/// uma semana cobre de sobra a mesma linha de erro sendo vista de novo.
const KEY_KEEP_S: f64 = 7.0 * 24.0 * 3600.0;
/// Paradas encerradas lembradas por sessão: só serve à mesma linha reemitida logo depois.
const ENDED_MAX: usize = 32;
/// Os códigos que a agenda leva ao diário (`take_problem`).
pub const UNREADABLE: &str = "auto_continue_ledger_unreadable";
pub const SAVE_FAILED: &str = "auto_continue_ledger_save_failed";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Transient,
    Reboot,
    LimitSwitch,
    LoginRetry,
    OomButton,
}

/// O teto que barrou a continuação; o `code` vai ao diário como `auto_continue_capped:<teto>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cap {
    Once,
    Chain,
    Hourly,
    #[serde(rename = "server_5xx")]
    Server5xx,
}

impl Cap {
    pub fn code(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Chain => "chain",
            Self::Hourly => "hourly",
            Self::Server5xx => "server_5xx",
        }
    }
}

pub fn path(home: &Path) -> PathBuf {
    home.join(".hangar").join("auto-continue.json")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Sent {
    at: f64,
    source: Source,
    kind: StopKind,
    key: String,
}

/// A parada que espera a rede, gravada para que reiniciar o servidor não deixe a nota de espera sem desfecho.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Waiting {
    pub hit: StopHit,
    /// Trocas da nota já tentadas.
    pub count: u64,
    /// A nota de espera chegou à conversa.
    pub note_out: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Ended {
    key: String,
    at: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Entry {
    /// A corrente atual; a resposta real do modelo a esvazia.
    #[serde(default)]
    chain: Vec<Sent>,
    /// Todas as continuações dentro de `KEY_KEEP_S`: servem à chave já usada e à janela da hora.
    #[serde(default)]
    sent: Vec<Sent>,
    /// Paradas com desfecho (continuada, cancelada, descartada): a mesma linha vista de novo não volta a
    /// esperar nem ganha outra nota.
    #[serde(default)]
    ended: Vec<Ended>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    waiting: Option<Waiting>,
    /// A chave da espera que o Cancelar da nota marcou; a agenda confere antes de mandar e troca a nota.
    /// Gravada: reiniciar entre o Cancelar e a troca não pode mandar o continue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancelled: Option<String>,
}

impl Entry {
    fn live_chain(&self, now: f64) -> &[Sent] {
        match self.chain.last() {
            Some(last) if now - last.at <= CHAIN_RESET_S => self.chain.as_slice(),
            _ => &[],
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Doc {
    #[serde(default)]
    sessions: BTreeMap<String, Entry>,
}

#[derive(Debug)]
pub struct Ledger {
    path: PathBuf,
    doc: Doc,
    /// A última falha de leitura ou gravação, até a agenda a levar ao diário.
    problem: Option<&'static str>,
}

impl Ledger {
    /// Arquivo ausente é registro vazio. Ilegível também, com aviso no log e no diário (`take_problem`):
    /// travar a continuação por um arquivo estragado deixaria a sessão parada. O estragado fica ao lado,
    /// como `.bad`, antes que a gravação seguinte o cubra.
    pub fn load(path: PathBuf) -> Self {
        let mut problem = None;
        let doc = match hangar_core::fs::read_replaced(&path) {
            Ok(raw) => serde_json::from_slice(&raw).unwrap_or_else(|error| {
                tracing::warn!(path = %path.display(), %error, "registro de continuação ilegível; recomeça vazio");
                let bad = path.with_extension("json.bad");
                if let Err(error) = std::fs::rename(&path, &bad) {
                    tracing::warn!(path = %bad.display(), %error, "registro de continuação ilegível não foi guardado à parte");
                }
                problem = Some(UNREADABLE);
                Doc::default()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Doc::default(),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "registro de continuação sem leitura; recomeça vazio");
                problem = Some(UNREADABLE);
                Doc::default()
            }
        };
        Self { path, doc, problem }
    }

    /// A falha guardada desde a última consulta; a agenda a leva ao diário.
    pub fn take_problem(&mut self) -> Option<&'static str> {
        self.problem.take()
    }

    /// Pode continuar? Não grava nada: quem continua de fato chama `record` depois de mandar o texto.
    pub fn allow(&mut self, session: &str, _source: Source, key: &str, kind: StopKind, now: f64) -> Result<(), Cap> {
        let Some(entry) = self.doc.sessions.get(session) else { return Ok(()) };
        if entry.sent.iter().any(|sent| sent.key == key && now - sent.at < KEY_KEEP_S) {
            return Err(Cap::Once);
        }
        let chain = entry.live_chain(now);
        if chain.len() >= CHAIN_MAX {
            return Err(Cap::Chain);
        }
        if kind == StopKind::Server5xx && chain.iter().filter(|sent| sent.kind == StopKind::Server5xx).count() >= SERVER_5XX_MAX {
            return Err(Cap::Server5xx);
        }
        if entry.sent.iter().filter(|sent| now - sent.at < HOURLY_WINDOW_S).count() >= HOURLY_MAX {
            return Err(Cap::Hourly);
        }
        Ok(())
    }

    pub fn record(&mut self, session: &str, source: Source, key: &str, kind: StopKind, now: f64) {
        let entry = self.doc.sessions.entry(session.to_owned()).or_default();
        entry.sent.retain(|sent| now - sent.at < KEY_KEEP_S);
        if entry.live_chain(now).is_empty() {
            entry.chain.clear();
        }
        let sent = Sent { at: now, source, kind, key: key.to_owned() };
        entry.chain.push(sent.clone());
        entry.sent.push(sent);
        self.save();
    }

    /// O modelo respondeu de verdade: a corrente zera. A janela da hora e as chaves seguem valendo.
    pub fn real_reply(&mut self, session: &str) {
        let Some(entry) = self.doc.sessions.get_mut(session) else { return };
        if entry.chain.is_empty() {
            return;
        }
        entry.chain.clear();
        self.save();
    }

    /// A pessoa escreveu: as continuações antes da mensagem dela não são seguidas. A janela da hora e as
    /// chaves seguem valendo.
    pub fn person_wrote(&mut self, session: &str) {
        self.real_reply(session);
    }

    /// Grava a parada que espera a rede; a da sessão, se havia, é trocada.
    pub fn park(&mut self, session: &str, waiting: Waiting) {
        self.doc.sessions.entry(session.to_owned()).or_default().waiting = Some(waiting);
        self.save();
    }

    /// As paradas que esperavam quando o registro foi gravado, com o nome atual da sessão.
    pub fn parked(&self) -> Vec<Waiting> {
        self.doc.sessions.iter().filter_map(|(session, entry)| {
            let mut waiting = entry.waiting.clone()?;
            // Renomear move a entrada inteira; o nome dentro da parada é o de antes.
            waiting.hit.name.clone_from(session);
            Some(waiting)
        }).collect()
    }

    pub fn has_ended(&self, session: &str, key: &str, now: f64) -> bool {
        self.doc.sessions.get(session)
            .is_some_and(|entry| entry.ended.iter().any(|ended| ended.key == key && now - ended.at < KEY_KEEP_S))
    }

    /// O Cancelar da nota, marcado na hora, sem esperar a agenda, que pode estar ocupada com outro envio.
    /// `false`: nada esperava.
    pub fn cancel(&mut self, session: &str) -> bool {
        let Some(entry) = self.doc.sessions.get_mut(session) else { return false };
        let Some(key) = entry.waiting.as_ref().map(|waiting| waiting.hit.key()) else { return false };
        if entry.cancelled.as_deref() != Some(key.as_str()) {
            entry.cancelled = Some(key);
            self.save();
        }
        true
    }

    pub fn is_cancelled(&self, session: &str, key: &str) -> bool {
        self.doc.sessions.get(session).is_some_and(|entry| entry.cancelled.as_deref() == Some(key))
    }

    /// A parada teve desfecho: sai da espera gravada e a mesma chave não espera de novo.
    pub fn end(&mut self, session: &str, key: &str, now: f64) {
        let entry = self.doc.sessions.entry(session.to_owned()).or_default();
        if entry.waiting.as_ref().is_some_and(|waiting| waiting.hit.key() == key) {
            entry.waiting = None;
        }
        if entry.cancelled.as_deref() == Some(key) {
            entry.cancelled = None;
        }
        entry.ended.retain(|ended| ended.key != key && now - ended.at < KEY_KEEP_S);
        entry.ended.push(Ended { key: key.to_owned(), at: now });
        let extra = entry.ended.len().saturating_sub(ENDED_MAX);
        entry.ended.drain(..extra);
        self.save();
    }

    /// Só ao fechar a sessão: outra com o mesmo nome nasce sem tetos.
    pub fn forget(&mut self, session: &str) {
        if self.doc.sessions.remove(session).is_some() {
            self.save();
        }
    }

    /// Renomear não pode servir de saída dos tetos.
    pub fn rename(&mut self, old: &str, new: &str) {
        if old == new {
            return;
        }
        let Some(entry) = self.doc.sessions.remove(old) else { return };
        self.doc.sessions.insert(new.to_owned(), entry);
        self.save();
    }

    /// Falha na gravação fica no log e no diário (`take_problem`); o registro em memória segue valendo até o
    /// servidor parar.
    fn save(&mut self) {
        if let Err(error) = self.write() {
            tracing::warn!(path = %self.path.display(), %error, "registro de continuação não gravado");
            self.problem = Some(SAVE_FAILED);
        }
    }

    fn write(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(&self.doc).map_err(io::Error::other)?;
        hangar_core::fs::atomic_write(&self.path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> (tempfile::TempDir, Ledger) {
        let home = tempfile::tempdir().unwrap();
        let ledger = Ledger::load(path(home.path()));
        (home, ledger)
    }

    /// Pergunta e, se pode, registra: o que o chamador faz a cada parada.
    fn try_continue(ledger: &mut Ledger, source: Source, key: &str, kind: StopKind, now: f64) -> Result<(), Cap> {
        ledger.allow("s", source, key, kind, now)?;
        ledger.record("s", source, key, kind, now);
        Ok(())
    }

    #[test]
    fn the_same_error_line_continues_once() {
        let (_home, mut ledger) = ledger();
        assert_eq!(try_continue(&mut ledger, Source::Transient, "line-1", StopKind::Network, 100.0), Ok(()));
        ledger.real_reply("s");
        assert_eq!(ledger.allow("s", Source::Transient, "line-1", StopKind::Network, 200.0), Err(Cap::Once));
        assert_eq!(ledger.allow("s", Source::Reboot, "line-1", StopKind::Other, 200.0), Err(Cap::Once));
        assert_eq!(ledger.allow("s", Source::Transient, "line-2", StopKind::Network, 200.0), Ok(()));
        assert_eq!(ledger.allow("other", Source::Transient, "line-1", StopKind::Network, 200.0), Ok(()));
    }

    #[test]
    fn a_chain_stops_at_two_without_a_real_reply() {
        let (_home, mut ledger) = ledger();
        assert_eq!(try_continue(&mut ledger, Source::Transient, "a", StopKind::Network, 0.0), Ok(()));
        assert_eq!(try_continue(&mut ledger, Source::Transient, "b", StopKind::StreamLost, 60.0), Ok(()));
        assert_eq!(try_continue(&mut ledger, Source::Transient, "c", StopKind::Network, 120.0), Err(Cap::Chain));
        assert_eq!(try_continue(&mut ledger, Source::LimitSwitch, "d", StopKind::Other, 180.0), Err(Cap::Chain));
    }

    #[test]
    fn a_real_reply_resets_the_chain() {
        let (_home, mut ledger) = ledger();
        try_continue(&mut ledger, Source::Transient, "a", StopKind::Network, 0.0).unwrap();
        try_continue(&mut ledger, Source::Transient, "b", StopKind::Network, 60.0).unwrap();
        assert_eq!(ledger.allow("s", Source::Transient, "c", StopKind::Network, 120.0), Err(Cap::Chain));
        ledger.real_reply("s");
        assert_eq!(ledger.allow("s", Source::Transient, "c", StopKind::Network, 120.0), Ok(()));
        // Resposta de sessão sem registro não cria nada.
        ledger.real_reply("nobody");
        assert_eq!(ledger.allow("nobody", Source::Transient, "a", StopKind::Network, 120.0), Ok(()));
    }

    #[test]
    fn a_chain_expires_after_thirty_minutes() {
        let (_home, mut ledger) = ledger();
        try_continue(&mut ledger, Source::Transient, "a", StopKind::Network, 0.0).unwrap();
        try_continue(&mut ledger, Source::Transient, "b", StopKind::Network, 100.0).unwrap();
        assert_eq!(ledger.allow("s", Source::Transient, "c", StopKind::Network, 100.0 + CHAIN_RESET_S), Err(Cap::Chain));
        let later = 100.0 + CHAIN_RESET_S + 1.0;
        assert_eq!(try_continue(&mut ledger, Source::Transient, "c", StopKind::Network, later), Ok(()));
        // A corrente nova tem lugar, mas a hora já tem três.
        assert_eq!(try_continue(&mut ledger, Source::Transient, "d", StopKind::Network, later + 60.0), Err(Cap::Hourly));
        let next_hour = later + HOURLY_WINDOW_S;
        assert_eq!(try_continue(&mut ledger, Source::Transient, "d", StopKind::Network, next_hour), Ok(()));
        assert_eq!(try_continue(&mut ledger, Source::Transient, "e", StopKind::Network, next_hour + 10.0), Ok(()));
        assert_eq!(ledger.allow("s", Source::Transient, "f", StopKind::Network, next_hour + 20.0), Err(Cap::Chain));
    }

    #[test]
    fn three_per_hour_count_every_source() {
        let (_home, mut ledger) = ledger();
        try_continue(&mut ledger, Source::Transient, "a", StopKind::Network, 0.0).unwrap();
        ledger.real_reply("s");
        try_continue(&mut ledger, Source::Reboot, "b", StopKind::Other, 600.0).unwrap();
        ledger.real_reply("s");
        try_continue(&mut ledger, Source::LimitSwitch, "c", StopKind::Other, 1200.0).unwrap();
        ledger.real_reply("s");
        for source in [Source::Transient, Source::Reboot, Source::LimitSwitch, Source::LoginRetry, Source::OomButton] {
            assert_eq!(ledger.allow("s", source, "d", StopKind::Network, 1800.0), Err(Cap::Hourly), "{source:?}");
        }
        assert_eq!(ledger.allow("s", Source::LoginRetry, "d", StopKind::Other, HOURLY_WINDOW_S + 1.0), Ok(()));
    }

    #[test]
    fn a_server_5xx_continues_once_per_chain() {
        let (_home, mut ledger) = ledger();
        try_continue(&mut ledger, Source::Transient, "a", StopKind::Server5xx, 0.0).unwrap();
        assert_eq!(ledger.allow("s", Source::Transient, "b", StopKind::Server5xx, 30.0), Err(Cap::Server5xx));
        assert_eq!(ledger.allow("s", Source::Transient, "b", StopKind::Network, 30.0), Ok(()));
        ledger.real_reply("s");
        assert_eq!(ledger.allow("s", Source::Transient, "c", StopKind::Server5xx, 60.0), Ok(()));
    }

    #[test]
    fn the_ledger_survives_a_reload() {
        let home = tempfile::tempdir().unwrap();
        let file = path(home.path());
        let mut ledger = Ledger::load(file.clone());
        try_continue(&mut ledger, Source::Transient, "a", StopKind::Network, 0.0).unwrap();
        try_continue(&mut ledger, Source::LimitSwitch, "b", StopKind::Other, 10.0).unwrap();
        drop(ledger);
        assert!(file.exists());
        let mut ledger = Ledger::load(file.clone());
        assert_eq!(ledger.allow("s", Source::Transient, "c", StopKind::Network, 20.0), Err(Cap::Chain));
        assert_eq!(ledger.allow("s", Source::Transient, "a", StopKind::Network, 20.0), Err(Cap::Once));
        ledger.real_reply("s");
        let mut ledger = Ledger::load(file.clone());
        assert_eq!(ledger.allow("s", Source::Transient, "c", StopKind::Network, 20.0), Ok(()));
        ledger.forget("s");
        let mut ledger = Ledger::load(file.clone());
        assert_eq!(ledger.allow("s", Source::Transient, "a", StopKind::Network, 20.0), Ok(()));
        // Arquivo estragado recomeça vazio em vez de travar.
        std::fs::write(&file, b"{nao e json").unwrap();
        let mut ledger = Ledger::load(file);
        assert_eq!(ledger.allow("s", Source::Transient, "a", StopKind::Network, 20.0), Ok(()));
    }

    #[test]
    fn a_broken_ledger_is_kept_aside_and_reported() {
        let home = tempfile::tempdir().unwrap();
        let file = path(home.path());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"{nao e json").unwrap();
        let mut ledger = Ledger::load(file.clone());
        assert_eq!(ledger.take_problem(), Some(UNREADABLE));
        assert_eq!(ledger.take_problem(), None, "vai ao diário uma vez");
        assert_eq!(std::fs::read(file.with_extension("json.bad")).unwrap(), b"{nao e json");
        assert!(!file.exists());
        assert_eq!(Ledger::load(file).take_problem(), None, "ausente não é falha");
    }

    #[test]
    fn a_failed_save_is_reported() {
        let home = tempfile::tempdir().unwrap();
        // Um arquivo no lugar da pasta: a gravação não tem onde criar o registro.
        std::fs::write(home.path().join(".hangar"), b"x").unwrap();
        let mut ledger = Ledger::load(path(home.path()));
        let _ = ledger.take_problem();
        ledger.record("s", Source::Transient, "a", StopKind::Network, 0.0);
        assert_eq!(ledger.take_problem(), Some(SAVE_FAILED));
        assert_eq!(ledger.allow("s", Source::Transient, "a", StopKind::Network, 10.0), Err(Cap::Once), "a memória segue valendo");
    }

    #[test]
    fn a_rename_moves_the_caps() {
        let home = tempfile::tempdir().unwrap();
        let file = path(home.path());
        let mut ledger = Ledger::load(file.clone());
        ledger.record("old", Source::Transient, "a", StopKind::Network, 0.0);
        ledger.record("old", Source::Transient, "b", StopKind::Network, 10.0);
        ledger.rename("old", "new");
        assert_eq!(ledger.allow("new", Source::Transient, "c", StopKind::Network, 20.0), Err(Cap::Chain));
        assert_eq!(ledger.allow("old", Source::Transient, "c", StopKind::Network, 20.0), Ok(()));
        let mut ledger = Ledger::load(file);
        assert_eq!(ledger.allow("new", Source::Transient, "a", StopKind::Network, 20.0), Err(Cap::Once));
    }

    #[test]
    fn a_person_message_breaks_the_chain_but_not_the_hour() {
        let (_home, mut ledger) = ledger();
        try_continue(&mut ledger, Source::LoginRetry, "a", StopKind::Other, 0.0).unwrap();
        try_continue(&mut ledger, Source::LoginRetry, "b", StopKind::Other, 10.0).unwrap();
        assert_eq!(ledger.allow("s", Source::LoginRetry, "c", StopKind::Other, 20.0), Err(Cap::Chain));
        ledger.person_wrote("s");
        assert_eq!(try_continue(&mut ledger, Source::LoginRetry, "c", StopKind::Other, 30.0), Ok(()));
        ledger.person_wrote("s");
        assert_eq!(ledger.allow("s", Source::LoginRetry, "d", StopKind::Other, 40.0), Err(Cap::Hourly));
    }

    #[test]
    fn the_cancel_mark_follows_the_waiting_stop() {
        let home = tempfile::tempdir().unwrap();
        let file = path(home.path());
        let mut ledger = Ledger::load(file.clone());
        let hit = StopHit { name: "s".into(), provider: "claude".into(), kind: StopKind::Network, line_id: Some("line-1".into()), at: 0.0, life: "L1".into() };
        assert!(!ledger.cancel("s"), "sem espera, nada a cancelar");
        ledger.park("s", Waiting { hit, count: 1, note_out: true });
        assert!(ledger.cancel("s"));
        assert!(ledger.is_cancelled("s", "line-1") && !ledger.is_cancelled("s", "line-2"));
        // Reiniciar não apaga a marca; renomear a leva junto.
        let mut ledger = Ledger::load(file);
        assert!(ledger.is_cancelled("s", "line-1"));
        ledger.rename("s", "t");
        assert!(ledger.is_cancelled("t", "line-1") && !ledger.is_cancelled("s", "line-1"));
        ledger.end("t", "line-1", 10.0);
        assert!(!ledger.is_cancelled("t", "line-1"));
        assert!(!ledger.cancel("t"), "o desfecho tirou a espera");
    }

    /// A pessoa escreveu "continua" depois de duas continuações: a queda seguinte, dentro dos 30 min, continua.
    #[test]
    fn a_person_continue_after_two_auto_continues_allows_the_next_drop() {
        let (_home, mut ledger) = ledger();
        try_continue(&mut ledger, Source::Transient, "a", StopKind::Network, 0.0).unwrap();
        try_continue(&mut ledger, Source::Transient, "b", StopKind::Network, 60.0).unwrap();
        assert_eq!(ledger.allow("s", Source::Transient, "c", StopKind::Network, 120.0), Err(Cap::Chain));
        ledger.person_wrote("s");
        assert_eq!(try_continue(&mut ledger, Source::Transient, "c", StopKind::Network, 180.0), Ok(()));
        // Os outros tetos seguem: a hora já tem três.
        ledger.person_wrote("s");
        assert_eq!(ledger.allow("s", Source::Transient, "d", StopKind::Network, 240.0), Err(Cap::Hourly));
    }

    /// O caso de 13/11: quatro logins recusados em 38 s, cada um com sua linha.
    #[test]
    fn four_login_retries_in_38_s_send_two() {
        let (_home, mut ledger) = ledger();
        let got: Vec<_> = [(0.0, "login-1"), (12.0, "login-2"), (25.0, "login-3"), (38.0, "login-4")].into_iter()
            .map(|(now, key)| try_continue(&mut ledger, Source::LoginRetry, key, StopKind::Other, now))
            .collect();
        assert_eq!(got, [Ok(()), Ok(()), Err(Cap::Chain), Err(Cap::Chain)]);
    }
}

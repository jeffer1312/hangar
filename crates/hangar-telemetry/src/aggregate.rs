//! O motor: contadores do dia (UTC) em memória, na forma de fio, gravados na fila pela tarefa de
//! fundo. `count` só segura uma trava curta em memória; disco e rede ficam com a tarefa.
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::buckets::Bucket;
use crate::diary::DetailSource;
use crate::events::{Dims, Event};
use crate::queue::{self, DayFile};
use crate::send::{Failure, Post};
use crate::state::{self, Acceptance, Collection, State};
use crate::switch::{self, Disabled};
use crate::wire::{Counter, Env, MAX_COUNT, Refusal, RefusalKind, is_id};

/// Relógio de parede; os testes injetam o deles.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;
/// Leitura do ambiente do processo; os testes injetam a deles.
pub type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// O que o servidor sabe na subida.
pub struct Config {
    pub home: PathBuf,
    /// A `backend/` de máquina não migrada, onde mora o `.env` antigo.
    pub backend: Option<PathBuf>,
    pub version: String,
    /// Commit do pacote, só quando ele é de release.
    pub release_commit: Option<String>,
    pub channel: String,
}

pub struct Telemetry {
    pub(crate) home: PathBuf,
    backend: Option<PathBuf>,
    pub(crate) batch_env: Env,
    pub(crate) lookup: EnvLookup,
    pub(crate) clock: Clock,
    enabled: AtomicBool,
    /// Quantas vezes a contagem ligou: o que foi capturado antes da ligação atual não conta.
    consent_epoch: AtomicU64,
    pub(crate) settings: Mutex<Settings>,
    live: Mutex<Live>,
    /// Ordena quem toca o disco: gravação, envio, consentimento. Ordem das travas: `io`, `settings`, `live`.
    pub(crate) io: Mutex<()>,
    /// Cada poll do POST passa por ela; nunca fica presa enquanto ele espera a rede.
    pub(crate) post_gate: Mutex<Option<String>>,
    /// O que não fez a operação falhar mas o diário precisa saber (arquivo ilegível recomeçado, dias
    /// apagados pelo teto); a tarefa de fundo entrega.
    notes: Mutex<Vec<Failure>>,
    /// O dia (UTC) do último receptor fora do ar já avisado: um aviso por dia.
    pub(crate) unreachable_day: Mutex<Option<String>>,
    /// Quem projeta falhas e trilha do diário; sem ele o pacote sai só com contadores.
    pub(crate) detail: OnceLock<DetailSource>,
    /// Quem leva o aviso único de recusa; sem ele, o aviso não sai.
    pub(crate) refusal_post: OnceLock<Post>,
}

pub(crate) struct Settings {
    pub(crate) state: State,
    disabled: Option<Disabled>,
    /// `.env` ilegível pausa o envio e invalida a época, sem apagar os arquivos na hora.
    env_unreadable: bool,
    invalidate_period: bool,
    revocation_unwritten: bool,
}

type Key = (String, BTreeMap<String, String>, Option<String>);

struct Day {
    day: String,
    counters: BTreeMap<Key, u64>,
    snapshot_keys: BTreeSet<String>,
}

impl Day {
    fn new(day: String) -> Day { Day { day, counters: BTreeMap::new(), snapshot_keys: BTreeSet::new() } }

    fn add(&mut self, counter: Counter) {
        let slot = self.counters.entry((counter.event, counter.dims, counter.bucket)).or_default();
        // Acima do teto o pacote seria recusado e o dia inteiro, apagado.
        *slot = slot.saturating_add(counter.count).min(MAX_COUNT);
    }

    fn from_file(file: DayFile) -> Day {
        let mut day = Day::new(file.day);
        for counter in file.counters { day.add(counter); }
        day.snapshot_keys = file.snapshot_keys;
        day
    }

    fn to_counters(&self) -> Vec<Counter> {
        self.counters.iter()
            .map(|((event, dims, bucket), count)| Counter { event: event.clone(), dims: dims.clone(), bucket: bucket.clone(), count: *count })
            .collect()
    }

    fn file(&self) -> DayFile {
        // Os produtores antigos desses contadores não comprovam autoria ou horário da ocorrência.
        let counters = self.to_counters().into_iter().filter(|counter| !matches!(counter.event.as_str(),
            "turn_finished" | "limit_switch" | "problem_hidden" | "problem_shown" | "session_fallen" | "server_exit"))
            .collect();
        DayFile { day: self.day.clone(), counters, snapshot_keys: self.snapshot_keys.clone(), ..DayFile::default() }
    }
}

struct Live {
    current: Day,
    /// O dia corrente mudou desde a última gravação.
    dirty: bool,
    /// O dia corrente já tem arquivo: o primeiro arquivo de um dia é o que pode passar do teto.
    on_disk: bool,
    /// Dias que viraram com mudança e ainda não foram gravados.
    closed: Vec<Day>,
    /// O que os apps devolvem com o retrato; nasce a cada ligação e nunca vai ao receptor.
    snapshot_period: Option<String>,
}

impl Live {
    fn new(current: Day, on_disk: bool) -> Live { Live { current, dirty: false, on_disk, closed: Vec::new(), snapshot_period: None } }

    /// Só para a frente: relógio que volta não abre de novo um dia que pode já estar na fila.
    fn roll(&mut self, today: String) {
        if today > self.current.day {
            let old = std::mem::replace(&mut self.current, Day::new(today));
            self.on_disk = false;
            if std::mem::take(&mut self.dirty) {
                self.closed.push(old);
            }
        }
    }

    /// A gravação de `file` falhou: a mudança volta a valer para a próxima passada. O dia que virou
    /// durante a gravação volta para `closed`, salvo se o giro já o pôs lá com contagem mais nova.
    fn unsaved(&mut self, file: DayFile) {
        if file.day == self.current.day {
            self.dirty = true;
        } else if !self.closed.iter().any(|day| day.day == file.day) {
            let at = self.closed.iter().position(|day| day.day > file.day).unwrap_or(self.closed.len());
            self.closed.insert(at, Day::from_file(file));
        }
    }

    /// Os dias que ainda dependem da memória: o corrente e os que viraram sem gravar.
    pub(crate) fn pending(&self) -> (String, Vec<String>) {
        (self.current.day.clone(), self.closed.iter().map(|day| day.day.clone()).collect())
    }
}

pub(crate) fn day_of(now: DateTime<Utc>) -> String { now.format("%Y-%m-%d").to_string() }

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> io::Result<MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| io::Error::other("trava da telemetria envenenada"))
}

impl Telemetry {
    /// Lê (ou cria) `telemetry.json` e aplica o consentimento e os desligamentos. Bloqueante.
    pub fn new(config: Config) -> io::Result<Telemetry> {
        Telemetry::with(config, Arc::new(Utc::now), Arc::new(|key: &str| std::env::var(key).ok()))
    }

    pub fn with(config: Config, clock: Clock, lookup: EnvLookup) -> io::Result<Telemetry> {
        let now = clock();
        let (state, recovered) = state::load(&config.home, now)?;
        let telemetry = Telemetry {
            batch_env: Env::new(&config.version, config.release_commit.as_deref(), &config.channel),
            home: config.home,
            backend: config.backend,
            lookup,
            enabled: AtomicBool::new(false),
            consent_epoch: AtomicU64::new(0),
            settings: Mutex::new(Settings { state, disabled: None, env_unreadable: false, invalidate_period: false, revocation_unwritten: false }),
            live: Mutex::new(Live::new(Day::new(day_of(now)), false)),
            io: Mutex::new(()),
            post_gate: Mutex::new(None),
            notes: Mutex::new(recovered.into_iter().map(Failure::Disk).collect()),
            unreachable_day: Mutex::new(None),
            detail: OnceLock::new(),
            refusal_post: OnceLock::new(),
            clock,
        };
        telemetry.refresh()?;
        Ok(telemetry)
    }

    /// Para os testes deste crate e do servidor: sem resposta ao aviso, ambiente vazio, sem `.env`.
    #[cfg(any(test, feature = "test-util"))]
    pub fn for_test(home: &std::path::Path, clock: Clock) -> Telemetry {
        let config = Config {
            home: home.to_owned(),
            backend: None,
            version: "0.42.0".into(),
            release_commit: None,
            channel: "latest".into(),
        };
        Telemetry::with(config, clock, Arc::new(|_: &str| -> Option<String> { None })).expect("telemetria de teste")
    }

    pub fn count(&self, event: Event, dims: Dims, n: u64) {
        if self.enabled() {
            self.add(Counter::new(event, dims, None, n));
        }
    }

    /// `true` quando o dia UTC `day` já tem a marca `key`, gravada junto com os contadores dela.
    pub fn recorded(&self, day: &str, key: &str) -> bool {
        self.live.lock().is_ok_and(|live| live.current.day == day && live.current.snapshot_keys.contains(key))
    }

    /// O retrato do servidor: marca e contadores só no dia e no aceite em que ele começou.
    ///
    /// # Errors
    /// Falha de trava ou de gravação do arquivo do dia.
    pub fn record_day(&self, day: &str, epoch: u64, key: &str, counters: Vec<Counter>) -> io::Result<bool> {
        self.record_once(key, counters, |_, now| self.consent_epoch() == epoch && day_of(now) == day)
    }

    /// Identifica a ligação em vigor para o retrato dos apps sem enviar o identificador ao receptor.
    pub fn snapshot_period(&self) -> Option<String> {
        let live = self.live.lock().ok()?;
        if self.enabled() { live.snapshot_period.clone() } else { None }
    }

    /// Confirma o retrato inteiro ou nada. Entrega repetida no mesmo período também é confirmada.
    ///
    /// # Errors
    /// Falha de trava ou de gravação do arquivo do dia.
    pub fn record_snapshot(&self, period: &str, captured: DateTime<Utc>, key: &str, counters: Vec<Counter>) -> io::Result<bool> {
        // Um minuto admite pequena diferença de relógio sem admitir capturas de outro dia.
        self.record_once(key, counters, |live, now| live.snapshot_period.as_deref() == Some(period)
            && captured.date_naive() == now.date_naive() && captured - now <= chrono::TimeDelta::minutes(1))
    }

    /// Marca e contadores entram juntos no arquivo do dia corrente, ou nada; dia fechado nunca reabre.
    fn record_once(&self, key: &str, counters: Vec<Counter>, accept: impl FnOnce(&Live, DateTime<Utc>) -> bool) -> io::Result<bool> {
        let _io = lock(&self.io)?;
        let Some(period) = self.collection() else { return Ok(false) };
        {
            let mut live = lock(&self.live)?;
            let now = (self.clock)();
            let day = day_of(now);
            if !self.enabled() || live.current.day > day || !accept(&live, now) {
                return Ok(false);
            }
            live.roll(day);
            if live.current.snapshot_keys.insert(key.to_owned()) {
                for counter in counters { live.current.add(counter); }
                live.dirty = true;
            }
        }
        // A repetição não soma, mas ainda precisa completar a gravação que pode ter falhado.
        self.save_held(&period, self.today())?;
        Ok(true)
    }

    pub fn duration(&self, event: Event, dims: Dims, took: Duration) {
        if self.enabled() { self.duration_in(self.consent_epoch(), event, dims, took); }
    }

    /// Só soma se o aceite ainda for o de `epoch`, conferido junto com a soma sob a trava `live`: o que foi
    /// capturado antes de uma revogação não entra no período do aceite seguinte.
    pub fn duration_in(&self, epoch: u64, event: Event, dims: Dims, took: Duration) {
        self.add_in_epoch(epoch, Counter::new(event, dims, Some(Bucket::of(took)), 1));
    }

    pub fn count_in(&self, epoch: u64, event: Event, dims: Dims, n: u64) {
        self.add_in_epoch(epoch, Counter::new(event, dims, None, n));
    }

    fn add(&self, counter: Counter) { self.add_in_epoch(self.consent_epoch(), counter); }

    fn add_in_epoch(&self, epoch: u64, counter: Counter) {
        let today = self.today();
        let Ok(mut live) = self.live.lock() else { return };
        // A época sobe sob esta mesma trava (`refresh_held`): conferida aqui, não há janela entre ver e somar.
        if !self.enabled() || self.consent_epoch() != epoch {
            return;
        }
        live.roll(today);
        live.current.add(counter);
        live.dirty = true;
    }

    /// `Ok(true)` só na primeira vez que `key` passa por aqui, também entre reinícios; desligada,
    /// `Ok(false)`. Marca que não se lê nem se grava volta `Err`: quem chama não conta nem dá o
    /// feito por feito. Arquivo ilegível recomeça vazio e vai ao diário. Lê e grava disco.
    pub fn once(&self, key: &str) -> io::Result<bool> { self.once_in_epoch(self.consent_epoch(), key) }

    pub fn once_in_epoch(&self, epoch: u64, key: &str) -> io::Result<bool> {
        // ponytail: a poda é pelo dia no fim da chave; o teto só segura chave sem dia que cresça sem fim.
        const CAP: usize = 4096;
        if !self.enabled() { return Ok(false); }
        let _io = lock(&self.io)?;
        let Some(period) = self.collection().filter(|_| self.consent_epoch() == epoch) else { return Ok(false) };
        let key = format!("{}:{key}", period.id);
        let mut keys = match queue::read_once(&self.home) {
            Ok(keys) => keys,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                self.note(Failure::Disk(e));
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        if keys.contains(&key) { return Ok(false); }
        // Ontem fica: o dia é UTC, e a marca de quem mandou perto da meia-noite ainda vale.
        let yesterday = (self.clock)().date_naive().pred_opt();
        keys.retain(|known| {
            let day = known.get(known.len().saturating_sub(10)..).and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
            day.zip(yesterday).is_none_or(|(day, yesterday)| day >= yesterday)
        });
        keys.push(key);
        let excess = keys.len().saturating_sub(CAP);
        keys.drain(..excess);
        queue::write_once(&self.home, &keys)?;
        Ok(true)
    }

    pub fn enabled(&self) -> bool { self.enabled.load(Ordering::SeqCst) }

    /// O dia (UTC) do relógio da telemetria, o mesmo do pacote.
    pub fn today(&self) -> String { day_of((self.clock)()) }

    /// Sobe a cada vez que a contagem liga (consentimento dado, `.env` de volta).
    pub fn consent_epoch(&self) -> u64 { self.consent_epoch.load(Ordering::SeqCst) }

    /// `None` = ainda sem resposta.
    pub fn consent(&self) -> Option<bool> { lock(&self.settings).ok().and_then(|s| s.state.consent()) }

    /// Carimbo local para só marcar linhas capturadas no período em vigor.
    pub fn collection(&self) -> Option<Collection> {
        let settings = lock(&self.settings).ok()?;
        self.enabled().then_some(())?;
        settings.state.collection.clone()
    }

    /// Liga a projeção de falhas e trilha do diário no pacote; só a primeira vale.
    pub fn set_detail(&self, source: DetailSource) { let _ = self.detail.set(source); }

    pub(crate) fn seal_through_held(&self, day: String) -> io::Result<()> {
        let mut settings = lock(&self.settings)?;
        let mut next = settings.state.clone();
        if let Some(period) = next.collection.as_mut() {
            period.sealed_through = Some(day);
            state::store(&self.home, &next)?;
            settings.state = next;
        }
        Ok(())
    }

    /// O que desliga apesar do consentimento, na última leitura (subida, consentimento ou tarefa de fundo).
    pub fn disabled_by(&self) -> Option<Disabled> { lock(&self.settings).ok().and_then(|s| s.disabled) }

    /// Grava a escolha; `false` apaga a fila. `Err` só quando a escolha não foi gravada. Gravada, ela
    /// vale: a falha ao aplicá-la (ler o dia, apagar a fila) volta em `Ok(Some)` para o diário, e a
    /// passada seguinte tenta de novo. Bloqueante.
    pub fn set_consent(&self, consent: bool) -> io::Result<Option<io::Error>> {
        let _io = lock(&self.io)?;
        {
            let mut settings = lock(&self.settings)?;
            let mut next = settings.state.clone();
            let now = (self.clock)();
            if consent && now.timestamp() <= 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "relógio inválido para gravar consentimento"));
            }
            next.legacy = Some(false);
            if !consent {
                next.option_b = None;
                next.collection = None;
            } else if next.option_b.as_ref().is_none_or(|choice| choice.at > now) {
                next.option_b = Some(Acceptance { scope: state::CONSENT_SCOPE, at: now });
                next.collection = None;
            }
            if !consent || next.collection != settings.state.collection { self.close_post_gate()?; }
            let stored = state::store(&self.home, &next);
            // Se o disco recusar a revogação, não volta a enviar nesta vida.
            if !consent || stored.is_ok() {
                settings.state = next;
                settings.revocation_unwritten = stored.is_err();
            }
            stored?;
        }
        Ok(self.refresh_held().err())
    }

    /// "Não enviar" dito pelo dono: grava como `set_consent(false)` e devolve o aviso único de recusa quando
    /// esta máquina nunca mandou um e nenhum desligamento do ambiente vale. A marca é gravada antes de
    /// devolver: no máximo um aviso por máquina, mesmo que o envio falhe. A marca que não se grava não
    /// manda nada e volta como a falha a registrar. Bloqueante.
    pub fn refuse(&self) -> io::Result<(Option<io::Error>, Option<Refusal>)> {
        let accepted = self.consent() == Some(true);
        let applied = self.set_consent(false)?;
        let _io = lock(&self.io)?;
        let mut settings = lock(&self.settings)?;
        if settings.state.refusal_reported || settings.disabled.is_some() || settings.env_unreadable {
            return Ok((applied, None));
        }
        let mut next = settings.state.clone();
        next.refusal_reported = true;
        if let Err(e) = state::store(&self.home, &next) { return Ok((applied.or(Some(e)), None)); }
        settings.state = next;
        let kind = if accepted { RefusalKind::Revoked } else { RefusalKind::Declined };
        Ok((applied, Some(Refusal::new(kind, &self.batch_env))))
    }

    /// O dono desta máquina e quando ele nasceu (segundos Unix), para a convergência com os peers.
    pub fn owner(&self) -> io::Result<(String, i64)> {
        lock(&self.settings).map(|s| (s.state.owner_id.clone(), s.state.owner_created))
    }

    /// Adota o dono de um peer quando ele é mais antigo (empate: menor id); devolve se adotou. Bloqueante.
    pub fn adopt_owner(&self, owner_id: &str, owner_created: i64) -> io::Result<bool> {
        if !is_id(owner_id) {
            return Ok(false);
        }
        let _io = lock(&self.io)?;
        let mut settings = lock(&self.settings)?;
        let older = (owner_created, owner_id) < (settings.state.owner_created, settings.state.owner_id.as_str());
        if !older {
            return Ok(false);
        }
        let mut next = settings.state.clone();
        next.owner_id = owner_id.to_owned();
        next.owner_created = owner_created;
        state::store(&self.home, &next)?;
        settings.state = next;
        Ok(true)
    }

    /// Os contadores do dia corrente em memória.
    pub fn counters(&self) -> Vec<Counter> { lock(&self.live).map(|live| live.current.to_counters()).unwrap_or_default() }

    /// Grava na fila os dias que viraram e o corrente, e congela os dias encerrados. Bloqueante.
    pub fn save(&self) -> io::Result<()> {
        let _io = lock(&self.io)?;
        self.refresh_held()?;
        if let Some(period) = self.collection() {
            let now = (self.clock)();
            self.save_held(&period, day_of(now))?;
            self.checkpoint_held(&period, now)?;
        }
        Ok(())
    }

    fn save_held(&self, period: &Collection, today: String) -> io::Result<()> {
        let (closed, current, new_file) = {
            let mut live = lock(&self.live)?;
            live.roll(today);
            let current = std::mem::take(&mut live.dirty).then(|| live.current.file());
            let new_file = !live.closed.is_empty() || (current.is_some() && !live.on_disk);
            (live.closed.iter().map(Day::file).collect::<Vec<_>>(), current, new_file)
        };
        let written = closed.iter().chain(&current).try_for_each(|latest| {
            let stored = match queue::read_day(&self.home, &latest.day) {
                Ok(file) => file,
                // A memória é a cópia boa do dia: o arquivo ilegível sai e ela o substitui.
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    queue::remove_day(&self.home, &latest.day)?;
                    self.note(Failure::Disk(e));
                    None
                }
                Err(e) => return Err(e),
            };
            let mut file = stored.filter(|file| file.collection == period.id && file.day == latest.day).unwrap_or_else(|| DayFile {
                day: latest.day.clone(), collection: period.id.clone(), ..DayFile::default()
            });
            if !file.sealed {
                file.counters.clone_from(&latest.counters);
                file.snapshot_keys.clone_from(&latest.snapshot_keys);
                if !file.counters.is_empty() || !file.detail.is_empty() || !file.snapshot_keys.is_empty() {
                    queue::write_day(&self.home, &file)?;
                }
            }
            Ok(())
        });
        {
            let mut live = lock(&self.live)?;
            let same_day = current.as_ref().is_some_and(|file| file.day == live.current.day);
            if let Err(e) = written {
                if let Some(file) = current {
                    live.unsaved(file);
                }
                return Err(e);
            }
            // Só sai da memória o que já está no disco; o que virou no meio fica para a próxima.
            live.closed.drain(..closed.len());
            live.on_disk |= same_day;
        }
        if new_file {
            let pruned = queue::cap(&self.home)?;
            if pruned > 0 {
                self.note(Failure::Pruned(pruned));
            }
        }
        Ok(())
    }

    pub(crate) fn pending_days(&self) -> io::Result<(String, Vec<String>)> { lock(&self.live).map(|live| live.pending()) }

    pub(crate) fn note(&self, failure: Failure) {
        if let Ok(mut notes) = self.notes.lock() {
            notes.push(failure);
        }
    }

    pub(crate) fn take_notes(&self) -> Vec<Failure> {
        self.notes.lock().map(|mut notes| std::mem::take(&mut *notes)).unwrap_or_default()
    }

    /// Relê os desligamentos e grava: a passada periódica da tarefa de fundo.
    #[cfg(test)]
    fn tick(&self) -> io::Result<()> { self.save() }

    fn refresh(&self) -> io::Result<()> {
        let _io = lock(&self.io)?;
        self.refresh_held()
    }

    /// Relê o ambiente; invalidar o período no disco vem antes de apagar a fila.
    pub(crate) fn refresh_held(&self) -> io::Result<()> {
        let dotenv = hangar_core::user_data::read_env_raw(&self.home, self.backend.as_deref());
        let disabled = switch::disabled_by(&*self.lookup, dotenv.as_ref().ok().and_then(|text| text.as_deref()));
        let now = (self.clock)();
        let today = day_of(now);
        let mut settings = lock(&self.settings)?;
        let was_unreadable = std::mem::replace(&mut settings.env_unreadable, dotenv.is_err());
        if let Err(error) = &dotenv && !was_unreadable { self.note(Failure::Disk(io::Error::other(format!("telemetria: .env não lido: {error}")))); }
        settings.disabled = disabled;
        let on = settings.state.option_b.as_ref().is_some_and(|choice| choice.at <= now)
            && disabled.is_none() && !settings.env_unreadable;
        let mut next = settings.state.clone();
        settings.invalidate_period |= !on || next.collection.as_ref().is_some_and(|period| period.since > now);
        if settings.invalidate_period {
            self.close_post_gate()?;
            next.collection = None;
            *lock(&self.live)? = Live::new(Day::new(today.clone()), false);
        }
        if on && next.collection.is_none() {
            next.collection = Some(Collection { id: hangar_core::ids::random_hex(16), since: now, sealed_through: None });
        }
        if next != settings.state || settings.revocation_unwritten {
            state::store(&self.home, &next)?;
            settings.state = next;
            settings.revocation_unwritten = false;
        }
        settings.invalidate_period = false;
        if !on {
            if disabled.is_some() || settings.state.consent() == Some(false) {
                queue::wipe(&self.home)?;
                queue::wipe_legacy(&self.home)?;
            }
            return Ok(());
        }
        if !self.enabled() {
            let period = settings.state.collection.as_ref().expect("coleta ativa tem período");
            let file = match queue::read_day(&self.home, &today) {
                Ok(file) => file.filter(|file| file.collection == period.id && file.day == today && !file.sealed),
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    queue::remove_day(&self.home, &today)?;
                    self.note(Failure::Disk(e));
                    None
                }
                Err(e) => return Err(e),
            };
            let mut live = match file {
                Some(file) => Live::new(Day::from_file(file), true),
                None => Live::new(Day::new(today), false),
            };
            live.snapshot_period = Some(hangar_core::ids::random_hex(16));
            *lock(&self.post_gate)? = Some(period.id.clone());
            // A época sobe sob a trava `live`: quem soma confere a época sob ela.
            let mut current = lock(&self.live)?;
            *current = live;
            self.consent_epoch.fetch_add(1, Ordering::SeqCst);
            self.enabled.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn close_post_gate(&self) -> io::Result<()> {
        let mut gate = lock(&self.post_gate)?;
        *gate = None;
        self.enabled.store(false, Ordering::SeqCst);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::events::{Provider, Surface};
    use chrono::TimeZone;

    pub(crate) struct Fixture {
        pub(crate) home: tempfile::TempDir,
        pub(crate) now: Arc<Mutex<DateTime<Utc>>>,
    }

    impl Fixture {
        pub(crate) fn new() -> Fixture {
            Fixture { home: tempfile::tempdir().unwrap(), now: Arc::new(Mutex::new(Utc.with_ymd_and_hms(2026, 11, 26, 10, 0, 0).unwrap())) }
        }

        fn clock(&self) -> Clock {
            let now = self.now.clone();
            Arc::new(move || *now.lock().unwrap())
        }

        /// Com consentimento dado.
        pub(crate) fn start(&self) -> Telemetry {
            let telemetry = Telemetry::for_test(self.home.path(), self.clock());
            telemetry.set_consent(true).unwrap();
            telemetry
        }

        /// Com o consentimento que estiver gravado (ou nenhum).
        pub(crate) fn start_as_is(&self) -> Telemetry { self.start_with(Arc::new(|_: &str| -> Option<String> { None })) }

        /// Como `start_as_is`, com o ambiente do processo de `lookup`.
        pub(crate) fn start_with(&self, lookup: EnvLookup) -> Telemetry {
            let config = Config {
                home: self.home.path().to_owned(),
                backend: None,
                version: "0.42.0".into(),
                release_commit: None,
                channel: "latest".into(),
            };
            Telemetry::with(config, self.clock(), lookup).unwrap()
        }

        pub(crate) fn next_day(&self) { *self.now.lock().unwrap() += chrono::TimeDelta::days(1); }

        pub(crate) fn queue(&self) -> PathBuf { self.home.path().join(".hangar/telemetry-v3/queue") }
    }

    pub(crate) fn message() -> Dims { Dims::new().with("provider", Provider::Claude).with("surface", Surface::Terminal) }

    #[test]
    fn legacy_and_scope_2_yes_do_not_enable_and_a_no_wipes_the_old_queues() {
        use serde_json::{Value, json};
        let fx = Fixture::new();
        let path = fx.home.path().join(".hangar/telemetry.json");
        let with = |extra: Value| {
            let mut stored = json!({"machine_id": "1".repeat(32), "owner_id": "2".repeat(32), "owner_created": 123});
            stored.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            stored.to_string()
        };
        let read = || serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap();
        let old_queues = [fx.home.path().join(".hangar/telemetry/queue/2026-11-26.json"),
            fx.home.path().join(".hangar/telemetry-scope-2/queue/2026-11-26.json")];
        for old in &old_queues {
            std::fs::create_dir_all(old.parent().unwrap()).unwrap();
            std::fs::write(old, r#"{"day": "2026-11-26", "counters": []}"#).unwrap();
        }
        for old_yes in [json!({"consent": true}), json!({"consent_scope": 2})] {
            std::fs::write(&path, with(old_yes)).unwrap();
            let telemetry = fx.start_as_is();
            assert_eq!(telemetry.consent(), None);
            assert!(!telemetry.enabled());
            assert_eq!(telemetry.owner().unwrap(), ("2".repeat(32), 123));
            let stored = read();
            assert!(stored.get("consent_scope").is_none() && stored["consent"] != json!(true), "{stored}");
        }
        let telemetry = fx.start_as_is();
        telemetry.set_consent(true).unwrap();
        assert!(old_queues.iter().all(|old| old.is_file()), "o aceite não mexe nas filas das versões anteriores");
        assert_eq!(read()["consent"], false, "servidor revertido não enxerga um sim");
        assert_eq!(read()["option_b"]["scope"], 3);
        drop(telemetry);
        assert!(fx.start_as_is().enabled());

        std::fs::write(&path, with(json!({"consent": false}))).unwrap();
        let telemetry = fx.start_as_is();
        assert_eq!(telemetry.consent(), Some(false));
        assert!(!telemetry.enabled());
        assert!(old_queues.iter().all(|old| !old.parent().unwrap().parent().unwrap().exists()), "o \"Não enviar\" apaga também as filas antigas");
    }

    #[test]
    fn a_whole_snapshot_survives_midnight_during_the_flush() {
        use std::sync::atomic::AtomicUsize;
        let fx = Fixture::new();
        let before = Utc.with_ymd_and_hms(2026, 11, 27, 23, 59, 59).unwrap();
        let after = before + chrono::TimeDelta::seconds(1);
        let reads = Arc::new(AtomicUsize::new(0));
        let inspecting = Arc::new(AtomicBool::new(false));
        let clock = {
            let (reads, inspecting) = (reads.clone(), inspecting.clone());
            Arc::new(move || if inspecting.load(Ordering::SeqCst) && reads.fetch_add(1, Ordering::SeqCst) > 0 { after } else { before })
        };
        let telemetry = Telemetry::for_test(fx.home.path(), clock);
        telemetry.set_consent(true).unwrap();
        let period = telemetry.snapshot_period().unwrap();
        inspecting.store(true, Ordering::SeqCst);
        let counters = vec![Counter::new(Event::Peers, Dims::new(), None, 3), Counter::new(Event::MessageSent, message(), None, 4)];
        assert!(telemetry.record_snapshot(&period, before, "fixture", counters).unwrap());
        assert!(telemetry.counters().is_empty(), "o dia novo não recebe parte do retrato");
        let file = queue::read_day(fx.home.path(), "2026-11-27").unwrap().unwrap();
        assert_eq!(file.counters.len(), 2);
        assert_eq!(file.snapshot_keys, BTreeSet::from(["fixture".to_owned()]));
        assert!(queue::read_once(fx.home.path()).unwrap().is_empty());
    }

    #[test]
    fn once_prunes_marks_by_day_not_by_count() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        assert!(telemetry.once("peers:2026-11-26").unwrap());
        for n in 0..100 {
            assert!(telemetry.once(&format!("settings:native:d{n}:2026-11-26")).unwrap());
        }
        assert!(!telemetry.once("peers:2026-11-26").unwrap(), "muitas marcas não apagam a de hoje");
        assert!(telemetry.once("update:a->b:2026-11-20T10:00:00+00:00").unwrap());
        fx.next_day();
        assert!(!telemetry.once("peers:2026-11-26").unwrap(), "a de ontem ainda vale");
        fx.next_day();
        assert!(telemetry.once("peers:2026-11-28").unwrap());
        let keys = queue::read_once(fx.home.path()).unwrap();
        assert!(!keys.iter().any(|k| k.ends_with("2026-11-26")), "{keys:?}");
        assert!(keys.iter().any(|k| k.contains(":update:")), "marca sem dia no fim fica");
    }

    #[test]
    fn repeated_yes_keeps_the_period_the_queue_and_the_marks() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        let active = telemetry.snapshot_period().unwrap();
        telemetry.count(Event::MessageSent, message(), 2);
        telemetry.save().unwrap();
        assert!(telemetry.once("fixture").unwrap());
        telemetry.set_consent(true).unwrap();
        assert_eq!(telemetry.snapshot_period().as_deref(), Some(active.as_str()));
        assert_eq!(queue::read_day(fx.home.path(), "2026-11-26").unwrap().unwrap().counters[0].count, 2);
        assert!(!telemetry.once("fixture").unwrap());
        telemetry.set_consent(false).unwrap();
        assert!(telemetry.snapshot_period().is_none());
        telemetry.set_consent(true).unwrap();
        assert_ne!(telemetry.snapshot_period().unwrap(), active, "aceite novo, período novo");
    }

    #[test]
    fn consent_epoch_rises_each_time_counting_turns_on() {
        let fx = Fixture::new();
        let telemetry = fx.start_as_is();
        assert_eq!(telemetry.consent_epoch(), 0);
        telemetry.set_consent(true).unwrap();
        telemetry.set_consent(true).unwrap();
        assert_eq!(telemetry.consent_epoch(), 1, "já ligada não sobe");
        telemetry.set_consent(false).unwrap();
        telemetry.set_consent(true).unwrap();
        assert_eq!(telemetry.consent_epoch(), 2);
    }

    #[test]
    fn counts_stop_at_the_wire_ceiling() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), MAX_COUNT);
        telemetry.count(Event::MessageSent, message(), 5);
        assert_eq!(telemetry.counters()[0].count, MAX_COUNT);
    }

    #[test]
    fn unanswered_consent_counts_and_writes_nothing() {
        let fx = Fixture::new();
        let telemetry = fx.start_as_is();
        assert_eq!(telemetry.consent(), None);
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.duration(Event::SessionOpened, Dims::new(), Duration::from_millis(50));
        telemetry.save().unwrap();
        assert!(telemetry.counters().is_empty());
        assert!(!fx.queue().exists());
        let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(fx.home.path().join(".hangar/telemetry.json")).unwrap()).unwrap();
        assert_eq!(stored["consent"], serde_json::Value::Null);
        assert!(is_id(stored["machine_id"].as_str().unwrap()));
        assert!(is_id(stored["owner_id"].as_str().unwrap()));
    }

    #[test]
    fn a_count_from_before_a_new_consent_is_not_added_to_it() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        let before = telemetry.consent_epoch();
        telemetry.set_consent(false).unwrap();
        telemetry.set_consent(true).unwrap();
        telemetry.count_in(before, Event::MessageSent, message(), 5);
        telemetry.duration_in(before, Event::MessageSent, message(), Duration::from_millis(10));
        assert!(telemetry.counters().is_empty(), "captura da época anterior não soma no aceite novo");
        telemetry.count_in(telemetry.consent_epoch(), Event::MessageSent, message(), 1);
        assert_eq!(telemetry.counters().len(), 1);
    }

    #[test]
    fn refusing_wipes_the_queue() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        assert!(fx.queue().join("2026-11-26.json").exists());
        telemetry.set_consent(false).unwrap();
        assert!(!fx.queue().exists());
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        assert!(telemetry.counters().is_empty());
        assert!(!fx.queue().exists());
        assert_eq!(fx.start_as_is().consent(), Some(false));
    }

    #[test]
    fn do_not_track_in_dotenv_beats_consent() {
        let fx = Fixture::new();
        let config = fx.home.path().join(".hangar/config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(".env"), "CP_PORT=8765\nDo_Not_Track=1\n").unwrap();
        let telemetry = fx.start();
        assert_eq!(telemetry.consent(), Some(true));
        assert_eq!(telemetry.disabled_by(), Some(Disabled::Env));
        assert!(!telemetry.enabled());
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        assert!(telemetry.counters().is_empty());
        assert!(!fx.queue().exists());
    }

    #[test]
    fn restart_keeps_counting_the_same_day() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 2);
        telemetry.save().unwrap();
        drop(telemetry);

        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 3);
        let counters = telemetry.counters();
        assert_eq!(counters.len(), 1);
        assert_eq!(counters[0].count, 5);
        telemetry.save().unwrap();
        let file = queue::read_day(fx.home.path(), "2026-11-26").unwrap().unwrap();
        assert_eq!(file.counters, counters);
    }

    #[test]
    fn once_is_true_only_the_first_time() {
        let fx = Fixture::new();
        let telemetry = fx.start_as_is();
        assert!(!telemetry.once("x").unwrap(), "sem consentimento nada é marcado");
        telemetry.set_consent(true).unwrap();
        assert!(telemetry.once("x").unwrap());
        assert!(!telemetry.once("x").unwrap());
        drop(telemetry);
        let telemetry = fx.start();
        assert!(!telemetry.once("x").unwrap());
        assert!(telemetry.once("y").unwrap());
    }

    #[test]
    fn once_that_cannot_read_its_marks_is_an_error() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        // Uma pasta no lugar do arquivo: a leitura falha em todo sistema, sem ser arquivo ilegível.
        std::fs::create_dir_all(fx.home.path().join(".hangar/telemetry-v3/once.json")).unwrap();
        assert!(telemetry.once("x").is_err());
        std::fs::remove_dir(fx.home.path().join(".hangar/telemetry-v3/once.json")).unwrap();
        assert!(telemetry.once("x").unwrap(), "a marca não ficou dada por feita");
    }

    #[test]
    fn consent_is_kept_when_applying_it_fails() {
        let fx = Fixture::new();
        let telemetry = fx.start_as_is();
        let today = fx.queue().join("2026-11-26.json");
        std::fs::create_dir_all(&today).unwrap();
        assert!(telemetry.set_consent(true).unwrap().is_some());
        assert_eq!(telemetry.consent(), Some(true));
        let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(fx.home.path().join(".hangar/telemetry.json")).unwrap()).unwrap();
        assert_eq!(stored["consent"], false, "a versão revertida não pode ligar logs");
        assert_eq!(stored["option_b"]["scope"], 3, "a escolha de B foi gravada em separado");
        assert!(!telemetry.enabled());
        std::fs::remove_dir(&today).unwrap();
        telemetry.tick().unwrap();
        assert!(telemetry.enabled(), "a passada seguinte aplica");
    }

    #[test]
    fn telemetry_unreadable_environment_cannot_revive_a_previous_collection_on_restart() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        let first = telemetry.collection().unwrap();
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        let dotenv = fx.home.path().join(".hangar/config/.env");
        std::fs::create_dir_all(&dotenv).unwrap();
        telemetry.tick().unwrap();
        assert!(!telemetry.enabled());
        telemetry.count(Event::MessageSent, message(), 5);
        drop(telemetry);
        std::fs::remove_dir(&dotenv).unwrap();
        fx.next_day();
        let telemetry = fx.start_as_is();
        assert!(telemetry.enabled());
        assert_ne!(telemetry.collection().unwrap().id, first.id);
        assert!(telemetry.counters().is_empty());
    }

    #[test]
    fn day_that_rolled_while_its_write_failed_goes_back_to_closed() {
        let day = |name: &str, n: u64| {
            let mut day = Day::new(name.into());
            day.add(Counter::new(Event::MessageSent, message(), None, n));
            day
        };
        let mut live = Live::new(Day::new("2026-11-27".into()), false);
        live.unsaved(day("2026-11-26", 4).file());
        assert_eq!(live.pending().1, ["2026-11-26"]);
        assert_eq!(live.closed[0].to_counters()[0].count, 4);
        assert!(!live.dirty);

        let mut live = Live::new(Day::new("2026-11-27".into()), false);
        live.closed.push(day("2026-11-26", 5));
        live.unsaved(day("2026-11-26", 4).file());
        assert_eq!(live.closed.len(), 1);
        assert_eq!(live.closed[0].to_counters()[0].count, 5, "o giro já guardou a contagem mais nova");

        let mut live = Live::new(day("2026-11-26", 4), true);
        live.unsaved(day("2026-11-26", 4).file());
        assert!(live.dirty && live.closed.is_empty());
    }

    #[test]
    fn process_switch_wipes_even_with_an_unreadable_dotenv() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        drop(telemetry);
        std::fs::create_dir_all(fx.home.path().join(".hangar/config/.env")).unwrap();
        let telemetry = fx.start_with(Arc::new(|key: &str| (key == "HANGAR_TELEMETRY").then(|| "0".to_owned())));
        assert!(!telemetry.enabled());
        assert_eq!(telemetry.disabled_by(), Some(Disabled::Env));
        assert!(!fx.queue().join("2026-11-26.json").exists(), "a fila foi apagada");
    }

    #[test]
    fn older_owner_is_adopted() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        let (mine, created) = telemetry.owner().unwrap();
        let newer = "f".repeat(32);
        assert!(!telemetry.adopt_owner(&newer, created + 10).unwrap());
        assert!(!telemetry.adopt_owner("NOT-AN-ID", created - 10).unwrap());
        let older = "0".repeat(32);
        assert!(telemetry.adopt_owner(&older, created - 10).unwrap());
        assert_eq!(telemetry.owner().unwrap(), (older.clone(), created - 10));
        assert_ne!(mine, older);
        assert_eq!(fx.start().owner().unwrap(), (older, created - 10));
    }

    #[test]
    fn unreadable_day_file_starts_the_day_over() {
        let fx = Fixture::new();
        drop(fx.start());
        std::fs::create_dir_all(fx.queue()).unwrap();
        std::fs::write(fx.queue().join("2026-11-26.json"), "{").unwrap();
        let telemetry = fx.start();
        assert!(telemetry.enabled());
        assert_eq!(telemetry.take_notes().len(), 1);
        assert!(!fx.queue().join("2026-11-26.json").exists());
        telemetry.count(Event::MessageSent, message(), 2);
        telemetry.save().unwrap();
        assert_eq!(queue::read_day(fx.home.path(), "2026-11-26").unwrap().unwrap().counters[0].count, 2);
    }

    #[test]
    fn unreadable_state_starts_over_unanswered() {
        let fx = Fixture::new();
        let (owner, _) = fx.start().owner().unwrap();
        std::fs::write(fx.home.path().join(".hangar/telemetry.json"), "{").unwrap();
        let telemetry = fx.start_as_is();
        assert_eq!(telemetry.consent(), None);
        assert!(!telemetry.enabled());
        assert_eq!(telemetry.take_notes().len(), 1);
        assert_ne!(telemetry.owner().unwrap().0, owner);
        assert_eq!(fx.start_as_is().owner().unwrap(), telemetry.owner().unwrap(), "o estado novo foi gravado");
    }

    #[test]
    fn unreadable_dotenv_stops_without_wiping() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        drop(telemetry);
        // Uma pasta no lugar do arquivo: a leitura falha em todo sistema.
        std::fs::create_dir_all(fx.home.path().join(".hangar/config/.env")).unwrap();
        let telemetry = fx.start();
        assert!(!telemetry.enabled());
        assert_eq!(telemetry.disabled_by(), None);
        assert_eq!(telemetry.take_notes().len(), 1);
        assert!(fx.queue().join("2026-11-26.json").exists());
    }

    #[test]
    fn unreadable_once_file_counts_as_empty() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        std::fs::create_dir_all(fx.home.path().join(".hangar/telemetry-v3")).unwrap();
        std::fs::write(fx.home.path().join(".hangar/telemetry-v3/once.json"), "{").unwrap();
        assert!(telemetry.once("x").unwrap());
        assert!(matches!(telemetry.take_notes().as_slice(), [Failure::Disk(e)] if e.kind() == io::ErrorKind::InvalidData));
        assert!(!telemetry.once("x").unwrap());
    }

    #[test]
    fn untouched_day_is_not_written_and_a_new_day_file_respects_the_cap() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.save().unwrap();
        assert!(!fx.queue().exists(), "sem contagem nada é gravado");
        for n in 12..=25 {
            queue::write_day(fx.home.path(), &DayFile { day: format!("2026-11-{n:02}"), ..DayFile::default() }).unwrap();
        }
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        let days = queue::days(fx.home.path()).unwrap();
        assert_eq!(days.len(), queue::MAX_DAYS);
        assert_eq!(days.first().map(String::as_str), Some("2026-11-13"));
        assert_eq!(days.last().map(String::as_str), Some("2026-11-26"));
        assert!(matches!(telemetry.take_notes().as_slice(), [Failure::Pruned(1)]), "o dia apagado vai ao diário");
    }

    #[test]
    fn unreadable_day_file_is_replaced_by_the_counts_in_memory() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 2);
        telemetry.save().unwrap();
        std::fs::write(fx.queue().join("2026-11-26.json"), "{").unwrap();
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        let file = queue::read_day(fx.home.path(), "2026-11-26").unwrap().unwrap();
        assert_eq!(file.counters[0].count, 3);
        assert!(matches!(telemetry.take_notes().as_slice(), [Failure::Disk(e)] if e.kind() == io::ErrorKind::InvalidData));
    }

    #[test]
    fn a_snapshot_without_counters_keeps_its_mark_across_restart() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        let period = telemetry.snapshot_period().unwrap();
        let now = *fx.now.lock().unwrap();
        assert!(telemetry.record_snapshot(&period, now, "settings:native:d1", Vec::new()).unwrap());
        drop(telemetry);
        assert!(fx.start_as_is().recorded("2026-11-26", "settings:native:d1"));
    }
}

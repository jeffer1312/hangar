//! O motor: contadores do dia (UTC) em memória, na forma de fio, gravados na fila pela tarefa de
//! fundo. `count` só segura uma trava curta em memória; disco e rede ficam com a tarefa.
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::buckets::Bucket;
use crate::events::{Dims, Event, LogLevel};
use crate::queue::{self, DayFile};
use crate::send::Failure;
use crate::state::{self, SETTINGS_CONSENT_SCOPE, State};
use crate::switch::{self, Disabled};
use crate::wire::{Counter, Env, LogLine, MAX_COUNT, MAX_LOG_CHARS, MAX_LOG_LINES, is_id, is_invisible, is_name};

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
    /// O que não fez a operação falhar mas o diário precisa saber (arquivo ilegível recomeçado, dias
    /// apagados pelo teto); a tarefa de fundo entrega.
    notes: Mutex<Vec<Failure>>,
    /// O dia (UTC) do último receptor fora do ar já avisado: um aviso por dia.
    pub(crate) unreachable_day: Mutex<Option<String>>,
}

pub(crate) struct Settings {
    pub(crate) state: State,
    disabled: Option<Disabled>,
    /// `.env` que não se lê: desliga sem apagar a fila, porque a pessoa não pediu para parar.
    env_unreadable: bool,
}

type Key = (String, BTreeMap<String, String>, Option<String>);
/// Nível, alvo e mensagem.
type LogKey = (String, String, String);

struct Day {
    day: String,
    counters: BTreeMap<Key, u64>,
    logs: BTreeMap<LogKey, u64>,
    snapshot_keys: BTreeSet<String>,
}

impl Day {
    fn new(day: String) -> Day { Day { day, counters: BTreeMap::new(), logs: BTreeMap::new(), snapshot_keys: BTreeSet::new() } }

    /// `false` quando a linha é nova e o dia já tem `MAX_LOG_LINES` distintas: o pacote não passa do teto.
    fn add_log(&mut self, line: LogLine) -> bool {
        let key = (line.level, line.target, line.message);
        if let Some(slot) = self.logs.get_mut(&key) {
            *slot = slot.saturating_add(line.count).min(MAX_COUNT);
            return true;
        }
        if self.logs.len() >= MAX_LOG_LINES {
            return false;
        }
        self.logs.insert(key, line.count.min(MAX_COUNT));
        true
    }

    fn to_logs(&self) -> Vec<LogLine> {
        self.logs.iter()
            .map(|((level, target, message), count)| LogLine { level: level.clone(), target: target.clone(), message: message.clone(), count: *count })
            .collect()
    }

    fn add(&mut self, counter: Counter) {
        let slot = self.counters.entry((counter.event, counter.dims, counter.bucket)).or_default();
        // Acima do teto o pacote seria recusado e o dia inteiro, apagado.
        *slot = slot.saturating_add(counter.count).min(MAX_COUNT);
    }

    fn from_file(file: DayFile) -> Day {
        let mut day = Day::new(file.day);
        file.counters.into_iter().for_each(|counter| day.add(counter));
        file.logs.into_iter().for_each(|line| { day.add_log(line); });
        day.snapshot_keys = file.snapshot_keys;
        day
    }

    fn to_counters(&self) -> Vec<Counter> {
        self.counters.iter()
            .map(|((event, dims, bucket), count)| Counter { event: event.clone(), dims: dims.clone(), bucket: bucket.clone(), count: *count })
            .collect()
    }

    fn file(&self) -> DayFile {
        DayFile { day: self.day.clone(), batch_id: None, counters: self.to_counters(),
            snapshot_keys: self.snapshot_keys.clone(), logs: self.to_logs() }
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

    fn add_snapshot(&mut self, day: String, counters: Vec<Counter>) {
        if counters.is_empty() { return; }
        self.roll(day);
        for counter in counters { self.current.add(counter); }
        self.dirty = true;
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
        let (state, recovered) = state::load(&config.home, now.timestamp())?;
        let telemetry = Telemetry {
            batch_env: Env::new(&config.version, config.release_commit.as_deref(), &config.channel),
            home: config.home,
            backend: config.backend,
            lookup,
            enabled: AtomicBool::new(false),
            consent_epoch: AtomicU64::new(0),
            settings: Mutex::new(Settings { state, disabled: None, env_unreadable: false }),
            live: Mutex::new(Live::new(Day::new(day_of(now)), false)),
            io: Mutex::new(()),
            notes: Mutex::new(recovered.into_iter().map(Failure::Disk).collect()),
            unreachable_day: Mutex::new(None),
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

    /// Conta um retrato só no dia e no período de aceite em que ele começou. Nunca reabre dia fechado.
    pub fn count_for_day(&self, day: &str, epoch: u64, counters: Vec<Counter>) {
        let Ok(mut live) = self.live.lock() else { return };
        if !self.enabled() || self.consent_epoch() != epoch || self.today() != day || live.current.day.as_str() > day {
            return;
        }
        live.add_snapshot(day.to_owned(), counters);
    }

    /// Identifica o período de coleta ativo sem enviar o identificador ao receptor.
    pub fn snapshot_period(&self) -> Option<String> {
        let live = self.live.lock().ok()?;
        if self.enabled() { live.snapshot_period.clone() } else { None }
    }

    /// Confirma o retrato inteiro ou nada. Entrega repetida no mesmo período também é confirmada.
    ///
    /// # Errors
    /// Falha de trava ou de gravação do arquivo do dia.
    pub fn record_snapshot(&self, period: &str, captured: DateTime<Utc>, key: &str, counters: Vec<Counter>) -> io::Result<bool> {
        let _io = lock(&self.io)?;
        {
            let mut live = lock(&self.live)?;
            let now = (self.clock)();
            let day = day_of(now);
            // Um minuto admite pequena diferença de relógio sem admitir capturas de outro dia.
            if !self.enabled() || live.snapshot_period.as_deref() != Some(period) || live.current.day > day
                || captured.date_naive() != now.date_naive() || captured - now > chrono::TimeDelta::minutes(1)
            {
                return Ok(false);
            }
            live.roll(day);
            if live.current.snapshot_keys.insert(key.to_owned()) {
                for counter in counters { live.current.add(counter); }
                live.dirty = true;
            }
        }
        // A repetição não soma, mas ainda precisa completar a gravação que pode ter falhado.
        self.save_held()?;
        Ok(true)
    }

    pub fn duration(&self, event: Event, dims: Dims, took: Duration) {
        if self.enabled() {
            self.add(Counter::new(event, dims, Some(Bucket::of(took)), 1));
        }
    }

    /// Uma linha de aviso ou erro do log, já limpa por quem chama; o mesmo portão do `count`. Alvo fora
    /// do formato sai `other`; caractere de controle vira espaço e a mensagem é cortada em `MAX_LOG_CHARS`.
    pub fn log(&self, level: LogLevel, target: &str, message: &str) {
        if !self.enabled() {
            return;
        }
        let message: String = message.chars().map(|c| if is_invisible(c) { ' ' } else { c }).take(MAX_LOG_CHARS).collect();
        let message = message.trim();
        if message.is_empty() {
            return;
        }
        let target = if is_name(target) { target } else { "other" };
        let line = LogLine { level: level.as_str().to_owned(), target: target.to_owned(), message: message.to_owned(), count: 1 };
        let today = day_of((self.clock)());
        let Ok(mut live) = self.live.lock() else { return };
        // A revogação desliga antes de limpar a memória sob esta trava: conferida aqui, não grava depois dela.
        if !self.enabled() {
            return;
        }
        live.roll(today);
        if !live.current.add_log(line) {
            live.current.add(Counter::new(Event::LogDropped, Dims::new(), None, 1));
        }
        live.dirty = true;
    }

    fn add(&self, counter: Counter) {
        let today = day_of((self.clock)());
        let Ok(mut live) = self.live.lock() else { return };
        if !self.enabled() {
            return;
        }
        live.roll(today);
        live.current.add(counter);
        live.dirty = true;
    }

    /// `Ok(true)` só na primeira vez que `key` passa por aqui, também entre reinícios; desligada,
    /// `Ok(false)`. Marca que não se lê nem se grava volta `Err`: quem chama não conta nem dá o
    /// feito por feito. Arquivo ilegível recomeça vazio e vai ao diário. Lê e grava disco.
    pub fn once(&self, key: &str) -> io::Result<bool> {
        if !self.enabled() {
            return Ok(false);
        }
        let _io = lock(&self.io)?;
        if !self.enabled() {
            return Ok(false);
        }
        self.mark_once(key, (self.clock)().date_naive())
    }

    /// A marca do retrato usa o mesmo dia UTC e o mesmo aceite da contagem.
    ///
    /// # Errors
    /// Propaga falhas de leitura ou gravação das marcas.
    pub fn once_for_day(&self, day: &str, epoch: u64, key: &str) -> io::Result<bool> {
        let _io = lock(&self.io)?;
        let now = (self.clock)();
        if !self.enabled() || self.consent_epoch() != epoch || day_of(now) != day {
            return Ok(false);
        }
        self.mark_once(&format!("{key}:{day}"), now.date_naive())
    }

    fn mark_once(&self, key: &str, day: chrono::NaiveDate) -> io::Result<bool> {
        // ponytail: a poda é pelo dia no fim da chave; o teto só segura chave sem dia que cresça sem fim.
        const CAP: usize = 4096;
        let mut keys = match queue::read_once(&self.home) {
            Ok(keys) => keys,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                self.note(Failure::Disk(e));
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        if keys.iter().any(|known| known == key) {
            return Ok(false);
        }
        // Ontem fica: o dia é UTC, e a marca de quem mandou perto da meia-noite ainda vale.
        let yesterday = day.pred_opt();
        keys.retain(|known| {
            let day = known.get(known.len().saturating_sub(10)..).and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
            day.zip(yesterday).is_none_or(|(day, yesterday)| day >= yesterday)
        });
        keys.push(key.to_owned());
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
    pub fn consent(&self) -> Option<bool> { lock(&self.settings).ok().and_then(|s| s.state.effective_consent()) }

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
            if consent && next.effective_consent() != Some(true) {
                queue::wipe(&self.home)?;
            }
            next.consent = (!consent).then_some(false);
            next.consent_scope = consent.then_some(SETTINGS_CONSENT_SCOPE);
            state::store(&self.home, &next)?;
            settings.state = next;
            if !consent {
                self.enabled.store(false, Ordering::SeqCst);
            }
        }
        Ok(self.refresh_held().err())
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

    /// As linhas de log do dia corrente em memória.
    pub fn logs(&self) -> Vec<LogLine> { lock(&self.live).map(|live| live.current.to_logs()).unwrap_or_default() }

    /// Grava na fila os dias que viraram e o corrente, se mudaram; o desligamento do servidor
    /// chama. Bloqueante.
    pub fn save(&self) -> io::Result<()> {
        let _io = lock(&self.io)?;
        if !self.enabled() {
            return Ok(());
        }
        self.save_held()
    }

    /// `save` com a trava `io` já segura.
    fn save_held(&self) -> io::Result<()> {
        let (closed, current, new_file) = {
            let mut live = lock(&self.live)?;
            live.roll(day_of((self.clock)()));
            let current = std::mem::take(&mut live.dirty).then(|| live.current.file());
            let new_file = !live.closed.is_empty() || (current.is_some() && !live.on_disk);
            (live.closed.iter().map(Day::file).collect::<Vec<_>>(), current, new_file)
        };
        let written = closed.iter().chain(&current).try_for_each(|file| queue::write_day(&self.home, file));
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

    fn note(&self, failure: Failure) {
        if let Ok(mut notes) = self.notes.lock() {
            notes.push(failure);
        }
    }

    pub(crate) fn take_notes(&self) -> Vec<Failure> {
        self.notes.lock().map(|mut notes| std::mem::take(&mut *notes)).unwrap_or_default()
    }

    /// Relê os desligamentos e grava: a passada periódica da tarefa de fundo.
    pub(crate) fn tick(&self) -> io::Result<()> {
        self.refresh()?;
        self.save()
    }

    /// Aplica consentimento e desligamentos. Ligar relê o dia corrente da fila; desligar ou `false`
    /// apaga a fila.
    fn refresh(&self) -> io::Result<()> {
        let _io = lock(&self.io)?;
        self.refresh_held()
    }

    fn refresh_held(&self) -> io::Result<()> {
        let dotenv = hangar_core::user_data::read_env_raw(&self.home, self.backend.as_deref());
        // `.env` que não se lê não confirma a escolha da pessoa: desliga, sem dizer que ela pediu. O
        // desligamento pelo ambiente do processo vale mesmo assim.
        let (disabled, unreadable) = match &dotenv {
            Ok(text) => (switch::disabled_by(&*self.lookup, text.as_deref()), None),
            Err(e) => (switch::disabled_by(&*self.lookup, None), Some(io::Error::other(format!("telemetria: .env não lido: {e}")))),
        };
        let env_unreadable = unreadable.is_some();
        let consent = {
            let mut settings = lock(&self.settings)?;
            settings.disabled = disabled;
            let was_unreadable = std::mem::replace(&mut settings.env_unreadable, env_unreadable);
            if let Some(error) = unreadable.filter(|_| !was_unreadable) {
                self.note(Failure::Disk(error));
            }
            settings.state.effective_consent()
        };
        let on = consent == Some(true) && disabled.is_none() && !env_unreadable;
        let today = day_of((self.clock)());
        if on {
            // O que a passada do `.env` ilegível não conseguiu gravar ainda está na memória e vale.
            let held = lock(&self.live).map(|live| live.dirty || !live.closed.is_empty())?;
            if !self.enabled() && !held {
                let live = match queue::read_day(&self.home, &today) {
                    Ok(Some(file)) => Live::new(Day::from_file(file), true),
                    Ok(None) => Live::new(Day::new(today), false),
                    Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                        queue::remove_day(&self.home, &today)?;
                        self.note(Failure::Disk(e));
                        Live::new(Day::new(today), false)
                    }
                    Err(e) => return Err(e),
                };
                *lock(&self.live)? = live;
            }
            // A época sobe antes de ligar: quem vê ligada já vê a época nova.
            if !self.enabled() {
                lock(&self.live)?.snapshot_period = Some(hangar_core::ids::random_hex(16));
                self.consent_epoch.fetch_add(1, Ordering::SeqCst);
            }
            self.enabled.store(true, Ordering::SeqCst);
            return Ok(());
        }
        // `.env` que não se lê não é a pessoa pedindo para parar: o que só está na memória vai à fila antes.
        // A contagem para já; gravação que falha deixa o dia na memória para a passada seguinte.
        if env_unreadable && consent == Some(true) && disabled.is_none() {
            self.enabled.store(false, Ordering::SeqCst);
            self.save_held()?;
        }
        self.enabled.store(false, Ordering::SeqCst);
        *lock(&self.live)? = Live::new(Day::new(today), false);
        if consent == Some(false) || disabled.is_some() {
            queue::wipe(&self.home)?;
        }
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
        fn start_with(&self, lookup: EnvLookup) -> Telemetry {
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

        pub(crate) fn queue(&self) -> PathBuf { self.home.path().join(".hangar/telemetry-scope-2/queue") }
    }

    pub(crate) fn message() -> Dims { Dims::new().with("provider", Provider::Claude).with("surface", Surface::Terminal) }

    #[test]
    fn log_lines_stop_at_the_cap_and_count_what_was_dropped() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        for n in 0..MAX_LOG_LINES {
            telemetry.log(LogLevel::Warn, "hangar_server::list", &format!("aviso {n}"));
        }
        telemetry.log(LogLevel::Warn, "hangar_server::list", "aviso 0");
        telemetry.log(LogLevel::Error, "hangar_server::list", "nova depois do teto");
        telemetry.log(LogLevel::Error, "hangar_server::list", "outra nova");
        let logs = telemetry.logs();
        assert_eq!(logs.len(), MAX_LOG_LINES);
        assert_eq!(logs.iter().find(|l| l.message == "aviso 0").map(|l| l.count), Some(2), "a linha já vista ainda soma");
        assert!(!logs.iter().any(|l| l.message.contains("nova")));
        assert_eq!(telemetry.counters(), vec![Counter::new(Event::LogDropped, Dims::new(), None, 2)]);
    }

    #[test]
    fn log_message_is_cut_and_has_no_control_character() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.log(LogLevel::Error, "Fora Do Formato", &format!("primeira\nsegunda {}", "é".repeat(400)));
        telemetry.log(LogLevel::Error, "hangar_server::x", " \n ");
        let logs = telemetry.logs();
        assert_eq!(logs.len(), 1, "mensagem vazia não entra");
        assert_eq!(logs[0].target, "other");
        assert!(logs[0].message.starts_with("primeira segunda é"));
        assert_eq!(logs[0].message.chars().count(), MAX_LOG_CHARS);
    }

    #[test]
    fn log_waiting_for_the_live_lock_is_not_written_after_revocation() {
        let fx = Fixture::new();
        let telemetry = Arc::new(fx.start());
        let held = telemetry.live.lock().unwrap();
        let writer = { let telemetry = telemetry.clone(); std::thread::spawn(move || telemetry.log(LogLevel::Error, "hangar_server::x", "falhou")) };
        // Com a trava presa aqui, a escrita só a pega depois de desligada, como na revogação.
        telemetry.enabled.store(false, Ordering::SeqCst);
        drop(held);
        writer.join().unwrap();
        assert!(telemetry.logs().is_empty());
    }

    #[test]
    fn legacy_yes_does_not_enable_settings_and_legacy_no_survives() {
        let fx = Fixture::new();
        let path = fx.home.path().join(".hangar/telemetry.json");
        let mut stored = serde_json::json!({"consent": true, "machine_id": "1".repeat(32),
            "owner_id": "2".repeat(32), "owner_created": 123});
        queue::write_day(fx.home.path(), &DayFile { day: "2026-11-26".into(), batch_id: None,
            counters: vec![Counter::new(Event::MessageSent, message(), None, 7)], snapshot_keys: Default::default(), logs: vec![] }).unwrap();
        let legacy = fx.home.path().join(".hangar/telemetry/queue/2026-11-26.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::copy(fx.queue().join("2026-11-26.json"), &legacy).unwrap();
        std::fs::write(&path, stored.to_string()).unwrap();
        let telemetry = fx.start_as_is();
        assert_eq!(telemetry.consent(), None);
        assert_eq!(telemetry.owner().unwrap(), ("2".repeat(32), 123));
        assert!(!telemetry.enabled());
        telemetry.set_consent(true).unwrap();
        assert!(telemetry.counters().is_empty(), "a fila antiga não foi reaproveitada");
        assert!(legacy.is_file(), "a nova versão não mexe na fila do binário legado");
        let current: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(current["consent"], serde_json::Value::Null, "servidor revertido não enxerga um sim");
        assert_eq!(current["consent_scope"], 2);
        drop(telemetry);
        assert!(fx.start_as_is().enabled());

        stored["consent"] = serde_json::json!(false);
        std::fs::write(path, stored.to_string()).unwrap();
        let telemetry = fx.start_as_is();
        assert_eq!(telemetry.consent(), Some(false));
        assert!(!telemetry.enabled());
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
    fn counts_stop_at_the_wire_ceiling() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), MAX_COUNT);
        telemetry.count(Event::MessageSent, message(), 5);
        assert_eq!(telemetry.counters()[0].count, MAX_COUNT);
    }

    #[test]
    fn invisible_characters_become_spaces() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.log(LogLevel::Error, "hangar_server::x", "a\u{2028}b\u{2029}c\u{202E}d\u{200B}e\u{FEFF}f");
        assert_eq!(telemetry.logs()[0].message, "a b c d e f");
    }

    #[test]
    fn log_without_consent_records_nothing() {
        let fx = Fixture::new();
        let telemetry = fx.start_as_is();
        telemetry.log(LogLevel::Error, "hangar_server::list", "falhou");
        telemetry.save().unwrap();
        assert!(telemetry.logs().is_empty());
        telemetry.set_consent(false).unwrap();
        telemetry.log(LogLevel::Error, "hangar_server::list", "falhou");
        telemetry.save().unwrap();
        assert!(telemetry.logs().is_empty() && telemetry.counters().is_empty());
        assert!(!fx.queue().exists());
    }

    #[test]
    fn log_lines_go_through_the_queue_and_back() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.log(LogLevel::Error, "hangar_server::list", "falhou");
        telemetry.log(LogLevel::Warn, "hangar_server::list", "devagar");
        telemetry.save().unwrap();
        drop(telemetry);
        let file = queue::read_day(fx.home.path(), "2026-11-26").unwrap().unwrap();
        assert_eq!(file.logs.len(), 2);
        let telemetry = fx.start();
        telemetry.log(LogLevel::Error, "hangar_server::list", "falhou");
        let logs = telemetry.logs();
        assert_eq!(logs.iter().find(|l| l.message == "falhou").map(|l| (l.level.as_str(), l.count)), Some(("error", 2)));
        assert_eq!(logs.len(), 2);
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
        std::fs::create_dir_all(fx.home.path().join(".hangar/telemetry-scope-2/once.json")).unwrap();
        assert!(telemetry.once("x").is_err());
        std::fs::remove_dir(fx.home.path().join(".hangar/telemetry-scope-2/once.json")).unwrap();
        assert!(telemetry.once("x").unwrap(), "a marca não ficou dada por feita");
    }

    #[test]
    fn once_prunes_marks_by_day_not_by_count() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        assert!(telemetry.once("peers:2026-11-26").unwrap());
        for n in 0..100 {
            assert!(telemetry.once(&format!("settings:native:d{n}:2026-11-26")).unwrap());
        }
        assert!(!telemetry.once("peers:2026-11-26").unwrap(), "muitos aparelhos não apagam a marca de hoje");
        assert!(telemetry.once("update:a->b:2026-11-20T10:00:00+00:00").unwrap());
        fx.next_day();
        assert!(!telemetry.once("peers:2026-11-26").unwrap(), "a de ontem ainda vale");
        fx.next_day();
        assert!(telemetry.once("peers:2026-11-28").unwrap());
        let keys = queue::read_once(fx.home.path()).unwrap();
        assert!(!keys.iter().any(|k| k.ends_with("2026-11-26")), "{keys:?}");
        assert!(keys.iter().any(|k| k.starts_with("update:")), "marca sem dia no fim fica");
    }

    #[test]
    fn new_consent_waits_for_cleanup_but_repeated_yes_does_not_erase() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        let root = fx.home.path().join(".hangar/telemetry-scope-2");
        std::fs::write(&root, "blocked").unwrap();
        assert!(telemetry.set_consent(false).unwrap().is_some(), "revogação vale mesmo se não conseguiu limpar");
        assert!(telemetry.snapshot_period().is_none());
        assert!(telemetry.set_consent(true).is_err());
        assert_eq!(telemetry.consent(), Some(false));
        drop(telemetry);
        std::fs::write(fx.home.path().join(".hangar/telemetry.json"), "{").unwrap();
        let telemetry = fx.start_as_is();
        assert!(telemetry.set_consent(true).is_err(), "estado ilegível também exige limpar antes de aceitar");
        assert_eq!(telemetry.consent(), None);
        assert!(!telemetry.enabled());
        std::fs::remove_file(root).unwrap();
        telemetry.set_consent(true).unwrap();
        let active = telemetry.snapshot_period().unwrap();
        telemetry.count(Event::MessageSent, message(), 2);
        telemetry.save().unwrap();
        assert!(telemetry.once("fixture").unwrap());
        telemetry.set_consent(true).unwrap();
        assert_eq!(telemetry.snapshot_period().as_deref(), Some(active.as_str()));
        assert_eq!(queue::read_day(fx.home.path(), "2026-11-26").unwrap().unwrap().counters[0].count, 2);
        assert!(!telemetry.once("fixture").unwrap());
    }

    #[test]
    fn unreadable_dotenv_saves_what_was_only_in_memory() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 1);
        fx.next_day();
        telemetry.count(Event::MessageSent, message(), 2);
        let dotenv = fx.home.path().join(".hangar/config/.env");
        std::fs::create_dir_all(&dotenv).unwrap();
        telemetry.tick().unwrap();
        assert!(!telemetry.enabled());
        assert_eq!(queue::read_day(fx.home.path(), "2026-11-26").unwrap().unwrap().counters[0].count, 1);
        assert_eq!(queue::read_day(fx.home.path(), "2026-11-27").unwrap().unwrap().counters[0].count, 2);
        std::fs::remove_dir(&dotenv).unwrap();
        telemetry.tick().unwrap();
        assert!(telemetry.enabled());
        assert_eq!(telemetry.counters()[0].count, 2, "o dia volta da fila");
    }

    #[test]
    fn unreadable_dotenv_stops_counting_even_when_saving_fails() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 1);
        let dotenv = fx.home.path().join(".hangar/config/.env");
        std::fs::create_dir_all(&dotenv).unwrap();
        // Uma pasta no lugar do dia: a gravação falha.
        let today = fx.queue().join("2026-11-26.json");
        std::fs::create_dir_all(&today).unwrap();
        assert!(telemetry.tick().is_err());
        assert!(!telemetry.enabled(), "a contagem parou");
        telemetry.count(Event::MessageSent, message(), 5);
        std::fs::remove_dir(&today).unwrap();
        telemetry.tick().unwrap();
        assert_eq!(queue::read_day(fx.home.path(), "2026-11-26").unwrap().unwrap().counters[0].count, 1, "a passada seguinte grava");
        std::fs::remove_dir(&dotenv).unwrap();
        telemetry.tick().unwrap();
        assert!(telemetry.enabled());
        assert_eq!(telemetry.counters()[0].count, 1);
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
    fn unsaved_day_survives_the_dotenv_coming_back() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        telemetry.count(Event::MessageSent, message(), 3);
        let dotenv = fx.home.path().join(".hangar/config/.env");
        std::fs::create_dir_all(&dotenv).unwrap();
        let today = fx.queue().join("2026-11-26.json");
        std::fs::create_dir_all(&today).unwrap();
        assert!(telemetry.tick().is_err());
        std::fs::remove_dir(&dotenv).unwrap();
        std::fs::remove_dir(&today).unwrap();
        telemetry.tick().unwrap();
        assert!(telemetry.enabled());
        assert_eq!(telemetry.counters()[0].count, 3, "o dia da memória não foi trocado pelo da fila");
        assert_eq!(queue::read_day(fx.home.path(), "2026-11-26").unwrap().unwrap().counters[0].count, 3);
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
        std::fs::create_dir_all(fx.home.path().join(".hangar/telemetry-scope-2")).unwrap();
        std::fs::write(fx.home.path().join(".hangar/telemetry-scope-2/once.json"), "{").unwrap();
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
            queue::write_day(fx.home.path(), &DayFile { day: format!("2026-11-{n:02}"), batch_id: None, counters: Vec::new(), snapshot_keys: Default::default(), logs: Vec::new() }).unwrap();
        }
        telemetry.count(Event::MessageSent, message(), 1);
        telemetry.save().unwrap();
        let days = queue::days(fx.home.path()).unwrap();
        assert_eq!(days.len(), queue::MAX_DAYS);
        assert_eq!(days.first().map(String::as_str), Some("2026-11-13"));
        assert_eq!(days.last().map(String::as_str), Some("2026-11-26"));
        assert!(matches!(telemetry.take_notes().as_slice(), [Failure::Pruned(1)]), "o dia apagado vai ao diário");
    }
}

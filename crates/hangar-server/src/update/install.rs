//! Instalação nova direto pelo pacote, sem git:
//!
//!   hangar-server install-package [--channel <canal>] [--release-base <url>]
//!
//! Baixa e extrai o pacote mais novo do canal pelo mesmo download conferido da migração (índice,
//! sha256, extração e a tela web da chave), copia o lançador do pacote para
//! `~/.hangar/bin/hangar-launcher`, registra os passos de `docs/atualizacoes/` como aplicados, grava a
//! marca `~/.hangar/migration.json` (pacote, sem checkout) e só então aponta o `current`. Serviço,
//! hooks, skills e agentes são do `install.sh` que vem dentro do pacote; o `bootstrap.sh` o roda depois.
//!
//! Saída: 0 instalado, 1 falhou (o `current` não foi criado), 2 uso errado, 3 o índice não tem
//! pacote para este sistema (o bootstrap segue pelo git), 4 já há uma instalação aqui.
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use super::migrate::{self, Fetched, Fetcher, Mode, Record};
use super::{Layout, UpdateError, UpdateLock};

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed { version: String, notes: Vec<String> },
    /// O índice não tem pacote para este sistema; nada foi instalado.
    NoPlatform(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Já há uma instalação: atualizar é pelo app; checkout é `--dev` ou a migração.
    Refused(String),
    Failed(String),
}

/// A máquina como a instalação a vê; os caminhos vêm de fora para o teste.
pub struct Machine<'a> {
    pub home: PathBuf,
    /// `<config do Claude>/.hangar-update`, onde mora o registro dos passos.
    pub state_dir: PathBuf,
    /// `~/.config/systemd/user`.
    pub systemd_dir: PathBuf,
    pub windows: bool,
    pub fetcher: &'a dyn Fetcher,
}

/// O motivo de recusar: uma instalação, migração ou serviço do Hangar já no lugar.
fn existing(m: &Machine<'_>) -> Option<String> {
    let layout = Layout::new(&m.home);
    if std::fs::symlink_metadata(layout.current()).is_ok() {
        return Some(format!("o Hangar já está instalado aqui ({} existe); para atualizar, use o Atualizar do app", layout.current().display()));
    }
    // A marca desta própria instalação, interrompida antes do `current`, não trava a próxima tentativa.
    let marker = layout.hangar.join(migrate::MARKER);
    let migrated = migrate::read_record(&marker).is_some_and(|r| !r.checkout.as_os_str().is_empty()) || layout.hangar.join(migrate::PARTIAL).exists();
    if migrated {
        return Some(format!("esta máquina já passou pela migração para o pacote (registro em {}); confira à mão", layout.hangar.display()));
    }
    let noted = std::fs::read_to_string(layout.hangar.join("backend-dir")).ok().map(|t| PathBuf::from(t.trim()));
    if let Some(checkout) = noted.as_deref().and_then(Path::parent).filter(|d| migrate::is_checkout(d)) {
        return Some(format!("o Hangar já está instalado pelo git em {}; ele passa para o pacote pelo Atualizar do app, ou siga nele com --dev",
            checkout.display()));
    }
    let unit = m.systemd_dir.join("hangar-backend.service");
    unit.exists().then(|| format!("já existe o serviço do Hangar ({}); confira a instalação que ele sobe antes de instalar outra", unit.display()))
}

/// Instala o pacote mais novo de `channel`.
pub async fn install(m: &Machine<'_>, channel: &str) -> Result<Outcome, Error> {
    if let Some(why) = existing(m) { return Err(Error::Refused(why)); }
    let layout = Layout::new(&m.home);
    let _lock = UpdateLock::try_acquire(&layout).map_err(|e| match e {
        UpdateError::Locked => Error::Failed("uma atualização ou instalação está rodando nesta máquina; tente de novo depois".into()),
        e => Error::Failed(e.to_string()),
    })?;
    let (dir, manifest, notes) = match m.fetcher.fetch(&layout, channel).await.map_err(Error::Failed)? {
        Fetched::Version { dir, manifest, notes } => (dir, manifest, notes),
        Fetched::NoPlatform(reason) => return Ok(Outcome::NoPlatform(reason)),
    };
    let launcher = migrate::launcher_path(&m.home, m.windows);
    std::fs::create_dir_all(launcher.parent().expect("bin")).map_err(|e| Error::Failed(format!("{}: {e}", launcher.display())))?;
    let packaged = dir.join("bin").join(if m.windows { "hangar-server.exe" } else { "hangar-server" });
    migrate::write_launcher(&packaged, &launcher).map_err(Error::Failed)?;
    super::steps::mark_all(&dir, &m.state_dir).map_err(|e| Error::Failed(e.to_string()))?;
    // A marca vem antes do `current`: com ela, o passo de migração e o `migrate-package` não rodam aqui.
    let marker = layout.hangar.join(migrate::MARKER);
    migrate::save(&marker, &Record { mode: Mode::Package, date: migrate::now(), checkout: PathBuf::new(), version: Some(manifest.version.clone()),
        reason: Some("instalação nova pelo pacote, sem checkout".into()), notes: notes.clone(), done: Vec::new() }).map_err(Error::Failed)?;
    if let Err(e) = super::swap(&layout, &dir) {
        let _ = std::fs::remove_file(&marker);
        return Err(Error::Failed(e.to_string()));
    }
    Ok(Outcome::Installed { version: manifest.version, notes })
}

/// Canal como nome de release: o mesmo conjunto da limpeza do `package.yml`.
fn valid_channel(channel: &str) -> bool {
    !channel.is_empty() && !channel.starts_with('.') && channel.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// `hangar-server install-package …`: `args` sem o nome do programa nem o subcomando.
pub fn cli(args: &[String]) -> ExitCode {
    let usage = "uso: hangar-server install-package [--channel <canal>] [--release-base <url>]";
    let (mut channel, mut base_url) = ("latest".to_owned(), super::RELEASE_BASE.to_owned());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let value = match arg.as_str() {
            "--channel" => &mut channel,
            "--release-base" => &mut base_url,
            _ => { eprintln!("{usage}"); return ExitCode::from(2); }
        };
        match it.next() {
            Some(v) => *value = v.clone(),
            None => { eprintln!("{usage}"); return ExitCode::from(2); }
        }
    }
    if !valid_channel(&channel) { eprintln!("canal inválido: {channel:?}\n{usage}"); return ExitCode::from(2); }
    let env = crate::accounts::environment::AccountEnvironment::capture();
    let home = env.home.clone();
    let config = env.base.get("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map_or_else(|| home.join(".config"), PathBuf::from);
    let client = match crate::http_client().build() {
        Ok(client) => client,
        Err(e) => { eprintln!("cliente http: {e}"); return ExitCode::from(1); }
    };
    let fetcher = migrate::ReleaseFetcher { client, platform: super::platform(), home: home.clone(), deadline: Duration::from_secs(900),
        base_url: base_url.trim_end_matches('/').to_owned() };
    let machine = Machine { home: home.clone(), state_dir: super::state_dir(&env.claude_base), systemd_dir: config.join("systemd").join("user"),
        windows: cfg!(windows), fetcher: &fetcher };
    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => { eprintln!("runtime: {e}"); return ExitCode::from(1); }
    };
    match runtime.block_on(install(&machine, &channel)) {
        Ok(Outcome::Installed { version, notes }) => {
            for note in notes { println!("aviso: {note}"); }
            println!("instalado: versão {version} do canal {channel} em {}", Layout::new(&home).current().display());
            ExitCode::SUCCESS
        }
        Ok(Outcome::NoPlatform(reason)) => { eprintln!("{reason}"); ExitCode::from(3) }
        Err(Error::Refused(why)) => { eprintln!("{why}"); ExitCode::from(4) }
        Err(Error::Failed(why)) => { eprintln!("a instalação falhou: {why}"); ExitCode::from(1) }
    }
}

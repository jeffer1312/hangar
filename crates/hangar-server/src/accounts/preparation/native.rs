//! Liga a herança entre contas aos escritores e processos administrativos já usados pelo servidor.
use crate::{
    accounts::{AccountService, Provider, catalog::Account, environment::AccountEnvironment},
    codex_setup::{
        files, instructions_toml, native::Native as Admin, native_cli::AdminCli, plugins, trust,
    },
};
use futures_util::future::BoxFuture;
use hangar_codex_account_sync::{
    ConfigPlan, Native, Paths,
    plugins::{Inventory, NativeError, PluginsNative},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const CONFIG_TIMEOUT: Duration = Duration::from_secs(60);

struct Adapter {
    env: AccountEnvironment,
    destination: PathBuf,
}

pub(super) async fn inherit(service: &AccountService, account: &Account, force: bool) -> Value {
    let source = match service.resolve(Provider::Codex, "default") {
        Ok(account) => account.home,
        Err(_) => {
            return json!({"status":"error","trust_pending":false,"issues":[{"code":"codex_account_source_invalid","params":{"path":"ValueError"}}]});
        }
    };
    let mut adapter = Adapter {
        env: service.env.clone(),
        destination: account.home.clone(),
    };
    hangar_codex_account_sync::prepare(
        Paths {
            home: &service.env.home,
            source: &source,
            destination: &account.home,
        },
        account.is_default,
        force,
        &mut adapter,
    )
    .await
}

pub(super) async fn pretrust(
    service: &AccountService,
    account: &Account,
    cwd: &str,
) -> Result<(), String> {
    let _guard = files::exclusive(&account.home.join(".hangar-integracao.lock"))
        .await
        .map_err(|e| e.to_string())?;
    let directory = std::path::absolute(cwd).map_err(|e| e.to_string())?;
    if !Path::new(cwd).is_absolute() || !directory.is_dir() {
        return Err("Pasta de trabalho inválida".into());
    }
    let cwd = directory.to_str().ok_or("Pasta de trabalho não é UTF-8")?;
    let config = account.home.join("config.toml");
    let private = crate::codex_setup::status::root(&service.env.home, &account.home)
        .map_err(|e| e.message)?;
    files::edit_config(
        &service.env,
        &config,
        &private.join("backups"),
        &private,
        |current| Ok(hangar_codex_account_sync::project_trust_edits(current, cwd)),
    )
    .await?;
    let raw = std::fs::read_to_string(config).map_err(|e| e.to_string())?;
    let config = instructions_toml::parse(&raw).map_err(|e| e.to_string())?;
    if config
        .get("projects")
        .and_then(|p| p.get(cwd))
        .and_then(|p| p.get("trust_level"))
        .and_then(Value::as_str)
        != Some("trusted")
    {
        return Err("Confiança da pasta não foi confirmada".into());
    }
    Ok(())
}

fn native_error() -> NativeError {
    NativeError {
        kind: "CodexNativoErro".into(),
        stderr: String::new(),
    }
}

impl Adapter {
    fn admin_cli(&self, account: &Path) -> AdminCli {
        AdminCli::new(self.env.clone(), &self.env.home, account)
    }
    async fn read_trust(&self, account: &Path, approve: bool) -> Result<Option<bool>, String> {
        let mut admin = Admin::open(&self.env, &self.env.home, account, false, None)
            .await
            .map_err(|e| e.to_string())?;
        let result = async {
            let data = admin
                .request("hooks/list", json!({"cwds":[]}), COMMAND_TIMEOUT)
                .await?;
            if approve {
                let (edits, manual) = trust::edits(&data, &self.env.home, account, false)?;
                if !edits.is_empty() {
                    admin
                        .request(
                            "config/batchWrite",
                            json!({"edits":edits,"reloadUserConfig":true}),
                            CONFIG_TIMEOUT,
                        )
                        .await?;
                }
                return Ok(Some(manual > 0));
            }
            let entries = data
                .get("data")
                .and_then(Value::as_array)
                .ok_or("Lista de hooks inválida")?;
            let mut pending = false;
            for entry in entries {
                let entry = entry.as_object().ok_or("Entrada de hooks inválida")?;
                if entry
                    .get("errors")
                    .is_some_and(crate::orchestration::truthy)
                    || entry
                        .get("warnings")
                        .is_some_and(crate::orchestration::truthy)
                {
                    return Err("Lista de hooks incompleta".into());
                }
                if let Some(hooks) = entry.get("hooks") {
                    for hook in hooks.as_array().ok_or("Lista de hooks inválida")? {
                        let hook = hook.as_object().ok_or("Hook inválido")?;
                        pending |= hook.get("enabled").and_then(Value::as_bool) == Some(true)
                            && matches!(
                                hook.get("trustStatus").and_then(Value::as_str),
                                Some("untrusted" | "modified")
                            );
                    }
                }
            }
            Ok(Some(pending))
        }
        .await;
        admin.close().await?;
        result
    }
}

impl Native for Adapter {
    fn parse_toml(&self, raw: &str) -> Result<Value, String> {
        instructions_toml::parse(raw).map_err(|e| e.to_string())
    }
    fn edit_config<'a>(
        &'a mut self,
        path: &'a Path,
        backups: &'a Path,
        work: &'a Path,
        plan: &'a mut ConfigPlan<'_>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { files::edit_config(&self.env, path, backups, work, plan).await })
    }
    fn sync_plugins<'a>(
        &'a mut self,
        paths: Paths<'a>,
        previous: &'a Value,
    ) -> BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            Ok(hangar_codex_account_sync::plugins::sync(paths, previous, self).await)
        })
    }
    fn check_trust(&mut self) -> BoxFuture<'_, Result<Option<bool>, String>> {
        Box::pin(async move { self.read_trust(&self.destination, false).await })
    }
    fn cli_version(&mut self, force: bool) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move { Ok(version(&self.env, force).await) })
    }
    fn now(&self) -> f64 {
        crate::accounts::quotas::now()
    }
}

impl PluginsNative for Adapter {
    fn inventory<'a>(&'a self, account: &'a Path) -> BoxFuture<'a, Result<Inventory, NativeError>> {
        Box::pin(async move {
            let cli = self.admin_cli(account);
            let (plugins, marketplaces) = futures_util::join!(
                cli.plugins(COMMAND_TIMEOUT),
                cli.marketplaces(COMMAND_TIMEOUT)
            );
            cli.close().await.map_err(|_| native_error())?;
            Ok(Inventory {
                plugins: plugins.map_err(|_| native_error())?,
                marketplaces: marketplaces.map_err(|_| native_error())?,
            })
        })
    }
    fn plugins<'a>(&'a self, account: &'a Path) -> BoxFuture<'a, Result<Vec<Value>, NativeError>> {
        Box::pin(async move {
            let cli = self.admin_cli(account);
            let result = cli.plugins(COMMAND_TIMEOUT).await;
            cli.close().await.map_err(|_| native_error())?;
            result.map_err(|_| native_error())
        })
    }
    fn marketplaces<'a>(
        &'a self,
        account: &'a Path,
    ) -> BoxFuture<'a, Result<Vec<Value>, NativeError>> {
        Box::pin(async move {
            let cli = self.admin_cli(account);
            let result = cli.marketplaces(COMMAND_TIMEOUT).await;
            cli.close().await.map_err(|_| native_error())?;
            result.map_err(|_| native_error())
        })
    }
    fn cli<'a>(
        &'a self,
        account: &'a Path,
        args: &'a [String],
    ) -> BoxFuture<'a, Result<Value, NativeError>> {
        Box::pin(async move {
            let cli = self.admin_cli(account);
            let result = cli
                .account_sync_command(args.to_vec(), COMMAND_TIMEOUT)
                .await;
            cli.close().await.map_err(|_| native_error())?;
            result.map_err(|orphan| NativeError {
                kind: "CodexNativoErro".into(),
                stderr: if orphan {
                    hangar_codex_account_sync::plugins::ORPHAN_MARKETPLACE.into()
                } else {
                    String::new()
                },
            })
        })
    }
    fn install<'a>(
        &'a self,
        account: &'a Path,
        id: &'a str,
    ) -> BoxFuture<'a, Result<Value, NativeError>> {
        Box::pin(async move {
            let cli = self.admin_cli(account);
            let result = cli.add(id, COMMAND_TIMEOUT).await;
            cli.close().await.map_err(|_| native_error())?;
            result.map_err(|_| native_error())
        })
    }
    fn parse_toml(&self, raw: &str) -> Result<Value, String> {
        instructions_toml::parse(raw).map_err(|e| e.to_string())
    }
    fn edit_config<'a>(
        &'a self,
        path: &'a Path,
        backups: &'a Path,
        work: &'a Path,
        plan: &'a mut hangar_codex_account_sync::plugins::ConfigPlan<'_>,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { files::edit_config(&self.env, path, backups, work, plan).await })
    }
    fn adapt_hooks<'a>(
        &'a self,
        account: &'a Path,
        root: &'a Path,
        backups: &'a Path,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let (server, wrapper) = crate::codex_setup::hook_command::allow_wrapper()?;
            let context = plugins::Context {
                account,
                backups,
                server: &server,
                windows: cfg!(windows),
            };
            plugins::adapt(
                root,
                &context,
                |value| {
                    Ok(crate::codex_setup::normalize::hooks(
                        value,
                        &server,
                        &wrapper,
                        cfg!(windows),
                    ))
                },
                || {},
                &plugins::NativeRuntime(self.env.clone()),
            )
            .await
        })
    }
    fn trust<'a>(
        &'a self,
        account: &'a Path,
        approve: bool,
    ) -> BoxFuture<'a, Result<Option<bool>, String>> {
        Box::pin(async move { self.read_trust(account, approve).await })
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Signature {
    path: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    identity: (u64, i64, i64),
}
fn signature(path: PathBuf) -> Option<Signature> {
    let metadata = std::fs::metadata(&path).ok()?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        (metadata.ino(), metadata.ctime(), metadata.ctime_nsec())
    };
    #[cfg(not(unix))]
    let identity = (0, 0, 0);
    Some(Signature {
        path: std::fs::canonicalize(path).ok()?,
        length: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        identity,
    })
}
async fn version(env: &AccountEnvironment, force: bool) -> String {
    static PROBE: OnceLock<crate::cli_probe::Probe> = OnceLock::new();
    static SIGNATURES: OnceLock<Mutex<BTreeMap<[u8; 32], Signature>>> = OnceLock::new();
    let Some(signature) = crate::cli_probe::resolve_program("codex", env).and_then(signature)
    else {
        return "indisponível".into();
    };
    let probe = PROBE.get_or_init(|| {
        let mut probe = crate::cli_probe::Probe::default();
        probe.ttl = Duration::from_secs(300);
        probe.timeout = Duration::from_secs(2);
        probe
    });
    let key = crate::cli_probe::environment_key(env);
    {
        let mut signatures = SIGNATURES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if force || signatures.get(&key) != Some(&signature) {
            probe.invalidate_cli("codex");
            signatures.insert(key, signature);
        }
    }
    probe
        .version_stdout(env, "codex")
        .await
        .unwrap_or_else(|| "indisponível".into())
}

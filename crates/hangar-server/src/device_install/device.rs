//! O aparelho de verdade, pelo usbmuxd (Linux/macOS) ou pelo serviço da Apple (Windows).
//! Portado do device-spike, provado num iPad com iPadOS 26.5.
use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use futures_util::future::BoxFuture;
use idevice::afc::errors::AfcError;
use idevice::afc::opcode::AfcFopenMode;
use idevice::afc::AfcClient;
use idevice::house_arrest::HouseArrestClient;
use idevice::installation_proxy::InstallationProxyClient;
use idevice::lockdown::LockdownClient;
use idevice::misagent::MisagentClient;
use idevice::provider::UsbmuxdProvider;
use idevice::remote_pairing::{RemotePairingLockdownService, RpPairingFile};
use idevice::usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection};
use idevice::{IdeviceError, IdeviceService};
use plist::Dictionary;

use super::pairing::{self, ALT_FILE, LOCKDOWN_FILE, REMOTE_FILE};
use super::job::Failure;
use super::{DeviceError, DeviceInfo, DeviceLink};

const LABEL: &str = "hangar";
// O aviso "Confiar" fica na tela até a pessoa responder; sem prazo o pedido nunca volta.
const TRUST_WAIT: Duration = Duration::from_secs(120);

pub struct UsbLink {
    /// Cache do RPPairing de UMA instalação: o arquivo assinado no IPA e o gravado no app têm de ser
    /// o mesmo. Reset do aparelho ou troca de confiança o invalidam, por isso cada instalação gera outro.
    state_dir: PathBuf,
}

impl UsbLink {
    /// `hangar_dir` é o `~/.hangar` do `AppState`.
    pub fn new(hangar_dir: &Path) -> Self {
        Self { state_dir: hangar_dir.join("devices") }
    }

    async fn pairing_files(&self, udid: &str, p: &UsbmuxdProvider) -> Result<Vec<(&'static str, Vec<u8>)>, DeviceError> {
        let mut pr = mux().await?.get_pair_record(udid).await.map_err(|_| DeviceError::NotTrusted)?;
        pr.udid = Some(udid.to_string());
        let mut lc = LockdownClient::connect(p).await.map_err(from_idevice)?;
        lc.start_session(&pr).await.map_err(from_idevice)?;
        // Aqui a versão escolhe os arquivos: erro de leitura sobe, nunca vira "".
        let ios = lc
            .get_value(Some("ProductVersion"), None)
            .await
            .map_err(from_idevice)?
            .as_string()
            .map(str::to_string)
            .unwrap_or_default();
        lc.set_value("EnableWifiDebugging", true.into(), Some("com.apple.mobile.wireless_lockdown"))
            .await
            .map_err(from_idevice)?;
        let lockdown = dict(&pr.serialize().map_err(from_idevice)?)?;
        let rp = if pairing::needs_rppairing(&ios) { Some(self.rppairing(udid, p).await?) } else { None };
        pairing::files(&ios, &lockdown, rp.as_ref())
    }

    async fn rppairing(&self, udid: &str, p: &UsbmuxdProvider) -> Result<Dictionary, DeviceError> {
        let file = self.rppairing_path(udid)?;
        if let Ok(cached) = std::fs::read(&file)
            && let Ok(d) = dict(&cached)
        {
            return Ok(d);
        }
        let host = format!("hangar-{}", &udid[udid.len().saturating_sub(6)..]);
        let svc = RemotePairingLockdownService::connect(p).await.map_err(from_idevice)?;
        let mut client = svc.into_client(&host).map_err(from_idevice)?;
        let mut rp = RpPairingFile::generate(&host);
        client.connect(&mut rp, async || "000000".to_string()).await.map_err(from_idevice)?;
        let bytes = rp.to_bytes();
        create_private_dir(&self.state_dir).map_err(io_err)?;
        crate::pages::store::write_atomic(&file, &bytes).map_err(io_err)?;
        dict(&bytes)
    }

    fn rppairing_path(&self, udid: &str) -> Result<PathBuf, DeviceError> {
        // O udid vem do cliente e vira nome de arquivo.
        if udid.is_empty() || !udid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(DeviceError::Other(format!("invalid udid {udid:?}")));
        }
        Ok(self.state_dir.join(format!("rppairing-{udid}.plist")))
    }

    fn forget_rppairing(&self, udid: &str) -> Result<(), DeviceError> {
        match std::fs::remove_file(self.rppairing_path(udid)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io_err(e)),
            _ => Ok(()),
        }
    }
}

impl DeviceLink for UsbLink {
    fn list(&self) -> BoxFuture<'_, Result<Vec<DeviceInfo>, DeviceError>> {
        Box::pin(async move {
            let mut m = match mux().await {
                Ok(m) => m,
                Err(DeviceError::ServiceMissing) => return without_mux(cfg!(target_os = "linux"), usbmuxd_installed),
                Err(e) => return Err(e),
            };
            let mut out = Vec::new();
            for d in m.get_devices().await.map_err(from_idevice)? {
                if !matches!(d.connection_type, Connection::Usb) {
                    continue;
                }
                let p = d.to_provider(addr()?, LABEL);
                // Um aparelho que não responde não esconde os outros: vai na lista com o próprio erro.
                let mut lc = match LockdownClient::connect(&p).await {
                    Ok(lc) => lc,
                    Err(e) => {
                        let error = Some(Failure::from(from_idevice(e)));
                        out.push(DeviceInfo { udid: d.udid, name: String::new(), ios_version: String::new(), trusted: false, error });
                        continue;
                    }
                };
                let name = lockdown_str(&mut lc, "DeviceName").await;
                let ios_version = lockdown_str(&mut lc, "ProductVersion").await;
                // Confiado = há registro de pareamento E o aparelho aceita sessão com ele. Bloqueado ou
                // desconectado não diz nada sobre a confiança: vai como erro, não como "não confiado".
                let (trusted, error) = match m.get_pair_record(&d.udid).await {
                    Ok(pr) => match lc.start_session(&pr).await.map_err(from_idevice) {
                        Ok(_) => (true, None),
                        Err(e @ (DeviceError::Locked | DeviceError::Disconnected)) => (false, Some(Failure::from(e))),
                        Err(_) => (false, None),
                    },
                    Err(_) => (false, None),
                };
                out.push(DeviceInfo { udid: d.udid, name, ios_version, trusted, error });
            }
            Ok(out)
        })
    }

    fn request_trust<'a>(&'a self, udid: &'a str) -> BoxFuture<'a, Result<(), DeviceError>> {
        Box::pin(async move {
            let p = provider(udid).await?;
            let mut m = mux().await?;
            if let Ok(pr) = m.get_pair_record(udid).await {
                let mut lc = LockdownClient::connect(&p).await.map_err(from_idevice)?;
                if lc.start_session(&pr).await.is_ok() {
                    return Ok(());
                }
            }
            let buid = m.get_buid().await.map_err(from_idevice)?;
            let host_id = crate::session_open::open::uuid4(&crate::mods::state::random_hex(16)).to_uppercase();
            let mut lc = LockdownClient::connect(&p).await.map_err(from_idevice)?;
            let pr = tokio::time::timeout(TRUST_WAIT, lc.pair(host_id, buid, Some("Hangar")))
                .await
                .map_err(|_| DeviceError::NotTrusted)?
                .map_err(from_idevice)?;
            m.save_pair_record(udid, pr.serialize().map_err(from_idevice)?).await.map_err(from_idevice)
        })
    }

    fn dev_apps<'a>(&'a self, udid: &'a str) -> BoxFuture<'a, Result<u32, DeviceError>> {
        Box::pin(async move {
            let p = provider(udid).await?;
            let mut mis = MisagentClient::connect(&p).await.map_err(from_idevice)?;
            let profiles = mis.copy_all().await.map_err(from_idevice)?;
            let mut ip = InstallationProxyClient::connect(&p).await.map_err(from_idevice)?;
            let installed = ip.get_apps(Some("User"), None).await.map_err(from_idevice)?.into_keys().collect();
            Ok(count_dev_apps(&profiles, SystemTime::now(), &installed))
        })
    }

    fn install_sidestore<'a>(&'a self, udid: &'a str, signed_ipa: &'a Path) -> BoxFuture<'a, Result<(), DeviceError>> {
        Box::pin(async move {
            let p = provider(udid).await?;
            idevice::utils::installation::install_package(&p, signed_ipa, None).await.map_err(from_idevice)
        })
    }

    fn is_installed<'a>(&'a self, udid: &'a str, bundle: &'a str) -> BoxFuture<'a, Result<bool, DeviceError>> {
        Box::pin(async move {
            let p = provider(udid).await?;
            let mut ip = InstallationProxyClient::connect(&p).await.map_err(from_idevice)?;
            let apps = ip.get_apps(Some("User"), Some(vec![bundle.to_string()])).await.map_err(from_idevice)?;
            Ok(apps.contains_key(bundle))
        })
    }

    fn write_pairing<'a>(&'a self, udid: &'a str, app_bundle: &'a str) -> BoxFuture<'a, Result<(), DeviceError>> {
        Box::pin(async move {
            let p = provider(udid).await?;
            let files = self.pairing_files(udid, &p).await?;
            let ha = HouseArrestClient::connect(&p).await.map_err(from_idevice)?;
            let mut afc = ha.vend_documents(app_bundle.to_string()).await.map_err(from_idevice)?;
            ignore_already_done(afc.mk_dir("/Documents").await)?;
            // Arquivo de uma gravação anterior que não vale mais nesta versão deixa o SideStore "ambiguous".
            for stale in [ALT_FILE, REMOTE_FILE, LOCKDOWN_FILE] {
                if !files.iter().any(|(n, _)| *n == stale) {
                    ignore_already_done(afc.remove(format!("/Documents/{stale}")).await)?;
                }
            }
            for (name, bytes) in &files {
                write_file(&mut afc, &format!("/Documents/{name}"), bytes).await?;
            }
            Ok(())
        })
    }

    fn write_sidestore_prefs<'a>(&'a self, udid: &'a str, app_bundle: &'a str) -> BoxFuture<'a, Result<(), DeviceError>> {
        Box::pin(async move {
            let p = provider(udid).await?;
            let ha = HouseArrestClient::connect(&p).await.map_err(from_idevice)?;
            let mut afc = ha.vend_container(app_bundle.to_string()).await.map_err(from_idevice)?;
            let path = format!("/Library/Preferences/{app_bundle}.plist");
            let existing = match afc.open(path.clone(), AfcFopenMode::RdOnly).await {
                Ok(mut f) => {
                    let bytes = f.read_entire().await.map_err(from_idevice)?;
                    f.close().await.map_err(from_idevice)?;
                    bytes
                }
                Err(e) if is_not_found(&e) => Vec::new(),
                Err(e) => return Err(from_idevice(e)),
            };
            let merged = pairing::merge_sidestore_prefs(&existing)?;
            ignore_already_done(afc.mk_dir("/Library/Preferences").await)?;
            // O cabeçalho antes do servidor: falhando aqui, as preferências da pessoa ficam como estavam.
            write_file(&mut afc, pairing::ANISETTE_CONFIG_FILE, &pairing::sidestore_anisette_config()).await?;
            write_file(&mut afc, &path, &merged).await
        })
    }

    fn pairing_file<'a>(&'a self, udid: &'a str) -> BoxFuture<'a, Result<Vec<u8>, DeviceError>> {
        Box::pin(async move {
            let p = provider(udid).await?;
            // É o começo de cada instalação: o `write_pairing` dela reaproveita o que for gerado aqui.
            self.forget_rppairing(udid)?;
            // O primeiro é o que o SideStore lê nesta versão (o mesmo que o spike provou).
            let (_, bytes) = self.pairing_files(udid, &p).await?.into_iter().next()
                .ok_or_else(|| DeviceError::Other("no pairing file".into()))?;
            Ok(bytes)
        })
    }
}

/// Erro do idevice para o que a tela sabe explicar.
pub fn from_idevice(e: IdeviceError) -> DeviceError {
    match e {
        IdeviceError::DeviceNotFound | IdeviceError::Socket(_) => DeviceError::Disconnected,
        IdeviceError::DeviceLocked | IdeviceError::PasswordProtected => DeviceError::Locked,
        IdeviceError::InvalidHostID
        | IdeviceError::UserDeniedPairing
        | IdeviceError::PairingDialogResponsePending
        | IdeviceError::SessionInactive => DeviceError::NotTrusted,
        e => DeviceError::Other(e.to_string()),
    }
}

fn io_err(e: std::io::Error) -> DeviceError {
    DeviceError::Other(e.to_string())
}

fn addr() -> Result<UsbmuxdAddr, DeviceError> {
    UsbmuxdAddr::from_env_var().map_err(|e| DeviceError::Other(format!("USBMUXD_SOCKET_ADDRESS: {e}")))
}

// Não conectar no usbmuxd (ou no serviço da Apple no Windows) é sempre "falta o serviço".
async fn mux() -> Result<UsbmuxdConnection, DeviceError> {
    mux_at(addr()?).await
}

async fn mux_at(addr: UsbmuxdAddr) -> Result<UsbmuxdConnection, DeviceError> {
    addr.connect(0).await.map_err(|_| DeviceError::ServiceMissing)
}

async fn provider(udid: &str) -> Result<UsbmuxdProvider, DeviceError> {
    let d = mux().await?.get_device(udid).await.map_err(from_idevice)?;
    Ok(d.to_provider(addr()?, LABEL))
}

async fn lockdown_str(lc: &mut LockdownClient, key: &str) -> String {
    lc.get_value(Some(key), None).await.ok().and_then(|v| v.as_string().map(str::to_string)).unwrap_or_default()
}

fn dict(bytes: &[u8]) -> Result<Dictionary, DeviceError> {
    plist::Value::from_reader(Cursor::new(bytes))
        .ok()
        .and_then(plist::Value::into_dictionary)
        .ok_or_else(|| DeviceError::Other("plist is not a dictionary".into()))
}

/// Só ocupa vaga o perfil de desenvolvimento válido de um app ainda instalado: quem apagou apps
/// do SideStore continua com os perfis deles no aparelho.
fn count_dev_apps(profiles: &[Vec<u8>], now: SystemTime, installed: &HashSet<String>) -> u32 {
    profiles.iter().filter_map(|raw| active_dev_app(raw, now)).filter(|b| installed.contains(b)).count() as u32
}

/// Perfis vêm embrulhados em CMS; o plist XML está em claro dentro deles.
fn active_dev_app(raw: &[u8], now: SystemTime) -> Option<String> {
    let start = raw.windows(5).position(|w| w == b"<?xml")?;
    let end = raw.windows(8).rposition(|w| w == b"</plist>")?;
    let d = dict(&raw[start..end + 8]).ok()?;
    if SystemTime::from(d.get("ExpirationDate")?.as_date()?) <= now {
        return None;
    }
    let ent = d.get("Entitlements")?.as_dictionary()?;
    if !ent.get("get-task-allow")?.as_boolean()? {
        return None;
    }
    // "TEAMID.com.exemplo.app" → o bundle instalado.
    let (_, bundle) = ent.get("application-identifier")?.as_string()?.split_once('.')?;
    Some(bundle.to_string())
}

fn is_not_found(e: &IdeviceError) -> bool {
    matches!(e, IdeviceError::Afc(AfcError::ObjectNotFound))
}

/// Pasta que já existe ou arquivo que já não existe: o estado pedido já está lá.
fn ignore_already_done(r: Result<(), IdeviceError>) -> Result<(), DeviceError> {
    match r {
        Err(e) if !is_not_found(&e) && !matches!(e, IdeviceError::Afc(AfcError::ObjectExists)) => Err(from_idevice(e)),
        _ => Ok(()),
    }
}

async fn write_file(afc: &mut AfcClient, path: &str, bytes: &[u8]) -> Result<(), DeviceError> {
    let mut f = afc.open(path.to_string(), AfcFopenMode::Wr).await.map_err(from_idevice)?;
    f.write_entire(bytes).await.map_err(from_idevice)?;
    f.close().await.map_err(from_idevice)
}

/// No Linux o udev só sobe o usbmuxd com um aparelho Apple no cabo: sem o socket e com ele
/// instalado, é só não ter aparelho ligado.
fn without_mux(linux: bool, installed: impl FnOnce() -> bool) -> Result<Vec<DeviceInfo>, DeviceError> {
    if linux && installed() { Ok(Vec::new()) } else { Err(DeviceError::ServiceMissing) }
}

fn usbmuxd_installed() -> bool {
    const FIXED: [&str; 5] = ["/usr/sbin/usbmuxd", "/usr/bin/usbmuxd", "/usr/lib/systemd/system/usbmuxd.service",
        "/lib/systemd/system/usbmuxd.service", "/etc/systemd/system/usbmuxd.service"];
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|d| d.join("usbmuxd"));
    FIXED.iter().map(PathBuf::from).chain(on_path).any(|p| p.is_file())
}

pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idevice_errors_map_to_screen_codes() {
        assert_eq!(from_idevice(IdeviceError::DeviceNotFound), DeviceError::Disconnected);
        assert_eq!(from_idevice(IdeviceError::Socket(std::io::ErrorKind::BrokenPipe.into())), DeviceError::Disconnected);
        assert_eq!(from_idevice(IdeviceError::DeviceLocked), DeviceError::Locked);
        assert_eq!(from_idevice(IdeviceError::PasswordProtected), DeviceError::Locked);
        assert_eq!(from_idevice(IdeviceError::UserDeniedPairing), DeviceError::NotTrusted);
        assert_eq!(from_idevice(IdeviceError::InvalidHostID), DeviceError::NotTrusted);
        assert!(matches!(from_idevice(IdeviceError::Timeout), DeviceError::Other(_)));
    }

    #[tokio::test]
    async fn no_usbmuxd_is_service_missing() {
        let nobody = UsbmuxdAddr::TcpSocket("127.0.0.1:1".parse().unwrap());
        assert_eq!(mux_at(nobody).await.err(), Some(DeviceError::ServiceMissing));
    }

    #[test]
    fn linux_without_socket_is_no_device_only_with_usbmuxd_installed() {
        assert_eq!(without_mux(true, || true), Ok(vec![]));
        assert_eq!(without_mux(true, || false), Err(DeviceError::ServiceMissing));
        assert_eq!(without_mux(false, || panic!("fora do Linux não se procura o usbmuxd")), Err(DeviceError::ServiceMissing));
    }

    #[test]
    fn udid_that_is_not_hex_never_becomes_a_path() {
        let link = UsbLink::new(Path::new("/h"));
        assert!(link.rppairing_path("../x").is_err());
        assert_eq!(link.rppairing_path("0000-AB").unwrap(), Path::new("/h/devices/rppairing-0000-AB.plist"));
    }

    #[test]
    fn each_install_starts_without_the_cached_rppairing() {
        let home = tempfile::tempdir().unwrap();
        let link = UsbLink::new(home.path());
        assert!(link.forget_rppairing("0000-AB").is_ok(), "nothing cached yet is fine");
        let file = link.rppairing_path("0000-AB").unwrap();
        create_private_dir(&link.state_dir).unwrap();
        std::fs::write(&file, b"stale").unwrap();
        link.forget_rppairing("0000-AB").unwrap();
        assert!(!file.exists());
    }

    fn profile(expires: &str, app_id: &str) -> Vec<u8> {
        let mut raw = b"\x30\x82junk".to_vec();
        raw.extend_from_slice(
            format!(
                "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
                 <key>ExpirationDate</key><date>{expires}</date>\
                 <key>Entitlements</key><dict><key>get-task-allow</key><true/>\
                 <key>application-identifier</key><string>{app_id}</string></dict></dict></plist>"
            )
            .as_bytes(),
        );
        raw.extend_from_slice(b"\x00sig");
        raw
    }

    #[test]
    fn only_unexpired_profiles_of_installed_apps_use_a_slot() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000); // 2027-01-15
        let installed: std::collections::HashSet<String> = ["com.a.app".into(), "com.b.app".into()].into();
        let profiles = [
            profile("2027-01-20T00:00:00Z", "TEAM1.com.a.app"),
            profile("2026-12-01T00:00:00Z", "TEAM1.com.b.app"), // vencido
            profile("2027-01-20T00:00:00Z", "TEAM1.com.gone.app"), // app apagado
            b"no plist".to_vec(),
        ];
        assert_eq!(count_dev_apps(&profiles, now, &installed), 1);
    }
}

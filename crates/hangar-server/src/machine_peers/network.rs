//! Sondas assíncronas com prazos e sem ocupar o pool Python.
use super::{Error, Service};
use crate::orchestration::truthy;
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub fn lan_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .ok()
        .and_then(|s| {
            s.connect("192.0.2.1:80").ok()?;
            Some(s.local_addr().ok()?.ip().to_string())
        })
        .unwrap_or_else(|| "127.0.0.1".into())
}
pub fn lan_url(s: &Service) -> String {
    let configured = s.value("CP_LAN_BIND_IP");
    let bind = if configured.is_empty() {
        s.bind.as_str()
    } else {
        configured.as_str()
    };
    if ["127.0.0.1", "localhost", "::1"].contains(&bind) {
        return String::new();
    }
    let host = if ["0.0.0.0", "::", "auto"].contains(&bind) {
        lan_ip()
    } else {
        bind.into()
    };
    if host == "127.0.0.1" {
        String::new()
    } else {
        format!("http://{host}:{}", s.port)
    }
}
pub async fn tailscale() -> Value {
    let mut command = tokio::process::Command::new("tailscale");
    command.args(["status", "--json"]).kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(2), command.output()).await {
        Ok(Ok(output)) => serde_json::from_str::<Value>(&String::from_utf8_lossy(&output.stdout))
            .ok()
            .filter(Value::is_object)
            .unwrap_or(json!({})),
        _ => json!({}),
    }
}
fn dns(status: &Value) -> String {
    status["Self"]["DNSName"]
        .as_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .into()
}
/// Um cliente por política de redirect, montado uma vez; sem conexão reaproveitada, cada sonda
/// prova o alcance com uma conexão nova.
fn client(redirect: bool) -> Result<reqwest::Client, reqwest::Error> {
    static CLIENTS: [std::sync::OnceLock<reqwest::Client>; 2] = [std::sync::OnceLock::new(), std::sync::OnceLock::new()];
    let slot = &CLIENTS[usize::from(redirect)];
    if let Some(client) = slot.get() {
        return Ok(client.clone());
    }
    let built = crate::http_client()
        .timeout(Duration::from_secs(3))
        .pool_max_idle_per_host(0)
        .redirect(if redirect {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        })
        .build()?;
    Ok(slot.get_or_init(|| built).clone())
}
fn reason(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        let mut cause: Option<&dyn std::error::Error> = Some(error);
        while let Some(e) = cause {
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused)
            {
                return "recusou";
            }
            cause = e.source();
        }
        "erro"
    } else {
        "erro"
    }
}
pub async fn probe(url: &str) -> Value {
    if url.is_empty() {
        return json!({"estado":"nao_configurado","tempo_ms":null,"motivo":""});
    }
    let start = Instant::now();
    let result = match client(false) {
        Ok(c) => c.get(format!("{url}/")).send().await,
        Err(e) => Err(e),
    };
    let ms = (start.elapsed().as_secs_f64() * 1000.0).round_ties_even() as u128;
    match result {
        Ok(_) => json!({"estado":"ok","tempo_ms":ms,"motivo":""}),
        Err(e) => json!({"estado":"falhou","tempo_ms":ms,"motivo":reason(&e)}),
    }
}
async fn get(url: &str, path: &str, token: Option<&str>) -> Result<(u16, Value), reqwest::Error> {
    let mut r = client(true)?.get(format!("{url}{path}"));
    if let Some(token) = token.filter(|v| !v.is_empty()) {
        r = r.bearer_auth(token);
    }
    let r = r.send().await?;
    let status = r.status().as_u16();
    let bytes = r.bytes().await?;
    Ok((
        status,
        serde_json::from_str(&String::from_utf8_lossy(&bytes)).unwrap_or(Value::Null),
    ))
}
/// Esquema, host e porta: `http://` no host certo levaria o token em claro.
fn same_origin(url: &str, base: &Value) -> bool {
    let origin = |raw: &str| reqwest::Url::parse(raw).ok().map(|url| url.origin());
    let (Some(url), Some(base)) = (origin(url), base.as_str().and_then(origin)) else { return false };
    url == base
}
pub async fn check(s: &Service, url: &str, id: &str) -> Value {
    if url.is_empty() {
        return json!({"estado":"nao_configurado","identificador":"","motivo":"","tempo_ms":null});
    }
    let start = Instant::now();
    let data = s.store.tolerant();
    let token = data
        .get(id)
        .filter(|v| truthy(&v["base_url"]) && truthy(&v["token"]))
        // Aparelho não escolhe para onde vai o token de outra máquina: `/api/peers/{id}/token` já o recusa.
        .filter(|v| s.device_caller.is_none() || same_origin(url, &v["base_url"]))
        .and_then(|v| v["token"].as_str());
    let result = get(url, "/api/peers/identificador", token).await.map_err(|e| reason(&e));
    let ms = (start.elapsed().as_secs_f64() * 1000.0).round_ties_even() as u128;
    match result {
        Err(e) => json!({"estado":"falhou","identificador":"","motivo":e,"tempo_ms":ms}),
        Ok((401, _)) => {
            json!({"estado":"recusou","identificador":"","motivo":"credencial","tempo_ms":ms})
        }
        Ok((_, body)) => {
            let reported = body["identificador"].as_str();
            let ok = reported == Some(id);
            json!({"estado":if ok{"ok"}else{"estranho"},"identificador":reported.unwrap_or(""),"motivo":if ok{""}else{"identidade"},"tempo_ms":ms})
        }
    }
}
pub async fn discover(s: &Service) -> Result<Value, Error> {
    discover_with(s, tailscale().await).await
}
/// `status` é o `tailscale status --json`: vazio, sobra a conta do Connect. Máquina vista pelas duas
/// fontes fica com o endereço do Tailscale. Conta que falha é erro quando não sobra nenhuma linha; com
/// linhas do Tailscale, cada uma leva `connect_error`, senão a lista pareceria completa.
pub async fn discover_with(s: &Service, status: Value) -> Result<Value, Error> {
    let (rows, account) = tokio::join!(tailscale_rows(s, &status), account_machines(s));
    let mut connect_error = None;
    let machines = match account {
        None if !truthy(&status) => {
            return Err(Error {
                status: 503,
                code: "descoberta_sem_tailscale",
                msg: "tailscale não respondeu".into(),
            });
        }
        None => Vec::new(),
        Some(Ok(machines)) => machines,
        Some(Err(error)) if !truthy(&status) || rows.is_empty() => {
            return Err(Error { status: error.status.as_u16(), code: error.code, msg: error.msg });
        }
        Some(Err(error)) => {
            tracing::warn!(code = error.code, "descoberta só pelo Tailscale: {}", error.msg);
            connect_error = Some(json!({"code": error.code, "msg": error.msg}));
            Vec::new()
        }
    };
    let mut rows = crate::connect::peers::join_discovery(rows, machines);
    rows.sort_by(|a, b| a["nome"].as_str().cmp(&b["nome"].as_str()));
    if let Some(error) = connect_error {
        rows.iter_mut().for_each(|row| row["connect_error"] = error.clone());
    }
    Ok(json!(rows))
}
/// `None`: esta máquina não está ligada ao Connect.
async fn account_machines(s: &Service) -> Option<Result<Vec<crate::connect::peers::AccountMachine>, crate::connect::ConnectError>> {
    let dir = s.connect_dir.clone()?;
    match tokio::task::spawn_blocking(move || crate::connect::state::read(&dir)).await.ok()? {
        Ok(linked) => Some(crate::connect::peers::account_machines(&linked?).await),
        Err(error) => Some(Err(error)),
    }
}
async fn tailscale_rows(s: &Service, status: &Value) -> Vec<Value> {
    let peers = status["Peer"]
        .as_object()
        .map(|m| {
            m.values()
                .filter(|v| v.is_object() && truthy(&v["Online"]))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let port = s.port;
    let results=stream::iter(peers).map(|peer|async move{let ips:Vec<String>=peer["TailscaleIPs"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|s|!s.contains(':')).map(str::to_owned).collect();let dns=peer["DNSName"].as_str().unwrap_or("").trim_end_matches('.');let mut urls:Vec<_>=ips.iter().map(|ip|format!("http://{ip}:{port}")).collect();let mut hosts=ips;if !dns.is_empty(){urls.push(format!("https://{dns}"));hosts.push(dns.into());}for url in urls{let recognized=match get(&url,"/api/peers/ping",None).await{Ok((200,b))=>b["hangar"]==true,Ok((404,_))=>matches!(get(&url,"/api/peers/identificador",None).await,Ok((401,ref b)) if b["detail"]["code"]=="erro_nao_autorizado"),_=>false};if recognized{return Some(json!({"nome":peer["HostName"].as_str().unwrap_or(""),"base_url":url,"hosts":hosts}))}}None}).buffer_unordered(8).collect::<Vec<_>>().await;
    results.into_iter().flatten().collect()
}
pub async fn reach(s: &Service) -> Value {
    let configured = s.value("CP_LAN_BIND_IP");
    let bind = if configured.is_empty() {
        s.bind.clone()
    } else if configured == "auto" {
        lan_ip()
    } else {
        configured
    };
    let local = ["127.0.0.1", "localhost", "::1"].contains(&bind.as_str());
    let front = s
        .value("CP_FRONT_PORT")
        .parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .unwrap_or(s.port);
    let mut ports = vec![front];
    if front != s.port {
        ports.push(s.port)
    }
    let mut rows: Vec<(&str, Vec<String>)> = Vec::new();
    if local {
        rows.push((
            "nesta_maquina",
            ports.iter().map(|p| format!("http://{bind}:{p}")).collect(),
        ))
    }
    let lan = lan_ip();
    rows.push((
        "rede_local",
        ports.iter().map(|p| format!("http://{lan}:{p}")).collect(),
    ));
    let name = dns(&tailscale().await);
    if !name.is_empty() {
        rows.push(("tailscale", vec![format!("https://{name}")]))
    }
    rows.push((
        "publico",
        vec![s.value("CP_PUBLIC_URL").trim_end_matches('/').into()],
    ));
    let output = futures_util::future::join_all(rows.into_iter().map(|(kind, urls)| async move {
        let results = futures_util::future::join_all(urls.iter().map(|u| probe(u))).await;
        let index = results
            .iter()
            .position(|v| v["estado"] == "ok")
            .unwrap_or(results.len() - 1);
        let mut row = results[index].clone();
        row["tipo"] = json!(kind);
        row["url"] = json!(urls[index]);
        row
    }))
    .await;
    json!({"loopback":local,"bind":bind,"enderecos":output,"connect":s.connect})
}
/// Pela marca ou pelo endereço: o dono também grava par do Connect, e aí a marca não existe.
fn through_connect(s: &Service, entry: &Value) -> bool {
    super::store::is_device_entry(entry)
        || entry["base_url"].as_str().is_some_and(|url| crate::connect::peers::on_connect_host(s.connect.as_deref(), url))
}
/// O corpo de `POST /api/peers` para `entry` e o aparelho renovado nele com o token novo, ou o motivo
/// de não mandar nada. Pelo Connect vai um token novo do aparelho desse par: a senha do dono é
/// recusada lá e não sai desta máquina.
async fn announcement(s: &Service, id: &str, base: &str, entry: &Value) -> Result<(Value, Option<(String, String)>), &'static str> {
    if through_connect(s, entry) {
        let (Some(link), Some(device), Some(devices)) = (s.connect.as_deref(), entry["connect_return_device"].as_str(), s.connect_devices.clone()) else {
            return Err("sem_credencial_connect");
        };
        let rotated = device.to_owned();
        let token = tokio::task::spawn_blocking(move || devices.rotate(&rotated)).await.ok().and_then(Result::ok).ok_or("sem_credencial_connect")?;
        return Ok((json!({"id":id,"base_url":link,"token":token,"connect":true}), Some((device.to_owned(), token))));
    }
    if base.is_empty() {
        return Err("sem_endereco");
    }
    Ok((json!({"id":id,"base_url":base,"token":s.token}), None))
}
/// Só depois da entrega: quem ficou com o token antigo segue entrando até a próxima.
async fn settle(s: &Service, device: String, token: String) {
    let Some(devices) = s.connect_devices.clone() else { return };
    if !matches!(tokio::task::spawn_blocking(move || devices.settle(&device, &token)).await, Ok(Ok(()))) {
        tracing::warn!(code = "connect_store_failed", "o token antigo do par segue valendo: a troca não foi gravada");
    }
}
pub async fn announce(s: &Service, old: &str) -> Value {
    let entries = s.store.tolerant();
    let ids: Vec<_> = entries
        .iter()
        .filter(|(_, v)| v.is_object() && v["enabled"] != false)
        .map(|(k, _)| k.clone())
        .collect();
    if ids.is_empty() {
        return json!([]);
    }
    let name = dns(&tailscale().await);
    let base = if name.is_empty() {
        lan_url(s)
    } else {
        format!("https://{name}")
    };
    if base.is_empty() && !entries.values().any(|entry| through_connect(s, entry)) {
        return json!([{"id":"","estado":"falhou","motivo":"sem_endereco"}]);
    }
    let id = s.identity();
    let mut client = crate::groups::peers::PeerClient::new(crate::groups::peers::PeerBook::new(Some(
        s.store.path.clone(),
    )));
    let mut environment=crate::accounts::environment::AccountEnvironment::from_map(s.environment.clone());
    environment.home=s.home.clone();
    if let Some(directory)=crate::diag::diary_dir_in(&environment){client=client.with_diagnostics(directory,s.request_id.clone());}
    let mut rows = Vec::new();
    for peer in ids {
        let Some(entry) = entries.get(&peer) else { continue };
        let (body, rotated) = match announcement(s, &id, &base, entry).await {
            Ok(announced) => announced,
            Err(motivo) => {
                rows.push(json!({"id":peer,"estado":"falhou","motivo":motivo}));
                continue;
            }
        };
        let mut result = client
            .call(&peer, reqwest::Method::POST, "/api/peers", Some(&body))
            .await
            .map(|_| ());
        if result.is_ok() && let Some((device, token)) = rotated {
            settle(s, device, token).await;
        }
        if result.is_ok() && !old.is_empty() && old != id {
            result = match client
                .call(
                    &peer,
                    reqwest::Method::DELETE,
                    &format!("/api/peers/{old}"),
                    None,
                )
                .await
            {
                Err(crate::groups::peers::PeerError::Refused { status: 404, .. }) => Ok(()),
                other => other.map(|_| ()),
            };
            if let Err(e) = result {
                rows.push(json!({"id":peer,"estado":"falhou","motivo":format!("nome antigo '{old}' ficou lá: {}",e.text(&peer))}));
                continue;
            }
        }
        rows.push(match result {
            Ok(()) => json!({"id":peer,"estado":"ok","motivo":""}),
            Err(e) => json!({"id":peer,"estado":"falhou","motivo":e.text(&peer)}),
        });
    }
    json!(rows)
}
/// O aparelho do caminho do Connect de `id` mora na outra máquina: a revogação lá, pelo endereço salvo com
/// a senha do dono dela ou pelo Connect com a própria credencial. Lida antes de a entrada sair; quem chama
/// apaga e só depois a roda, sem segurar a resposta.
pub fn connect_device_revoke(s: &Service, id: &str) -> Option<impl Future<Output = ()> + Send + 'static> {
    let entries = s.store.tolerant();
    // Com a marca, o `token` é aparelho, e aparelho não revoga o que não aprovou.
    let entry = entries.get(id).filter(|entry| !super::store::is_device_entry(entry))?;
    let device = entry["connect_device"].as_str().filter(|device| super::store::valid_device(device))?.to_owned();
    let pair = |url: &str, token: &str| Some((entry[url].as_str().filter(|v| !v.is_empty())?.to_owned(), entry[token].as_str().filter(|v| !v.is_empty())?.to_owned()));
    let (saved, connect, id) = (pair("base_url", "token"), pair("connect_url", "connect_token"), id.to_owned());
    Some(async move { revoke_device(&id, saved, connect, &device).await })
}
impl super::store::OldRoute {
    /// Caminho que saiu: o aparelho dele se revoga pelo próprio Connect, com a credencial dele.
    pub async fn revoke(self) {
        revoke_device(&self.id, None, Some((self.url, self.token)), &self.device).await;
    }
}
/// `DELETE /api/connect/devices/{device}`: primeiro em `saved` (endereço e senha do dono de lá); sem
/// atender, em `connect` (endereço do Connect e a credencial do próprio aparelho, que pode revogar a si
/// mesmo, `remove_by`), para o Tailscale fora não deixar o aparelho valendo. Revogar duas vezes não faz
/// mal: qualquer falha de transporte tenta o outro. Falha só vai ao log.
pub async fn revoke_device(label: &str, saved: Option<(String, String)>, connect: Option<(String, String)>, device: &str) {
    use crate::groups::peers::PeerError;
    if !super::store::valid_device(device) { return; }
    let path = format!("/api/connect/devices/{device}");
    let mut failure = "sem endereço".to_owned();
    for ((base, token), own) in saved.into_iter().map(|route| (route, false)).chain(connect.into_iter().map(|route| (route, true))) {
        // `pairs::call` não segue redirect: a senha do dono não vai a outro endereço.
        match crate::share::pairs::call(base.trim_end_matches('/'), Some(&token), reqwest::Method::DELETE, &path, None).await {
            Ok(_) | Err(PeerError::Refused { status: 404, .. }) => return,
            // A própria credencial recusada já não abre nada lá.
            Err(PeerError::Refused { status: 401, .. }) if own => return,
            Err(e @ PeerError::Transport(_)) => failure = e.text(label),
            Err(e) => { failure = e.text(label); break; }
        }
    }
    tracing::warn!(code = "connect.peer_revoke_failed", peer = label, device = %device, "o aparelho do Connect ficou valendo na outra máquina: {failure}");
}
pub fn qr_svg(url: &str) -> Result<String, Error> {
    let qr = qrcode::QrCode::new(url.as_bytes())
        .map_err(|_| Error::invalid("endereço de pareamento longo demais"))?;
    let n = qr.width();
    let mut path = String::new();
    for y in 0..n {
        for x in 0..n {
            if qr[(x, y)] == qrcode::Color::Dark {
                path.push_str(&format!("M{},{}h1v1h-1z", x + 1, y + 1));
            }
        }
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\"><path d=\"{path}\"/></svg>",
        n + 2,
        n + 2
    ))
}
pub async fn pairing(s: &Service, kind: &str) -> Result<Value, Error> {
    let states = reach(s).await;
    let row = states["enderecos"]
        .as_array()
        .and_then(|a| a.iter().find(|v| v["tipo"] == kind))
        .ok_or_else(|| Error {
            status: 404,
            code: "alcance_endereco_desconhecido",
            msg: "candidato de pareamento desconhecido".into(),
        })?;
    if s.token == "change-me" {
        return Err(Error {
            status: 400,
            code: "alcance_sem_credencial",
            msg: "credencial de pareamento nao configurada".into(),
        });
    }
    let url = match row["url"].as_str().filter(|value|!value.is_empty()) {
        Some(base)=>format!("{}/?token={}",base.trim_end_matches('/'),super::pairing::token_query(&s.token)),
        None=>super::doctor::pairing_url(s),
    };
    Ok(json!({"url":url,"qr_svg":qr_svg(&url)?}))
}

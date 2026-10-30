//! Sondas assíncronas com prazos e sem ocupar o pool Python.
use super::{Error, Service, store::truthy};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub trait Network: Send + Sync {
    fn lan_ip(&self) -> String;
    fn tailscale(&self) -> futures_util::future::BoxFuture<'_, Value>;
    fn get<'a>(
        &'a self,
        url: &'a str,
        path: &'a str,
        token: Option<&'a str>,
    ) -> futures_util::future::BoxFuture<'a, Result<(u16, Value), String>>;
    fn probe<'a>(&'a self, url: &'a str) -> futures_util::future::BoxFuture<'a, Value>;
    fn elapsed_ms(&self, start: Instant) -> u128 {
        (start.elapsed().as_secs_f64() * 1000.0).round_ties_even() as u128
    }
}
pub struct SystemNetwork;
impl Network for SystemNetwork {
    fn lan_ip(&self) -> String {
        lan_ip()
    }
    fn tailscale(&self) -> futures_util::future::BoxFuture<'_, Value> {
        Box::pin(tailscale())
    }
    fn get<'a>(
        &'a self,
        url: &'a str,
        path: &'a str,
        token: Option<&'a str>,
    ) -> futures_util::future::BoxFuture<'a, Result<(u16, Value), String>> {
        Box::pin(async move { get(url, path, token).await.map_err(|e| reason(&e).into()) })
    }
    fn probe<'a>(&'a self, url: &'a str) -> futures_util::future::BoxFuture<'a, Value> {
        Box::pin(probe(url))
    }
}

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
        s.network.lan_ip()
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
fn client(redirect: bool) -> Result<reqwest::Client, reqwest::Error> {
    crate::http_client()
        .timeout(Duration::from_secs(3))
        .redirect(if redirect {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        })
        .build()
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
pub async fn check(s: &Service, url: &str, id: &str) -> Value {
    if url.is_empty() {
        return json!({"estado":"nao_configurado","identificador":"","motivo":"","tempo_ms":null});
    }
    let start = Instant::now();
    let data = s.store.tolerant();
    let token = data
        .get(id)
        .filter(|v| truthy(&v["base_url"]) && truthy(&v["token"]))
        .and_then(|v| v["token"].as_str());
    let result = s.network.get(url, "/api/peers/identificador", token).await;
    let ms = s.network.elapsed_ms(start);
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
    let status = s.network.tailscale().await;
    if !truthy(&status) {
        return Err(Error {
            status: 503,
            code: "descoberta_sem_tailscale",
            msg: "tailscale não respondeu".into(),
        });
    }
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
    let results=stream::iter(peers).map(|peer|async move{let ips:Vec<String>=peer["TailscaleIPs"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|s|!s.contains(':')).map(str::to_owned).collect();let dns=peer["DNSName"].as_str().unwrap_or("").trim_end_matches('.');let mut urls:Vec<_>=ips.iter().map(|ip|format!("http://{ip}:{port}")).collect();let mut hosts=ips;if !dns.is_empty(){urls.push(format!("https://{dns}"));hosts.push(dns.into());}for url in urls{let recognized=match s.network.get(&url,"/api/peers/ping",None).await{Ok((200,b))=>b["hangar"]==true,Ok((404,_))=>matches!(s.network.get(&url,"/api/peers/identificador",None).await,Ok((401,ref b)) if b["detail"]["code"]=="erro_nao_autorizado"),_=>false};if recognized{return Some(json!({"nome":peer["HostName"].as_str().unwrap_or(""),"base_url":url,"hosts":hosts}))}}None}).buffer_unordered(8).collect::<Vec<_>>().await;
    let mut rows: Vec<Value> = results.into_iter().flatten().collect();
    rows.sort_by(|a, b| a["nome"].as_str().cmp(&b["nome"].as_str()));
    Ok(json!(rows))
}
pub async fn reach(s: &Service) -> Value {
    let configured = s.value("CP_LAN_BIND_IP");
    let bind = if configured.is_empty() {
        s.bind.clone()
    } else if configured == "auto" {
        s.network.lan_ip()
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
    let lan = s.network.lan_ip();
    rows.push((
        "rede_local",
        ports.iter().map(|p| format!("http://{lan}:{p}")).collect(),
    ));
    let name = dns(&s.network.tailscale().await);
    if !name.is_empty() {
        rows.push(("tailscale", vec![format!("https://{name}")]))
    }
    rows.push((
        "publico",
        vec![s.value("CP_PUBLIC_URL").trim_end_matches('/').into()],
    ));
    let output = futures_util::future::join_all(rows.into_iter().map(|(kind, urls)| async move {
        let results = futures_util::future::join_all(urls.iter().map(|u| s.network.probe(u))).await;
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
    json!({"loopback":local,"bind":bind,"enderecos":output})
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
    let name = dns(&s.network.tailscale().await);
    let base = if name.is_empty() {
        lan_url(s)
    } else {
        format!("https://{name}")
    };
    if base.is_empty() {
        return json!([{"id":"","estado":"falhou","motivo":"sem_endereco"}]);
    }
    let id = s.identity();
    let body = json!({"id":id,"base_url":base,"token":s.token});
    let mut client = crate::groups::peers::PeerClient::new(crate::groups::peers::PeerBook::new(Some(
        s.store.path.clone(),
    )));
    let mut environment=crate::accounts::environment::AccountEnvironment::from_map(s.environment.clone());
    environment.home=s.home.clone();
    if let Some(directory)=crate::diag::diary_dir_in(&environment){client=client.with_diagnostics(directory,s.request_id.clone());}
    let mut rows = Vec::new();
    for peer in ids {
        let mut result = client
            .call(&peer, reqwest::Method::POST, "/api/peers", Some(&body))
            .await
            .map(|_| ());
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
        Some(base)=>format!("{}/?token={}",base.trim_end_matches('/'),s.token),
        None=>super::doctor::pairing_url(s),
    };
    Ok(json!({"url":url,"qr_svg":qr_svg(&url)?}))
}

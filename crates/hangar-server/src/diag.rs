//! O diário mora no `hangar-core`; aqui fica a rota que acrescenta a versão do Python ao diário, que
//! lê o estado do servidor.
use axum::body::Body;
use serde_json::json;
use std::net::SocketAddr;

pub use hangar_core::diag::*;

/// O produtor não Git do diagnóstico continua onde já existe; só esta publicação acrescenta a versão confirmada.
pub fn version_matches(method: &axum::http::Method, path: &str) -> bool {
    method == axum::http::Method::GET && matches!(crate::workspace_http::decoded(path).as_ref(), "/api/diag" | "/api/diag/arquivo")
}

pub async fn version_route(st: std::sync::Arc<crate::routes::AppState>, peer: SocketAddr, req: axum::extract::Request) -> axum::response::Response {
    use http_body_util::BodyExt;
    let (forward, owner) = match crate::routes::gate(&st, peer, &req).await {
        Ok(value) => value, Err(error) => return crate::auth::unavailable(error),
    };
    let download = crate::workspace_http::decoded(req.uri().path()) == "/api/diag/arquivo";
    let requested = req.headers().clone();
    let response = crate::routes::pass(&st, req, &forward).await;
    if !owner || !response.status().is_success() { return response; }
    let (mut parts, body) = response.into_parts();
    let bytes = match body.collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return crate::session_write::detail(axum::http::StatusCode::BAD_GATEWAY,
            "diag_version_body_unavailable", "não foi possível ler o diário", json!({})),
    };
    let base = crate::runtime::local_policy::log_base_in(&st.accounts.env);
    let compressed = parts.headers.get(axum::http::header::CONTENT_ENCODING).is_some_and(|value| value == "gzip");
    let mut headers = parts.headers.clone();
    let transformed = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let bytes = if compressed {
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(bytes.as_ref()).read_to_end(&mut decoded).map_err(|_| ())?;
            decoded
        } else { bytes.to_vec() };
        let mut versions = crate::boot_version::PythonDiary::load(base);
        let bind = |versions: &mut crate::boot_version::PythonDiary, value: &serde_json::Value| {
            if let Err(error) = versions.bind(value) { tracing::warn!(kind = ?error.kind(), "diário: identidade Python não persistida"); }
        };
        let result = if download {
            let text = String::from_utf8(bytes).map_err(|_| ())?;
            let mut output = String::with_capacity(text.len());
            for line in text.split_inclusive('\n') {
                if let Ok(value) = serde_json::from_str(line) { bind(&mut versions, &value); }
                output.push_str(&versions.enrich_line(line));
            }
            Ok(output.into_bytes())
        } else {
            let text = std::str::from_utf8(&bytes).map_err(|_| ())?;
            versions.enrich_summary(text).map(String::into_bytes).map_err(|_| ())
        };
        let bytes = result?;
        let mut vary: Vec<String> = headers.get(axum::http::header::VARY).and_then(|value| value.to_str().ok()).unwrap_or("")
            .split(',').map(str::trim).filter(|value| !value.is_empty() && !(compressed && value.eq_ignore_ascii_case("Accept-Encoding"))).map(str::to_owned).collect();
        if compressed { headers.remove(axum::http::header::CONTENT_ENCODING); }
        // O mínimo vale para a representação final, não para o corpo antes da versão.
        let varies_by_encoding = bytes.len() >= 1024;
        let bytes = crate::routes::maybe_gzip(&requested, &mut headers, bytes);
        if varies_by_encoding && !vary.iter().any(|value| value.eq_ignore_ascii_case("Accept-Encoding")) { vary.push("Accept-Encoding".into()); }
        if vary.is_empty() { headers.remove(axum::http::header::VARY); }
        else { headers.insert(axum::http::header::VARY, vary.join(", ").parse().map_err(|_| ())?); }
        Ok::<_, ()>((bytes, headers))
    }).await;
    let (bytes, headers) = match transformed {
        Ok(Ok(value)) => value,
        _ => return crate::session_write::detail(axum::http::StatusCode::BAD_GATEWAY,
            "diag_version_body_unavailable", "não foi possível ler o diário", json!({})),
    };
    parts.headers = headers;
    parts.headers.remove(axum::http::header::TRANSFER_ENCODING);
    parts.headers.insert(axum::http::header::CONTENT_LENGTH, bytes.len().into());
    axum::http::Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
pub(crate) mod test_support {
    use serde_json::Value;
    use std::path::Path;
    pub fn rows(dir: &Path) -> Vec<Value> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .flat_map(|path| {
                let raw = std::fs::read_to_string(path).unwrap();
                let complete = raw.rsplit_once('\n').map_or("", |(complete, _)| complete);
                complete
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    pub async fn wait_for(dir: &Path, predicate: impl Fn(&Value) -> bool) -> Vec<Value> {
        use notify::Watcher;
        let (tx, mut changes) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let _ = tx.send(event);
            })
            .unwrap();
        watcher
            .watch(dir, notify::RecursiveMode::NonRecursive)
            .unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let found = rows(dir);
            if found.iter().any(&predicate) {
                return found;
            }
            tokio::time::timeout_at(deadline, changes.recv())
                .await
                .expect("o evento não chegou ao diário local")
                .expect("a observação encerrou")
                .expect("a observação falhou");
        }
    }
}

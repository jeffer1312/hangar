//! Diagramas Mermaid da conversa (`POST /api/mermaid`): desenhados no Chromium do servidor, sem rede,
//! com a biblioteca embutida no binário, e guardados por hash da fonte, do tema e da largura.
pub mod cache;

use std::io::Read as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::pages::Theme;
use crate::pages::chrome::{self, ChromeError};
use crate::routes::{AppState, cors, gate, pass};
use cache::{Cache, Entry};

pub const PATH: &str = "/api/mermaid";
pub const MERMAID_VERSION: &str = "12.2.0";
pub const SOURCE_MAX_CHARS: usize = 50_000;
pub const WIDTH_MIN: u32 = 240;
pub const WIDTH_MAX: u32 = 1600;
/// Diagrama mais alto que isto sai cortado: na largura máxima, em escala 2, já são uns 200 MB no Chromium.
pub const HEIGHT_MAX: u32 = 8000;
const MESSAGE_MAX_CHARS: usize = 2000;
// Fonte de 50 mil caracteres escapada no JSON (`\uXXXX`) cabe com folga.
const BODY_LIMIT: usize = 512 * 1024;
const BODY_TIMEOUT: Duration = Duration::from_secs(10);
const FONT: &str = "system-ui, -apple-system, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, sans-serif";
const EXPOSED: &str = "ETag, x-hangar-css-width, x-hangar-css-height";

static LIBRARY_GZ: &[u8] = include_bytes!("../../assets/mermaid/mermaid.min.js.gz");
/// Descomprimida na primeira vez e mantida: são 5,5 MB por desenho que não precisam sair do gzip de novo.
static LIBRARY: LazyLock<Option<String>> = LazyLock::new(|| {
    let mut text = String::new();
    flate2::read::GzDecoder::new(LIBRARY_GZ).read_to_string(&mut text).ok().map(|_| text)
});

#[derive(Deserialize)]
struct Wire { source: String, theme: String, width: f64 }

#[derive(Debug, PartialEq)]
pub struct Diagram { pub source: String, pub theme: Theme, pub width: u32 }

/// Corpo fora de forma é `None`. A largura é da coluna do cliente: fora da faixa, vale a borda.
pub fn parse(bytes: &[u8]) -> Option<Diagram> {
    let b: Wire = serde_json::from_slice(bytes).ok()?;
    let theme = Theme::parse(&b.theme)?;
    let source = b.source.trim();
    if source.is_empty() || source.chars().count() > SOURCE_MAX_CHARS { return None; }
    let width = b.width.round().clamp(f64::from(WIDTH_MIN), f64::from(WIDTH_MAX)) as u32;
    Some(Diagram { source: source.to_owned(), theme, width })
}

/// Política da página do desenho: só roda o script da biblioteca (pelo hash, então atributo `on*` e
/// `<script>` que um diagrama conseguisse injetar não rodam), estilo inline para o SVG do Mermaid, e
/// nenhuma conexão (`connect-src` herda `'none'`, o que cobre `fetch`, WebSocket e EventSource, que o
/// `Fetch` do CDP não vê). O `Runtime.evaluate` do CDP não passa pela política.
fn policy(library: &str) -> String {
    use base64::Engine as _;
    let hash = base64::engine::general_purpose::STANDARD.encode(ring::digest::digest(&ring::digest::SHA256, library.as_bytes()));
    format!("default-src 'none'; script-src 'sha256-{hash}'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'")
}

/// O hash da biblioteca embutida, calculado uma vez: são 5,5 MB.
static POLICY: LazyLock<Option<String>> = LazyLock::new(|| LIBRARY.as_deref().map(policy));

/// Página com a biblioteca inline e a coluna na largura máxima: o diagrama sai no tamanho natural e o
/// cliente o reduz. O SVG que o Mermaid limita (`useMaxWidth`) fica no tamanho dele; o que vem sem
/// limite encolhe para caber na coluna.
fn page(library: &str, policy: &str) -> String {
    format!("<!doctype html><html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\"><style>html,body{{margin:0;background:transparent}}\
body{{font-family:{FONT}}}#c{{width:{WIDTH_MAX}px}}#c svg{{display:block;max-width:100%;height:auto}}</style></head>\
<body><div id=\"c\"></div><script>{library}</script></body></html>")
}

/// A página é a mesma em todo desenho: montada uma vez, em vez de copiar 5,5 MB a cada pedido.
static PAGE: LazyLock<Option<String>> = LazyLock::new(|| Some(page(LIBRARY.as_deref()?, POLICY.as_deref()?)));

/// A mesma configuração do T3: `secure` impede que diretivas do diagrama (`%%{init}%%`) afrouxem a segurança.
fn config(theme: Theme) -> serde_json::Value {
    json!({
        "startOnLoad": false,
        "securityLevel": "strict",
        "suppressErrorRendering": true,
        "secure": ["secure", "securityLevel", "startOnLoad", "maxTextSize", "suppressErrorRendering", "maxEdges", "htmlLabels", "themeCSS"],
        "htmlLabels": false,
        "flowchart": {"htmlLabels": false},
        "theme": if theme == Theme::Dark { "dark" } else { "default" },
        "fontFamily": FONT,
    })
}

/// Só a falha do Mermaid vira `{error}` (422, guardado); biblioteca ausente ou DOM sem SVG lançam, e
/// lançar é 503, que não fica no cache.
fn script(source: &str, theme: Theme) -> String {
    let source = serde_json::to_string(source).expect("texto vira JSON");
    format!("(async()=>{{if(typeof mermaid===\"undefined\")throw new Error(\"mermaid ausente\");\
const c=document.getElementById(\"c\");let out;\
try{{mermaid.initialize({config});await document.fonts.ready;out=await mermaid.render(\"hangar-mermaid\",{source});}}\
catch(e){{return {{error:e instanceof Error?e.message:String(e&&e.str||e)}}}}\
c.innerHTML=out.svg;const s=c.querySelector(\"svg\");if(!s)throw new Error(\"sem svg\");\
const r=s.getBoundingClientRect();return {{clip:{{x:r.x,y:r.y,width:r.width,height:Math.min(r.height,{HEIGHT_MAX})}}}}}})()",
        config = config(theme))
}

/// Desenha um diagrama; `Entry::Invalid` é o erro do parser.
pub async fn draw(bin: Option<PathBuf>, source: &str, theme: Theme) -> Result<Entry, ChromeError> {
    let Some(page) = PAGE.as_deref() else { return Err(ChromeError::Failed("biblioteca do Mermaid ilegível")) };
    let out = chrome::render_offline(bin, page, WIDTH_MAX, theme, &script(source, theme)).await?;
    if out.blocked > 0 { tracing::debug!(code = "mermaid_requests_blocked", blocked = out.blocked, "requisições do diagrama recusadas"); }
    Ok(match out.drawn {
        Ok(shot) => Entry::Png { png: shot.png, width: shot.width, height: shot.height },
        Err(message) => Entry::Invalid(message.chars().take(MESSAGE_MAX_CHARS).collect()),
    })
}

pub enum Reply { Drawn(Entry), BadBody, Unavailable(&'static str) }

impl IntoResponse for Reply {
    fn into_response(self) -> Response {
        let json_reply = |status: StatusCode, v: serde_json::Value| (status, [(header::CONTENT_TYPE, "application/json")], v.to_string()).into_response();
        match self {
            Reply::Drawn(Entry::Png { png, width, height }) => {
                let mut r = Response::new(Body::from(png));
                let h = r.headers_mut();
                h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
                h.insert("x-hangar-css-width", HeaderValue::from(width));
                h.insert("x-hangar-css-height", HeaderValue::from(height));
                h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable"));
                h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
                r
            }
            Reply::Drawn(Entry::Invalid(message)) => json_reply(StatusCode::UNPROCESSABLE_ENTITY, json!({"code": "mermaid_invalido", "message": message})),
            Reply::BadBody => json_reply(StatusCode::BAD_REQUEST, json!({"code": "erro_corpo_invalido"})),
            Reply::Unavailable(reason) => json_reply(StatusCode::SERVICE_UNAVAILABLE, json!({"code": "mermaid_indisponivel", "message": reason})),
        }
    }
}

/// O pedido inteiro sem a autenticação: cache, desenho e gravação. `chromium` só é chamado quando falta o desenho.
/// Pedidos iguais ao mesmo tempo dividem um voo: um desenho só, o mesmo resultado para todos.
pub async fn reply(cache: Arc<Cache>, chromium: fn() -> Option<PathBuf>, body: &[u8]) -> Reply {
    let Some(req) = parse(body) else { return Reply::BadBody };
    // Desenha na largura máxima, não na da coluna (a largura do corpo só é validada): na coluna estreita um
    // diagrama largo ficaria minúsculo, e a tela cheia herdaria a resolução baixa.
    let key = cache::key(MERMAID_VERSION, req.theme, WIDTH_MAX, &req.source);
    match cache.once(&key, || produce(cache.clone(), chromium, req, key.clone())).await {
        Ok(entry) => Reply::Drawn(entry),
        Err(reason) => Reply::Unavailable(reason),
    }
}

async fn produce(cache: Arc<Cache>, chromium: fn() -> Option<PathBuf>, req: Diagram, key: String) -> cache::Outcome {
    let (c, k) = (cache.clone(), key.clone());
    // Disco e busca do binário fora da thread do runtime.
    let found = tokio::task::spawn_blocking(move || match c.get(&k) {
        Ok(Some(entry)) => Ok(entry),
        Ok(None) => Err(chromium()),
        Err(e) => {
            tracing::warn!(code = "mermaid_cache_unreadable", io_kind = ?e.kind(), "diagrama guardado ilegível; desenhando de novo");
            Err(chromium())
        }
    }).await;
    let bin = match found {
        Ok(Ok(entry)) => return Ok(entry),
        Ok(Err(bin)) => bin,
        Err(e) => {
            tracing::error!(panic = e.is_panic(), "leitura do cache do diagrama interrompida");
            return Err("o cache do diagrama caiu no servidor");
        }
    };
    tracing::info!(code = "mermaid_drawing", "desenhando diagrama");
    let entry = draw(bin, &req.source, req.theme).await.map_err(|e| e.reason())?;
    // Gravação que falha não tira o desenho de quem pediu: o próximo pedido desenha de novo.
    let (entry, saved) = match tokio::task::spawn_blocking(move || { let saved = cache.put(&key, &entry); (entry, saved) }).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!(panic = e.is_panic(), "gravação do diagrama interrompida");
            return Err("a gravação do diagrama caiu no servidor");
        }
    };
    if let Err(e) = saved { tracing::warn!(code = "mermaid_cache_unwritten", io_kind = ?e.kind(), "diagrama não guardado"); }
    Ok(entry)
}

/// Só o dono; o resto segue ao Python, que repassa o dono do Connect pela ponte e recusa os demais.
pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request<Body>) -> Response {
    let (fwd, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner { return pass(&st, req, &fwd).await; }
    let headers = req.headers().clone();
    let mut r = serve(&st, req).await;
    cors(&headers, r.headers_mut());
    if headers.contains_key(header::ORIGIN) {
        r.headers_mut().insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static(EXPOSED));
    }
    r
}

/// O pedido do dono já decidido: pela porta pública ou pela ponte privada (`forwarded`), que traz o dono do
/// Connect autenticado no Python. O CORS é de quem recebeu o pedido do cliente.
pub(crate) async fn serve(st: &AppState, req: Request<Body>) -> Response {
    let answer = match tokio::time::timeout(BODY_TIMEOUT, to_bytes(req.into_body(), BODY_LIMIT)).await {
        Ok(Ok(bytes)) => reply(st.mermaid.clone(), st.chromium, &bytes).await,
        _ => Reply::BadBody,
    };
    if let Reply::Unavailable(reason) = &answer { st.diag.report("rust.mermaid_failed", "", "mermaid_indisponivel", reason); }
    answer.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_is_the_pinned_version_and_never_closes_the_script() {
        let lib = LIBRARY.as_deref().expect("biblioteca descomprime");
        assert!(lib.contains(&format!("{{version:\"{MERMAID_VERSION}\"}}")), "MERMAID_VERSION difere do arquivo embutido");
        assert!(lib.contains("globalThis[\"mermaid\"]"), "o IIFE define o global");
        let lower = lib.to_ascii_lowercase();
        assert!(!lower.contains("<script") && !lower.contains("</script"), "fecharia o <script> da página");
        assert!(!lib.contains("import("), "pedaço carregado por import() não chega sem rede");
    }

    #[test]
    fn body_is_validated_and_width_clamped() {
        let p = |v: serde_json::Value| parse(v.to_string().as_bytes());
        assert_eq!(p(json!({"source": "  graph TD; A-->B\n", "theme": "dark", "width": 727.6})),
            Some(Diagram { source: "graph TD; A-->B".into(), theme: Theme::Dark, width: 728 }));
        assert_eq!(p(json!({"source": "x", "theme": "light", "width": 10})).unwrap().width, WIDTH_MIN);
        assert_eq!(p(json!({"source": "x", "theme": "light", "width": 99999})).unwrap().width, WIDTH_MAX);
        assert_eq!(p(json!({"source": "x", "theme": "light", "width": -5})).unwrap().width, WIDTH_MIN);
        assert!(p(json!({"source": "x", "theme": "light", "width": 300, "extra": 1})).is_some(), "campo novo de cliente mais novo não quebra");
        assert!(p(json!({"source": "é".repeat(SOURCE_MAX_CHARS), "theme": "dark", "width": 300})).is_some());
        for bad in [json!({"source": "", "theme": "dark", "width": 300}), json!({"source": " \n ", "theme": "dark", "width": 300}),
            json!({"source": "x".repeat(SOURCE_MAX_CHARS + 1), "theme": "dark", "width": 300}),
            json!({"source": "x", "theme": "sepia", "width": 300}), json!({"source": "x", "theme": "dark", "width": "300"}),
            json!({"source": "x", "theme": "dark"}), json!({"source": 1, "theme": "dark", "width": 300}), json!([1])] {
            assert_eq!(p(bad.clone()), None, "{bad}");
        }
        assert_eq!(parse(b"{nao e json"), None);
    }

    #[test]
    fn source_goes_as_json_never_raw() {
        let evil = "graph TD\nA[\"</script><script>x()</script>\"]-->B`${x}`\u{2028}";
        let s = script(evil, Theme::Dark);
        let literal = serde_json::to_string(evil).unwrap();
        assert!(s.contains(&format!("mermaid.render(\"hangar-mermaid\",{literal})")));
        assert_eq!(s.matches(evil).count(), 0, "a fonte crua não aparece no script");
        assert!(s.contains("\"securityLevel\":\"strict\"") && s.contains("\"htmlLabels\":false") && s.contains("\"theme\":\"dark\""));
        assert!(s.contains("\"themeCSS\"") && s.contains("\"maxEdges\""), "lista secure do T3");
        assert!(script("x", Theme::Light).contains("\"theme\":\"default\""));
        assert!(s.contains(&format!("Math.min(r.height,{HEIGHT_MAX})")));
        let html = page("/*lib*/", &policy("/*lib*/"));
        assert!(html.contains(&format!("#c{{width:{WIDTH_MAX}px}}")) && html.contains("<script>/*lib*/</script>"));
    }

    #[test]
    fn page_policy_runs_only_the_library_and_opens_no_connection() {
        let lib = LIBRARY.as_deref().unwrap();
        assert!(!lib.contains('\r'), "o parser HTML troca CR LF por LF e o hash deixaria de casar");
        let policy = POLICY.as_deref().unwrap();
        let html = page("/*lib*/", policy);
        let meta = format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\">");
        assert!(html.find(&meta).unwrap() < html.find("<script>").unwrap(), "a política vem antes do script");
        let digest = ring::digest::digest(&ring::digest::SHA256, lib.as_bytes());
        use base64::Engine as _;
        let hash = base64::engine::general_purpose::STANDARD.encode(digest);
        let directives: Vec<&str> = policy.split("; ").collect();
        assert_eq!(directives[0], "default-src 'none'", "connect-src, frame-src e o resto herdam o nada");
        assert!(directives.contains(&format!("script-src 'sha256-{hash}'").as_str()), "{policy}");
        assert!(!policy.contains("connect-src") && !policy.contains("unsafe-eval"));
        assert!(!directives.iter().any(|d| d.starts_with("script-src") && d.contains("unsafe-inline")), "atributo on* não roda");
        assert!(directives.contains(&"style-src 'unsafe-inline'") && directives.contains(&"img-src data:") && directives.contains(&"font-src data:"));
    }

    static ABSENT_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// Chromium ausente que demora a responder: os dois pedidos se encontram no meio do voo.
    fn slow_absent() -> Option<PathBuf> {
        ABSENT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        None
    }

    #[tokio::test]
    async fn equal_requests_at_once_share_one_flight() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path().into()));
        let body = json!({"source": "graph TD; voo-unico", "theme": "dark", "width": 400}).to_string();
        let (a, b) = tokio::join!(reply(cache.clone(), slow_absent, body.as_bytes()), reply(cache.clone(), slow_absent, body.as_bytes()));
        for r in [a, b] {
            let Reply::Unavailable(reason) = r else { panic!("503") };
            assert_eq!(reason, "sem Chromium no servidor");
        }
        assert_eq!(ABSENT_CALLS.load(std::sync::atomic::Ordering::SeqCst), 1, "um desenho só para os dois");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "indisponível não fica no cache");
        // Acabado o voo, o 503 não fica: o pedido seguinte tenta de novo.
        let Reply::Unavailable(_) = reply(cache.clone(), slow_absent, body.as_bytes()).await else { panic!("503") };
        assert_eq!(ABSENT_CALLS.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn bad_body_and_absent_chromium() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path().into()));
        assert!(matches!(reply(cache.clone(), || None, b"{}").await, Reply::BadBody));
        let body = json!({"source": "graph TD; A-->B", "theme": "dark", "width": 400}).to_string();
        let Reply::Unavailable(reason) = reply(cache.clone(), || None, body.as_bytes()).await else { panic!("503") };
        assert_eq!(reason, "sem Chromium no servidor");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "indisponível não fica no cache");
        let r = Reply::Unavailable(reason).into_response();
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn cached_entries_answer_without_chromium() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Cache::new(dir.path().into()));
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&[0, 0, 3, 0, 0, 0, 0, 200, 8, 6, 0, 0, 0]);
        cache.put(&cache::key(MERMAID_VERSION, Theme::Light, WIDTH_MAX, "graph TD; A-->B"), &Entry::Png { png: png.clone(), width: 384, height: 100 }).unwrap();
        cache.put(&cache::key(MERMAID_VERSION, Theme::Dark, WIDTH_MAX, "graf"), &Entry::Invalid("No diagram type detected".into())).unwrap();
        let chromium: fn() -> Option<PathBuf> = || panic!("acerto no cache não procura o Chromium");
        let body = json!({"source": "graph TD; A-->B", "theme": "light", "width": 400}).to_string();
        let r = reply(cache.clone(), chromium, body.as_bytes()).await.into_response();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()["content-type"], "image/png");
        assert_eq!((r.headers()["x-hangar-css-width"].to_str().unwrap(), r.headers()["x-hangar-css-height"].to_str().unwrap()), ("384", "100"));
        assert_eq!(r.headers()["cache-control"], "private, max-age=31536000, immutable");
        assert_eq!(to_bytes(r.into_body(), usize::MAX).await.unwrap(), png);
        let body = json!({"source": "graf", "theme": "dark", "width": 400}).to_string();
        let r = reply(cache, chromium, body.as_bytes()).await.into_response();
        assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let v: serde_json::Value = serde_json::from_slice(&to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(v, json!({"code": "mermaid_invalido", "message": "No diagram type detected"}));
    }

    #[tokio::test]
    #[ignore = "sobe o Chrome da máquina"]
    async fn draws_a_real_flowchart_and_reports_a_parse_error() {
        let bin = chrome::find();
        let Entry::Png { png, width, height } = draw(bin.clone(), "graph TD\n  A[Início] --> B{Decide}\n  B -->|sim| C[Fim]", Theme::Dark).await.unwrap()
            else { panic!("flowchart válido") };
        assert_eq!(cache::css_size(&png), Some((width, height)), "PNG em escala 2 exata");
        assert!(width <= WIDTH_MAX && height > 50, "{width}x{height}");
        let Entry::Invalid(message) = draw(bin, "graph TD\n  A -->", Theme::Light).await.unwrap() else { panic!("fonte inválida") };
        assert!(message.contains("Parse error"), "{message}");
    }
}

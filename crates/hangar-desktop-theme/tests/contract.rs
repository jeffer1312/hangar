use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use hangar_core::auth::{Auth, TrustedHosts};
use hangar_core::diag::DiagClient;
use hangar_core::edge::Edge;
use hangar_desktop_theme::{DesktopTheme, Warning};

const BLUE: &str = include_str!("fixtures/blue.scss");
const RED: &str = include_str!("fixtures/red.scss");

fn service(home: &Path, warnings: Arc<Mutex<Vec<Warning>>>) -> DesktopTheme {
    let root = home.to_owned();
    let pointer = home.join(".local/state/quickshell/user/generated/wallpaper/path.txt");
    DesktopTheme::new(
        Arc::new(move || Ok(root.clone())),
        Arc::new(move || {
            fs::read(pointer.clone()).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        }),
        Arc::new(move |warning| warnings.lock().unwrap().push(warning)),
    )
}

async fn request(
    app: &axum::Router,
    path: &str,
    method: &str,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut request = Request::builder()
        .uri(path)
        .method(method)
        .header("authorization", "Bearer desktop-proof");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut request = request.body(Body::empty()).unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:1".parse::<std::net::SocketAddr>().unwrap(),
    ));
    app.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
async fn desktop_contract_preserves_palette_wallpaper_and_local_http() {
    let home = tempfile::tempdir().unwrap();
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let theme = Arc::new(service(home.path(), warnings.clone()));
    let blue = theme
        .parse(BLUE)
        .expect("a paleta real azul deve ser aceita");
    assert_eq!(blue["cores"]["background"], "#111318");
    assert_eq!(blue["cores"]["surfaceContainer"], "#1D2024");
    assert_eq!(blue["cores"]["primary_paletteKeyColor"], "#5A77AB");
    assert_eq!(blue["escuro"], true);
    let red = theme.parse(RED).unwrap();
    assert_eq!(red["cores"]["background"], "#1C110D");
    assert_eq!(red["cores"]["primary"], "#AAC7FF");
    assert_eq!(
        theme
            .parse(&BLUE.replace("$darkmode: True;", "$darkmode: False;"))
            .unwrap()["escuro"],
        false
    );
    assert_eq!(
        theme.parse(&BLUE.replace("$darkmode: True;", "")).unwrap()["escuro"],
        true
    );
    assert!(
        theme
            .parse(&format!("{BLUE}\n$background: inválida;\n"))
            .is_none()
    );
    assert!(
        theme
            .parse(&BLUE.replace("$background:", " $background:"))
            .is_none()
    );
    theme.parse(BLUE).unwrap();
    let body = BLUE
        .lines()
        .filter(|line| !line.starts_with("$background:"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        theme
            .parse(&format!("\u{feff}$background: #111318;\n{body}\n"))
            .unwrap()["cores"]["background"],
        "#111318"
    );
    warnings.lock().unwrap().clear();
    assert!(theme.parse("$background: #111318;\n").is_none());
    assert!(theme.parse("$background: #111318;\n").is_none());
    assert_eq!(
        warnings
            .lock()
            .unwrap()
            .iter()
            .filter(|w| w.code == "desktop_palette_incomplete")
            .count(),
        1
    );
    assert!(theme.parse("").is_none());
    assert!(theme.palette().is_none());
    assert!(theme.parse("$background: #111318;\n").is_none());
    assert_eq!(
        warnings
            .lock()
            .unwrap()
            .iter()
            .filter(|w| w.code == "desktop_palette_incomplete")
            .count(),
        2
    );
    let generated = home.path().join(".local/state/quickshell/user/generated");
    fs::create_dir_all(generated.join("wallpaper")).unwrap();
    fs::write(generated.join("material_colors.scss"), BLUE).unwrap();
    fs::write(
        generated.join("colors.json"),
        "{\"background\":\"#f9f9ff\"}",
    )
    .unwrap();
    assert_eq!(theme.palette().unwrap()["cores"]["background"], "#111318");
    fs::write(
        generated.join("material_colors.scss"),
        RED.replace('\n', "\r\n"),
    )
    .unwrap();
    assert_eq!(theme.palette().unwrap()["cores"]["background"], "#1C110D");
    fs::remove_file(generated.join("material_colors.scss")).unwrap();
    fs::create_dir(generated.join("material_colors.scss")).unwrap();
    assert!(theme.palette().is_none());
    assert_eq!(
        warnings.lock().unwrap().last().unwrap().code,
        "desktop_palette_unreadable"
    );
    fs::remove_dir(generated.join("material_colors.scss")).unwrap();
    fs::write(
        generated.join("material_colors.scss"),
        b"$background: #11\xff\xfe1318;\n",
    )
    .unwrap();
    assert!(theme.palette().is_none());
    fs::write(generated.join("material_colors.scss"), BLUE).unwrap();
    let image = home.path().join("parede.PNG");
    fs::write(&image, b"\x89PNG fingindo ser imagem").unwrap();
    fs::File::options()
        .write(true)
        .open(&image)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(
            std::time::UNIX_EPOCH + std::time::Duration::new(1_234_567_890, 500_000_000),
        ))
        .unwrap();
    fs::write(generated.join("wallpaper/path.txt"), "~/parede.PNG\n").unwrap();
    assert_eq!(theme.wallpaper().unwrap().unwrap(), image);
    theme.parse("$background: #111318;\n");
    let warned = warnings.lock().unwrap().len();
    assert_eq!(theme.wallpaper().unwrap().unwrap(), image);
    assert!(theme.parse("$background: #111318;\n").is_none());
    assert_eq!(warnings.lock().unwrap().len(), warned);
    fs::write(generated.join("wallpaper/path.txt"), "\n").unwrap();
    assert!(theme.wallpaper().unwrap().is_none());
    theme.parse("$background: #111318;\n");
    assert_eq!(warnings.lock().unwrap().len(), warned);
    fs::write(
        generated.join("wallpaper/path.txt"),
        format!("{}\n", home.path().join("sumiu.png").display()),
    )
    .unwrap();
    assert!(theme.wallpaper().unwrap().is_none());
    let secret = home.path().join("arquivo.conf");
    fs::write(&secret, "dado de prova").unwrap();
    fs::write(
        generated.join("wallpaper/path.txt"),
        secret.to_str().unwrap(),
    )
    .unwrap();
    assert!(theme.wallpaper().unwrap().is_none());
    fs::write(
        generated.join("wallpaper/path.txt"),
        format!("\u{feff}{}", image.display()),
    )
    .unwrap();
    assert!(theme.wallpaper().unwrap().is_none());
    fs::write(
        generated.join("wallpaper/path.txt"),
        image.to_str().unwrap(),
    )
    .unwrap();
    let registry_value = image.to_string_lossy().into_owned();
    let home_warnings = Arc::new(Mutex::new(Vec::<Warning>::new()));
    let captured = home_warnings.clone();
    let registry = DesktopTheme::new(
        Arc::new(|| Err(std::io::Error::other("pasta pessoal indisponível"))),
        Arc::new(move || Ok(registry_value.clone())),
        Arc::new(move |warning| captured.lock().unwrap().push(warning)),
    );
    assert_eq!(registry.wallpaper().unwrap().unwrap(), image);
    assert!(registry.palette().is_none());
    assert!(registry.palette().is_none());
    let events = home_warnings.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.code == "desktop_palette_unreadable")
            .count(),
        1
    );
    drop(events);
    let no_home = DesktopTheme::new(
        Arc::new(|| Err(std::io::Error::other("sem pasta pessoal"))),
        Arc::new(|| Ok("~/parede.png".into())),
        Arc::new(|_| {}),
    );
    assert!(no_home.wallpaper().is_err());
    let relative_directory = tempfile::tempdir_in(".").unwrap();
    let relative_image = std::path::PathBuf::from(relative_directory.path().file_name().unwrap())
        .join("relativa.png");
    fs::write(&relative_image, b"imagem relativa").unwrap();
    let relative_value = relative_image.to_string_lossy().into_owned();
    let relative = DesktopTheme::new(
        Arc::new(|| Err(std::io::Error::other("pasta pessoal não necessária"))),
        Arc::new(move || Ok(relative_value.clone())),
        Arc::new(|_| {}),
    );
    assert_eq!(relative.wallpaper().unwrap().unwrap(), relative_image);
    for value in [None, Some("")] {
        let source = DesktopTheme::new(
            Arc::new(|| Err(std::io::Error::other("sem pasta pessoal"))),
            Arc::new(move || {
                value
                    .map(str::to_owned)
                    .ok_or_else(|| std::io::ErrorKind::NotFound.into())
            }),
            Arc::new(|_| {}),
        );
        assert!(source.wallpaper().unwrap().is_none());
    }
    let edge = Edge {
        trusted: TrustedHosts::parse("127.0.0.1"),
        auth: Auth::configured("desktop-proof"),
        diag: DiagClient::new("127.0.0.1:9".parse().unwrap(), String::new())
            .with_diary(Some(home.path().join("diary"))),
    };
    let app = hangar_desktop_theme::router::<()>(edge, theme);
    for (path, ip, auth, expected) in [
        ("palette", Some("127.0.0.1:1"), true, StatusCode::OK),
        ("palette", Some("[::1]:1"), true, StatusCode::OK),
        (
            "palette",
            Some("192.168.15.28:1"),
            true,
            StatusCode::FORBIDDEN,
        ),
        ("palette", None, true, StatusCode::FORBIDDEN),
        (
            "palette",
            Some("127.0.0.1:1"),
            false,
            StatusCode::UNAUTHORIZED,
        ),
        ("wallpaper", Some("127.0.0.1:1"), true, StatusCode::OK),
    ] {
        let mut req = Request::builder().uri(format!("/api/desktop/{path}"));
        if auth {
            req = req.header("authorization", "Bearer desktop-proof");
        }
        let mut req = req.body(Body::empty()).unwrap();
        if let Some(ip) = ip {
            req.extensions_mut()
                .insert(ConnectInfo(ip.parse::<std::net::SocketAddr>().unwrap()));
        }
        let response = app.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), expected, "{path}, {ip:?}");
        if path == "wallpaper" && expected == StatusCode::OK {
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(response.headers()["content-type"], "image/png");
            assert_eq!(
                to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
                b"\x89PNG fingindo ser imagem"
            );
        }
    }
    let image_response = request(&app, "/api/desktop/wallpaper", "GET", &[]).await;
    let etag = image_response.headers()["etag"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(etag, "\"0d8420b7115926dbfac7d57ff512fc0f\"");
    assert_eq!(
        image_response.headers()["last-modified"],
        "Fri, 13 Feb 2009 23:31:30 GMT"
    );
    let unchanged = request(
        &app,
        "/api/desktop/wallpaper",
        "GET",
        &[("if-none-match", &etag)],
    )
    .await;
    assert_eq!(unchanged.status(), StatusCode::OK);
    let part = request(
        &app,
        "/api/desktop/wallpaper",
        "GET",
        &[
            ("range", "bytes=0-3"),
            ("if-range", "Fri, 13 Feb 2009 23:31:30 GMT"),
        ],
    )
    .await;
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.headers()["content-range"], "bytes 0-3/24");
    assert_eq!(
        to_bytes(part.into_body(), 1024).await.unwrap().as_ref(),
        b"\x89PNG"
    );
    let multiple = request(
        &app,
        "/api/desktop/wallpaper",
        "GET",
        &[("range", "bytes=0-1,6-7")],
    )
    .await;
    assert_eq!(multiple.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(multiple.headers()["cache-control"], "no-store");
    let content_type = multiple.headers()["content-type"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(content_type.starts_with("multipart/byteranges; boundary="));
    let boundary = content_type.split("boundary=").nth(1).unwrap();
    let bytes = to_bytes(multiple.into_body(), 4096).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("Content-Range: bytes 0-1/24"));
    assert!(text.contains("Content-Range: bytes 6-7/24"));
    assert!(text.ends_with(&format!("--{boundary}--")));
    for method in ["POST", "HEAD"] {
        let response = request(&app, "/api/desktop/palette", method, &[]).await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()["allow"], "GET");
    }
    let redirect = request(&app, "/api/desktop/palette/?x=1", "GET", &[]).await;
    assert_eq!(redirect.status(), StatusCode::TEMPORARY_REDIRECT);
    assert!(
        redirect.headers()["location"]
            .to_str()
            .unwrap()
            .ends_with("/api/desktop/palette?x=1")
    );
    let external = request(
        &app,
        "/api/desktop/palette",
        "GET",
        &[("x-forwarded-for", "192.168.15.28")],
    )
    .await;
    assert_eq!(external.status(), StatusCode::FORBIDDEN);
    fs::remove_file(generated.join("material_colors.scss")).unwrap();
    let absent = request(&app, "/api/desktop/palette", "GET", &[]).await;
    assert_eq!(absent.status(), StatusCode::NOT_FOUND);
    let absent: serde_json::Value =
        serde_json::from_slice(&to_bytes(absent.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(absent["detail"]["code"], "erro_sem_paleta");
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 32, rng_seed: proptest::test_runner::RngSeed::Fixed(42), ..proptest::test_runner::Config::default() })]
    #[test]
    fn external_palette_bytes_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..1024)) {
        let home = tempfile::tempdir().unwrap();
        let theme = service(home.path(), Arc::new(Mutex::new(Vec::new())));
        let arbitrary = String::from_utf8_lossy(&bytes);
        if let Some(value) = theme.parse(&arbitrary) { proptest::prop_assert!(value["cores"].is_object()); }
        let comment = arbitrary.replace(['\n','\r'], " ");
        let valid = theme.parse(&format!("{BLUE}\n// {comment}"));
        proptest::prop_assert!(valid.is_some());
        let valid = valid.unwrap();
        proptest::prop_assert_eq!(valid["cores"]["background"].as_str(), Some("#111318"));
    }
}

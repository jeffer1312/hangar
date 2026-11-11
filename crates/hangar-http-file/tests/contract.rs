use axum::body::to_bytes;
use axum::http::{HeaderMap, StatusCode};

#[tokio::test]
async fn opened_files_keep_html_isolation_conditional_cache_and_ranges() {
    let root = tempfile::tempdir().unwrap();
    let html = root.path().join("page.html");
    std::fs::write(&html, "<script>window.proof=1</script>").unwrap();
    let response = hangar_http_file::serve_open_file(
        tokio::fs::File::open(&html).await.unwrap(),
        &html,
        &HeaderMap::new(),
        false,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()["etag"].clone();
    let body =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("sandbox=\"allow-scripts allow-popups\""));
    assert!(!body.contains("allow-same-origin"));
    assert!(body.contains("data:text/html;charset=utf-8;base64,"));
    let mut headers = HeaderMap::new();
    headers.insert("if-none-match", etag);
    let cached = hangar_http_file::serve_open_file(
        tokio::fs::File::open(&html).await.unwrap(),
        &html,
        &headers,
        false,
        None,
    )
    .await;
    assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
    let image = root.path().join("image.png");
    std::fs::write(&image, b"abcdefghi").unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("range", "bytes=1-3".parse().unwrap());
    let part = hangar_http_file::serve_open_file(
        tokio::fs::File::open(&image).await.unwrap(),
        &image,
        &headers,
        false,
        None,
    )
    .await;
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.headers()["content-range"], "bytes 1-3/9");
    assert_eq!(
        to_bytes(part.into_body(), 1024).await.unwrap().as_ref(),
        b"bcd"
    );
    for (raw, status) in [
        ("invalid", StatusCode::BAD_REQUEST),
        ("bytes=99-", StatusCode::RANGE_NOT_SATISFIABLE),
    ] {
        headers.insert("range", raw.parse().unwrap());
        let response = hangar_http_file::serve_open_file(
            tokio::fs::File::open(&image).await.unwrap(),
            &image,
            &headers,
            false,
            None,
        )
        .await;
        assert_eq!(response.status(), status);
    }
}

use axum::http::{HeaderMap,HeaderValue};
#[test]
fn mime_matches_received_headers_in_python_capture() {
    let fixture:serde_json::Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/http-body-input/mime.json")).unwrap();
    for row in fixture["cases"].as_array().unwrap() {
        assert_eq!(row["headers_hex"],row["received_headers_hex"]);
        let mut headers=HeaderMap::new();
        for encoded in row["headers_hex"].as_array().unwrap() {
            let bytes:Vec<u8>=encoded.as_str().unwrap().as_bytes().chunks_exact(2).map(|pair|u8::from_str_radix(std::str::from_utf8(pair).unwrap(),16).unwrap()).collect();
            headers.append("content-type",HeaderValue::from_bytes(&bytes).unwrap());
        }
        assert_eq!(hangar_server::http_body_input::is_json(&headers),row["status"]==200,"{}",row["headers_hex"]);
    }
}
proptest::proptest! {
    #[test]
    fn arbitrary_header_bytes_never_panic(bytes in proptest::collection::vec(proptest::arbitrary::any::<u8>(),0..2048)) {
        if let Ok(value)=HeaderValue::from_bytes(&bytes){let mut headers=HeaderMap::new();headers.insert("content-type",value);let _=hangar_server::http_body_input::is_json(&headers);}
    }
}

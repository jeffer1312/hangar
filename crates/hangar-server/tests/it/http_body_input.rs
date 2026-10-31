use axum::http::{HeaderMap,HeaderValue};
proptest::proptest! {
    #[test]
    fn arbitrary_header_bytes_never_panic(bytes in proptest::collection::vec(proptest::arbitrary::any::<u8>(),0..2048)) {
        if let Ok(value)=HeaderValue::from_bytes(&bytes){let mut headers=HeaderMap::new();headers.insert("content-type",value);let _=hangar_server::http_body_input::is_json(&headers);}
    }
}

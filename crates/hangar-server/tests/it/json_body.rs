use hangar_server::orchestration::body;
use proptest::prelude::*;

proptest! {
    #[test]
    fn arbitrary_json_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = body::parse(&bytes);
    }
}

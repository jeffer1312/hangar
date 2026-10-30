//! O prazo total do download não substitui o prazo de leitura do socket original.
use hangar_server::update::git_engine::releases::{HttpSource,Source};
use std::{io::{Read,Write},time::{Duration,Instant}};
#[test]
fn update_release_read_timeout_preserves_original_deadline() {
 let original:serde_json::Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/release-socket.json")).unwrap();
 let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let url=format!("http://{}/fixture",listener.local_addr().unwrap());let delay=original["response_delay"].as_u64().unwrap();
 let server=std::thread::spawn(move||{let(mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();let mut request=[0;4096];stream.read(&mut request).unwrap();stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n").unwrap();std::thread::sleep(Duration::from_secs(delay));let _=stream.write_all(b"ok");});
 let started=Instant::now();let result=HttpSource.get_release(&url,Duration::from_secs(original["deadline"].as_u64().unwrap()));let elapsed=started.elapsed().as_secs_f64();server.join().unwrap();
 assert!(result.is_err(),"leitura parada devolveu sucesso após {elapsed}s: {result:?}");
 assert_eq!(result.unwrap_err().kind,original["error"].as_str().unwrap());
 let socket=original["socket_timeout"].as_f64().unwrap();assert!(elapsed>=socket-1.0&&elapsed<=socket+2.0,"prazo de leitura: {elapsed}s");
}

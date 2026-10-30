use hangar_server::{
    accounts::environment::AccountEnvironment,
    machine_peers::panel::{Panel, Transport, decode_response, projection},
};
use proptest::strategy::Strategy;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/update-git/panel.json"
    ))
    .unwrap()
}
fn panel(root: &std::path::Path) -> Panel {
    let config = root.join(".hangar/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join(".env"),
        "CP_AUTH_TOKEN=fixture-token\nCP_PORT=12345\n",
    )
    .unwrap();
    std::fs::write(config.join("peers.json"), "{}").unwrap();
    Panel::new(
        AccountEnvironment::from_pairs([
            ("HOME", root.to_str().unwrap()),
            ("USERPROFILE", root.to_str().unwrap()),
            ("PATH", root.to_str().unwrap()),
        ]),
        None,
        root.into(),
    )
}
#[test]
fn panel_projections_match_python() {
    let root = tempfile::tempdir().unwrap();
    let panel = panel(root.path());
    for row in fixture()["cases"].as_array().unwrap() {
        match row["kind"].as_str().unwrap() {
            "peer_fields" => {
                let fields = projection::fields(&row["input"]);
                assert_eq!(json!([fields.0, fields.1, fields.2]), row["fields"]);
                assert_eq!(json!(projection::enabled(&row["input"])), row["enabled"]);
            }
            "attached" => assert_eq!(
                json!(projection::attached(row["input"].as_str().unwrap())),
                row["result"]
            ),
            "slim" => {
                let result = panel
                    .slim(
                        &row["input"],
                        row["server"].as_str(),
                        row["web"].as_str().unwrap(),
                        &std::collections::BTreeSet::from(["one".into()]),
                    )
                    .unwrap();
                let result: Value = serde_json::from_str(
                    &result
                        .to_string()
                        .replace(&root.path().display().to_string(), "<ROOT>"),
                )
                .unwrap();
                assert_eq!(result, row["result"], "{row}");
            }
            "detect_command" => {
                let project = tempfile::tempdir().unwrap();
                std::fs::write(
                    project.path().join("package.json"),
                    row["package"].to_string(),
                )
                .unwrap();
                for lock in row["locks"].as_array().unwrap() {
                    std::fs::write(project.path().join(lock.as_str().unwrap()), "").unwrap();
                }
                assert_eq!(
                    projection::detect_command(project.path()),
                    row["result"].as_str().unwrap(),
                    "{row}"
                );
            }
            "transport" if row["response_kind"] != "network" => {
                let actual = decode_response(
                    "https://peer.invalid",
                    row["status"].as_u64().unwrap() as u16,
                    "Synthetic reason",
                    row["body"].as_str().unwrap().as_bytes(),
                );
                if let Some(error) = row["result"]["error"].as_str() {
                    let actual = actual.unwrap_err();
                    if error.starts_with("resposta não-JSON") {
                        assert!(actual.starts_with("resposta não-JSON de https://peer.invalid: "));
                    } else {
                        assert_eq!(actual, error);
                    }
                } else {
                    assert_eq!(actual.unwrap(), row["result"]["value"]);
                }
            }
            _ => {}
        }
    }
}
#[derive(Default)]
struct Mock {
    responses: Mutex<VecDeque<Value>>,
    calls: Mutex<Vec<Value>>,
}
impl Transport for Mock {
    fn request<'a>(
        &'a self,
        base: &'a str,
        token: &'a str,
        method: &'a str,
        path: &'a str,
        body: Option<Value>,
        timeout: u64,
    ) -> futures_util::future::BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(json!({"base":base,"token":token,"method":method,"path":path,"body":body,"timeout":timeout}));
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("chamada além da captura");
            if let Some(error) = response.get("error").and_then(Value::as_str) {
                Err(error.into())
            } else {
                Ok(response)
            }
        })
    }
}
#[tokio::test]
async fn panel_actions_match_python_requests_and_results() {
    for row in fixture()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| matches!(row["kind"].as_str(), Some("action_api" | "dispatch")))
    {
        let root = tempfile::tempdir().unwrap();
        let mut panel = panel(root.path());
        let transport = Arc::new(Mock::default());
        *transport.responses.lock().unwrap() = row["responses"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into();
        panel.transport = transport.clone();
        let args = row["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let (code, result) = panel.action(&args).await;
        assert_eq!(u64::from(code), row["rc"].as_u64().unwrap(), "{row}");
        assert_eq!(result, row["result"], "{row}");
        let actual = transport.calls.lock().unwrap();
        let expected = row["calls"].as_array().cloned().unwrap_or_default();
        assert_eq!(actual.len(), expected.len(), "{row}");
        for (actual, expected) in actual.iter().zip(expected) {
            let args = expected["args"].as_array().unwrap();
            assert_eq!(actual["method"], args[1]);
            assert_eq!(
                actual["path"],
                args[2]
                    .as_str()
                    .unwrap()
                    .replace("{name}", args[0].as_str().unwrap())
            );
            assert_eq!(actual["body"], args.get(3).cloned().unwrap_or(Value::Null));
            assert_eq!(
                actual["timeout"],
                expected["kwargs"]
                    .get("timeout")
                    .cloned()
                    .unwrap_or(json!(8))
            );
        }
        assert!(transport.responses.lock().unwrap().is_empty());
    }
}
struct SnapshotTransport;
impl Transport for SnapshotTransport {
    fn request<'a>(
        &'a self,
        base: &'a str,
        _token: &'a str,
        _method: &'a str,
        path: &'a str,
        _body: Option<Value>,
        timeout: u64,
    ) -> futures_util::future::BoxFuture<'a, Result<Value, String>> {
        Box::pin(async move {
            assert_eq!(timeout, 4);
            if path == "/api/projects" {
                return Ok(json!([{"name":"project","cwd":"relative"}]));
            }
            if base == "http://bad" {
                return Err("falha sintética".into());
            }
            Ok(
                json!([{"name":"z","state":"idle"},{"name":"a","state":"working"},{"name":"b","state":"awaiting_input"}]),
            )
        })
    }
}
#[cfg(unix)]
#[tokio::test]
async fn panel_snapshot_matches_python_peer_visibility_order_and_projects() {
    use std::os::unix::fs::PermissionsExt;
    for row in fixture()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["kind"] == "snapshot")
    {
        let root = tempfile::tempdir().unwrap();
        let mut panel = panel(root.path());
        panel.transport = Arc::new(SnapshotTransport);
        std::fs::write(
            root.path().join(".hangar/config/peers.json"),
            row["peers"].to_string(),
        )
        .unwrap();
        let tmux = root.path().join("tmux");
        std::fs::write(&tmux, "#!/bin/sh\nprintf '0\\ta\\n'\n").unwrap();
        std::fs::set_permissions(tmux, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = panel.snapshot().await;
        let result: Value = serde_json::from_str(
            &result
                .to_string()
                .replace(&root.path().display().to_string(), "<ROOT>"),
        )
        .unwrap();
        assert_eq!(result, row["result"]);
    }
}
proptest::proptest! {
    #[test]
    fn panel_fields_never_coerce_non_text_values(value in proptest::prop_oneof![proptest::prelude::any::<bool>().prop_map(Value::Bool),proptest::prelude::any::<i64>().prop_map(|v|json!(v))]){
        proptest::prop_assert_eq!(projection::fields(&json!({"base_url":value,"token":value,"web_url":value})),(String::new(),String::new(),String::new()));
    }
}

#[tokio::test]
async fn panel_scan_preserves_candidates_and_backend_failure_notice() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/panel-scan.json")).unwrap();
    for row in fixture["cases"].as_array().unwrap() {
        let root=tempfile::tempdir().unwrap();let projects=root.path().join("projects");std::fs::create_dir(&projects).unwrap();
        for name in ["zeta","Alpha",".hidden"] {std::fs::create_dir(projects.join(name)).unwrap();}
        std::fs::write(projects.join("Alpha/package.json"),r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        std::fs::write(projects.join("loose-file"),"ignored").unwrap();
        let mut panel=panel(root.path());
        let mock=Arc::new(Mock::default());
        let escaped=serde_json::to_string(&root.path().display().to_string()).unwrap();
        let normalized=|value:&Value|serde_json::from_str::<Value>(&value.to_string().replace("<ROOT>",&escaped[1..escaped.len()-1])).unwrap();
        let result=if row["response"].is_null(){json!({"error":"fixture refusal"})}else{normalized(&row["response"])};
        mock.responses.lock().unwrap().push_back(result);panel.transport=mock.clone();
        let path=row["path"].as_str().unwrap().replace("<ROOT>",&root.path().display().to_string());
        let actual=panel.action(&["x".into(),"scan-folder".into(),path]).await;
        let mut expected=normalized(&row["result"]);
        if row["name"]=="backend-failed" {
            let message=expected["message"].as_str().unwrap().replace("\u{26a0} ","");expected["message"]=json!(message);
        }
        assert_eq!(actual.1,expected,"{}",row["name"]);
    }
}

#[tokio::test]
async fn panel_native_transport_matches_real_python_http_capture() {
    use std::{io::{Read,Write},net::{TcpListener,TcpStream},time::{Duration,Instant}};
    use hangar_server::machine_peers::panel::NativeTransport;
    fn serve(listener:TcpListener,count:usize,cross:String)->std::thread::JoinHandle<Vec<Value>> {
        std::thread::spawn(move||{
            listener.set_nonblocking(true).unwrap();let deadline=Instant::now()+Duration::from_secs(5);let mut calls=vec![];
            while calls.len()<count&&Instant::now()<deadline {
                let mut stream:TcpStream=match listener.accept(){Ok((stream,_))=>stream,Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{std::thread::sleep(Duration::from_millis(2));continue;},Err(error)=>panic!("{error}")};
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();let mut bytes=vec![];let mut buffer=[0u8;4096];
                let end=loop{let n=stream.read(&mut buffer).unwrap();assert_ne!(n,0);bytes.extend_from_slice(&buffer[..n]);if let Some(at)=bytes.windows(4).position(|part|part==b"\r\n\r\n"){break at+4;}};
                let head=String::from_utf8(bytes[..end].to_vec()).unwrap();let first=head.lines().next().unwrap();let mut words=first.split_whitespace();let method=words.next().unwrap();let path=words.next().unwrap();
                let header=|key:&str|head.lines().skip(1).find_map(|line|line.split_once(':').filter(|(name,_)|name.eq_ignore_ascii_case(key)).map(|(_,value)|value.trim().to_owned()));
                let length=header("content-length").map(|value|value.parse::<usize>().unwrap()).unwrap_or(0);
                while bytes.len()-end<length{let n=stream.read(&mut buffer).unwrap();assert_ne!(n,0);bytes.extend_from_slice(&buffer[..n]);}
                calls.push(json!({"method":method,"path":path,"authorization":header("authorization"),"content_type":header("content-type"),"body":String::from_utf8(bytes[end..end+length].to_vec()).unwrap()}));
                let redirected=matches!(path,"/same"|"/cross"|"/loop"|"/slow-chain")||path.starts_with("/r/")||path.starts_with("/chain/");
                let response=if redirected {
                    if path=="/slow-chain"{std::thread::sleep(Duration::from_millis(600));}
                    let status=path.strip_prefix("/r/").and_then(|value|value.parse::<u16>().ok()).unwrap_or(302);
                    let location=if path=="/cross"{cross.clone()}else if path=="/loop"{path.into()}else if path=="/slow-chain"{"/slow-end".into()}else if let Some(step)=path.strip_prefix("/chain/"){format!("/chain/{}",step.parse::<usize>().unwrap()+1)}else{"/ok".into()};
                    format!("HTTP/1.1 {status} Synthetic reason\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
                }else {
                    if path=="/slow"{std::thread::sleep(Duration::from_millis(1300));}
                    if path=="/slow-end"{std::thread::sleep(Duration::from_millis(600));}
                    let (status,body)=match path {"/error"=>(503,b"broken".to_vec()),"/error-bytes"=>(503,vec![255]),"/invalid-bytes"=>(200,vec![255]),_=>(200,b"{\"ok\":true}".to_vec())};
                    let mut response=format!("HTTP/1.1 {status} Synthetic reason\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).into_bytes();response.extend(body);response
                };
                let _=stream.write_all(&response);
            }
            assert_eq!(calls.len(),count,"número de chamadas HTTP");calls
        })
    }
    let mut rows=vec![];
    for raw in [include_str!("../../../../backend/tests/fixtures/contract/update-git/panel-network.json"),include_str!("../../../../backend/tests/fixtures/contract/update-git/panel-redirects.json"),include_str!("../../../../backend/tests/fixtures/contract/update-git/panel-encoding.json")] {
        let fixture:Value=serde_json::from_str(raw).unwrap();rows.extend(fixture["cases"].as_array().unwrap().iter().cloned());
    }
    let mut failures=vec![];
    for row in &rows {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let base=format!("http://{}",listener.local_addr().unwrap());
        let other=TcpListener::bind("127.0.0.1:0").unwrap();let cross=format!("http://{}/ok",other.local_addr().unwrap());
        let extra=(row["path"]=="/cross").then(||serve(other,1,String::new()));
        let main=serve(listener,row["calls"].as_array().unwrap().len()-usize::from(row["path"]=="/cross"),cross);
        let started=Instant::now();let actual=NativeTransport.request(&base,"fixture-token",row["method"].as_str().unwrap(),row["path"].as_str().unwrap(),(!row["body"].is_null()).then(||row["body"].clone()),row["timeout"].as_u64().unwrap()).await;
        if row["path"]=="/slow" {
            assert!(started.elapsed()>=Duration::from_millis(900)&&started.elapsed()<Duration::from_secs(3));
            let error=actual.unwrap_err();assert!(error.starts_with("servidor inacessível: "));assert!(!error.contains("fixture-token"));
        }else if row["result"]["exception"]=="UnicodeDecodeError" {
            assert!(actual.unwrap_err().starts_with(&format!("resposta não-JSON de {base}: ")));
        }else {
            let result=match actual{Ok(value)=>json!({"value":value}),Err(error)=>json!({"error":error})};
            if result!=row["result"]{failures.push(format!("{} {}: {result} != {}",row["method"],row["path"],row["result"]));}
        }
        let mut calls=main.join().unwrap();if let Some(extra)=extra{calls.extend(extra.join().unwrap());}
        let mut expected=row["calls"].clone();if row["path"]=="/cross"{expected[1]["authorization"]=Value::Null;}
        if json!(calls)!=expected{failures.push(format!("{}: {} != {expected}",row["path"],json!(calls)));}
    }
    assert!(failures.is_empty(),"{}",failures.join("\n"));
}


#[cfg(unix)]
#[test]
fn panel_public_cli_contracts_are_isolated_and_complete() {
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let python=std::env::var_os("HANGAR_TEST_PYTHON").unwrap_or_else(||"python3".into());
    let binary=std::env::var_os("HANGAR_TEST_SERVER_BIN").unwrap_or_else(||env!("CARGO_BIN_EXE_hangar-server").into());
    for name in ["cli","projects-cli","process-cli","depth-cli","snapshot-depth-cli","paths-cli","write-cli","stdout-cli","env-cli","user-environment-cli"] {
        let script=if name=="cli"{"test-panel-cli.py".to_owned()}else{format!("test-panel-{name}.py")};
        let output=std::process::Command::new(&python).arg(root.join("scripts").join(&script)).arg(&binary).current_dir(root).output().unwrap();
        assert!(output.status.success(),"{script}: {}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    }
}

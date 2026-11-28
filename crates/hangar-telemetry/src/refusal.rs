//! O aviso único de "Não enviar": um POST anônimo ao receptor, com poucas tentativas, e depois desiste.
use std::time::Duration;

use crate::aggregate::Telemetry;
use crate::send::Post;
use crate::wire::Refusal;

/// Segundos antes de cada tentativa.
const WAITS: [u64; 4] = [0, 60, 300, 1800];

impl Telemetry {
    /// `POST` ao `/v1/declined` do mesmo receptor do pacote.
    pub fn http_refusal_post(&self) -> reqwest::Result<Post> { self.post_to(refusal_url(&self.batch_url())) }

    /// Liga quem leva o aviso de recusa; só o primeiro vale.
    pub fn set_refusal_post(&self, post: Post) { let _ = self.refusal_post.set(post); }

    pub fn refusal_post(&self) -> Option<Post> { self.refusal_post.get().cloned() }
}

/// O último segmento `batch` vira `declined`; endereço sem esse segmento (a base do receptor) ganha `/v1/declined`.
fn refusal_url(batch: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(batch) else { return batch.to_owned() };
    let path = url.path().trim_end_matches('/').to_owned();
    let path = match path.strip_suffix("/batch") {
        Some(prefix) => format!("{prefix}/declined"),
        None => format!("{path}/v1/declined"),
    };
    url.set_path(&path);
    url.into()
}

impl Telemetry {
    /// Manda o aviso até um 2xx; 400 desiste na hora. Um desligamento do ambiente que passa a valer entre as
    /// tentativas encerra sem mandar (`Ok`). `Err` com o status da última tentativa, `None` sem resposta.
    pub async fn send_refusal(&self, refusal: Refusal, post: Post) -> Result<(), Option<u16>> {
        let body = serde_json::to_vec(&refusal).map_err(|_| None)?;
        let mut last = None;
        for wait in WAITS {
            tokio::time::sleep(Duration::from_secs(wait)).await;
            // ponytail: lê o .env no laço assíncrono; são no máximo quatro leituras de um arquivo pequeno.
            if self.env_off_now() { return Ok(()); }
            match post(body.clone()).await {
                Some(200..=299) => return Ok(()),
                Some(400) => return Err(Some(400)),
                status => last = status,
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};

    use super::*;
    use crate::aggregate::tests::Fixture;
    use crate::send::PostFuture;
    use crate::wire::RefusalKind;

    fn receiver(answers: Vec<Option<u16>>) -> (Post, Arc<Mutex<Vec<Value>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (log, answers) = (seen.clone(), Arc::new(Mutex::new(answers)));
        let post: Post = Arc::new(move |body: Vec<u8>| -> PostFuture {
            log.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
            let status = answers.lock().unwrap().remove(0);
            Box::pin(async move { status })
        });
        (post, seen)
    }

    #[test]
    fn telemetry_refusal_first_no_is_declined_and_only_once_per_machine() {
        let fx = Fixture::new();
        let telemetry = fx.start_as_is();
        let (_, refusal) = telemetry.refuse().unwrap();
        assert_eq!(refusal.map(|r| r.kind), Some(RefusalKind::Declined));
        assert_eq!(telemetry.consent(), Some(false));
        telemetry.set_consent(true).unwrap();
        assert_eq!(telemetry.refuse().unwrap().1, None, "mudar de ideia não manda de novo");
        drop(telemetry);
        let restarted = fx.start_as_is();
        restarted.set_consent(true).unwrap();
        assert_eq!(restarted.refuse().unwrap().1, None, "a marca sobrevive ao reinício");
    }

    #[test]
    fn telemetry_refusal_after_accepting_is_revoked() {
        let fx = Fixture::new();
        let telemetry = fx.start();
        let (_, refusal) = telemetry.refuse().unwrap();
        let refusal = refusal.unwrap();
        assert_eq!(refusal.kind, RefusalKind::Revoked);
        assert_eq!(serde_json::to_value(&refusal).unwrap(),
            json!({"kind": "revoked", "version": "0.42.0", "os": std::env::consts::OS, "channel": "latest"}));
        assert!(!telemetry.enabled());
    }

    #[test]
    fn telemetry_refusal_never_leaves_with_an_env_or_ci_switch() {
        for (key, value) in [("HANGAR_TELEMETRY", "0"), ("DO_NOT_TRACK", "1"), ("CI", "true")] {
            let fx = Fixture::new();
            let telemetry = fx.start_with(Arc::new(move |k: &str| (k == key).then(|| value.to_owned())));
            assert_eq!(telemetry.refuse().unwrap().1, None, "{key}");
            assert_eq!(telemetry.consent(), Some(false));
            let stored = std::fs::read_to_string(fx.home.path().join(".hangar/telemetry.json")).unwrap();
            assert!(!stored.contains("refusal_reported"), "{key}: desligado pelo ambiente não marca");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn telemetry_refusal_retries_then_gives_up_and_400_stops() {
        let fx = Fixture::new();
        let telemetry = fx.start_as_is();
        let send_refusal = |refusal, post| telemetry.send_refusal(refusal, post);
        let refusal = || Refusal { kind: RefusalKind::Declined, version: "0.42.0".into(), os: "linux".into(), channel: "latest".into() };
        let (post, seen) = receiver(vec![Some(503), None, Some(204)]);
        assert_eq!(send_refusal(refusal(), post).await, Ok(()));
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], json!({"kind": "declined", "version": "0.42.0", "os": "linux", "channel": "latest"}));
        let (post, seen) = receiver(vec![Some(400)]);
        assert_eq!(send_refusal(refusal(), post).await, Err(Some(400)));
        assert_eq!(seen.lock().unwrap().len(), 1);
        let (post, seen) = receiver(vec![Some(503), Some(503), None, None]);
        assert_eq!(send_refusal(refusal(), post).await, Err(None));
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn telemetry_refusal_stops_when_the_env_switch_turns_on_between_tries() {
        let fx = Fixture::new();
        let telemetry = fx.start_as_is();
        let dotenv = fx.home.path().join(".hangar/config/.env");
        std::fs::create_dir_all(dotenv.parent().unwrap()).unwrap();
        let tries = Arc::new(Mutex::new(0));
        let seen = tries.clone();
        let post: Post = Arc::new(move |_body: Vec<u8>| -> PostFuture {
            *seen.lock().unwrap() += 1;
            std::fs::write(&dotenv, "DO_NOT_TRACK=1\n").unwrap();
            Box::pin(async { Some(503) })
        });
        let refusal = Refusal { kind: RefusalKind::Declined, version: "0.42.0".into(), os: "linux".into(), channel: "latest".into() };
        assert_eq!(telemetry.send_refusal(refusal, post).await, Ok(()));
        assert_eq!(*tries.lock().unwrap(), 1, "a tentativa seguinte não sai");
    }

    #[test]
    fn telemetry_refusal_url_is_beside_the_batch_url() {
        let fx = Fixture::new();
        let telemetry = fx.start_with(Arc::new(|k: &str| (k == "HANGAR_TELEMETRY_URL").then(|| "http://127.0.0.1:9/v1/batch".to_owned())));
        assert_eq!(refusal_url(&telemetry.batch_url()), "http://127.0.0.1:9/v1/declined");
        assert!(telemetry.http_refusal_post().is_ok());
        for (batch, declined) in [
            ("https://telemetry.hangar.dev.br/v1/batch", "https://telemetry.hangar.dev.br/v1/declined"),
            ("https://telemetry.hangar.dev.br/v1/batch/", "https://telemetry.hangar.dev.br/v1/declined"),
            ("https://telemetry.hangar.dev.br", "https://telemetry.hangar.dev.br/v1/declined"),
            ("https://telemetry.hangar.dev.br/", "https://telemetry.hangar.dev.br/v1/declined"),
            ("http://127.0.0.1:9/proxy/v1/batch?x=1", "http://127.0.0.1:9/proxy/v1/declined?x=1"),
        ] {
            assert_eq!(refusal_url(batch), declined, "{batch}");
        }
    }
}

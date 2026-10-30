//! Tradução de pensamento sob demanda, sem prender a leitura quando o provedor falha.
use super::{config::SpeechConfig, text_client};
use std::{collections::HashMap, sync::Mutex, time::Duration};
use tokio::{sync::Semaphore, time::Instant};

#[derive(Default)]
struct Cache {
    values: HashMap<String, String>,
    paused: HashMap<String, Instant>,
}

pub struct Service {
    cache: Mutex<Cache>,
    slots: Semaphore,
}

impl Default for Service {
    fn default() -> Self {
        Self {
            cache: Mutex::default(),
            slots: Semaphore::new(2),
        }
    }
}

const CACHED_OUTPUT_LIMIT: usize = 64 * 1024;

const SYSTEM: &str = concat!(
    "Você reescreve, em português do Brasil, o resumo do raciocínio de um assistente de ",
    "programação. Regras: uma frase curta por passo, no gerúndio ou no infinitivo ",
    "('Conferindo se…', 'Buscando…'); no máximo 14 palavras por passo; preserve nomes próprios, ",
    "comandos, caminhos e URLs exatamente como estão; não invente nada que não esteja no texto; ",
    "não comente, não explique, não use aspas. Responda só com o texto reescrito, mantendo as ",
    "quebras de parágrafo do original."
);

impl Service {
    pub async fn translate(&self, config: &SpeechConfig, texts: &[String]) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(22);
        self.translate_before(config, texts, deadline).await
    }

    async fn translate_before(
        &self,
        config: &SpeechConfig,
        texts: &[String],
        deadline: Instant,
    ) -> Vec<String> {
        let mut translated = Vec::with_capacity(texts.len());
        for text in texts {
            translated.push(if Instant::now() < deadline {
                // O prazo do lote também cancela o item em curso, sem culpar o provedor.
                match tokio::time::timeout_at(deadline, self.one(config, text)).await {
                    Ok(output) => output,
                    Err(_) => {
                        tracing::warn!(
                            code = "speech_thought_batch_timeout",
                            "O prazo do lote de tradução terminou; o texto original foi preservado."
                        );
                        text.clone()
                    }
                }
            } else {
                text.clone()
            });
        }
        translated
    }

    async fn one(&self, config: &SpeechConfig, text: &str) -> String {
        let text = text_client::trim(text);
        if text.is_empty() || text.chars().count() > 4000 {
            return text.to_owned();
        }
        let profile = text_client::profile(config);
        let key = text_client::digest(&format!("{profile}\0{text}"));
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(value) = cache.values.get(&key) {
                return value.clone();
            }
            cache.paused.retain(|_, until| *until > Instant::now());
            if cache.paused.contains_key(&profile) {
                return text.to_owned();
            }
        }
        let Ok(_slot) = self.slots.try_acquire() else {
            return text.to_owned();
        };
        let result = text_client::chat(config, SYSTEM, text, 0.2, Duration::from_secs(12)).await;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(output) => {
                // A leitura aceita 4 MiB; sem teto por item, 500 entradas reteriam gigabytes.
                if output.len() <= CACHED_OUTPUT_LIMIT {
                    if cache.values.len() >= 500 {
                        cache.values.clear();
                    }
                    cache.values.insert(key, output.clone());
                }
                output
            }
            Err(error) => {
                // O código basta para diagnosticar; o detalhe externo pode repetir conteúdo privado.
                tracing::warn!(
                    status = error.status,
                    "A tradução do pensamento falhou; provedor pausado por 120 s."
                );
                if cache.paused.len() >= 16
                    && let Some(oldest) = cache
                        .paused
                        .iter()
                        .min_by_key(|(_, until)| **until)
                        .map(|(profile, _)| profile.clone())
                {
                    cache.paused.remove(&oldest);
                }
                cache
                    .paused
                    .insert(profile, Instant::now() + Duration::from_secs(120));
                text.to_owned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::text_client::testing::*;

    #[tokio::test]
    async fn reopening_a_thought_reuses_its_translation() {
        let server = fake(vec![content("Conferindo o código.")]).await;
        let service = Service::default();
        let texts = vec!["Checking code".to_owned()];
        assert_eq!(
            service.translate(&server.config, &texts).await,
            vec!["Conferindo o código."]
        );
        assert_eq!(
            service.translate(&server.config, &texts).await,
            vec!["Conferindo o código."]
        );
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn truncated_output_keeps_the_original_and_is_not_cached() {
        let server = fake(vec![
            (200, serde_json::json!({"choices":[{"finish_reason":"length",
                "message":{"content":"tradução interrompida"}}]})),
            content("Tradução completa."),
        ]).await;
        let service = Service::default();
        let texts = vec!["Checking code".to_owned()];
        assert_eq!(service.translate(&server.config, &texts).await, texts);
        assert_eq!(service.translate(&server.config, &texts).await, texts);
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        service.cache.lock().unwrap().paused.clear();
        assert_eq!(service.translate(&server.config, &texts).await, vec!["Tradução completa."]);
        assert_eq!(service.translate(&server.config, &texts).await, vec!["Tradução completa."]);
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn whitespace_around_the_same_effective_effort_reuses_the_translation() {
        let server = fake(vec![content("Tradução única."), content("Tradução repetida.")]).await;
        let service = Service::default();
        let texts = vec!["Checking code".to_owned()];
        let mut config = server.config.clone();
        config.values["llm_reasoning_effort"] = serde_json::json!("low");
        assert_eq!(service.translate(&config, &texts).await, vec!["Tradução única."]);
        config.values["llm_reasoning_effort"] = serde_json::json!(" \tlow\n ");
        assert_eq!(service.translate(&config, &texts).await, vec!["Tradução única."]);
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(server.bodies.lock().unwrap()[0]["reasoning_effort"], "low");
    }

    #[tokio::test]
    async fn provider_failure_pauses_calls_but_a_changed_provider_can_work() {
        let server = fake(vec![
            (500, serde_json::json!({"error":"indisponível"})),
            content("traduzido"),
        ])
        .await;
        let service = Service::default();
        let texts = vec!["one".to_owned(), "two".to_owned()];
        assert_eq!(service.translate(&server.config, &texts).await, texts);
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        let mut changed = server.config.clone();
        changed.values["llm_model"] = serde_json::json!("new-model");
        assert_eq!(
            service.translate(&changed, &["one".into()]).await,
            vec!["traduzido"]
        );
    }

    #[tokio::test]
    async fn a_cached_result_needs_no_slot_and_other_items_never_wait() {
        let server = fake(vec![content("traduzido")]).await;
        let service = Service::default();
        assert_eq!(
            service.translate(&server.config, &["cached".into()]).await,
            vec!["traduzido"]
        );
        let _slots = service.slots.acquire_many(2).await.unwrap();
        assert_eq!(
            service
                .translate(&server.config, &["not cached".into(), "cached".into()])
                .await,
            vec!["not cached", "traduzido"]
        );
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn whitespace_and_oversized_thoughts_match_python() {
        let cases = fixture();
        let replies = cases["thought_cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|case| {
                let input = case["input"].as_str().unwrap();
                (!text_client::trim(input).is_empty()
                    && text_client::trim(input).chars().count() <= 4000)
                    .then(|| content(case["expected"].as_str().unwrap()))
            })
            .collect();
        let server = fake(replies).await;
        for case in cases["thought_cases"].as_array().unwrap() {
            let service = Service::default();
            assert_eq!(
                service
                    .one(&server.config, case["input"].as_str().unwrap())
                    .await,
                case["expected"]
            );
        }
    }

    #[tokio::test]
    async fn a_new_configuration_does_not_reuse_an_old_translation() {
        let server = fake(vec![content("primeira tradução"), content("nova tradução")]).await;
        let service = Service::default();
        let texts = vec!["hello".to_owned()];
        assert_eq!(
            service.translate(&server.config, &texts).await,
            vec!["primeira tradução"]
        );
        let mut changed = server.config.clone();
        changed.values["llm_model"] = serde_json::json!("different");
        assert_eq!(
            service.translate(&changed, &texts).await,
            vec!["nova tradução"]
        );
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn expired_batches_return_the_remaining_text_exactly_as_received() {
        let server = fake(vec![content("em cache")]).await;
        let service = Service::default();
        assert_eq!(
            service.translate(&server.config, &["hello".into()]).await,
            vec!["em cache"]
        );
        let texts = vec!["hello".into(), "  untouched  ".into()];
        assert_eq!(
            service
                .translate_before(
                    &server.config,
                    &texts,
                    Instant::now() - Duration::from_secs(1)
                )
                .await,
            texts
        );
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn filling_the_memory_cache_clears_old_entries_before_inserting() {
        let server = fake(vec![content("novo")]).await;
        let service = Service::default();
        {
            let mut cache = service.cache.lock().unwrap();
            for n in 0..500 {
                cache.values.insert(n.to_string(), "antigo".into());
            }
        }
        assert_eq!(
            service.translate(&server.config, &["hello".into()]).await,
            vec!["novo"]
        );
        assert_eq!(service.cache.lock().unwrap().values.len(), 1);
    }

    #[tokio::test]
    async fn an_oversized_translation_is_returned_but_not_cached() {
        let large = "a".repeat(CACHED_OUTPUT_LIMIT + 1);
        let server = fake(vec![content(&large), content(&large)]).await;
        let service = Service::default();
        let texts = vec!["Checking code".to_owned()];
        assert_eq!(service.translate(&server.config, &texts).await, vec![large.clone()]);
        assert_eq!(service.translate(&server.config, &texts).await, vec![large]);
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert!(service.cache.lock().unwrap().values.is_empty());
    }

    #[tokio::test]
    async fn a_full_pause_list_only_releases_the_oldest_profile() {
        let server = fake(vec![(500, serde_json::json!({"error":"indisponível"}))]).await;
        let service = Service::default();
        let now = Instant::now();
        {
            let mut cache = service.cache.lock().unwrap();
            for n in 0..16u64 {
                cache
                    .paused
                    .insert(format!("perfil-{n}"), now + Duration::from_secs(60 + n));
            }
        }
        assert_eq!(service.translate(&server.config, &["one".into()]).await, vec!["one"]);
        let cache = service.cache.lock().unwrap();
        assert_eq!(cache.paused.len(), 16);
        assert!(!cache.paused.contains_key("perfil-0"));
        assert!((1..16).all(|n| cache.paused.contains_key(&format!("perfil-{n}"))));
        assert!(cache.paused.contains_key(&text_client::profile(&server.config)));
    }

    #[tokio::test]
    async fn the_batch_deadline_cancels_an_inflight_item_without_pausing_the_provider() {
        use std::sync::Arc;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let server = fake_gated(
            vec![content("resposta tardia")],
            Some((entered.clone(), release.clone())),
        )
        .await;
        let service = Arc::new(Service::default());
        let profile = text_client::profile(&server.config);
        let key = text_client::digest(&format!("{profile}\0cached"));
        service
            .cache
            .lock()
            .unwrap()
            .values
            .insert(key, "já traduzido".into());
        let texts = vec![
            "cached".to_owned(),
            "  pending  ".to_owned(),
            " untouched ".to_owned(),
        ];
        let mut task = tokio::spawn({
            let service = service.clone();
            let config = server.config.clone();
            let texts = texts.clone();
            let deadline = Instant::now() + Duration::from_secs(2);
            async move { service.translate_before(&config, &texts, deadline).await }
        });
        entered.notified().await;
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(3)).await;
        let completed = tokio::time::timeout(Duration::from_secs(1), &mut task).await;
        tokio::time::resume();
        release.notify_one();
        let in_time = completed.is_ok();
        let result = match completed {
            Ok(result) => result.unwrap(),
            Err(_) => task.await.unwrap(),
        };
        assert!(
            in_time,
            "o lote deve terminar mesmo com o provedor ainda segurando a resposta"
        );
        assert_eq!(
            result,
            vec![
                "já traduzido".to_owned(),
                texts[1].clone(),
                texts[2].clone()
            ]
        );
        assert_eq!(service.slots.available_permits(), 2);
        let cache = service.cache.lock().unwrap();
        assert_eq!(
            cache.values.len(),
            1,
            "somente a tradução concluída continua em cache"
        );
        assert!(
            cache.paused.is_empty(),
            "o fim do prazo do lote não é uma falha do provedor"
        );
        assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

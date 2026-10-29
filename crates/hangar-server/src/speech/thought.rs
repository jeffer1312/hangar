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
                self.one(config, text).await
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
                if cache.values.len() >= 500 {
                    cache.values.clear();
                }
                cache.values.insert(key, output.clone());
                output
            }
            Err(error) => {
                // O código basta para diagnosticar; o detalhe externo pode repetir conteúdo privado.
                tracing::warn!(
                    status = error.status,
                    "A tradução do pensamento falhou; provedor pausado por 120 s."
                );
                if cache.paused.len() >= 16 {
                    cache.paused.clear();
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
}

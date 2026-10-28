//! Posse do tamanho imposto: o último espectador conserva o original.
use super::{input, shell, ws::Failure};
use crate::list::bridge::ListBridge;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
struct Record {
    owner: u64,
    original: String,
    imposed: String,
    controller: shell::Controller,
    life: Option<String>,
}
type Slot = Arc<tokio::sync::Mutex<Option<Record>>>;
pub(super) struct LayoutChange {
    pub event: Value,
    pub dimensions: Option<(i64, i64)>,
}

pub struct Layouts {
    slots: Mutex<HashMap<String, Slot>>,
    next: AtomicU64,
    pub(super) http: reqwest::Client,
}
impl Default for Layouts {
    fn default() -> Self {
        Self {
            slots: Mutex::default(),
            next: AtomicU64::new(1),
            http: reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(45))
                .read_timeout(Duration::from_secs(45))
                .build()
                .expect("cliente HTTP local"),
        }
    }
}
pub(super) fn label(answer: &str) -> Option<&str> {
    answer.strip_prefix("layout:").map(str::trim)
}
impl Layouts {
    fn slot(&self, name: &str) -> Slot {
        self.slots
            .lock()
            .unwrap()
            .entry(name.into())
            .or_default()
            .clone()
    }
    pub(super) async fn apply(
        &self,
        dir: &Path,
        name: &str,
        life: Option<&str>,
        message: &Value,
        owner: &mut Option<u64>,
    ) -> Result<Option<LayoutChange>, Failure> {
        if message.get("w").is_some() || message.get("h").is_some() {
            let dimension = |key| {
                input::integer(&message[key])
                    .ok()
                    .filter(|v| (1..=8192).contains(v))
            };
            let (Some(width), Some(height)) = (dimension("w"), dimension("h")) else {
                return Ok(None);
            };
            let id = *owner.get_or_insert_with(|| self.next.fetch_add(1, Ordering::Relaxed));
            let slot = self.slot(name);
            let mut record = slot.lock().await;
            let controller = shell::controller(dir, name).await?;
            let original = match record.as_ref().filter(|record| {
                record.controller == controller
                    && (record.owner == id
                        || record.life.as_deref().zip(life).is_some_and(|(old, new)| old == new))
            }) {
                Some(record) => record.original.clone(),
                None => {
                    let answer = controller.verb(&self.http, &[]).await?;
                    let Some(label) = label(&answer) else {
                        return Ok(Some(LayoutChange {
                            event: json!({"t":"erro","m":answer}),
                            dimensions: None,
                        }));
                    };
                    label.to_owned()
                }
            };
            let imposed_controller = shell::controller(dir, name).await?;
            let answer = imposed_controller
                .verb(&self.http, &[width.to_string(), height.to_string()])
                .await?;
            let dimensions = if label(&answer).is_some() {
                // Um original consultado no desktop anterior não pertence ao substituto.
                *record = (controller == imposed_controller).then_some(Record {
                    owner: id,
                    original,
                    imposed: format!("{width}x{height}"),
                    controller,
                    life: life.map(str::to_owned),
                });
                Some((width, height))
            } else {
                None
            };
            Ok(Some(LayoutChange {
                event: json!({"t":"layout","resposta":answer}),
                dimensions,
            }))
        } else {
            let mode = message["modo"].as_str().unwrap_or("");
            if !["mobile", "desktop"].contains(&mode) {
                return Ok(None);
            }
            let answer = shell::verb(&self.http, dir, name, &[mode.to_owned()]).await?;
            Ok(Some(LayoutChange {
                event: json!({"t":"layout","modo":mode,"resposta":answer}),
                dimensions: None,
            }))
        }
    }
    pub(super) async fn restore(
        &self,
        dir: &Path,
        name: &str,
        owner: Option<u64>,
        list: &ListBridge,
    ) {
        let slot = self.slot(name);
        let mut record = slot.lock().await;
        if record.as_ref().is_none_or(|r| Some(r.owner) != owner) {
            return;
        }
        let record = record.take().unwrap();
        let result = async {
            let Some(controller) = record.current(dir, name, list).await? else {
                return Ok::<_, Failure>(());
            };
            let current = controller.verb(&self.http, &[]).await?;
            if label(&current) != Some(record.imposed.as_str()) {
                return Ok::<_, Failure>(());
            }
            let args = if ["desktop", "mobile"].contains(&record.original.as_str()) {
                vec![record.original.clone()]
            } else {
                record.original.split('x').map(str::to_owned).collect()
            };
            let Some(controller) = record.current(dir, name, list).await? else {
                return Ok(());
            };
            let answer = controller.verb(&self.http, &args).await?;
            if label(&answer).is_none() {
                tracing::warn!(target: "hangar.navsock",
                    "navsock {name}: layout de antes ({}) não voltou: {answer}", record.original);
            }
            Ok(())
        }.await;
        if let Err(error) = result {
            tracing::warn!(target:"hangar.navsock","navsock {name}: layout de antes não voltou: {}",error.message);
        }
    }
}

impl Record {
    async fn current(
        &self,
        dir: &Path,
        name: &str,
        list: &ListBridge,
    ) -> Result<Option<shell::Controller>, Failure> {
        let controller = shell::controller(dir, name).await?;
        if controller != self.controller {
            return Ok(None);
        }
        if let Some(life) = self.life.as_ref() {
            // Uma recriação já publicada não pode usar a vida guardada pela lista.
            list.invalidate();
            match list.discovered_session(name).await {
                Ok(Some((row, _))) if row.lifecycle_id.as_ref() != Some(life) => return Ok(None),
                Err(_) => return Err(Failure::plain("remote_browser_discovery_unavailable")),
                _ => {}
            }
        }
        // O envio usa este mesmo destino; não relê por nome depois de conferir.
        Ok(Some(controller))
    }
}

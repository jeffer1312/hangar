//! O contador pertence à vida do ator e só avança quando há uma tentativa adiada.
use crate::terminal_input::DeliveryStage;

#[derive(Default)]
pub(super) struct DeferredEpisode {
    composer: u64,
    unavailable: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Notice {
    pub code: &'static str,
    pub count: u64,
}
impl DeferredEpisode {
    pub fn composer_checked(&mut self, empty: bool) {
        self.unavailable = 0;
        if empty {
            self.composer = 0;
        }
    }

    pub fn observe(&mut self, stage: DeliveryStage, code: &str) -> Option<Notice> {
        let (count, code) = if matches!(code, "question_open" | "not_ready" | "input_unavailable") {
            (&mut self.unavailable, "input_unavailable")
        } else {
            if matches!(
                stage,
                DeliveryStage::Validate
                    | DeliveryStage::Identity
                    | DeliveryStage::Native
                    | DeliveryStage::Ready
            ) {
                return None;
            }
            self.unavailable = 0;
            if code != "composer_busy" {
                return None;
            }
            (&mut self.composer, "composer_busy")
        };
        *count = count.saturating_add(1);
        (*count == 1 || *count == 6 || (*count > 5 && *count % 5 == 0)).then_some(Notice {
            code,
            count: *count,
        })
    }
    pub fn record(
        &mut self,
        target: &super::OtherTarget,
        options: &super::OtherOptions,
        stage: DeliveryStage,
        code: &str,
    ) {
        if let Some(notice) = self.observe(stage, code) {
            if let Some(registry) = options.registry.as_ref().and_then(std::sync::Weak::upgrade) {
                notice.report(&registry.diag, &target.name);
            }
            if notice.count == 1 {
                tracing::warn!(session=%target.name, code=notice.code, count=notice.count, "envio adiado no terminal");
            } else {
                tracing::error!(session=%target.name, code=notice.code, count=notice.count, "envio continua adiado no terminal");
            }
        }
    }
}
impl Notice {
    pub fn report(&self, diag: &crate::diag::DiagClient, session: &str) {
        diag.report_episode(
            "runtime.command_deferred",
            session,
            self.code,
            "envio adiado no terminal",
            serde_json::json!({"count":self.count}),
            self.count == 1,
        );
    }
}

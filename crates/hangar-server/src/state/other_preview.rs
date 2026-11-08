//! Prévia dos provedores de terminal, aplicada ao quadro compartilhado do hub.
use std::sync::Arc;

use hangar_api::preview::PreviewEvent;

use super::preview::{HookFile, Slot, hook_text};

pub struct OtherPreviewInput<'a> {
    pub session: &'a str,
    /// Capturada antes da E/S: reset invalida os quadros ainda em voo.
    pub generation: u64,
    /// Kimi conserva as cores; None significa que não houve captura nesta rodada.
    pub pane: Option<&'a str>,
    pub files: &'a [HookFile],
    pub marker: Option<(&'a str, f64)>,
    pub state: Option<&'a str>,
    pub now: f64,
    /// Resposta normalizada; o chamador espera o histórico conhecido antes da primeira leitura.
    pub committed: Option<Arc<str>>,
}

/// Um redutor por vida da sessão; o hub é quem compartilha sua fonte e encerra o produtor.
pub struct OtherPreview {
    provider: String,
    generation: u64,
    accumulated: String,
    discarded_text: Option<String>,
    slot: Slot,
}

impl OtherPreview {
    pub fn new(provider: &str) -> Option<Self> {
        matches!(provider, "pi" | "omp" | "kimi").then(|| Self {
            provider: provider.into(), generation: 0, accumulated: String::new(), discarded_text: None, slot: Slot::default(),
        })
    }

    pub fn generation(&self) -> u64 { self.generation }

    pub fn reset(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.accumulated.clear();
        self.slot = Slot::default();
        self.discarded_text = None;
        self.generation
    }

    fn discard_source(&mut self) {
        if self.discarded_text.is_none() {
            let text = self.slot.source_text();
            if !text.is_empty() { self.discarded_text = Some(text.to_owned()); }
        }
        self.accumulated.clear();
    }

    /// Emite somente mudança de texto/flags; uma captura antiga não altera o acumulado.
    pub fn update(&mut self, input: OtherPreviewInput<'_>) -> Option<PreviewEvent> {
        if input.generation != self.generation {
            tracing::debug!(session = input.session, captured = input.generation, generation = self.generation,
                "prévia: quadro de outra geração descartado");
            return None;
        }
        if matches!(input.state, Some("idle" | "dead")) {
            self.discard_source();
            let committed = self.slot.offer_event(input.session, None, input.committed);
            return self.slot.clear_event(input.session).or(committed);
        }
        let source = if let Some(text) = hook_text(input.files, input.marker, input.now) {
            if text.is_empty() { self.discard_source(); }
            Some((text, true, true))
        } else {
            input.pane.map(|pane| {
                let mut text = crate::terminal_state::other_pane_preview(&self.provider, pane);
                if self.provider != "kimi" { return (text, false, false); }
                if text.is_empty() && !self.accumulated.is_empty() {
                    let continuation = crate::terminal_state::kimi_continuation(pane);
                    let (continued, joined) = crate::terminal_state::stitch_preview(&self.accumulated, &continuation);
                    if joined { text = continued; }
                }
                let (text, joined) = crate::terminal_state::stitch_preview(&self.accumulated, &text);
                self.accumulated.clone_from(&text);
                (text, false, joined)
            })
        };
        if let Some((text, _, _)) = source.as_ref() {
            if self.discarded_text.as_deref() == Some(text.as_str()) {
                // A costura pode ter reiniciado com full diferente: isso não torna novo o texto parado.
                self.accumulated.clear();
                return self.slot.offer_event(input.session, None, input.committed);
            }
            if !text.is_empty() { self.discarded_text = None; }
        }
        self.slot.offer_event(input.session, source, input.committed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(pane: &str) -> OtherPreviewInput<'_> {
        OtherPreviewInput { session: "preview-test", generation: 0, pane: Some(pane), files: &[], marker: None,
            state: Some("working"), now: 1000., committed: None }
    }

    #[test]
    fn other_preview_pi_excludes_the_real_composer() {
        let pane = r####"
   Na teoria são complementares, mas na prática dois pacotes que mexem na interface do pi podem sim
   pisar um no outro. A documentação pode estar otimista.

   A única forma de ter certeza é testar. Se você acha que são concorrentes, acho justo remover os
   dois e instalar de um em um:

   ╭· bash ······················································································ ╮
   │   pi remove npm:pi-claude-code-tui                                                           │
   │   pi remove npm:pi-claude-code-ui                                                            │
   ╰······························································································╯

   Reinicia o pi. Aí instala só um deles:

   ╭· bash ······················································································ ╮
   │   pi install npm:pi-claude-code-tui                                                          │
   ╰······························································································╯

   Testa. Se ainda não resolver, remove e instala só o outro:

   ╭· bash ······················································································ ╮
   │   pi remove npm:pi-claude-code-tui                                                           │
   │   pi install npm:pi-claude-code-ui                                                           │
   ╰······························································································╯

   Quer fazer esse teste? Posso te acompanhar nos comandos.

   ✻ Turn took 17s (Total time 18m 5s · 10 turns)

›  eu nao consigo copiar do conteudo do pi?

   Thought for 3s

 ● Tem sim, algumas formas:

   Copiar a última resposta do assistente:
   ◉ Comando: /copy
   ◉ Atalho: Ctrl+X

   Copiar uma mensagem antiga específica:

╭─────────────────────────────────────────────────────────────────────────────────────────────────╮

╰─────────────────────────────────────────────────────────────────────────────────────────────────╯
🤖 kimi-for-coding (high) │ 📁 jefferson │ 📟 jefferson-2 │ 💬 sessão 44kin/16kout · cache 1.1M ..."####;
        let event = OtherPreview::new("pi").unwrap().update(input(pane));
        assert!(event.as_ref().is_some_and(|e| e.text.starts_with("Tem sim, algumas formas:")));
        assert!(event.as_ref().is_some_and(|e| e.text.ends_with("Copiar uma mensagem antiga específica:")));
        assert!(event.as_ref().is_some_and(|e| !e.md && !e.full));
    }

    fn other_preview_sidecar_keeps_empty_absence_and_freshness_distinct() {
        let cases = [
            (Some("texto em voo"), Some(1000.), None, "texto em voo", true, true),
            (Some(""), Some(0.), None, "", true, true),
            (None, None, None, "veio do pane", false, false),
            (Some("antigo"), Some(0.), None, "veio do pane", false, false),
            (Some("antigo"), Some(0.), Some(("working", 1000.)), "antigo", true, true),
            (Some("antigo"), Some(0.), Some(("working", 0.)), "veio do pane", false, false),
            (Some("antigo"), Some(0.), Some(("idle", 1000.)), "veio do pane", false, false),
            (Some("sem relógio"), None, None, "sem relógio", true, true),
        ];
        for (text, ts, marker, expected, md, full) in cases {
            let mut preview = OtherPreview::new("pi").unwrap();
            let files: Vec<_> = text.map(|text: &str| HookFile { text: text.into(), ts }).into_iter().collect();
            let mut frame = input("● veio do pane\n\n✻ Unfurling… (2s)\n");
            frame.files = &files;
            frame.marker = marker;
            let event = preview.update(frame).unwrap();
            assert_eq!((event.text.as_str(), event.md, event.full, event.vivo), (expected, md, full, false));
        }
        for raw in [b"null".as_slice(), b"{isso nao e json", b"[]", br#"{"text":null}"#] {
            let files: Vec<_> = super::super::preview::parse_hook(raw).into_iter().collect();
            let mut frame = input("● veio do pane");
            frame.files = &files;
            assert_eq!(OtherPreview::new("pi").unwrap().update(frame).unwrap().text, "veio do pane");
        }
    }

    fn other_preview_kimi_stitches_and_restarts_with_honest_flags() {
        let mut preview = OtherPreview::new("kimi").unwrap();
        let first = "● Primeira frase comprida pra valer. Segunda frase comprida pra valer.\n";
        let second = "Segunda frase comprida pra valer. Terceira frase, recem-chegada.\n";
        let event = preview.update(input(first)).unwrap();
        assert_eq!((event.text.as_str(), event.md, event.full), ("Primeira frase comprida pra valer. Segunda frase comprida pra valer.", false, true));
        let event = preview.update(input(second)).unwrap();
        assert_eq!((event.text.as_str(), event.md, event.full), ("Primeira frase comprida pra valer. Segunda frase comprida pra valer. Terceira frase, recem-chegada.", false, true));
        assert_eq!(preview.update(input(second)), None);
        let event = preview.update(input("● Assunto totalmente diferente agora.")).unwrap();
        assert_eq!((event.text.as_str(), event.md, event.full), ("Assunto totalmente diferente agora.", false, false));
        let event = preview.update(input("")).unwrap();
        assert_eq!((event.text.as_str(), event.md, event.full), ("Assunto totalmente diferente agora.", false, true));
    }

    fn other_preview_kimi_rejects_unrelated_continuation_and_preserves_missing_frames() {
        let mut preview = OtherPreview::new("kimi").unwrap();
        let first = "● Bloco em voo, primeira frase comprida pra valer.";
        assert_eq!(preview.update(input(first)).unwrap().text, "Bloco em voo, primeira frase comprida pra valer.");
        assert_eq!(preview.update(input("total 48\n-rw-r--r-- 1 user user 9131 ago 19 09:41 preview.py\n")), None);
        let mut frame = input("");
        frame.pane = None;
        assert_eq!(preview.update(frame), None);
        assert_eq!(preview.update(input(first)), None);
    }

    fn other_preview_reset_discards_in_flight_old_frames() {
        let mut preview = OtherPreview::new("kimi").unwrap();
        assert_eq!(preview.update(input("● Texto de antes do clear, em voo.")).unwrap().text, "Texto de antes do clear, em voo.");
        let old = preview.generation();
        assert_ne!(preview.reset(), old);
        assert_eq!(preview.update(input("● Texto de antes do clear, em voo.")), None);
        let mut frame = input("● Texto novo, com identidade nova.");
        frame.generation = preview.generation();
        assert_eq!(preview.update(frame).unwrap().text, "Texto novo, com identidade nova.");
    }

    fn other_preview_idle_and_commit_do_not_revive_the_same_source() {
        let mut preview = OtherPreview::new("pi").unwrap();
        let pane = "● O envio por e-mail falhou porque o servidor recusou a conexão hoje.";
        assert!(preview.update(input(pane)).is_some());
        let mut frame = input(pane);
        frame.state = Some("idle");
        assert_eq!(preview.update(frame).unwrap().text, "");
        assert_eq!(preview.update(input(pane)), None);
        assert!(preview.update(input("● Outra resposta foi iniciada.")).is_some());
        let mut frame = input("");
        frame.pane = None;
        frame.committed = Some(Arc::from(super::super::preview::norm("Outra resposta foi iniciada.")));
        assert_eq!(preview.update(frame).unwrap().text, "");
        assert_eq!(preview.update(input("● Outra resposta foi iniciada.")), None);
    }

    fn other_preview_empty_sidecar_clears_kimi_accumulation() {
        let mut preview = OtherPreview::new("kimi").unwrap();
        assert!(preview.update(input("● Bloco em voo, primeira frase comprida pra valer.")).is_some());
        let files = [HookFile { text: String::new(), ts: Some(0.) }];
        let mut frame = input("● Bloco em voo, primeira frase comprida pra valer.");
        frame.files = &files;
        let event = preview.update(frame).unwrap();
        assert_eq!((event.text.as_str(), event.md, event.full), ("", true, true));
        let mut frame = input("");
        frame.pane = None;
        assert_eq!(preview.update(frame), None);
    }

    #[test]
    fn other_preview_state_contract() {
        other_preview_sidecar_keeps_empty_absence_and_freshness_distinct();
        other_preview_kimi_stitches_and_restarts_with_honest_flags();
        other_preview_kimi_rejects_unrelated_continuation_and_preserves_missing_frames();
        other_preview_reset_discards_in_flight_old_frames();
        other_preview_idle_and_commit_do_not_revive_the_same_source();
        other_preview_empty_sidecar_clears_kimi_accumulation();
    }

    #[test]
    fn other_preview_discarded_text_does_not_revive_after_a_flag_change() {
        const LONG: &str = "● Primeira frase comprida pra valer. Segunda frase comprida pra valer.\n";
        const SHORT: &str = "● Assunto totalmente diferente agora.\n";
        const FRESH: &str = "● Fechei o wire e subi o commit; o back valida na sequência.\n";
        for state in ["idle", "dead"] {
            for missing in [false, true] {
                let mut preview = OtherPreview::new("kimi").unwrap();
                assert!(preview.update(input(LONG)).unwrap().full);
                let event = preview.update(input(SHORT)).unwrap();
                assert_eq!((event.text.as_str(), event.full), ("Assunto totalmente diferente agora.", false));
                let mut stopped = input(SHORT);
                stopped.state = Some(state);
                assert_eq!(preview.update(stopped).unwrap().text, "");
                if missing {
                    let mut frame = input("");
                    frame.pane = None;
                    assert_eq!(preview.update(frame), None);
                }
                assert_eq!(preview.update(input(SHORT)), None, "{state}, missing={missing}");
                let files = [HookFile { text: "Assunto totalmente diferente agora.".into(), ts: Some(1000.) }];
                let mut same = input(SHORT);
                same.files = &files;
                assert_eq!(preview.update(same), None);
                // O vazio explícito também não é uma resposta nova que rearme a fonte antiga.
                let empty = [HookFile { text: String::new(), ts: Some(1000.) }];
                let mut cleared = input(SHORT);
                cleared.files = &empty;
                assert_eq!(preview.update(cleared).unwrap().text, "");
                let mut absent = input("");
                absent.pane = None;
                assert_eq!(preview.update(absent), None);
                assert_eq!(preview.update(input(SHORT)), None);
                let event = preview.update(input(FRESH)).unwrap();
                assert_eq!(event.text, "Fechei o wire e subi o commit; o back valida na sequência.");
                let fresh = [HookFile { text: "Fechei o wire e subi o commit; o back valida na sequência.".into(), ts: Some(1000.) }];
                let mut changed_flags = input(FRESH);
                changed_flags.files = &fresh;
                let event = preview.update(changed_flags).unwrap();
                assert_eq!((event.text.as_str(), event.md, event.full), ("Fechei o wire e subi o commit; o back valida na sequência.", true, true));
                assert_eq!(preview.update(input(SHORT)).unwrap().text, "Assunto totalmente diferente agora.");
                let mut stopped = input(SHORT);
                stopped.state = Some(state);
                assert_eq!(preview.update(stopped).unwrap().text, "");
                let generation = preview.reset();
                let mut reopened = input(SHORT);
                reopened.generation = generation;
                assert_eq!(preview.update(reopened).unwrap().text, "Assunto totalmente diferente agora.");
            }
        }
        // O arquivo vazio pode chegar antes do estado parado; ausência posterior não desfaz o descarte.
        for state in ["idle", "dead"] {
            let mut preview = OtherPreview::new("kimi").unwrap();
            assert!(preview.update(input(LONG)).is_some());
            assert!(!preview.update(input(SHORT)).unwrap().full);
            let files = [HookFile { text: String::new(), ts: Some(1000.) }];
            let mut empty = input(SHORT);
            empty.files = &files;
            assert_eq!(preview.update(empty).unwrap().text, "");
            let mut stopped = input(SHORT);
            stopped.state = Some(state);
            assert_eq!(preview.update(stopped).unwrap().text, "");
            let mut absent = input("");
            absent.pane = None;
            assert_eq!(preview.update(absent), None);
            assert_eq!(preview.update(input(SHORT)), None);
            assert_eq!(preview.update(input(FRESH)).unwrap().text, "Fechei o wire e subi o commit; o back valida na sequência.");
        }
        // Enquanto a resposta é a atual, mudar full sem mudar texto continua sendo observável.
        let mut preview = OtherPreview::new("kimi").unwrap();
        assert!(preview.update(input(LONG)).unwrap().full);
        assert!(!preview.update(input(SHORT)).unwrap().full);
        let event = preview.update(input(SHORT)).unwrap();
        assert_eq!((event.text.as_str(), event.md, event.full), ("Assunto totalmente diferente agora.", false, true));
    }

    #[test]
    fn other_preview_prose_hyphen_is_not_a_committed_match() {
        let mut preview = OtherPreview::new("pi").unwrap();
        let mut frame = input("● O envio por email falhou porque o servidor recusou a conexão hoje.");
        frame.committed = Some(Arc::from(super::super::preview::norm("O envio por e-mail falhou porque o servidor recusou a conexão hoje.")));
        let event = preview.update(frame).unwrap();
        assert_eq!(event.text, "O envio por email falhou porque o servidor recusou a conexão hoje.");
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            cases: 128,
            rng_seed: proptest::test_runner::RngSeed::Fixed(0x4841_4e47_4152),
            failure_persistence: None,
            ..proptest::test_runner::Config::default()
        })]
        #[test]
        fn other_preview_external_text_and_sidecars_never_panic(
            pane in proptest::prelude::any::<String>(),
            next in proptest::prelude::any::<String>(),
            raw in proptest::prelude::any::<Vec<u8>>(),
            now in proptest::prelude::any::<f64>(),
        ) {
            let files: Vec<_> = super::super::preview::parse_hook(&raw).into_iter().collect();
            let stitched = crate::terminal_state::stitch_preview(&pane, &next);
            if stitched.1 {
                proptest::prop_assert!(stitched.0.starts_with(&pane));
                proptest::prop_assert!(stitched.0.ends_with(&next));
            } else {
                proptest::prop_assert_eq!(stitched.0, next.clone());
            }
            for provider in ["pi", "omp", "kimi"] {
                let mut preview = OtherPreview::new(provider).unwrap();
                let mut frame = input(&pane);
                frame.files = &files;
                frame.now = now;
                let _ = preview.update(frame);
                preview.reset();
                proptest::prop_assert_eq!(preview.update(input(&pane)), None);
            }
        }
    }

}

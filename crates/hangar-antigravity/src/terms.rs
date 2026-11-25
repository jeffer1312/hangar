//! O condutor dos Termos do Google no primeiro uso do `agy`: uma máquina de etapas sem I/O. Quem chama captura a
//! tela, passa o que viu e aperta a tecla devolvida. A tecla dos Termos só sai da opção que a pessoa tocou.

use std::time::{Duration, Instant};

use hangar_core::terminal_state::{AgyTermsFocus, AgyTermsKey, agy_analyze, agy_login_screen, agy_onboarding_list, agy_terms, agy_terms_key};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Starting,
    /// `text`: a pergunta da tela, com os links.
    Terms { text: String },
    Answering,
    Login,
    Done,
    Failed(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerError { OutsideTerms, InvalidOption }

/// Sem Termos, login nem aceite neste prazo, a tentativa falha.
const START_LIMIT: Duration = Duration::from_secs(60);
/// Depois do Enter no botão, o `onboarding.json` tem este prazo para confirmar.
const CONFIRM_LIMIT: Duration = Duration::from_secs(10);
const MAX_KEYS: u32 = 8;
const COLORS_RETRY: Duration = Duration::from_secs(3);

pub struct Driver {
    stage: Stage,
    started: Instant,
    /// A tela de cores já confirmada (título, cursor e quando): o Enter só se repete com ela ainda na tela depois de
    /// `COLORS_RETRY`, porque o psmux joga fora a tecla que chega antes do agy ler a entrada.
    colors: Option<(String, usize, Instant)>,
    option: usize,
    keys: u32,
    pressed: Option<Instant>,
    /// Desde quando a tela dos Termos sumiu no meio da resposta.
    lost: Option<Instant>,
}

impl Driver {
    pub fn new(now: Instant) -> Self {
        Self { stage: Stage::Starting, started: now, colors: None, option: 0, keys: 0, pressed: None, lost: None }
    }

    pub fn stage(&self) -> &Stage { &self.stage }

    /// Lê a tela capturada e devolve a tecla a mandar agora, no máximo uma. `accepted`: o `onboarding.json` diz
    /// `onboardingComplete`.
    pub fn observe(&mut self, screen: &str, accepted: bool, now: Instant) -> Option<AgyTermsKey> {
        if matches!(self.stage, Stage::Done | Stage::Failed(_)) { return None; }
        if accepted {
            self.stage = Stage::Done;
            return None;
        }
        if self.stage == Stage::Answering { return self.answering(screen, now); }
        // O login também parece menu para o `agy_analyze`: vem antes.
        let mut key = None;
        if agy_login_screen(screen) {
            self.stage = Stage::Login;
        } else if let Some((title, _, cursor)) = agy_onboarding_list(screen) {
            // A opção sob o cursor é confirmada pelo Hangar (a pessoa escolhe as cores depois, no agy).
            let again = self.colors.as_ref().is_none_or(|(t, c, sent)| *t != title || *c != cursor || now.duration_since(*sent) >= COLORS_RETRY);
            if again {
                self.colors = Some((title, cursor, now));
                key = Some(AgyTermsKey::Enter);
            }
        } else if agy_terms(screen).is_some() && let Some(text) = agy_analyze(screen).question {
            self.stage = Stage::Terms { text };
        }
        if self.stage == Stage::Starting && now.duration_since(self.started) >= START_LIMIT {
            self.stage = Stage::Failed("termos_prazo");
            return None;
        }
        key
    }

    /// A opção que a pessoa tocou: 1 aceita compartilhando os dados, 2 aceita sem compartilhar. A tecla sai no
    /// próximo `observe`.
    pub fn answer(&mut self, option: usize) -> Result<(), AnswerError> {
        if !matches!(self.stage, Stage::Terms { .. }) { return Err(AnswerError::OutsideTerms); }
        if !(1..=2).contains(&option) { return Err(AnswerError::InvalidOption); }
        self.option = option;
        self.stage = Stage::Answering;
        Ok(())
    }

    /// Fim vindo de fora do condutor (janela que caiu, prazo total): daí em diante nada é apertado nem respondido.
    pub fn fail(&mut self, code: &'static str) {
        if !matches!(self.stage, Stage::Done | Stage::Failed(_)) { self.stage = Stage::Failed(code); }
    }

    fn answering(&mut self, screen: &str, now: Instant) -> Option<AgyTermsKey> {
        if let Some(pressed) = self.pressed {
            if now.duration_since(pressed) >= CONFIRM_LIMIT { self.stage = Stage::Failed("termos_sem_confirmacao"); }
            return None;
        }
        // Tela não reconhecida (redesenhando): espera a próxima captura, até o prazo.
        let Some(terms) = agy_terms(screen) else {
            let lost = *self.lost.get_or_insert(now);
            if now.duration_since(lost) >= CONFIRM_LIMIT { self.stage = Stage::Failed("termos_tela_desconhecida"); }
            return None;
        };
        self.lost = None;
        let key = agy_terms_key(terms, self.option)?;
        if self.keys == MAX_KEYS {
            self.stage = Stage::Failed("termos_nao_convergiu");
            return None;
        }
        self.keys += 1;
        if key == AgyTermsKey::Enter && terms.focus != AgyTermsFocus::Checkbox { self.pressed = Some(now); }
        Some(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgyTermsKey::{Down, Enter};

    const COLORS: &str = include_str!("../../hangar-core/tests/fixtures/agy/onboarding-colors.txt");
    const LOGIN: &str = include_str!("../../hangar-core/tests/fixtures/agy/login.txt");
    const TERMS: &str = include_str!("../../hangar-core/tests/fixtures/agy/terms.txt");
    const TERMS_DONE: &str = include_str!("../../hangar-core/tests/fixtures/agy/terms-done.txt");
    const UNCHECKED: &str = include_str!("../../hangar-core/tests/fixtures/agy/terms-unchecked.txt");
    const UNCHECKED_DONE: &str = include_str!("../../hangar-core/tests/fixtures/agy/terms-unchecked-done.txt");
    const IDLE: &str = include_str!("../../hangar-core/tests/fixtures/agy/idle.txt");

    fn at(t0: Instant, secs: u64) -> Instant { t0 + Duration::from_secs(secs) }

    /// Um condutor já na tela dos Termos, com a opção respondida.
    fn answering(t0: Instant, option: usize) -> Driver {
        let mut driver = Driver::new(t0);
        driver.observe(TERMS, false, t0);
        driver.answer(option).unwrap();
        driver
    }

    #[test]
    fn terms_colors_are_confirmed_once_then_the_terms_wait_for_the_person() {
        let t0 = Instant::now();
        let mut driver = Driver::new(t0);
        assert_eq!(driver.observe(COLORS, false, t0), Some(Enter));
        assert_eq!(driver.stage(), &Stage::Starting);
        assert_eq!(driver.observe(COLORS, false, at(t0, 1)), None, "a mesma tela de cores não ganha outro Enter");
        assert_eq!(driver.observe(TERMS, false, at(t0, 2)), None, "nos Termos nada é apertado sem a pessoa");
        let Stage::Terms { text } = driver.stage() else { panic!("{:?}", driver.stage()) };
        assert!(text.contains("Terms of Service & Data Use"), "{text}");
        assert!(text.contains("https://antigravity.google/terms") && text.contains("https://policies.google.com/privacy"), "{text}");
    }

    #[test]
    fn terms_colors_screen_that_stays_gets_the_enter_again_after_3_s() {
        let t0 = Instant::now();
        let mut driver = Driver::new(t0);
        assert_eq!(driver.observe(COLORS, false, t0), Some(Enter));
        assert_eq!(driver.observe(COLORS, false, at(t0, 2)), None);
        assert_eq!(driver.observe(COLORS, false, at(t0, 3)), Some(Enter), "o psmux joga fora a tecla que chega cedo demais");
        assert_eq!(driver.observe(COLORS, false, at(t0, 4)), None);
    }

    /// O Enter das cores que chega atrasado nos Termos cai no checkbox (só desmarca); com foco em Done ele aceitaria.
    #[test]
    fn terms_screen_opens_with_the_focus_on_the_checkbox() {
        assert_eq!(agy_terms(TERMS).map(|t| t.focus), Some(AgyTermsFocus::Checkbox));
    }

    #[test]
    fn terms_screen_gone_while_answering_fails_after_10_s() {
        let t0 = Instant::now();
        let mut driver = answering(t0, 1);
        assert_eq!(driver.observe(IDLE, false, at(t0, 1)), None);
        assert_eq!(driver.observe(IDLE, false, at(t0, 10)), None);
        assert_eq!(driver.stage(), &Stage::Answering);
        driver.observe(IDLE, false, at(t0, 11));
        assert_eq!(driver.stage(), &Stage::Failed("termos_tela_desconhecida"));
    }

    #[test]
    fn terms_option_1_keeps_the_box_and_presses_done_then_waits_for_the_file() {
        let t0 = Instant::now();
        let mut driver = answering(t0, 1);
        assert_eq!(driver.stage(), &Stage::Answering);
        assert_eq!(driver.observe(TERMS, false, at(t0, 1)), Some(Down));
        assert_eq!(driver.observe(TERMS_DONE, false, at(t0, 2)), Some(Enter));
        assert_eq!(driver.observe(IDLE, false, at(t0, 3)), None);
        assert_eq!(driver.stage(), &Stage::Answering);
        assert_eq!(driver.observe(IDLE, true, at(t0, 4)), None);
        assert_eq!(driver.stage(), &Stage::Done);
    }

    #[test]
    fn terms_option_2_unchecks_the_box_before_done() {
        let t0 = Instant::now();
        let mut driver = answering(t0, 2);
        assert_eq!(driver.observe(TERMS, false, at(t0, 1)), Some(Enter), "Enter no checkbox só desmarca");
        assert_eq!(driver.observe(UNCHECKED, false, at(t0, 2)), Some(Down));
        assert_eq!(driver.observe(UNCHECKED_DONE, false, at(t0, 3)), Some(Enter));
        assert_eq!(driver.observe(IDLE, true, at(t0, 4)), None);
        assert_eq!(driver.stage(), &Stage::Done);
    }

    #[test]
    fn terms_login_screen_is_the_login_stage() {
        let t0 = Instant::now();
        let mut driver = Driver::new(t0);
        assert_eq!(driver.observe(LOGIN, false, t0), None);
        assert_eq!(driver.stage(), &Stage::Login);
    }

    #[test]
    fn terms_accepted_wins_on_any_screen() {
        let t0 = Instant::now();
        let mut driver = Driver::new(t0);
        assert_eq!(driver.observe(COLORS, true, t0), None);
        assert_eq!(driver.stage(), &Stage::Done);
    }

    #[test]
    fn terms_starting_fails_after_60_seconds_without_a_known_screen() {
        let t0 = Instant::now();
        let mut driver = Driver::new(t0);
        driver.observe(IDLE, false, at(t0, 59));
        assert_eq!(driver.stage(), &Stage::Starting);
        assert_eq!(driver.observe(COLORS, false, at(t0, 60)), None);
        assert_eq!(driver.stage(), &Stage::Failed("termos_prazo"));
    }

    #[test]
    fn terms_answer_is_refused_outside_the_terms_and_for_previous() {
        let t0 = Instant::now();
        let mut driver = Driver::new(t0);
        assert_eq!(driver.answer(1), Err(AnswerError::OutsideTerms));
        driver.observe(TERMS, false, t0);
        assert_eq!(driver.answer(3), Err(AnswerError::InvalidOption), "Previous não é oferecido");
        assert_eq!(driver.answer(0), Err(AnswerError::InvalidOption));
        assert!(matches!(driver.stage(), Stage::Terms { .. }));
        assert_eq!(driver.observe(TERMS, false, at(t0, 1)), None, "sem resposta válida nada é apertado");
    }

    #[test]
    fn terms_done_pressed_without_confirmation_fails_after_10_seconds() {
        let t0 = Instant::now();
        let mut driver = answering(t0, 1);
        driver.observe(TERMS, false, at(t0, 1));
        assert_eq!(driver.observe(TERMS_DONE, false, at(t0, 2)), Some(Enter));
        assert_eq!(driver.observe(IDLE, false, at(t0, 11)), None);
        assert_eq!(driver.stage(), &Stage::Answering);
        assert_eq!(driver.observe(IDLE, false, at(t0, 12)), None);
        assert_eq!(driver.stage(), &Stage::Failed("termos_sem_confirmacao"));
    }

    #[test]
    fn terms_unknown_screen_while_answering_sends_nothing_and_waits() {
        let t0 = Instant::now();
        let mut driver = answering(t0, 1);
        assert_eq!(driver.observe("", false, at(t0, 1)), None);
        assert_eq!(driver.stage(), &Stage::Answering);
        assert_eq!(driver.observe(TERMS, false, at(t0, 2)), Some(Down));
    }

    #[test]
    fn terms_more_than_8_keys_without_pressing_done_fails() {
        let t0 = Instant::now();
        let mut driver = answering(t0, 1);
        for second in 1..=8 { assert_eq!(driver.observe(TERMS, false, at(t0, second)), Some(Down)); }
        assert_eq!(driver.observe(TERMS, false, at(t0, 9)), None);
        assert_eq!(driver.stage(), &Stage::Failed("termos_nao_convergiu"));
    }

    #[test]
    fn terms_failure_from_outside_ends_the_terms_and_refuses_the_answer() {
        let t0 = Instant::now();
        let mut driver = Driver::new(t0);
        driver.observe(TERMS, false, t0);
        driver.fail("termos_terminal_falhou");
        assert_eq!(driver.stage(), &Stage::Failed("termos_terminal_falhou"));
        assert_eq!(driver.answer(1), Err(AnswerError::OutsideTerms));
        driver.fail("termos_expirou");
        assert_eq!(driver.stage(), &Stage::Failed("termos_terminal_falhou"), "o primeiro fim fica");
        let mut done = Driver::new(t0);
        done.observe(IDLE, true, t0);
        done.fail("termos_expirou");
        assert_eq!(done.stage(), &Stage::Done);
    }

    #[test]
    fn terms_done_and_failed_are_final() {
        let t0 = Instant::now();
        let mut done = Driver::new(t0);
        done.observe(IDLE, true, t0);
        assert_eq!(done.observe(COLORS, false, at(t0, 1)), None);
        assert_eq!(done.stage(), &Stage::Done);
        let mut failed = Driver::new(t0);
        failed.observe(IDLE, false, at(t0, 60));
        assert_eq!(failed.observe(TERMS, true, at(t0, 61)), None);
        assert_eq!(failed.observe(COLORS, false, at(t0, 62)), None);
        assert_eq!(failed.stage(), &Stage::Failed("termos_prazo"));
    }
}

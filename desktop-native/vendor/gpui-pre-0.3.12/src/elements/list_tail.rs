// A compensação só compara alturas medidas na mesma geometria.
pub(super) fn geometry_changed<T: PartialEq>(previous: Option<T>, current: T) -> bool {
    previous.is_some_and(|previous| previous != current)
}

pub(super) fn held_slack(current: f32, before: f32, after: f32, max: f32) -> f32 {
    (current + before - after).clamp(0., max)
}

/// Quanto tempo a folga segura o fim parado antes de ir embora.
pub(super) const TAIL_HOLD_MS: f32 = 500.;
const TAIL_GLIDE_TAU_MS: f32 = 70.;

/// Folga que ninguém consumiu não fica de vão para sempre: parada há `TAIL_HOLD_MS`, desliza até zero.
/// `quiet_ms` conta desde a última mudança da folga; `step_ms`, desde o passo anterior do deslize.
pub(super) fn released_slack(slack: f32, quiet_ms: f32, step_ms: f32) -> f32 {
    if quiet_ms < TAIL_HOLD_MS { return slack; }
    let left = slack * (-step_ms / TAIL_GLIDE_TAU_MS).exp();
    if left < 0.5 { 0. } else { left }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_and_height_changes_invalidate_the_old_reference() {
        let large = (1200., 800.);
        assert!(!geometry_changed(None, large));
        assert!(!geometry_changed(Some(large), large));
        assert!(geometry_changed(Some(large), (820., 800.)));
        assert!(geometry_changed(Some(large), (1200., 600.)));
        assert!(geometry_changed(Some((820., 600.)), large));
    }

    #[test]
    fn content_shrink_is_held_and_growth_consumes_only_the_slack() {
        assert_eq!(held_slack(0., 400., 300., 160.), 100.);
        assert_eq!(held_slack(100., 300., 340., 160.), 60.);
        assert_eq!(held_slack(60., 340., 500., 160.), 0.);
        assert_eq!(held_slack(0., 500., 100., 160.), 160.);
        assert_eq!(held_slack(160., 100., 100., 160.), 160.);
    }

    #[test]
    fn idle_slack_waits_then_glides_to_zero() {
        assert_eq!(released_slack(100., 499., 16.), 100.);
        let first = released_slack(100., 516., 16.);
        assert!(first > 0. && first < 100.);
        let mut slack = 160.;
        for _ in 0..60 { slack = released_slack(slack, 1_000., 16.); }
        assert_eq!(slack, 0.);
        assert_eq!(released_slack(0., 1_000., 16.), 0.);
    }
}

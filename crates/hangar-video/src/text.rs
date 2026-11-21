/// Relógio do player: `0:07`, `12:34`, `1:02:03`. Sem valor (ainda carregando, transmissão sem fim) é `--:--`.
pub fn clock(seconds: Option<f64>) -> String {
    let Some(seconds) = seconds.filter(|s| s.is_finite() && *s >= 0.) else { return "--:--".to_owned() };
    let total = seconds.floor() as u64;
    let (hours, minutes, secs) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 { format!("{hours}:{minutes:02}:{secs:02}") } else { format!("{minutes}:{secs:02}") }
}

#[cfg(test)]
mod tests {
    use super::clock;

    #[test]
    fn clock_formats_minutes_and_hours() {
        assert_eq!(clock(Some(0.)), "0:00");
        assert_eq!(clock(Some(7.9)), "0:07");
        assert_eq!(clock(Some(754.)), "12:34");
        assert_eq!(clock(Some(3723.)), "1:02:03");
        assert_eq!(clock(None), "--:--");
        assert_eq!(clock(Some(f64::NAN)), "--:--");
        assert_eq!(clock(Some(-1.)), "--:--");
    }
}

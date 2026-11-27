//! Faixas fixas de duração: dão mediana e p95 por versão sem mandar o tempo exato.
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bucket { Lt100ms, Lt300ms, Lt1s, Lt3s, Lt10s, Lt30s, Ge30s }

const LIMITS_MS: [(u128, Bucket); 6] = [
    (100, Bucket::Lt100ms), (300, Bucket::Lt300ms), (1_000, Bucket::Lt1s),
    (3_000, Bucket::Lt3s), (10_000, Bucket::Lt10s), (30_000, Bucket::Lt30s),
];

impl Bucket {
    pub fn of(duration: Duration) -> Bucket {
        let ms = duration.as_millis();
        LIMITS_MS.iter().find(|(limit, _)| ms < *limit).map_or(Bucket::Ge30s, |(_, bucket)| *bucket)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Bucket::Lt100ms => "lt100ms",
            Bucket::Lt300ms => "lt300ms",
            Bucket::Lt1s => "lt1s",
            Bucket::Lt3s => "lt3s",
            Bucket::Lt10s => "lt10s",
            Bucket::Lt30s => "lt30s",
            Bucket::Ge30s => "ge30s",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_fall_in_their_bucket() {
        assert_eq!(Bucket::of(Duration::from_millis(2_500)), Bucket::Lt3s);
        assert_eq!(Bucket::of(Duration::from_millis(99)), Bucket::Lt100ms);
        assert_eq!(Bucket::of(Duration::from_millis(100)), Bucket::Lt300ms);
        assert_eq!(Bucket::of(Duration::from_millis(29_999)), Bucket::Lt30s);
        assert_eq!(Bucket::of(Duration::from_secs(30)), Bucket::Ge30s);
        assert_eq!(Bucket::Lt3s.as_str(), "lt3s");
    }
}

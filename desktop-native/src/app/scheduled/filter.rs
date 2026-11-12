#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Filter {
    #[default]
    All,
    Active,
    Paused,
}

impl Filter {
    pub(super) fn includes(self, enabled: bool) -> bool {
        match self {
            Self::All => true,
            Self::Active => enabled,
            Self::Paused => !enabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Filter;

    #[test]
    fn active_and_paused_filters_are_disjoint_and_all_keeps_both() {
        let rows = [("daily", true), ("release", false), ("ci", true)];
        let shown = |filter: Filter| {
            rows.iter()
                .filter(|(_, enabled)| filter.includes(*enabled))
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(Filter::All), ["daily", "release", "ci"]);
        assert_eq!(shown(Filter::Active), ["daily", "ci"]);
        assert_eq!(shown(Filter::Paused), ["release"]);
    }
}

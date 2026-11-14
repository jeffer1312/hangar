//! Quanto cada rodada pode gastar e com quê: a chave da Anthropic (créditos da assinatura) ou o
//! limite de uma conta Claude com folga na semana.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};

use crate::state::{Mode, Month};

/// Estimativa por milhão de tokens de transcrição; a medição está em docs/decisoes/plataforma.md.
pub const USD_PER_MTOK_ESTIMATE: f64 = 2.70;
pub const CHARS_PER_TOKEN: f64 = 3.5;
pub const FIRST_DREAM_DAYS: i64 = 14;
/// A conta só paga o sonho se ainda sobra bastante semana para o trabalho da pessoa.
pub const FALLBACK_MAX_WEEK_PCT: f64 = 70.0;
pub const MAX_PARALLEL: usize = 4;
/// Parte do teto guardada para o organizador, que roda depois de todos os analisadores.
pub const ORGANIZER_SHARE: f64 = 0.25;

pub fn estimate_usd(chars: usize) -> f64 {
    chars as f64 / CHARS_PER_TOKEN / 1e6 * USD_PER_MTOK_ESTIMATE
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub mtime: DateTime<Utc>,
    /// Texto novo (depois do cursor), já renderizado.
    pub chars: usize,
}

/// Índices das sessões que entram, da mais nova para a mais velha, até a estimativa bater no teto.
/// O que fica de fora espera a próxima noite com o cursor parado. No primeiro sonho só entram as
/// sessões dos últimos `FIRST_DREAM_DAYS`.
pub fn pick_sessions(candidates: &[Candidate], cap_usd: f64, first: bool, now: DateTime<Utc>) -> Vec<usize> {
    let oldest = now - Duration::days(FIRST_DREAM_DAYS);
    let mut order: Vec<usize> = (0..candidates.len()).filter(|&i| !first || candidates[i].mtime >= oldest).collect();
    order.sort_by(|&a, &b| candidates[b].mtime.cmp(&candidates[a].mtime));
    let mut total = 0.0;
    let mut picked = Vec::new();
    for i in order {
        let cost = estimate_usd(candidates[i].chars);
        if total + cost <= cap_usd {
            total += cost;
            picked.push(i);
        }
    }
    picked
}

#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Key { cap_usd: f64 },
    Account { dir: PathBuf, cap_usd: f64 },
    Skip(SkipReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Nem a chave nem uma conta com folga podem pagar.
    NoBudget,
}

pub struct Limits {
    pub project_cap_usd: f64,
    /// Crédito do mês da chave; `None` = sem teto do mês.
    pub month_cap_usd: Option<f64>,
}

/// `key_usable`: há chave configurada e ela não respondeu "crédito esgotado" nesta rodada.
/// `accounts`: conta Claude e o `pct` da janela de 7 dias dela.
pub fn decide(mode: Mode, key_usable: bool, month: &Month, limits: &Limits, accounts: &[(PathBuf, f64)]) -> Plan {
    if mode == Mode::Key && key_usable {
        let month_left = limits.month_cap_usd.map_or(f64::INFINITY, |cap| cap - month.key_usd);
        if month_left > 0.0 {
            return Plan::Key { cap_usd: limits.project_cap_usd.min(month_left) };
        }
    }
    accounts
        .iter()
        .filter(|(_, pct)| *pct < FALLBACK_MAX_WEEK_PCT)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(Plan::Skip(SkipReason::NoBudget), |(dir, _)| Plan::Account { dir: dir.clone(), cap_usd: limits.project_cap_usd })
}

/// Teto de uma chamada de analisador: o que resta da parte deles dividido pelas chamadas que podem
/// estar rodando juntas, com folga porque o CLI só para entre um turno e outro.
pub fn analyzer_call_cap(cap_usd: f64, spent_usd: f64) -> f64 {
    let left = cap_usd * (1.0 - ORGANIZER_SHARE) - spent_usd;
    (left / MAX_PARALLEL as f64 * 0.8).max(0.0)
}

pub fn organizer_call_cap(cap_usd: f64, spent_usd: f64) -> f64 {
    ((cap_usd - spent_usd) * 0.8).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 13, 1, 0, 0).unwrap()
    }

    fn limits(month: Option<f64>) -> Limits {
        Limits { project_cap_usd: 15.0, month_cap_usd: month }
    }

    fn month(key: f64) -> Month {
        Month { month: "2026-11".into(), key_usd: key, account_usd: 0.0 }
    }

    #[test]
    fn estimate_matches_the_reference_rate() {
        // 3,5 M caracteres = 1 M tokens = US$ 2,70.
        assert!((estimate_usd(3_500_000) - 2.70).abs() < 1e-9);
    }

    #[test]
    fn key_with_credit_pays_up_to_project_and_month_caps() {
        assert_eq!(decide(Mode::Key, true, &month(0.0), &limits(None), &[]), Plan::Key { cap_usd: 15.0 });
        assert_eq!(decide(Mode::Key, true, &month(195.0), &limits(Some(200.0)), &[]), Plan::Key { cap_usd: 5.0 });
    }

    #[test]
    fn exhausted_key_falls_to_the_account_with_most_room_below_70() {
        let accounts = vec![(PathBuf::from("/a"), 65.0), (PathBuf::from("/b"), 20.0), (PathBuf::from("/c"), 90.0)];
        let want = Plan::Account { dir: "/b".into(), cap_usd: 15.0 };
        assert_eq!(decide(Mode::Key, true, &month(200.0), &limits(Some(200.0)), &accounts), want);
        assert_eq!(decide(Mode::Key, false, &month(0.0), &limits(None), &accounts), want);
        assert_eq!(decide(Mode::Account, true, &month(0.0), &limits(None), &accounts), want);
    }

    #[test]
    fn no_key_and_every_account_busy_skips() {
        let accounts = vec![(PathBuf::from("/a"), 70.0), (PathBuf::from("/b"), 99.0)];
        assert_eq!(decide(Mode::Key, false, &month(0.0), &limits(None), &accounts), Plan::Skip(SkipReason::NoBudget));
    }

    #[test]
    fn newest_sessions_first_until_the_cap() {
        let day = |d: u32| Utc.with_ymd_and_hms(2026, 11, d, 12, 0, 0).unwrap();
        // ~US$ 5,40 cada.
        let c = |d: u32| Candidate { mtime: day(d), chars: 7_000_000 };
        let picked = pick_sessions(&[c(10), c(12), c(11)], 15.0, false, now());
        assert_eq!(picked, vec![1, 2]);
    }

    #[test]
    fn first_dream_reads_only_the_last_14_days() {
        let old = Candidate { mtime: Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap(), chars: 10 };
        let new = Candidate { mtime: Utc.with_ymd_and_hms(2026, 11, 10, 0, 0, 0).unwrap(), chars: 10 };
        assert_eq!(pick_sessions(&[old.clone(), new.clone()], 15.0, true, now()), vec![1]);
        assert_eq!(pick_sessions(&[old, new], 15.0, false, now()).len(), 2);
    }

    #[test]
    fn call_caps_never_add_up_past_the_project_cap() {
        let per_call = analyzer_call_cap(15.0, 0.0);
        assert!(per_call * MAX_PARALLEL as f64 <= 15.0 * (1.0 - ORGANIZER_SHARE));
        assert_eq!(analyzer_call_cap(15.0, 20.0), 0.0);
        assert!(organizer_call_cap(15.0, 10.0) <= 5.0);
    }
}

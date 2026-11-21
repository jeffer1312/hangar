import type { AccountTarget, Credencial, SessionInfo } from '@hangar/core';

export type AccountSession = SessionInfo & { config_dir?: string | null; engine_account?: string | null; lifecycle_id?: string | null };
export type SwitchTarget = AccountTarget & { engineAccount?: string; engineBinding?: string; unconfirmed?: boolean; credential?: Credencial };
const normalize = (value?: string | null) => value?.replace(/\\/g, '/').replace(/\/$/, '');

export function sessionCredential(session: AccountSession, credentials: Credencial[]): Credencial | undefined {
  if (session.engine_binding) return credentials.find(c => c.id === `cliproxy-binding:${session.engine_binding}`);
  const found = credentials.find(c => c.id === session.conta
    || (session.provider === 'claude' && !session.engine && c.tipo === 'claude' && normalize(c.path ?? c.id.slice(7)) === normalize(session.config_dir))
    || (session.provider === 'codex' && c.tipo === 'codex' && ((!!session.codex_account && c.codex_account === session.codex_account)
      || (!!session.codex_home && normalize(c.path ?? c.id.slice(6)) === normalize(session.codex_home)))));
  if (session.engine_account && found?.tipo === 'codex') return { ...found, cota: found.cliproxy_quota };
  return found;
}

export function isCurrentAccount(target: SwitchTarget, session: AccountSession): boolean {
  return target.engineAccount ? (!target.engineBinding || target.engineBinding === session.engine_binding)
    && (target.path === session.conta || target.engineAccount === session.engine_account)
    : `claude:${target.path}` === session.conta || normalize(target.path) === normalize(session.config_dir);
}

export function accountFolder(credential?: Credencial, path?: string): string {
  return (credential?.path || path?.replace(/^(claude|codex):/, '') || credential?.id.replace(/^(claude|codex):/, '') || '')
    .replace(/[\\/]+$/, '').split(/[\\/]/).pop() || '';
}

export function accountOptions(targets: SwitchTarget[], credentials: Credencial[]): SwitchTarget[] {
  return targets.flatMap(target => {
    const row = credentials.find(c => c.id === (target.engineAccount ? target.path : `claude:${target.path}`));
    const credential = target.engineAccount && row?.tipo === 'codex' ? { ...row, cota: row.cliproxy_quota } : row;
    const proxyLogin = !!target.engineAccount && (target.path.startsWith('cliproxy:') || !!row && 'cliproxy_quota' in row);
    if (!proxyLogin && credential?.login?.loggedIn === false) return [];
    const windows = credential?.cota?.estado === 'lida' ? credential.cota.janelas.filter(w => !w.por_modelo && Number.isFinite(w.pct)) : [];
    const pct = windows.length ? Math.max(...windows.map(w => w.pct)) : target.pct;
    return [{ ...target, credential, pct, full: target.full || (pct !== null && pct >= 99),
      low: target.low || (pct !== null && pct >= 95),
      unconfirmed: !proxyLogin && (credential?.login?.loggedIn !== true || credential?.login?.estado === 'indisponivel') }];
  }).sort((a, b) => Number(!!a.unconfirmed) - Number(!!b.unconfirmed) || Number(!!a.full) - Number(!!b.full)
    || (a.pct ?? 101) - (b.pct ?? 101) || a.path.localeCompare(b.path));
}

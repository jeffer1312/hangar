import type { AccountTarget, Credencial, SessionInfo } from '@hangar/core';

export type AccountSession = SessionInfo & { config_dir?: string | null; engine_account?: string | null; lifecycle_id?: string | null };
export type SwitchTarget = AccountTarget & { engineAccount?: string; unconfirmed?: boolean; credential?: Credencial };
const normalize = (value?: string | null) => value?.replace(/\\/g, '/').replace(/\/$/, '');

export function sessionCredential(session: AccountSession, credentials: Credencial[]): Credencial | undefined {
  return credentials.find(c => c.id === session.conta
    || (session.provider === 'claude' && !session.engine && c.tipo === 'claude' && normalize(c.path ?? c.id.slice(7)) === normalize(session.config_dir))
    || (session.provider === 'codex' && c.tipo === 'codex' && ((!!session.codex_account && c.codex_account === session.codex_account)
      || (!!session.codex_home && normalize(c.path ?? c.id.slice(6)) === normalize(session.codex_home)))));
}

export function isCurrentAccount(target: SwitchTarget, session: AccountSession): boolean {
  return target.engineAccount ? target.path === session.conta || target.engineAccount === session.engine_account
    : `claude:${target.path}` === session.conta || normalize(target.path) === normalize(session.config_dir);
}

export function accountFolder(credential?: Credencial, path?: string): string {
  return (credential?.path || path?.replace(/^(claude|codex):/, '') || credential?.id.replace(/^(claude|codex):/, '') || '')
    .replace(/[\\/]+$/, '').split(/[\\/]/).pop() || '';
}

export function accountOptions(targets: SwitchTarget[], credentials: Credencial[]): SwitchTarget[] {
  return targets.flatMap(target => {
    const credential = credentials.find(c => c.id === (target.engineAccount ? target.path : `claude:${target.path}`));
    // Login incerto não comprova pasta sem conta; só a ausência conferida permite escondê-la.
    if (credential?.login?.loggedIn === false) return [];
    const windows = credential?.cota?.estado === 'lida' ? credential.cota.janelas.filter(w => !w.por_modelo && Number.isFinite(w.pct)) : [];
    const pct = windows.length ? Math.max(...windows.map(w => w.pct)) : target.pct;
    // Conta só do proxy não tem credencial no Hangar: o login dela é o do proxy, já conferido na lista.
    const proxyOnly = target.path.startsWith('cliproxy:');
    return [{ ...target, credential, pct, full: target.full || (pct !== null && pct >= 99),
      low: target.low || (pct !== null && pct >= 95),
      unconfirmed: !proxyOnly && (credential?.login?.loggedIn !== true || credential?.login?.estado === 'indisponivel') }];
  }).sort((a, b) => Number(!!a.unconfirmed) - Number(!!b.unconfirmed) || Number(!!a.full) - Number(!!b.full)
    || (a.pct ?? 101) - (b.pct ?? 101) || a.path.localeCompare(b.path));
}

import type { SessionInfo } from '@hangar/core';
import * as m from '../paraglide/messages';

function accountLabel(value: string | null | undefined, harness: string): string | null {
  const base = value?.replace(/[\\/]+$/, '').split(/[\\/]/).pop();
  if (!base) return null;
  const name = base === `.${harness}` ? '' : base.replace(new RegExp(`^\\.${harness}-`), '');
  return name.replace(new RegExp(`^${harness}-(?=.)`), '') || m.conta_padrao();
}

/** `~/.gemini` é a conta Google padrão; `~/.antigravity-<nome>` é a adicional. */
function antigravityLabel(path: string): string | null {
  const base = path.replace(/[\\/]+$/, '').split(/[\\/]/).pop();
  if (!base) return null;
  return base === '.gemini' ? m.mobile_antigravity_default_account() : base.replace(/^\.antigravity-(?=.)/, '');
}

export function sessionAccountLabel(session: Pick<SessionInfo, 'provider' | 'conta' | 'engine' | 'codex_home' | 'codex_account'> & { config_dir?: string | null; engine_account?: string | null }): string | null {
  const engine = session.engine || (session.conta?.startsWith('chave:') ? session.conta.slice(6) : null);
  if (session.conta?.startsWith('antigravity:')) return antigravityLabel(session.conta.slice('antigravity:'.length));
  const harness = ['claude', 'codex'].find(h => session.conta?.startsWith(`${h}:`));
  let label = harness ? accountLabel(session.conta!.slice(harness.length + 1), harness) : null;
  if (!label) {
    if (engine) label = session.engine_account || null;
    else if (session.provider === 'claude') label = accountLabel(session.config_dir, 'claude');
    else if (session.provider === 'codex') label = accountLabel(session.codex_home, 'codex') ?? accountLabel(session.codex_account, 'codex');
  }
  return engine ? (label ? `${engine} · ${label}` : engine) : label;
}

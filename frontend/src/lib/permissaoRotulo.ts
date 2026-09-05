// Glifo e rótulo curto de um modo de permissão do Claude Code, pra pill e pro seletor. Os glifos
// são os do rodapé do próprio Claude: ⏸ nos modos que param pra perguntar, ⏵⏵ nos que seguem.
import * as m from '../paraglide/messages';

const GLIFO: Record<string, string> = {
  plan: '⏸',
  manual: '⏸',
  auto: '⏵⏵',
  acceptEdits: '⏵⏵',
  bypassPermissions: '⏵⏵',
  dontAsk: '⏵⏵',
};

const ROTULO: Record<string, () => string> = {
  plan: () => m.permissao_modo_plan(),
  manual: () => m.permissao_modo_manual(),
  auto: () => m.permissao_modo_auto(),
  acceptEdits: () => m.permissao_modo_acceptEdits(),
  bypassPermissions: () => m.permissao_modo_bypassPermissions(),
  dontAsk: () => m.permissao_modo_dontAsk(),
};

export function glifoPermissao(modo: string): string {
  return GLIFO[modo] ?? '';
}

// Modo que o app não conhece (CLI novo) sai como o id cru: dado, não interface.
export function rotuloPermissao(modo: string): string {
  return ROTULO[modo]?.() ?? modo;
}

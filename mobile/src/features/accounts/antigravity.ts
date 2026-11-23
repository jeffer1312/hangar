import type { AntigravityAccount } from '@hangar/core';
import * as m from '../../paraglide/messages';

/** Nome da conta Google nas listas: a padrão tem nome próprio, a adicional é o nome da pasta. */
export function antigravityAccountLabel(account: Pick<AntigravityAccount, 'id' | 'padrao'>): string {
  return account.padrao ? m.mobile_antigravity_default_account() : account.id;
}

/** Estado do login pelo `agy models`; `null` é que ele não respondeu, não que a conta saiu. */
export function antigravityLoginText(logado: boolean | null | undefined): string {
  return logado === true ? m.mobile_antigravity_signed_in() : logado === false ? m.mobile_antigravity_signed_out() : m.mobile_antigravity_login_unknown();
}

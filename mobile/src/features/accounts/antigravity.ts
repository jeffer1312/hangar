import type { AntigravityAccount } from '@hangar/core';
import * as m from '../../paraglide/messages';

/** Nome da conta Google nas listas: a padrão tem nome próprio, a adicional é o nome da pasta. */
export function antigravityAccountLabel(account: Pick<AntigravityAccount, 'id' | 'padrao'>): string {
  return account.padrao ? m.mobile_antigravity_default_account() : account.id;
}

/** Conta adicional (`~/.antigravity-<nome>`): fica fora das telas, porque o agy com terminal ignora a
 *  pasta dela e grava o login na padrão. Sem a lista do agy, decide pela pasta da credencial. */
export function isAdditionalAntigravity(credentialId: string, accounts: readonly AntigravityAccount[] | undefined): boolean {
  const account = accounts?.find((a) => a.credencial === credentialId);
  return account ? !account.padrao : /[\\/]\.antigravity-[^\\/]+$/.test(credentialId);
}

/** Estado do login pelo `agy models`; `null` é que ele não respondeu, não que a conta saiu. */
export function antigravityLoginText(logado: boolean | null | undefined): string {
  return logado === true ? m.mobile_antigravity_signed_in() : logado === false ? m.mobile_antigravity_signed_out() : m.mobile_antigravity_login_unknown();
}

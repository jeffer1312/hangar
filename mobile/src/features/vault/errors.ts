import type { VaultErrorCode } from '@hangar/core';
import * as m from '../../paraglide/messages';

const TEXT: Partial<Record<VaultErrorCode, () => string>> = {
  erro_cofre_trancado: m.vault_app_err_locked,
  erro_cofre_chave_perdida: m.vault_app_err_key_lost,
  erro_cofre_dona_fora: m.vault_app_err_owner_offline,
  erro_cofre_nome_repetido: m.vault_app_err_name_taken,
  erro_cofre_sem_chaveiro: m.vault_app_err_no_keyring,
  erro_cofre_ja_e_dona: m.vault_app_err_already_owner,
  erro_interacao_desatualizada: m.vault_app_err_request_changed,
  erro_cofre_pedido_respondido: m.vault_app_err_request_changed,
  erro_cofre_pedido_perdido: m.vault_app_err_request_gone,
};

/** Texto da recusa do cofre; código sem frase própria mostra a mensagem crua do pedido. */
export function vaultErrorText(e: unknown): string {
  const code = (e as { code?: unknown } | null)?.code;
  const known = typeof code === 'string' ? TEXT[code as VaultErrorCode] : undefined;
  if (known) return known();
  return e instanceof Error ? e.message : String(e);
}

import type { VaultErrorCode } from '@hangar/core';
import * as m from '../../paraglide/messages';

// Os códigos só das tools do agente (negado, sem resposta, domínio, navegador…) não chegam a esta tela.
const TEXT: Partial<Record<VaultErrorCode, () => string>> = {
  erro_cofre_ausente: m.vault_app_err_absent,
  erro_cofre_dona_remota: m.vault_app_err_remote_owner,
  erro_cofre_entrada_desconhecida: m.vault_app_err_unknown_entry,
  erro_cofre_ilegivel: m.vault_app_err_unreadable,
  erro_cofre_indisponivel: m.vault_app_err_unavailable,
  erro_cofre_chaveiro_indisponivel: m.vault_app_err_keyring_unavailable,
  erro_cofre_so_dono: m.vault_app_err_owner_only,
  erro_cofre_peer_desconhecido: m.vault_app_err_unknown_peer,
  erro_cofre_config_ilegivel: m.vault_app_err_config_unreadable,
  erro_cofre_pedidos_demais: m.vault_app_err_too_many_requests,
  erro_cofre_backup_grande: m.vault_app_err_backup_too_big,
  erro_corpo_invalido: m.vault_app_err_invalid_body,
  erro_cofre_recuperacao_errada: m.vault_app_err_recovery_wrong,
  erro_cofre_recuperacao_curta: m.vault_app_err_recovery_short,
  erro_cofre_recuperacao_nao_gravada: m.vault_app_err_recovery_not_saved,
  erro_cofre_nao_vazio: m.vault_app_err_not_empty,
  erro_cofre_backup_ausente: m.vault_app_err_backup_missing,
  erro_cofre_backup_ilegivel: m.vault_app_err_backup_unreadable,
  erro_cofre_backup_peer_fora: m.vault_app_err_backup_peer_offline,
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
  const known = typeof code === 'string' ? TEXT[code as keyof typeof TEXT] : undefined;
  if (known) return known();
  return e instanceof Error ? e.message : String(e);
}

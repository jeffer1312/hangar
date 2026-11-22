import { syncSignIn, type Server } from '@hangar/core';
import { syncCrypto } from './syncCrypto';
import * as m from '../../paraglide/messages';

const TEXT: Record<string, () => string> = {
  credentials_mismatch: m.sync_credenciais_invalidas,
  sync_too_many_attempts: m.sync_muitas_tentativas,
  sync_insecure: m.mobile_sync_login_https,
  sync_vault_unreadable: m.mobile_sync_login_vault,
  sync_unavailable: m.mobile_sync_login_unavailable,
  network_failed: m.falha_conexao,
};

/** Texto de uma falha do login de sincronização numa máquina (`code` e `status` do erro). */
export function syncFailureText(error: unknown): string {
  const { code, status } = (error ?? {}) as { code?: string; status?: number };
  // Proxy responde 5xx em HTML: o código basta para a tela.
  if (status !== undefined && status >= 500) return `${m.falha_conexao()}: HTTP ${status}`;
  const text = TEXT[code ?? ''];
  if (text) return text();
  return status ? `${m.login_falha()}: HTTP ${status}` : m.login_falha();
}

/** Entrada por endereço, usuário e senha: só lê o cofre daquela máquina; a lista daqui nunca sobe. */
export async function signInAtAddress(address: string, user: string, password: string): Promise<Server[]> {
  try {
    return await syncSignIn(address, user, password, syncCrypto);
  } catch (error) {
    throw new Error(syncFailureText(error));
  }
}

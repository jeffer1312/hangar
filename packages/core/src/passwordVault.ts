// Cofre de senhas: rotas do dono, evento `vault` da lista e códigos de recusa. As rotas entre
// máquinas (`use`, `ask-entry`, `requests/{id}/wait`, `backup`) são do servidor, não dos clientes.
// A senha só viaja no corpo, nunca na URL.
import { vaultFetchForServer } from './api';
import type { Server } from './servers';
import * as m from './paraglide/messages';

export type VaultRole = 'owner' | 'remote' | 'none';
export type VaultState = 'open' | 'locked' | 'absent' | 'offline' | 'key_lost';
export type VaultPolicy = 'ask' | 'free';

export interface VaultBackupStatus { peer: string; last_ok_at?: number; last_error?: string }

export interface VaultStatus {
  role: VaultRole;
  owner?: string;
  state: VaultState;
  /** Motivo do `offline` ou do `absent` na dona. */
  detail?: string;
  backup?: VaultBackupStatus;
  /** Id da máquina que respondeu, quando ela fala do próprio cofre. */
  server_id?: string;
  /** `role: "remote"`: o endereço da dona no `peers.json` desta máquina (ver `findOwnerServer`). */
  owner_url?: string;
}

/** Host, porta e caminho, sem barra final e sem caixa: o mesmo endereço escrito de dois jeitos casa. */
function hostKey(url: string): string {
  try {
    const u = new URL(url.trim());
    return `${u.host}${u.pathname.replace(/\/+$/, '')}`.toLowerCase();
  } catch { return url.trim().replace(/\/+$/, '').toLowerCase(); }
}

/** O servidor salvo neste aparelho que é a dona em `ownerUrl` (pelo endereço principal, da rede
 *  local ou do Tailscale); `null` se nenhum. Convite e servidor desligado não contam. */
export function findOwnerServer(ownerUrl: string, servers: Server[]): Server | null {
  const want = hostKey(ownerUrl);
  return servers.find((s) => !s.invite && !s.disabled
    && [s.baseUrl, s.lan?.url, s.remote?.url].some((u) => !!u && hostKey(u) === want)) ?? null;
}

/** `owner`: id do servidor dono ou `"self"`; `null` apaga o campo. */
export interface VaultConfigChange { owner?: string | null; backup_peer?: string | null }

/** Entrada sem a senha, como a lista mostra. */
export interface VaultEntry {
  id: string;
  name: string;
  username?: string;
  url?: string;
  notes?: string;
  policy: VaultPolicy;
  updated_at: number;
}

export interface VaultNewEntry {
  name: string;
  username?: string | null;
  password: string;
  url?: string | null;
  notes?: string | null;
  policy?: VaultPolicy;
}

/** Campo ausente não muda; `null` apaga. */
export interface VaultEntryChange {
  name?: string;
  username?: string | null;
  password?: string;
  url?: string | null;
  notes?: string | null;
  policy?: VaultPolicy;
}

export type VaultRequestKind = 'approve' | 'entry';
export type VaultUseKind = 'fill' | 'exec' | 'file';

/** Um cartão de pedido (evento `vault`). `target`: domínio no `fill`, comando no `exec`, `"arquivo"` no `file`. */
export interface VaultRequest {
  id: string;
  interaction_id: string;
  kind: VaultRequestKind;
  /** `servidor::sessao` de quem pediu. */
  origin: string;
  entry_name?: string;
  label?: string;
  reason: string;
  use?: VaultUseKind;
  target?: string;
  placeholder?: string;
  /** Segundos desde a época. */
  expires_at: number;
}

/** Evento `vault` da lista do dono. `state` é `null` antes de a máquina ler o cofre pela primeira vez. */
export interface VaultEvent { state: VaultState | null; requests: VaultRequest[] }

/** Resposta ao cartão: `approve` aceita `allow`/`deny`; `entry` aceita `save`/`decline`. */
export type VaultAnswer =
  | { interaction_id: string; decision: 'allow' | 'deny' }
  | { interaction_id: string; decision: 'save'; password: string; keep: boolean; name?: string; policy?: VaultPolicy }
  | { interaction_id: string; decision: 'decline' };

export type VaultErrorCode =
  | 'erro_cofre_trancado'
  | 'erro_cofre_chave_perdida'
  | 'erro_cofre_dona_fora'
  | 'erro_cofre_entrada_desconhecida'
  | 'erro_cofre_sem_chaveiro'
  | 'erro_cofre_pedido_perdido'
  | 'erro_cofre_nome_repetido'
  | 'erro_cofre_ausente'
  | 'erro_cofre_ilegivel'
  | 'erro_cofre_indisponivel'
  | 'erro_cofre_chaveiro_indisponivel'
  | 'erro_cofre_so_dono'
  | 'erro_cofre_ja_e_dona'
  | 'erro_cofre_peer_desconhecido'
  | 'erro_cofre_config_ilegivel'
  | 'erro_cofre_pedido_respondido'
  | 'erro_cofre_pedidos_demais'
  | 'erro_cofre_recuperacao_errada'
  | 'erro_cofre_recuperacao_curta'
  | 'erro_cofre_dona_remota'
  | 'erro_cofre_recuperacao_nao_gravada'
  | 'erro_cofre_nao_vazio'
  | 'erro_cofre_backup_ausente'
  | 'erro_cofre_backup_grande'
  | 'erro_cofre_backup_ilegivel'
  | 'erro_cofre_backup_peer_fora'
  | 'erro_interacao_desatualizada'
  | 'erro_corpo_invalido';

// Os códigos só das tools do agente (negado, sem resposta, domínio, navegador…) não chegam às telas.
const ERROR_TEXT: Partial<Record<VaultErrorCode, () => string>> = {
  erro_cofre_trancado: m.vault_err_trancado,
  erro_cofre_chave_perdida: m.vault_err_chave_perdida,
  erro_cofre_dona_fora: m.vault_err_dona_fora,
  erro_cofre_nome_repetido: m.vault_err_nome_repetido,
  erro_cofre_sem_chaveiro: m.vault_err_sem_chaveiro,
  erro_cofre_ja_e_dona: m.vault_err_ja_e_dona,
  erro_interacao_desatualizada: m.vault_err_interacao_desatualizada,
  erro_cofre_pedido_respondido: m.vault_err_interacao_desatualizada,
  erro_cofre_pedido_perdido: m.vault_err_pedido_perdido,
  erro_cofre_recuperacao_errada: m.vault_err_recuperacao_errada,
  erro_cofre_recuperacao_curta: m.vault_err_recuperacao_curta,
  erro_cofre_recuperacao_nao_gravada: m.vault_err_recuperacao_nao_gravada,
  erro_cofre_nao_vazio: m.vault_err_nao_vazio,
  erro_cofre_backup_ausente: m.vault_err_backup_ausente,
  erro_cofre_backup_ilegivel: m.vault_err_backup_ilegivel,
  erro_cofre_backup_peer_fora: m.vault_err_backup_peer_fora,
  erro_cofre_backup_grande: m.vault_err_backup_grande,
  erro_cofre_ausente: m.vault_err_ausente,
  erro_cofre_dona_remota: m.vault_err_dona_remota,
  erro_cofre_entrada_desconhecida: m.vault_err_entrada_desconhecida,
  erro_cofre_ilegivel: m.vault_err_ilegivel,
  erro_cofre_indisponivel: m.vault_err_indisponivel,
  erro_cofre_chaveiro_indisponivel: m.vault_err_chaveiro_indisponivel,
  erro_cofre_so_dono: m.vault_err_so_dono,
  erro_cofre_peer_desconhecido: m.vault_err_peer_desconhecido,
  erro_cofre_config_ilegivel: m.vault_err_config_ilegivel,
  erro_cofre_pedidos_demais: m.vault_err_pedidos_demais,
  erro_corpo_invalido: m.vault_err_corpo_invalido,
};

/** Frase da recusa do cofre; `undefined` para código sem frase própria (quem chama mostra a do servidor). */
export function vaultErrorText(code: string): string | undefined {
  return Object.prototype.hasOwnProperty.call(ERROR_TEXT, code) ? ERROR_TEXT[code as VaultErrorCode]?.() : undefined;
}

/** Criar o cofre ou gravar nele pode esperar o chaveiro. */
const KEYRING_MS = 45_000;

const entryPath = (id: string) => `/api/vault/entries/${encodeURIComponent(id)}`;
const requestPath = (id: string) => `/api/vault/requests/${encodeURIComponent(id)}`;

export function getVaultStatus(s: Server): Promise<VaultStatus> {
  return vaultFetchForServer(s, '/api/vault/status', undefined, KEYRING_MS);
}

export function putVaultConfig(s: Server, change: VaultConfigChange): Promise<VaultStatus> {
  return vaultFetchForServer(s, '/api/vault/config', { method: 'PUT', body: JSON.stringify(change) }, KEYRING_MS);
}

export interface VaultPeer { id: string; base_url: string }

/** Máquinas do `peers.json` de `s`: os candidatos a dona e a destino do backup. */
export async function listVaultPeers(s: Server): Promise<VaultPeer[]> {
  const list: unknown = await vaultFetchForServer(s, '/api/peers', undefined, KEYRING_MS);
  if (!Array.isArray(list)) return [];
  return list.flatMap((p) => (p && typeof p.id === 'string' && typeof p.base_url === 'string' ? [{ id: p.id, base_url: p.base_url }] : []));
}

export function listVaultEntries(s: Server): Promise<VaultEntry[]> {
  return vaultFetchForServer(s, '/api/vault/entries', undefined, KEYRING_MS);
}

export function addVaultEntry(s: Server, entry: VaultNewEntry): Promise<{ id: string }> {
  return vaultFetchForServer(s, '/api/vault/entries', { method: 'POST', body: JSON.stringify(entry) }, KEYRING_MS);
}

export function updateVaultEntry(s: Server, id: string, change: VaultEntryChange): Promise<{ id: string }> {
  return vaultFetchForServer(s, entryPath(id), { method: 'PATCH', body: JSON.stringify(change) }, KEYRING_MS);
}

export function deleteVaultEntry(s: Server, id: string): Promise<void> {
  return vaultFetchForServer(s, entryPath(id), { method: 'DELETE' }, KEYRING_MS);
}

/** Revelar ou copiar na tela; o valor não vai para estado persistente nem log. */
export function revealVaultPassword(s: Server, id: string): Promise<{ password: string }> {
  return vaultFetchForServer(s, `${entryPath(id)}/password`, { cache: 'no-store' }, KEYRING_MS);
}

/** Responde o cartão na máquina dona (`s` é a dona, nunca a que pediu). */
export function answerVaultRequest(s: Server, id: string, answer: VaultAnswer): Promise<void> {
  return vaultFetchForServer(s, requestPath(id), { method: 'POST', body: JSON.stringify(answer) }, KEYRING_MS);
}

/** Define a senha de recuperação da cópia na VPS; responde o estado, com `backup`. */
export function setVaultRecovery(s: Server, password: string): Promise<VaultStatus> {
  return vaultFetchForServer(s, '/api/vault/recovery', { method: 'POST', body: JSON.stringify({ password }) }, KEYRING_MS);
}

/** Restaura num cofre vazio a partir da cópia guardada em `fromPeer`. `serverId`: de qual máquina
 * é a cópia (a dona perdida); sem ele, a de `s`. */
export function restoreVault(s: Server, password: string, fromPeer: string, serverId?: string): Promise<VaultStatus> {
  const body = serverId ? { password, from_peer: fromPeer, server_id: serverId } : { password, from_peer: fromPeer };
  return vaultFetchForServer(s, '/api/vault/restore', { method: 'POST', body: JSON.stringify(body) }, KEYRING_MS);
}

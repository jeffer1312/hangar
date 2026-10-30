// Cofre de senhas: rotas do dono, evento `vault` da lista e códigos de recusa. As rotas entre
// máquinas (`use`, `ask-entry`, `requests/{id}/wait`, `backup`) são do servidor, não dos clientes.
// A senha só viaja no corpo, nunca na URL.
import { vaultFetchForServer } from './api';
import type { Server } from './servers';

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
  | 'erro_cofre_negado'
  | 'erro_cofre_sem_resposta'
  | 'erro_cofre_entrada_desconhecida'
  | 'erro_cofre_sem_chaveiro'
  | 'erro_cofre_sessao_sem_prazo'
  | 'erro_cofre_pedido_perdido'
  | 'erro_cofre_dominio_diferente'
  | 'erro_cofre_campo_nao_senha'
  | 'erro_cofre_sem_navegador'
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
  | 'erro_interacao_desatualizada'
  | 'erro_corpo_invalido';

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

export function cancelVaultRequest(s: Server, id: string): Promise<void> {
  return vaultFetchForServer(s, requestPath(id), { method: 'DELETE' });
}

/** Define a senha de recuperação da cópia na VPS (Task 6). */
export function setVaultRecovery(s: Server, password: string): Promise<void> {
  return vaultFetchForServer(s, '/api/vault/recovery', { method: 'POST', body: JSON.stringify({ password }) }, KEYRING_MS);
}

/** Restaura num cofre vazio a partir da cópia guardada em `fromPeer` (Task 6). */
export function restoreVault(s: Server, password: string, fromPeer: string): Promise<void> {
  return vaultFetchForServer(s, '/api/vault/restore', { method: 'POST', body: JSON.stringify({ password, from_peer: fromPeer }) }, KEYRING_MS);
}

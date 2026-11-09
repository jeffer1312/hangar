// Plugins do Claude Code da máquina (`/api/plugins`, só do dono): tipos do contrato e as chamadas.
// Prazo = o pior caso do servidor + 10 s, para o erro dele (504 `plugins_cli_timeout`) chegar antes
// do nosso. Toda mudança relê o estado depois (`plugin list`, 20 s), e isso entra na conta.
import { pluginsFetchForServer } from './api';
import type { Server } from './servers';
import type { SessionInfo } from './types';

export interface InstalledPlugin {
  id: string;
  name: string;
  marketplace: string;
  version: string;
  /** ISO 8601 UTC. */
  last_updated: string;
  enabled: boolean;
  available_version: string | null;
  /** `false` em fonte externa: a versão nova só aparece ao atualizar. */
  update_known: boolean;
  description: string | null;
  notes: string[];
}

export interface PluginMarketplace {
  name: string;
  source_label: string;
  last_updated: string | null;
  auto_update: boolean;
  /** `false`: fora de `extraKnownMarketplaces`, a chave fica desligada. */
  auto_update_editable: boolean;
  builtin: boolean;
}

export interface PluginsState {
  plugins: InstalledPlugin[];
  marketplaces: PluginMarketplace[];
  /** Último `check` bem-sucedido desta máquina. */
  checked_at: string | null;
}

export interface PluginOperationResult {
  outcome: string;
  message: string;
  old_version?: string;
  new_version?: string;
}

/** Resposta de toda mudança: o estado relido do disco e o que a CLI disse. */
export interface PluginsChange extends PluginsState {
  result?: PluginOperationResult;
}

export interface AvailablePlugin {
  id: string;
  name: string;
  marketplace: string;
  version: string | null;
  description: string | null;
}

export interface PluginInventory {
  skills: number | null;
  agents: number | null;
  hooks: number | null;
  mcp: number | null;
  lsp: number | null;
}

export interface PluginDetails {
  id: string;
  inventory: PluginInventory;
  always_on_tokens: number | null;
}

export type PluginAction = 'install' | 'update' | 'enable' | 'disable' | 'uninstall';

const READ_MS = 30_000;
const AVAILABLE_MS = 70_000;
const CHANGE_MS = 50_000;
const DOWNLOAD_MS = 210_000;
/** Remover marketplace lê a lista antes, para desligar os plugins dele nas contas. */
const MARKETPLACE_REMOVE_MS = 70_000;

const pluginPath = (id: string) => `/api/plugins/${encodeURIComponent(id)}`;
const marketplacePath = (name: string) => `/api/plugins/marketplaces/${encodeURIComponent(name)}`;

export function getPluginsForServer(s: Server): Promise<PluginsState> {
  return pluginsFetchForServer(s, '/api/plugins', undefined, READ_MS);
}

/** Baixa todos os marketplaces; responde o estado com `checked_at` novo. */
export function checkPluginsForServer(s: Server): Promise<PluginsState> {
  return pluginsFetchForServer(s, '/api/plugins/check', { method: 'POST' }, DOWNLOAD_MS);
}

export function getAvailablePluginsForServer(s: Server): Promise<{ plugins: AvailablePlugin[] }> {
  return pluginsFetchForServer(s, '/api/plugins/available', undefined, AVAILABLE_MS);
}

export function getPluginDetailsForServer(s: Server, id: string): Promise<PluginDetails> {
  return pluginsFetchForServer(s, `${pluginPath(id)}/details`, undefined, READ_MS);
}

export function pluginActionForServer(s: Server, id: string, action: PluginAction): Promise<PluginsChange> {
  if (action === 'uninstall') return pluginsFetchForServer(s, pluginPath(id), { method: 'DELETE' }, CHANGE_MS);
  const prazo = action === 'install' || action === 'update' ? DOWNLOAD_MS : CHANGE_MS;
  return pluginsFetchForServer(s, `${pluginPath(id)}/${action}`, { method: 'POST' }, prazo);
}

export function addMarketplaceForServer(s: Server, source: string): Promise<PluginsChange> {
  return pluginsFetchForServer(s, '/api/plugins/marketplaces', { method: 'POST', body: JSON.stringify({ source }) }, DOWNLOAD_MS);
}

export function updateMarketplaceForServer(s: Server, name: string): Promise<PluginsChange> {
  return pluginsFetchForServer(s, `${marketplacePath(name)}/update`, { method: 'POST' }, DOWNLOAD_MS);
}

export function removeMarketplaceForServer(s: Server, name: string): Promise<PluginsChange> {
  return pluginsFetchForServer(s, marketplacePath(name), { method: 'DELETE' }, MARKETPLACE_REMOVE_MS);
}

export function setMarketplaceAutoUpdateForServer(s: Server, name: string, enabled: boolean): Promise<PluginsChange> {
  return pluginsFetchForServer(s, `${marketplacePath(name)}/auto-update`, { method: 'PUT', body: JSON.stringify({ enabled }) }, CHANGE_MS);
}

/** Sessões Claude abertas que ainda estão com o estado anterior à mudança: só reabrindo pegam a nova. */
export function liveClaudeSessions<T extends Pick<SessionInfo, 'provider' | 'state' | 'asleep'>>(sessions: T[]): T[] {
  return sessions.filter((x) => (x.provider ?? 'claude') === 'claude' && x.state !== 'dead' && !x.asleep);
}

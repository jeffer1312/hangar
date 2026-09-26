// Configuração compartilhada: forma das respostas do backend (/api/config-sync), a prévia entre
// dois manifestos e os textos. Mora no core para o app nativo usar a mesma lógica depois.
import * as m from './paraglide/messages';

export const CONFIG_SYNC_ITEMS = [
  'claude_instructions', 'claude_skills', 'claude_agents', 'claude_hooks', 'claude_plugins',
  'claude_mcp', 'claude_env', 'claude_settings', 'codex', 'engines', 'hangar_prefs',
] as const;
export type ConfigSyncItem = (typeof CONFIG_SYNC_ITEMS)[number];

export interface ConfigSyncWarning { code: string; params: Record<string, string> }
export interface ConfigSyncManifestItem {
  ok: boolean; hashes: Record<string, string>; bytes: number; warnings: ConfigSyncWarning[];
  /** Scripts que cada evento de hook roda; servidor antigo não manda. */
  labels?: Record<string, string[]>;
  /** O que cada entrada faz, lido do próprio arquivo; servidor antigo não manda. */
  descriptions?: Record<string, string>;
}
export interface ConfigSyncManifest { version: number; machine: string; items: Partial<Record<ConfigSyncItem, ConfigSyncManifestItem>> }
export interface ConfigSyncItemResult { status: 'applied' | 'same' | 'failed'; changed: string[]; warnings: ConfigSyncWarning[] }
export interface ConfigSyncReport { items: Partial<Record<ConfigSyncItem, ConfigSyncItemResult>>; backup: string }
export interface ConfigSyncDiff { added: string[]; changed: string[]; same: string[]; onlyTarget: string[] }

/** Prévia por item: o que a origem tem e o destino não, o que difere, o que já é igual e o que
 * só existe no destino (fica lá). Compara os hashes canônicos, então casas diferentes não contam. */
export function diffManifests(
  origin: ConfigSyncManifest,
  target: ConfigSyncManifest,
  items: readonly ConfigSyncItem[],
): Partial<Record<ConfigSyncItem, ConfigSyncDiff>> {
  const out: Partial<Record<ConfigSyncItem, ConfigSyncDiff>> = {};
  for (const item of items) {
    const a = origin.items[item]?.hashes ?? {};
    const b = target.items[item]?.hashes ?? {};
    const diff: ConfigSyncDiff = { added: [], changed: [], same: [], onlyTarget: [] };
    for (const [name, hash] of Object.entries(a)) {
      if (!(name in b)) diff.added.push(name);
      else if (b[name] !== hash) diff.changed.push(name);
      else diff.same.push(name);
    }
    for (const name of Object.keys(b)) if (!(name in a)) diff.onlyTarget.push(name);
    for (const list of Object.values(diff)) list.sort();
    out[item] = diff;
  }
  return out;
}

export type ConfigSyncRowStatus = 'added' | 'changed' | 'same' | 'onlyTarget';
export type ConfigSyncGroup = 'settings' | 'files' | 'refs' | 'entries';
export interface ConfigSyncScript { name: string; description: string }
export interface ConfigSyncRow {
  key: string; name: string; group: ConfigSyncGroup; status: ConfigSyncRowStatus;
  description: string; scripts: ConfigSyncScript[]; selectable: boolean;
}

const MARKERS: Record<string, string> = { HOME: '~', CLAUDE: '~/.claude', CODEX: '~/.codex', HANGAR: 'hangar' };

/** `⟦HOME⟧/x` como a pessoa escreveria: `~/x`. */
export function configSyncPath(marked: string): string {
  return marked.replace(/⟦([A-Z]+)⟧/g, (all, name: string) => MARKERS[name] ?? all);
}

function rowOf(item: ConfigSyncItem, key: string): Pick<ConfigSyncRow, 'name' | 'group'> {
  if (key.startsWith('ref:')) return { name: configSyncPath(key.slice(4)), group: 'refs' };
  if (item === 'claude_skills') return { name: key.replace(/^skills\//, ''), group: 'entries' };
  if (item !== 'claude_hooks') return { name: key, group: 'entries' };
  if (key === 'statusLine') return { name: key, group: 'settings' };
  if (key.startsWith('hooks:')) return { name: key.slice(6), group: 'settings' };
  return { name: key.replace(/^hooks\//, ''), group: 'files' };
}

const RANK: Record<ConfigSyncRowStatus, number> = { added: 0, changed: 1, same: 2, onlyTarget: 3 };

/** Uma linha por entrada, juntando os destinos: basta um destino sem a entrada para ela ser
 * "novo" (e alterada em um para ser "alterado"), porque é o que o envio muda em algum lugar. */
export function configSyncRows(
  item: ConfigSyncItem,
  diffs: readonly ConfigSyncDiff[],
  meta: Pick<ConfigSyncManifestItem, 'labels' | 'descriptions'> = {},
): ConfigSyncRow[] {
  const descriptions = meta.descriptions ?? {};
  // Script de um evento: a descrição é a do arquivo com o mesmo nome (pasta hooks/ ou ref).
  const scriptText = (name: string) =>
    Object.entries(descriptions).find(([k]) => k.endsWith(`/${name}`))?.[1] ?? '';
  const status = new Map<string, ConfigSyncRowStatus>();
  const mark = (key: string, s: ConfigSyncRowStatus) => {
    const now = status.get(key);
    if (now === undefined || RANK[s] < RANK[now]) status.set(key, s);
  };
  for (const d of diffs) {
    d.added.forEach((k) => mark(k, 'added'));
    d.changed.forEach((k) => mark(k, 'changed'));
    d.same.forEach((k) => mark(k, 'same'));
  }
  for (const d of diffs) d.onlyTarget.forEach((k) => { if (!status.has(k)) status.set(k, 'onlyTarget'); });
  return [...status].map(([key, s]) => {
    const { name, group } = rowOf(item, key);
    const scripts = (meta.labels?.[key] ?? []).map((n) => ({ name: n, description: scriptText(n) }));
    return { key, name, group, status: s, description: descriptions[key] ?? '', scripts, selectable: s !== 'onlyTarget' };
  }).sort((a, b) => RANK[a.status] - RANK[b.status] || a.name.localeCompare(b.name));
}

const ITEM_LABEL: Record<ConfigSyncItem, () => string> = {
  claude_instructions: m.shared_config_item_claude_instructions,
  claude_skills: m.shared_config_item_claude_skills,
  claude_agents: m.shared_config_item_claude_agents,
  claude_hooks: m.shared_config_item_claude_hooks,
  claude_plugins: m.shared_config_item_claude_plugins,
  claude_mcp: m.shared_config_item_claude_mcp,
  claude_env: m.shared_config_item_claude_env,
  claude_settings: m.shared_config_item_claude_settings,
  codex: m.shared_config_item_codex,
  engines: m.shared_config_item_engines,
  hangar_prefs: m.shared_config_item_hangar_prefs,
};

export function configSyncItemLabel(item: ConfigSyncItem): string {
  return ITEM_LABEL[item]?.() ?? item;
}

type P = Record<string, string>;
const WARNING: Record<string, (p: P) => string> = {
  config_sync_heavy_dir_skipped: (p) => m.config_sync_heavy_dir_skipped({ entry: p.entry ?? '', dir: p.dir ?? '' }),
  config_sync_unreadable: (p) => m.config_sync_unreadable({ entry: p.entry ?? '', error: p.error ?? '' }),
  config_sync_broken_link: (p) => m.config_sync_broken_link({ entry: p.entry ?? '' }),
  config_sync_invalid_json: (p) => m.config_sync_invalid_json({ file: p.file ?? '' }),
  config_sync_item_failed: (p) => m.config_sync_item_failed({ error: p.error ?? '' }),
  config_sync_marketplace_unknown: (p) => m.config_sync_marketplace_unknown({ marketplace: p.marketplace ?? '' }),
  config_sync_marketplace_failed: (p) => m.config_sync_marketplace_failed({ marketplace: p.marketplace ?? '', detail: p.detail ?? '' }),
  config_sync_plugin_failed: (p) => m.config_sync_plugin_failed({ plugin: p.plugin ?? '', detail: p.detail ?? '' }),
  config_sync_missing_program: (p) => m.config_sync_missing_program({ program: p.program ?? '', where: p.where ?? '' }),
  config_sync_hangar_outdated: (p) => m.config_sync_hangar_outdated({ file: p.file ?? '' }),
  config_sync_hangar_skill_kept: (p) => m.config_sync_hangar_skill_kept({ entry: p.entry ?? '' }),
  config_sync_hook_replaced: (p) => m.config_sync_hook_replaced({ event: p.event ?? '', command: p.command ?? '' }),
  config_sync_invalid_entry: (p) => m.config_sync_invalid_entry({ entry: p.entry ?? '' }),
  config_sync_pref_rejected: (p) => m.config_sync_pref_rejected({ key: p.key ?? '', error: p.error ?? '' }),
  config_sync_after_failed: (p) => m.config_sync_after_failed({ step: p.step ?? '', error: p.error ?? '' }),
  config_sync_item_missing: () => m.config_sync_item_missing(),
  config_sync_unsupported_value: (p) => m.config_sync_unsupported_value({ key: p.key ?? '' }),
  config_sync_link_replaced: (p) => m.config_sync_link_replaced({ entry: p.entry ?? '', target: p.target ?? '' }),
};

export function configSyncWarningText(w: ConfigSyncWarning): string {
  return Object.prototype.hasOwnProperty.call(WARNING, w.code) ? WARNING[w.code](w.params ?? {}) : w.code;
}

// Etiqueta da máquina e das contas, gravada no servidor de cada máquina (`/api/tags`, Rust
// `tags.rs`). Cor e ícone viajam como chave: cada cliente traduz para o próprio tema.
import type { Server } from './servers';

export const TAG_COLORS = ['slate', 'blue', 'green', 'lime', 'amber', 'orange', 'pink', 'violet'] as const;
export const MACHINE_ICONS = ['laptop', 'monitor', 'server', 'house', 'briefcase', 'cloud'] as const;
export const ACCOUNT_ICONS = ['credit-card', 'provider', 'key-round', 'wallet', 'user-round', 'circle-user', 'contact',
  'badge-check', 'at-sign', 'ticket', 'id-card'] as const;
export const TAG_NAME_MAX = 24;

export type TagColor = typeof TAG_COLORS[number];
export type MachineIcon = typeof MACHINE_ICONS[number];
export type AccountIcon = typeof ACCOUNT_ICONS[number];

export interface Tag { name: string; icon: MachineIcon; color: TagColor | null }
export interface AccountTag { icon: AccountIcon; color: TagColor | null }
/** Conta como a lista a mostra: o nome já vem do servidor, cortado no `@`. */
export interface AccountBadge { name: string; icon: AccountIcon; color: TagColor | null }

/** `GET /api/tags`: contas pelo id de `/api/credenciais`. */
export interface TagsDoc { machine: Tag | null; accounts: Record<string, AccountTag> }
/** Corpo do `PUT /api/tags`: chave ausente não mexe, `null` apaga. */
export interface TagsChange { machine?: Tag | null; account?: { id: string; tag: AccountTag | null } }

/** Evento `tags` da lista do dono; contas pela `conta` da linha. */
export interface TagsEvent { machine_id: string | null; machine: Tag | null; accounts: Record<string, AccountBadge> }

/** Cor padrão de cada chave, para quem desenha `serverColor` direto no CSS; o tema do cliente pode trocar. */
export const TAG_COLOR_HEX: Record<TagColor, string> = {
  slate: '#64748b', blue: '#3b82f6', green: '#22c55e', lime: '#84cc16',
  amber: '#f59e0b', orange: '#f97316', pink: '#ec4899', violet: '#8b5cf6',
};

export const tagHex = (color: TagColor | null | undefined): string | null => (color ? TAG_COLOR_HEX[color] : null);

const isColor = (v: unknown): v is TagColor | null => v === null || (TAG_COLORS as readonly unknown[]).includes(v);
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

function parseTag<I extends string>(v: unknown, icons: readonly I[]): { name: string; icon: I; color: TagColor | null } | null | undefined {
  if (v === null) return null;
  if (!isObject(v) || typeof v.name !== 'string' || !(icons as readonly unknown[]).includes(v.icon)
    || !isColor(v.color ?? null)) return undefined;
  return { name: v.name, icon: v.icon as I, color: (v.color ?? null) as TagColor | null };
}

/** Lê o `data` do evento; dado torto = `null` (fica a etiqueta anterior). Conta torta sai sozinha. */
export function parseTagsEvent(data: string): TagsEvent | null {
  let raw: unknown;
  try { raw = JSON.parse(data); } catch { return null; }
  if (!isObject(raw)) return null;
  const machine = parseTag(raw.machine ?? null, MACHINE_ICONS);
  const id = raw.machine_id ?? null;
  if (machine === undefined || (id !== null && typeof id !== 'string')) return null;
  const accounts: Record<string, AccountBadge> = {};
  if (isObject(raw.accounts)) {
    for (const [key, a] of Object.entries(raw.accounts)) {
      const tag = parseTag(a, ACCOUNT_ICONS);
      if (tag) accounts[key] = tag;
    }
  }
  return { machine_id: id as string | null, machine, accounts };
}

/** Nome mostrado da máquina: etiqueta do servidor, senão o nome local, senão o host. */
export function machineLabel(server: Pick<Server, 'label' | 'baseUrl'>, tags?: { machine: Tag | null } | null): string {
  const name = tags?.machine?.name.trim();
  if (name) return name;
  if (server.label.trim()) return server.label;
  // `baseUrl` e não `baseOf`: o nome é da identidade do servidor, não da rota da vez.
  try { return new URL(server.baseUrl).host; } catch { return server.baseUrl; }
}

/** Rascunho do editor de etiqueta; `account` = id de `/api/credenciais`, ausente = a máquina. */
export interface TagDraft { name: string; icon: MachineIcon | AccountIcon; color: TagColor | null }

export function tagDraftOf(doc: TagsDoc, account?: string | null): TagDraft {
  if (!account) return { name: doc.machine?.name ?? '', icon: doc.machine?.icon ?? 'monitor', color: doc.machine?.color ?? null };
  const tag = doc.accounts[account];
  return { name: '', icon: tag?.icon ?? 'credit-card', color: tag?.color ?? null };
}

export function tagChangeOf(draft: TagDraft, account?: string | null): TagsChange {
  return account
    ? { account: { id: account, tag: { icon: draft.icon as AccountIcon, color: draft.color } } }
    : { machine: { name: draft.name.trim(), icon: draft.icon as MachineIcon, color: draft.color } };
}

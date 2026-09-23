// Fileira de atalhos configurável do painel de sessão. A lista mora no servidor
// (runtime_config, campo `shortcuts`: JSON numa string; vazio = conjunto nativo) e as
// superfícies (fileira desktop, "⋯" da NavBar, command palette) consomem o resolve daqui.

/** Ações internas: os botões nativos de hoje. Rótulo/ícone vêm do front (i18n), não da config. */
export type ShortcutInternalAction = 'terminal' | 'modo' | 'navegador' | 'anexos' | 'rodar';

export interface ShortcutInternal {
  id: string;
  type: 'internal';
  action: ShortcutInternalAction;
  confirm?: boolean;
}

export interface ShortcutSendText {
  id: string;
  type: 'send_text';
  label: string;
  icon?: string;          // "emoji:🚀" ou "glifo:bolt" (chave do mapa do ShortcutIcon)
  text: string;           // texto completo a enviar (ex.: "/relatorio-pm" ou um prompt)
  send_direct?: boolean;  // ausente = true; false pré-preenche o composer
  confirm?: boolean;
}

export interface ShortcutShell {
  id: string;
  type: 'shell';
  label: string;
  icon?: string;
  command: string;        // roda na máquina do servidor, cwd da sessão, dispara-e-esquece
  confirm?: boolean;
}

export type Shortcut = ShortcutInternal | ShortcutSendText | ShortcutShell;

const INTERNAL_ACTIONS: ShortcutInternalAction[] = [
  'terminal', 'modo', 'navegador', 'anexos', 'rodar',
];

/** A fileira de hoje, na ordem de hoje. É o que vale com config vazia. */
export function defaultShortcuts(): Shortcut[] {
  return INTERNAL_ACTIONS.map((action) => ({ id: action, type: 'internal', action }));
}

function isValid(item: unknown): item is Shortcut {
  if (typeof item !== 'object' || item === null) return false;
  const o = item as Record<string, unknown>;
  if (typeof o.id !== 'string' || !o.id.trim()) return false;
  // Opcionais com tipo errado derrubariam quem lê (o ícone chama `.startsWith`).
  if (o.icon !== undefined && typeof o.icon !== 'string') return false;
  if (o.confirm !== undefined && typeof o.confirm !== 'boolean') return false;
  if (o.send_direct !== undefined && typeof o.send_direct !== 'boolean') return false;
  if (o.type === 'internal') {
    return INTERNAL_ACTIONS.includes(o.action as ShortcutInternalAction);
  }
  if (o.type === 'send_text') {
    return typeof o.label === 'string' && !!o.label.trim()
      && typeof o.text === 'string' && !!o.text.trim();
  }
  if (o.type === 'shell') {
    return typeof o.label === 'string' && !!o.label.trim()
      && typeof o.command === 'string' && !!o.command.trim();
  }
  return false;
}

/** Resolve a string da config numa lista renderizável. NUNCA lança: config vazia, JSON quebrado
 * ou shape estranho caem no conjunto nativo — o backend valida na gravação, então lixo aqui é
 * config de versão antiga, e a fileira não pode sumir por causa dela. Item individual inválido
 * é descartado (não derruba a lista inteira). */
export function resolveShortcuts(raw: string | null | undefined): Shortcut[] {
  if (!raw || !raw.trim()) return defaultShortcuts();
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return defaultShortcuts();
  }
  if (!Array.isArray(data)) return defaultShortcuts();
  // id repetido quebra o {#each} com chave: fica o primeiro.
  const seen = new Set<string>();
  return data.filter(isValid).filter((s) => !seen.has(s.id) && !!seen.add(s.id));
}

export function serializeShortcuts(list: Shortcut[]): string {
  return JSON.stringify(list);
}

/** true quando o atalho envia direto (padrão do send_text; flag desligada = pré-preencher). */
export function sendsDirect(s: ShortcutSendText): boolean {
  return s.send_direct !== false;
}

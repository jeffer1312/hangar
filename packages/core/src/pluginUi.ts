import { mensagemDeErro } from './errosApi';

/** Árvore de elementos que os mods do Claude Code desenham (`ui.render`), como o engine a entrega
 *  a uma superfície remota. O Hangar não conhece mod nenhum: só traduz estes elementos. */
export interface PluginElement {
  type: string;
  props?: Record<string, unknown>;
  children?: PluginNode[];
  /** Estilos que valem com o ponteiro sobre o escopo (Box com `key`); o engine manda fora de `props`. */
  hover?: Record<string, unknown>;
  /** Endereço do clique no engine; o app manda ao servidor o `plugin` daqui e a `key` (`PluginControl`). */
  press?: { plugin?: string; handle?: number };
}

export type PluginNode = PluginElement | string | number | boolean | null | undefined;

export interface RasterCell {
  ch: string;
  /** `#rrggbb`, ou null para a cor padrão da superfície. */
  fg: string | null;
  bg: string | null;
}

/** Faixa sem nada para mostrar: ninguém desenhou, ou só o marcador do próprio engine. */
export function isEmptyBand(tree: PluginNode): boolean {
  return !tree || typeof tree !== 'object' || tree.type === 'engine';
}

const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';

// Sem `atob`: o core também roda no app Expo, onde ele não é garantido.
function base64Bytes(text: string): Uint8Array {
  const clean = text.replace(/[^A-Za-z0-9+/]/g, '');
  const out = new Uint8Array(Math.floor((clean.length * 3) / 4));
  let bits = 0;
  let acc = 0;
  let n = 0;
  for (const c of clean) {
    acc = (acc << 6) | B64.indexOf(c);
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out[n++] = (acc >> bits) & 0xff;
    }
  }
  return out.subarray(0, n);
}

// Bit 24 sozinho é a cor padrão do terminal; o resto é 0x00RRGGBB.
function cellColor(v: number): string | null {
  if (v & 0x01000000) return null;
  return `#${(v & 0xffffff).toString(16).padStart(6, '0')}`;
}

/** Células do `Raster`: base64 de triplas u32 little-endian `[código, frente, fundo]`, por linha. */
export function decodeRaster(cells: string, columns: number, rows: number): RasterCell[][] {
  const bytes = base64Bytes(cells);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const grid: RasterCell[][] = [];
  // `rows` vem do mod: sem o teto pelos bytes que chegaram, um número enorme trava a aba.
  const cells12 = Math.floor(bytes.length / 12);
  const lastRow = columns >= 1 ? Math.min(rows, Math.ceil(cells12 / Math.floor(columns))) : 0;
  for (let r = 0; r < lastRow; r++) {
    const row: RasterCell[] = [];
    for (let c = 0; c < columns; c++) {
      const at = (r * columns + c) * 12;
      if (at + 12 > bytes.length) break;
      const code = view.getUint32(at, true);
      row.push({
        ch: code >= 32 ? String.fromCharCode(code) : ' ',
        fg: cellColor(view.getUint32(at + 4, true)),
        bg: cellColor(view.getUint32(at + 8, true)),
      });
    }
    grid.push(row);
  }
  return grid;
}

// Nomes de cor do Ink (o terminal do Claude Code) que não existem em CSS ou lá têm outro tom.
const INK_COLORS: Record<string, string> = {
  black: '#000000',
  red: '#cd3131',
  green: '#0dbc79',
  yellow: '#e5e510',
  blue: '#2472c8',
  magenta: '#bc3fbc',
  cyan: '#11a8cd',
  white: '#e5e5e5',
  gray: '#808080',
  grey: '#808080',
  blackBright: '#666666',
  redBright: '#f14c4c',
  greenBright: '#23d18b',
  yellowBright: '#f5f543',
  blueBright: '#3b8eea',
  magentaBright: '#d670d6',
  cyanBright: '#29b8db',
  whiteBright: '#ffffff',
};

// A cor vem do mod e vai para um `style`: só formatos de cor, nunca texto que feche a declaração.
const SAFE_COLOR = /^(#[0-9a-f]{3,8}|rgba?\([\d\s.,%]+\)|[a-z]+)$/i;

/** Cor de um `Text`/`Box` em CSS: hex e `rgb()` passam; nome do Ink vira o tom do terminal. */
export function inkColor(value: unknown): string | null {
  if (typeof value !== 'string' || !value) return null;
  return INK_COLORS[value] ?? (SAFE_COLOR.test(value) ? value : null);
}

/** Texto direto dos filhos (strings e números), na ordem. */
export function textOf(children: PluginNode[] | undefined): string {
  return (children ?? []).map((c) => (typeof c === 'string' || typeof c === 'number' ? String(c) : '')).join('');
}

/** Site da faixa acima do prompt, como o engine chama (`requestId` do `AbovePrompt`). */
export const BAND_SITE = 'above-prompt';

/** Painel que um mod abriu e o terminal desenhou; `placement` é onde o terminal o pôs. */
export interface PluginPane {
  id: string;
  title: string;
  placement: 'dock' | 'inline';
  columns: number | null;
  tree: PluginNode;
}

/** De onde vem a interface dos mods: superfície remota (sessão sem terminal) ou o plugin no terminal. */
export type PluginSource = 'surface' | 'terminal';

export interface PluginSurfaces {
  above: PluginNode;
  panes: PluginPane[];
  /** Painel na frente segundo o servidor; `null` sem painel; `undefined` quando o servidor não manda (antigo). */
  shownId: string | null | undefined;
  /** Largura, em colunas, para a qual a faixa foi desenhada; `null` quando o servidor não manda. */
  columns: number | null;
  /** `null` quando o servidor não manda: vale como terminal, sem digitação pelo app. */
  source: PluginSource | null;
}

/** O dado do SSE `plugin_ui`, tolerante: o que não for painel com id fica de fora, e campo novo ausente ou
 *  estranho vale como "o servidor não mandou". */
export function parsePluginUi(data: unknown): PluginSurfaces {
  const d = (data && typeof data === 'object' ? data : {}) as {
    above?: PluginNode; panes?: unknown; shown_id?: unknown; columns?: unknown; source?: unknown;
  };
  const panes = Array.isArray(d.panes) ? d.panes : [];
  return {
    above: d.above ?? null,
    panes: panes.flatMap((p): PluginPane[] => {
      const o = (p && typeof p === 'object' ? p : {}) as Record<string, unknown>;
      if (typeof o.id !== 'string' || !o.id) return [];
      return [{
        id: o.id,
        title: typeof o.title === 'string' && o.title ? o.title : o.id,
        placement: o.placement === 'dock' ? 'dock' : 'inline',
        columns: typeof o.columns === 'number' ? o.columns : null,
        tree: (o.tree ?? null) as PluginNode,
      }];
    }),
    shownId: typeof d.shown_id === 'string' && d.shown_id ? d.shown_id : d.shown_id === null ? null : undefined,
    columns: typeof d.columns === 'number' && Number.isFinite(d.columns) && d.columns > 0 ? d.columns : null,
    source: d.source === 'surface' || d.source === 'terminal' ? d.source : null,
  };
}

/** A vista inteira a partir da anterior (`prev`, o dado cru do último `plugin_ui`) e do `plugin_ui_delta`: a faixa
 *  ausente fica a de antes, e o painel `{id, same: true}` volta ao de mesmo id na anterior. O servidor só manda a
 *  diferença a quem já tem a vista de que ela parte. */
export function applyPluginUiDelta(prev: unknown, delta: unknown): Record<string, unknown> {
  const p = (prev && typeof prev === 'object' ? prev : {}) as Record<string, unknown>;
  const d = (delta && typeof delta === 'object' ? delta : {}) as Record<string, unknown>;
  const before = new Map<string, unknown>();
  for (const pane of Array.isArray(p.panes) ? p.panes : []) {
    const id = (pane as { id?: unknown } | null)?.id;
    if (typeof id === 'string') before.set(id, pane);
  }
  const panes = (Array.isArray(d.panes) ? d.panes : []).map((pane) => {
    const o = pane as { id?: unknown; same?: unknown } | null;
    return o?.same === true && typeof o.id === 'string' && before.has(o.id) ? before.get(o.id) : pane;
  });
  return { ...d, above: 'above' in d ? d.above : p.above ?? null, panes };
}

/** O servidor diz qual painel está na frente, e ele está na lista: a aba segue o servidor. */
export function tabFollowsServer(ids: readonly string[], shownId: string | null | undefined): boolean {
  return typeof shownId === 'string' && ids.includes(shownId);
}

/** O painel desenhado: o do servidor quando ele diz um da lista; senão a escolha local; senão o último aberto. */
export function activePaneId(ids: readonly string[], shownId: string | null | undefined, local: string | null): string | null {
  if (typeof shownId === 'string' && ids.includes(shownId)) return shownId;
  if (local && ids.includes(local)) return local;
  return ids.length ? ids[ids.length - 1] : null;
}

/** A escolha local depois de um evento novo. Painel que acabou de abrir vai para a frente, como no terminal;
 *  fechado o escolhido, fica o vizinho anterior (o seguinte, se não houver anterior); senão ela sobrevive. */
export function followLocalTab(prev: readonly string[], next: readonly string[], local: string | null): string | null {
  if (!next.length) return null;
  const opened = next.filter((id) => !prev.includes(id));
  if (opened.length) return opened[opened.length - 1];
  if (local && next.includes(local)) return local;
  const at = local ? prev.indexOf(local) : -1;
  if (at < 0) return next[next.length - 1];
  for (let i = at - 1; i >= 0; i--) if (next.includes(prev[i])) return prev[i];
  return next[0];
}

/** Recusa de um servidor anterior à rota (`plugin/show` só chega nas fases 2 e 3): o servidor atual responde
 *  405 a POST em rota desconhecida (o mount estático recusa o método) e um mais novo sem a rota, 404.
 *  A troca de aba fica no app. */
export function isMissingRoute(err: unknown): boolean {
  const status = err && typeof err === 'object' ? (err as { status?: unknown }).status : undefined;
  return status === 404 || status === 405;
}

/** Falha numa rota de mod sem resposta (rede, tempo esgotado) ou com 5xx: o texto cru do erro não diz nada a quem
 *  digitou, e o app mostra a frase dele. Com 4xx (o 409 da recusa) o servidor manda o motivo, que é o que se mostra. */
export function isPluginServerFailure(err: unknown): boolean {
  const status = err && typeof err === 'object' ? (err as { status?: unknown }).status : undefined;
  return typeof status !== 'number' || status >= 500;
}

/** Frase de uma falha numa rota de mod. Código que o app conhece (`ERROS`) vira a frase dele em qualquer status,
 *  inclusive o 503 do dono único (`erro_mod_guarda_indisponivel`). Sem código: sem resposta ou 5xx, a frase
 *  genérica de quem chama; com 4xx, o motivo que o servidor mandou. */
export function pluginFailureText(err: unknown, generic: () => string): string {
  const fields = err && typeof err === 'object' ? (err as { code?: unknown; envelope?: { params?: unknown } }) : {};
  if (typeof fields.code === 'string') {
    const params = fields.envelope?.params;
    const text = mensagemDeErro(fields.code, params && typeof params === 'object' ? (params as Record<string, unknown>) : {});
    if (text) return text;
  }
  if (isPluginServerFailure(err)) return generic();
  return err instanceof Error ? err.message : String(err);
}

/** Aviso (`$.ui.toast`) que um mod mostrou no terminal; `plugin` é o mod que o emitiu. */
export interface PluginToast {
  id: string;
  text: string;
  plugin: string;
  timeoutMs: number;
}

/** O dado do SSE `plugin_toast`; sem id, sem texto ou sem prazo não é aviso. */
export function parsePluginToast(data: unknown): PluginToast | null {
  const o = (data && typeof data === 'object' ? data : {}) as Record<string, unknown>;
  if (typeof o.id !== 'string' || !o.id || typeof o.text !== 'string' || !o.text.trim()) return null;
  if (typeof o.timeoutMs !== 'number' || !(o.timeoutMs > 0)) return null;
  return { id: o.id, text: o.text, plugin: typeof o.plugin === 'string' ? o.plugin : '', timeoutMs: o.timeoutMs };
}

/** Endereço que pode virar link ou ser aberto: só http(s). `javascript:` executaria no clique. */
export function safeHref(v: unknown): string | null {
  return typeof v === 'string' && /^https?:\/\//i.test(v) ? v : null;
}

/** O que um clique ou uma digitação manda ao backend para achar o controle: o mod que o desenhou e a `key`,
 *  que só é única dentro do mod. */
export interface PluginControl { plugin: string; key: string }

function controlOf(el: PluginElement, type: string): PluginControl | null {
  const key = el.props?.key;
  const plugin = el.press?.plugin;
  return el.type === type && typeof key === 'string' && key && typeof plugin === 'string' && plugin ? { plugin, key } : null;
}

/** O botão de mod que o clique aciona; sem `key` ou sem o mod, é só rótulo. */
export function buttonControl(el: PluginElement): PluginControl | null {
  return controlOf(el, 'Button');
}

/** Box com `key` é escopo de hover: o `hover` dele e o dos filhos valem com o ponteiro sobre ele. */
export function isHoverScope(el: PluginElement): boolean {
  const key = el.props?.key;
  return el.type === 'Box' && typeof key === 'string' && key !== '';
}

/** As props do nó com o `hover` aplicado quando o escopo dele está aceso. `hover` com `scope` (grupo entre
 *  lugares) fica para depois: o nó segue sem hover. */
export function hoverProps(el: PluginElement, lit: boolean): Record<string, unknown> {
  const base = el.props ?? {};
  const hover = el.hover;
  if (!lit || !hover || typeof hover !== 'object' || 'scope' in hover) return base;
  return { ...base, ...hover };
}

/** O campo (`Input`) de mod que a digitação alcança; sem `key` ou sem o mod, não há a quem mandar. */
export function inputControl(el: PluginElement): PluginControl | null {
  return controlOf(el, 'Input');
}

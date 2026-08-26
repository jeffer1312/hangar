// Arquivos CITADOS na conversa, pra visão "Citados" da aba Arquivos. Separado do
// `parseFilePaths` de `format.ts` de propósito: aquele alimenta o chat (só mídia/html/pdf viram
// preview) e não pode ganhar extensão de código sem mudar o que a bolha desenha.
import type { ChatEvent } from './types';

// Lista FECHADA: regex aberta ("qualquer extensão") casa `repo.git` em URL e some com `config.py`
// solto em prosa — os dois lados errados.
const _EXTS = 'svelte|tsx|ts|jsx|js|mjs|cjs|py|pas|dfm|cs|dart|md|json|yaml|yml|toml|scss|css|html|sql|sh|fish|ps1|env|lock|txt|csv|xml|ini|cfg|conf';
const _ESPECIAIS = 'Dockerfile|Makefile';
// Absoluto (/ ou ~/): não exige pasta. Lookbehind tira o "/" de dentro de URL e o "./" do relativo.
const _ABS_RE = new RegExp(`(?<![\\w.~:/*])(~?/[^\\s"'\`)\\]]*?(?:\\.(?:${_EXTS})|/(?:${_ESPECIAIS})))(?=$|[\\s)\\]"'\`,;:*])`, 'g');
// Relativo: exige `dir/nome.ext` (mesma regra do _REL_RE do format.ts) — `app.main` não casa.
const _REL_RE = new RegExp(`(?<![\\w/~.:*-])((?:[\\w.-]+/)+(?:[\\w.-]+\\.(?:${_EXTS})|(?:${_ESPECIAIS})))(?=$|[\\s)\\]"'\`,;:*])`, 'g');

export function parseCodePaths(texto: string): string[] {
  const out: string[] = [];
  const vistos = new Set<string>();
  for (const re of [_ABS_RE, _REL_RE]) {
    for (const m of texto.matchAll(re)) {
      const p = m[1];
      if (/^https?:/.test(p) || p.includes('://')) continue;
      if (/(^|\/)(\.git|node_modules)\//.test(p)) continue;
      if (!vistos.has(p)) { vistos.add(p); out.push(p); }
    }
  }
  return out;
}

export type Origem = 'Read' | 'Edit' | 'Write' | 'MultiEdit' | 'NotebookEdit' | 'Bash' | 'tool' | 'voce' | 'citado';
const _TOOLS = new Set(['Read', 'Edit', 'Write', 'MultiEdit', 'NotebookEdit', 'Bash']);

export interface Citado {
  cru: string;                 // a string como apareceu — é o que `path_in_transcript` casa no /file
  relativo: string | null;     // dentro do cwd: caminho relativo (abre na árvore); fora: null
  nome: string;
  pasta: string;               // relativa ao cwd, ou `~/…` abreviada
  origens: Partial<Record<Origem, number>>;
  ultimoTs: number;
  primeiroTs: number;
}

export interface EstadoCitados {
  desde: number;
  porCru: Map<string, Citado>;
  lista: Citado[];             // ordenada por ultimoTs desc
}

export const estadoVazio = (): EstadoCitados => ({ desde: 0, porCru: new Map(), lista: [] });

function* strings(v: unknown): Generator<string> {
  if (typeof v === 'string') yield v;
  else if (Array.isArray(v)) for (const x of v) yield* strings(x);
  else if (v && typeof v === 'object') for (const x of Object.values(v as Record<string, unknown>)) yield* strings(x);
}

const _HOME = '/home/';
function expandir(p: string): string {
  return p.startsWith('~/') ? `${_HOME}~${p.slice(1)}` : p; // ponytail: só pra comparar com o cwd; o cru fica intacto
}
function abreviar(p: string): string {
  const i = p.indexOf('/', _HOME.length);
  return p.startsWith(_HOME) && i > 0 ? `~${p.slice(i)}` : p;
}

function classificar(cru: string, cwd: string): Pick<Citado, 'relativo' | 'nome' | 'pasta'> {
  const base = cwd.replace(/\/+$/, '');
  const abs = expandir(cru);
  let relativo: string | null = null;
  if (!abs.startsWith('/')) relativo = abs.replace(/^\.\//, '');
  else if (abs === base || abs.startsWith(base + '/')) relativo = abs.slice(base.length + 1);
  // `../x` sai do cwd: fora
  if (relativo !== null && relativo.split('/').includes('..')) relativo = null;
  const partes = (relativo ?? abreviar(abs)).split('/');
  const nome = partes.pop() ?? cru;
  return { relativo, nome, pasta: partes.join('/') || (relativo !== null ? '.' : '') };
}

function origemDe(ev: ChatEvent): Origem {
  if (ev.kind === 'user_msg') return 'voce';
  if (ev.kind === 'assistant_msg') return 'citado';
  const t = ev.tool_name ?? '';
  return (_TOOLS.has(t) ? t : 'tool') as Origem;
}

function* caminhosDe(ev: ChatEvent): Generator<string> {
  if (ev.kind === 'tool_use') {
    for (const s of strings(ev.tool_input)) yield* parseCodePaths(s);
  } else if ((ev.kind === 'user_msg' || ev.kind === 'assistant_msg') && ev.text) {
    yield* parseCodePaths(ev.text);
  }
}

// Incremental: processa só `eventos[desde..]` e devolve o estado NOVO (o antigo não é mutado).
// Re-varrer 5k eventos a cada tick do SSE é o erro que o deriveActivity do Chat já documenta.
export function acumularCitados(estado: EstadoCitados, eventos: ChatEvent[], desde: number, cwd: string): EstadoCitados {
  const porCru = new Map(estado.porCru);
  for (let i = desde; i < eventos.length; i++) {
    const ev = eventos[i];
    const ts = ev.ts ?? 0;
    const origem = origemDe(ev);
    for (const cru of caminhosDe(ev)) {
      const atual = porCru.get(cru);
      if (atual) {
        const c = { ...atual, origens: { ...atual.origens } };
        c.origens[origem] = (c.origens[origem] ?? 0) + 1;
        c.ultimoTs = Math.max(c.ultimoTs, ts);
        c.primeiroTs = Math.min(c.primeiroTs, ts);
        porCru.set(cru, c);
      } else {
        porCru.set(cru, { cru, ...classificar(cru, cwd), origens: { [origem]: 1 }, ultimoTs: ts, primeiroTs: ts });
      }
    }
  }
  const lista = [...porCru.values()].sort((a, b) => b.ultimoTs - a.ultimoTs);
  return { desde: eventos.length, porCru, lista };
}

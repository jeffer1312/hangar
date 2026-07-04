// Syntax highlighting viewer-only pro diff viewer, via Shiki fine-grained (engine JS, SEM WASM,
// SEM CDN — tudo bundlado). Temas reais do VS Code (dark-plus/light-plus) casam com o data-theme.
// Gotcha central: grammars TextMate sao STATEFUL entre linhas -> tokeniza o BLOB do codigo inteiro
// (linhas sem os prefixos +/- do diff) de uma vez e recasa linha-a-linha; nunca linha isolada.
import { createHighlighterCore, type HighlighterCore } from 'shiki/core';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
import darkPlus from '@shikijs/themes/dark-plus';
import lightPlus from '@shikijs/themes/light-plus';

// Linguagens do proprio repo. Extensao nova = 1 import a mais aqui.
const LANG_LOADERS: Record<string, () => Promise<unknown>> = {
  ts: () => import('@shikijs/langs/typescript'),
  tsx: () => import('@shikijs/langs/tsx'),
  js: () => import('@shikijs/langs/javascript'),
  jsx: () => import('@shikijs/langs/jsx'),
  svelte: () => import('@shikijs/langs/svelte'),
  py: () => import('@shikijs/langs/python'),
  sh: () => import('@shikijs/langs/bash'),
  json: () => import('@shikijs/langs/json'),
  yaml: () => import('@shikijs/langs/yaml'),
  yml: () => import('@shikijs/langs/yaml'),
  md: () => import('@shikijs/langs/markdown'),
  css: () => import('@shikijs/langs/css'),
  html: () => import('@shikijs/langs/html'),
};

function langFromPath(path: string): string {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  return ext in LANG_LOADERS ? ext : 'txt';
}

// Singleton do core (criacao async carrega engine/temas UMA vez; tokenizar depois e sync).
let corePromise: Promise<HighlighterCore> | null = null;
const loadedLangs = new Set<string>();

function getCore(): Promise<HighlighterCore> {
  if (!corePromise) {
    corePromise = createHighlighterCore({
      themes: [darkPlus, lightPlus],
      langs: [],
      engine: createJavaScriptRegexEngine(),   // sem WASM -> leve, ok pra mobile/LAN
    });
  }
  return corePromise;
}

async function ensureLang(core: HighlighterCore, lang: string): Promise<boolean> {
  if (lang === 'txt') return false;
  if (loadedLangs.has(lang)) return true;
  const loader = LANG_LOADERS[lang];
  if (!loader) return false;
  try {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await core.loadLanguage((await loader()) as any);
    loadedLangs.add(lang);
    return true;
  } catch {
    return false;   // grammar falhou -> cai no plain text
  }
}

export type DiffKind = 'add' | 'del' | 'ctx' | 'meta' | 'hunk';
export interface DiffToken { content: string; color?: string }
export interface DiffRow { kind: DiffKind; prefix: string; tokens: DiffToken[] }

function classify(line: string): DiffKind {
  if (line.startsWith('@@')) return 'hunk';
  if (line.startsWith('diff ') || line.startsWith('index ') || line.startsWith('+++') || line.startsWith('---')) return 'meta';
  if (line.startsWith('+')) return 'add';
  if (line.startsWith('-')) return 'del';
  return 'ctx';
}

// Monta as linhas SEM highlight ainda (prefixo +/-/espaco separado do codigo). meta/hunk ficam com o
// texto cru inteiro num token so; add/del/ctx separam o prefixo do codigo (o codigo e o que highlighta).
function baseRows(lines: string[]): DiffRow[] {
  return lines.map((l) => {
    const kind = classify(l);
    if (kind === 'meta' || kind === 'hunk') return { kind, prefix: '', tokens: [{ content: l }] };
    // ctx sempre carrega um prefixo ' ' (garante altura mesmo em linha de codigo vazia).
    const prefix = kind === 'add' ? '+' : kind === 'del' ? '-' : ' ';
    const code = kind === 'ctx' ? (l.startsWith(' ') ? l.slice(1) : l) : l.slice(1);
    return { kind, prefix, tokens: [{ content: code }] };
  });
}

/**
 * Highlighta um unified diff. Retorna uma linha por row com o prefixo do diff separado + os tokens
 * do codigo (com cor do tema VS Code). Qualquer falha (lang desconhecida, erro do Shiki) cai no
 * fallback = texto puro por linha, mantendo prefixo + as classes add/del/hunk/meta pro CSS de fundo.
 */
// Acima disto, tokenizar o blob inteiro no thread principal (engine JS, sem WASM) travaria a UI no
// celular. Diff grande -> pula o highlight e mostra plain (com prefixo/fundo add/del intactos).
const MAX_HL_LINES = 2000;

export async function highlightDiff(diffText: string, path: string): Promise<DiffRow[]> {
  const rows = baseRows(diffText.split('\n'));

  const lang = langFromPath(path);
  if (lang === 'txt' || rows.length > MAX_HL_LINES) return rows;

  let core: HighlighterCore;
  try {
    core = await getCore();
  } catch {
    return rows;
  }
  if (!(await ensureLang(core, lang))) return rows;

  // So as linhas de codigo, na ordem, como UM blob (estado da grammar preservado entre linhas).
  const codeIdx = rows.map((r, i) => (r.kind === 'add' || r.kind === 'del' || r.kind === 'ctx' ? i : -1)).filter((i) => i >= 0);
  const blob = codeIdx.map((i) => rows[i].tokens[0].content).join('\n');
  const theme = document.documentElement.dataset.theme === 'light' ? 'light-plus' : 'dark-plus';
  try {
    const { tokens } = core.codeToTokens(blob, { lang, theme });
    codeIdx.forEach((rowI, k) => {
      const toks = tokens[k];
      if (toks) rows[rowI].tokens = toks.map((t) => ({ content: t.content, color: t.color }));
    });
  } catch {
    return baseRows(diffText.split('\n'));   // erro na tokenizacao -> plain
  }
  return rows;
}

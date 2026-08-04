// Syntax highlighting viewer-only pro diff viewer, via Shiki fine-grained (engine JS, SEM WASM,
// SEM CDN — tudo bundlado). Temas reais do VS Code (dark-plus/light-plus) casam com o data-theme.
// Gotcha central: grammars TextMate sao STATEFUL entre linhas -> tokeniza o BLOB do codigo inteiro
// (linhas sem os prefixos +/- do diff) de uma vez e recasa linha-a-linha; nunca linha isolada.
import { createHighlighterCore, type HighlighterCore } from 'shiki/core';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
import darkPlus from '@shikijs/themes/dark-plus';
import lightPlus from '@shikijs/themes/light-plus';

// Linguagens do proprio repo + as das conversas (Jenkinsfile, SQL, Delphi, C#, Flutter, Docker).
// Extensao nova = 1 import a mais aqui. Carregam sob demanda: so pesam se aparecerem na tela.
const LANG_LOADERS: Record<string, () => Promise<unknown>> = {
  ts: () => import('@shikijs/langs/typescript'),
  tsx: () => import('@shikijs/langs/tsx'),
  js: () => import('@shikijs/langs/javascript'),
  jsx: () => import('@shikijs/langs/jsx'),
  svelte: () => import('@shikijs/langs/svelte'),
  py: () => import('@shikijs/langs/python'),
  sh: () => import('@shikijs/langs/bash'),
  bash: () => import('@shikijs/langs/bash'),
  json: () => import('@shikijs/langs/json'),
  yaml: () => import('@shikijs/langs/yaml'),
  yml: () => import('@shikijs/langs/yaml'),
  md: () => import('@shikijs/langs/markdown'),
  css: () => import('@shikijs/langs/css'),
  html: () => import('@shikijs/langs/html'),
  sql: () => import('@shikijs/langs/sql'),
  groovy: () => import('@shikijs/langs/groovy'),
  jenkinsfile: () => import('@shikijs/langs/groovy'),
  dockerfile: () => import('@shikijs/langs/dockerfile'),
  docker: () => import('@shikijs/langs/dockerfile'),
  pas: () => import('@shikijs/langs/pascal'),
  pascal: () => import('@shikijs/langs/pascal'),
  cs: () => import('@shikijs/langs/csharp'),
  csharp: () => import('@shikijs/langs/csharp'),
  dart: () => import('@shikijs/langs/dart'),
};

function langFromPath(path: string): string {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  return ext in LANG_LOADERS ? ext : 'txt';
}

// Singleton do core (criacao async carrega engine/temas UMA vez; tokenizar depois e sync).
let corePromise: Promise<HighlighterCore> | null = null;
const loadedLangs = new Set<string>();
// Cache NEGATIVO de grammar: chunk que falhou (LAN/offline) nao re-importa nem repete warn a cada
// re-render — sem isto, uma mensagem em streaming gerava dezenas de fetches/warns pela mesma lang.
const failedLangs = new Set<string>();

function getCore(): Promise<HighlighterCore> {
  if (!corePromise) {
    corePromise = createHighlighterCore({
      themes: [darkPlus, lightPlus],
      langs: [],
      engine: createJavaScriptRegexEngine(),   // sem WASM -> leve, ok pra mobile/LAN
    });
    // A promise REJEITADA nao pode ficar cacheada: com ela, todo retry cai na mesma falha pra
    // sempre (o "tenta de novo no proximo render" virava warn infinito sem chance de recuperar).
    corePromise.catch(() => { corePromise = null; });
  }
  return corePromise;
}

async function ensureLang(core: HighlighterCore, lang: string): Promise<boolean> {
  if (lang === 'txt') return false;
  if (loadedLangs.has(lang)) return true;
  if (failedLangs.has(lang)) return false;
  const loader = LANG_LOADERS[lang];
  if (!loader) return false;
  try {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    await core.loadLanguage((await loader()) as any);
    loadedLangs.add(lang);
    return true;
  } catch (err) {
    console.warn(`[hl] grammar '${lang}' indisponível`, err);
    failedLangs.add(lang);
    return false;   // grammar falhou -> cai no plain text (sem re-tentar em loop)
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

// ── Blocos de codigo da conversa (nao-diff) ─────────────────────────────────
// Os <pre><code class="language-X"> vem do renderMarkdown como TEXTO ESCAPADO, monocromatico.
// Esta passa coloriza in-place (spans com cor inline do tema VS Code) depois da montagem.
// Bloco sem lang conhecida / grammar que falhou / bloco gigante -> fica plain, sem erro.
const MAX_HL_BLOCK_LINES = 800;

function escapeAttr(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
}

/** Coloriza todos os `pre code[class^="language-"]` ainda nao tratados sob `root`.
 * Idempotente (marca data-hl). Segura pra chamar a cada re-render: so trabalha nos novos.
 * data-hl-theme: a cor vira inline style do tema da hora — trocou o tema, re-coloriza. */
export async function highlightCodeBlocks(root: HTMLElement): Promise<void> {
  const theme = document.documentElement.dataset.theme === 'light' ? 'light-plus' : 'dark-plus';
  const codes = [...root.querySelectorAll<HTMLElement>('pre code[class^="language-"]')]
    .filter((el) => el.dataset.hl !== '1' || el.dataset.hlTheme !== theme);
  if (!codes.length) return;
  // Marca ANTES do await: um segundo render enquanto a grammar carrega nao agenda trabalho dobrado.
  for (const el of codes) el.dataset.hl = '1';

  let core: HighlighterCore;
  try {
    core = await getCore();
  } catch (err) {
    // Falha de INFRA (engine nao subiu): desmarca pra re-tentar no proximo render — ficar plain
    // pra sempre em silencio era o bug.
    console.warn('[hl] core Shiki falhou', err);
    for (const el of codes) delete el.dataset.hl;
    return;
  }

  for (const el of codes) {
    const lang = (el.className.match(/language-([\w-]+)/)?.[1] ?? '').toLowerCase();
    const texto = el.textContent ?? '';
    // Bloco pulado (sem lang conhecida, vazio, gigante): marca o tema TAMBEM, senao o filtro o
    // reinclui em toda chamada e o scan (caro no bloco gigante) se repete a cada render.
    if (!lang || !(lang in LANG_LOADERS) || !texto.trim() || texto.split('\n').length > MAX_HL_BLOCK_LINES) {
      el.dataset.hlTheme = theme;
      continue;
    }
    if (!(await ensureLang(core, lang))) {
      // Grammar falhou: o cache negativo (failedLangs) impede re-import/warn em loop; o data-hl
      // fica desmarcado pra uma re-tentativa valer se o usuario recarregar a pagina.
      delete el.dataset.hl;
      continue;
    }
    // O element pode ter saido do DOM enquanto a grammar carregava (mensagem re-renderizada).
    if (!el.isConnected) continue;
    try {
      const { tokens } = core.codeToTokens(texto.replace(/\n$/, ''), { lang, theme });
      el.innerHTML = tokens
        .map((linha) => linha.map((t) => `<span style="color:${escapeAttr(t.color ?? 'inherit')}">${escapeAttr(t.content)}</span>`).join(''))
        .join('\n');
      el.dataset.hlTheme = theme;
    } catch {
      // tokenizacao falhou neste bloco -> fica plain (data-hl ja marca pra nao re-tentar em loop)
    }
  }
}

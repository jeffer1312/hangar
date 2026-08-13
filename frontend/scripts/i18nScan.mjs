// Extrator de string crua de interface. Mesmo criterio da medicao de 13/08/2026 (docs/superpowers/
// plans/2026-08-13-internacionalizacao-pt-en.md). Nao e um parser: e uma heuristica com ~89% de
// precisao, calibrada numa amostra de 45 achados. Falso positivo se resolve pondo a string no
// i18n-allow.json; falso negativo o `svelte-check` nao pega, e e por isso que a linha de base
// existe — ela mede o que sobra, nao o que e perfeito.
//
// Comentarios de BLOCO e comentarios de linha no inicio sao removidos ANTES da procura de
// literais: sem isso, aspas dentro de comentario (// mostra "Salvar alterações") contariam como
// string crua — 17% da baseline era ruido fantasma de prosa e o zero da Task 12 ficava
// inalcancavel. Comentario de LINHA no fim (codigo // tal) NAO e removido de proposito: cortar
// `//` ingenuamente come o resto de strings com https://… — falso negativo silencioso, pior que
// o ruido. Residuo aceito: comentario de linha com aspas continua contando (padrao raro).
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const ATTRS = ['title', 'placeholder', 'aria-label', 'alt', 'label', 'aria-description'];

const PT = /[áàâãéêíóôõúüç]|\b(?:de|da|do|das|dos|em|no|na|nos|nas|para|pra|por|com|sem|que|nao|nada|uma|um|os|as|ao|aos|ou|se|ja|so|mais|menos|todo|toda|todos|todas|esta|este|essa|esse|isso|aqui|agora|ainda|depois|antes|quando|onde|como|qual|seu|sua|voce|foi|ser|sao|tem|ver|abrir|fechar|salvar|criar|enviar|copiar|apagar|remover|buscar|carregar|nenhum|nenhuma|sessao|sessoes|arquivo|arquivos|pasta|mensagem|mensagens|configuracao|idioma|tema|fundo|clique|toque)\b/i;
const RUIDO = /[{}()=;]|=>|\bconst\b|\bfunction\b|\breturn\b|^\s*\/\//;
const IDENT = /^[a-z][A-Za-z0-9]*$|^[a-z0-9_]+$|^[a-z0-9-]+$/;
const TECLA = /^(?:Enter|Escape|Tab|Shift|Control|Alt|Backspace|Arrow(?:Up|Down|Left|Right)|Home|End|PageUp|PageDown|Delete)$/;
const PALAVRA = /[A-Za-zÀ-ÿ]{3,}/;

/**
 * @param {string} raizProjeto
 * @returns {Set<string>}
 */
export function carregarPermitidas(raizProjeto) {
  const caminho = join(raizProjeto, 'i18n-allow.json');
  try { return new Set(JSON.parse(readFileSync(caminho, 'utf8'))); }
  catch { return new Set(); }
}

/**
 * @param {string} s
 * @param {Set<string>} permitidas
 * @returns {string | null}
 */
function pareceTexto(s, permitidas) {
  const t = s.trim().replace(/\s+/g, ' ');
  if (t.length < 3 || permitidas.has(t)) return null;
  if (RUIDO.test(t) || IDENT.test(t) || TECLA.test(t)) return null;
  if (!PALAVRA.test(t)) return null;
  if (t.startsWith('$') || t.startsWith('#') || t.startsWith('http')) return null;
  if (PT.test(t) || /^[A-ZÀ-Ý][a-zà-ÿ]+\s/.test(t + ' ')) return t;
  return null;
}

/**
 * @param {string} caminho
 * @param {string} fonte
 * @param {Set<string>} [permitidas]
 * @returns {string[]}
 */
export function escanearArquivo(caminho, fonte, permitidas = new Set()) {
  const achados = [];
  const scripts = [...fonte.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g)].map((m) => m[1]);
  const markup = fonte
    .replace(/<script[^>]*>[\s\S]*?<\/script>/g, '')
    .replace(/<style[^>]*>[\s\S]*?<\/style>/g, '');

  for (const pedaco of markup.split(/<[^>]*>/)) {
    for (const linha of pedaco.replace(/\{[^{}]*\}/g, ' ').split('\n')) {
      const t = pareceTexto(linha, permitidas);
      if (t) achados.push(t);
    }
  }
  for (const attr of ATTRS) {
    const re = new RegExp(`\\b${attr}\\s*=\\s*"([^"{}]+)"`, 'g');
    for (const m of markup.matchAll(re)) {
      const t = pareceTexto(m[1], permitidas);
      if (t) achados.push(t);
    }
  }
  const corpos = caminho.endsWith('.svelte') ? scripts : [fonte];
  for (const corpo of corpos) {
    const semImport = corpo.replace(/^\s*import .*$/gm, '');
    // Comentario de bloco sai inteiro; comentario de linha so quando ocupa a linha sozinho (ver
    // cabecalho do arquivo: o `//` no fim de codigo nao e cortado, pra nao comer https://…).
    const semComentario = semImport
      .replace(/\/\*[\s\S]*?\*\//g, ' ')
      .replace(/^\s*\/\/.*$/gm, '');
    for (const m of semComentario.matchAll(/'([^'\\\n]{3,200})'|"([^"\\\n]{3,200})"|`([^`\\$\n]{3,200})`/g)) {
      const t = pareceTexto(m[1] ?? m[2] ?? m[3], permitidas);
      if (t) achados.push(t);
    }
  }
  return achados;
}

/**
 * @param {string} raizSrc
 * @param {Set<string>} [permitidas]
 * @returns {Record<string, string[]>}
 */
export function escanearArvore(raizSrc, permitidas = new Set()) {
  // JSDoc obrigatorio: o tsconfig.app.json liga allowJs+checkJs e o @tsconfig/svelte liga strict,
  // entao este .mjs entra no programa do `svelte-check` pelo import do teste. Sem o tipo, o
  // `fora[rel] = achados` e TS7053 ("nao pode indexar {} com string") e o gate fecha em vermelho.
  /** @type {Record<string, string[]>} */
  const fora = {};
  /** @param {string} dir */
  const anda = (dir) => {
    for (const nome of readdirSync(dir)) {
      const p = join(dir, nome);
      if (statSync(p).isDirectory()) {
        if (nome === 'paraglide' || nome === 'node_modules') continue; // gerado / dependencia
        anda(p);
        continue;
      }
      if (!/\.(svelte|ts)$/.test(nome) || /\.test\.(svelte\.)?ts$/.test(nome)) continue;
      const rel = relative(raizSrc, p);
      const achados = escanearArquivo(p, readFileSync(p, 'utf8'), permitidas);
      if (achados.length) fora[rel] = achados;
    }
  };
  anda(raizSrc);
  return fora;
}


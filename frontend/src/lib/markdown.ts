/**
 * Lightweight markdown → HTML renderer (no deps). Escape-first (todo texto é escapado antes de virar
 * HTML) -> seguro pra {@html}. Suporta: **bold**, *italic*, `inline code`, [links](http…), fenced
 * code blocks, headings, listas (marcadores - + ou numeradas), tabelas GFM (pipe + separador), links.
 */

import * as m from '../paraglide/messages';
import { parseCodeReferences, svgIcone } from '@hangar/core';

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

// Escapa o texto antes do HTML; fragmentos de citação são gerados separadamente.
function renderInline(input: string, opts: MarkdownOptions): string {
  const fragments: string[] = [];
  const keep = (html: string) => `\u0000${fragments.push(html) - 1}\u0000`;
  const chip = (path: string, line: number | null) => {
    const name = path.split('/').pop() ?? path;
    const label = path + (line ? `:${line}` : '');
    const icon = svgIcone(name, false);
    return keep(`<button type="button" class="file-citation" data-file-path="${escapeHtml(path)}"${line ? ` data-file-line="${line}"` : ''} title="${escapeHtml(m.arquivo_abrir({ nome: label }))}"><span class="file-citation-icon" aria-hidden="true">${icon}</span><span class="file-citation-name">${escapeHtml(name)}</span>${line ? `<span class="file-citation-line">:${line}</span>` : ''}</button>`);
  };
  let source = input.replace(/\u0000/g, '\ufffd');
  if (opts.fileLinks) {
    const reference = (value: string, link = false) => {
      const path = value.startsWith('<') && value.endsWith('>') ? value.slice(1, -1) : value;
      if (/^\.[^./:]+(?::\d+(?::\d+)?)?$/.test(path) && path !== '.env') return null;
      if (/^[a-z][a-z\d+.-]*:/i.test(path) && !/:\d+(?::\d+)?$/.test(path)) return null;
      if (!link && /\s/.test(path) && !path.startsWith('/') && !path.startsWith('~/')) return null;
      const prefix = path.startsWith('/') || path.startsWith('~/') ? '' : '/';
      const candidate = prefix + path.replace(/ /g, '%20');
      const refs = parseCodeReferences(candidate);
      const ref = refs[0];
      // O destino de um link já declara um caminho, inclusive para executáveis sem extensão.
      if (link && path && !path.startsWith('#') && !path.startsWith('?') && !path.includes('://')) {
        const suffix = /:(\d+)(?::\d+)?$/.exec(path);
        const line = suffix ? Number(suffix[1]) : null;
        return { path: suffix ? path.slice(0, suffix.index) : path,
          line: line && Number.isSafeInteger(line) ? line : null };
      }
      return ref && ref.start === 0 && ref.end === candidate.length
        ? { path: path.slice(0, path.length - (ref.end - ref.path.length)), line: ref.line } : null;
    };
    // Protege links e código antes de procurar caminhos soltos, sem tocar em atributos HTML.
    source = source.replace(/`([^`]+)`|\[([^\]]+)\]\((<[^>]+>|[^)]+)\)|(https?:\/\/[^\s<]+)/g,
      (whole, code: string | undefined, label: string | undefined, target: string | undefined) => {
        const ref = reference(code ?? target ?? '', target !== undefined);
        if (ref) return chip(ref.path, ref.line);
        return keep(code !== undefined ? `<code>${escapeHtml(code)}</code>` : renderInline(whole, {}));
      });
    const refs = parseCodeReferences(source);
    for (const ref of refs.reverse()) {
      source = source.slice(0, ref.start) + chip(ref.path, ref.line) + source.slice(ref.end);
    }
  }
  let text = escapeHtml(source);
  // inline code primeiro (pra não interpretar ** dentro de código)
  text = text.replace(/`([^`]+)`/g, (_, code) => `<code>${code}</code>`);
  text = text.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
  text = text.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, '<em>$1</em>');
  // links [texto](url) — só http(s) (evita javascript:). escapeHtml não toca em "/" -> url intacta;
  // " já viraram &quot; -> seguro no atributo.
  text = text.replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g,
    (_, label, url) => `<a href="${url}" target="_blank" rel="noopener noreferrer">${label}</a>`);
  // URL "pelada" (sem sintaxe [..](..)) -> vira link clicavel, pra nao ter que copiar e abrir no
  // navegador. Roda DEPOIS dos links markdown: o lookbehind (?<![">=\]]) pula URL colada em
  // href="..." ou logo apos > (texto de ancora) -> nao re-linka o que ja virou <a>. Pontuacao final
  // (.,;:!?) e ) ]  ficam FORA do link (senao "(https://x)." engoliria parentese/ponto). &amp; de
  // querystring fica no href (o browser decodifica) -> link valido.
  text = text.replace(/(?<![">=\]])(https?:\/\/[^\s<]+)/g, (_m, url: string) => {
    const trail = url.match(/[.,;:!?)\]]+$/);
    const u = trail ? url.slice(0, -trail[0].length) : url;
    const t = trail ? trail[0] : '';
    return `<a href="${u}" target="_blank" rel="noopener noreferrer">${u}</a>${t}`;
  });
  return text.replace(/\u0000(\d+)\u0000/g, (_, i) => fragments[Number(i)]);
}

const _SEP_RE = /^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)+\|?\s*$/;   // linha separadora |---|---|

function _cells(line: string): string[] {
  let s = line.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|')) s = s.slice(0, -1);
  return s.split('|').map((c) => c.trim());
}

// `joinWrapped`: markdown vindo de ARQUIVO (.md no disco) costuma vir com quebra dura de linha em
// ~90 colunas; aqui cada linha vira um <p>, o que e certo pro chat (onde a quebra e intencional) e
// errado pro arquivo — o paragrafo sai picado no meio da frase. Com a opcao ligada, linhas
// consecutivas de texto sao juntadas, como manda o CommonMark. Blocos (lista, heading, cerca,
// tabela, citacao, regra) nunca sao juntados.
export interface MarkdownOptions { joinWrapped?: boolean; fileLinks?: boolean }

function joinSoftWraps(input: string): string {
  const src = input.split('\n');
  const out: string[] = [];
  let fence = false;
  // Abre um bloco NOVO (não pode ser colado no anterior).
  const opensBlock = (t: string) =>
    t.trim() === '' ||
    /^ {0,3}(#{1,6}\s|[-*+]\s|\d+[.)]\s|>|\||---|\*\*\*|___|```)/.test(t);
  // Aceita continuação: parágrafo comum E item de lista (a "lazy continuation" do CommonMark —
  // sem isto, a 2ª linha de um item virava parágrafo solto e a lista quebrava, reiniciando a
  // numeração no item seguinte. Foi o que despencou o contrato do grupo na tela).
  const acceptsMore = (t: string) =>
    t.trim() !== '' && !/^ {0,3}(#{1,6}\s|>|\||---|\*\*\*|___|```)/.test(t);
  for (const line of src) {
    if (/^ {0,3}```/.test(line)) fence = !fence;
    const prev = out[out.length - 1];
    if (!fence && out.length && prev !== undefined && acceptsMore(prev) && !opensBlock(line)) {
      out[out.length - 1] = prev.replace(/\s+$/, '') + ' ' + line.trim();
    } else {
      out.push(line);
    }
  }
  return out.join('\n');
}

export function renderMarkdown(input: string, opts: MarkdownOptions = {}): string {
  const lines = (opts.joinWrapped ? joinSoftWraps(input) : input).split('\n');
  const out: string[] = [];
  let i = 0;
  // Linha em branco = quebra de PARAGRAFO: marca o proximo <p> com class="para" (respiro maior no
  // CSS). Antes virava <br> solto e o espacamento saia INVERTIDO (linha simples 12px via p+p,
  // paragrafo real 8px via br). Blocos (code/tabela/heading/lista/quote) resetam a marca — eles
  // tem margem propria.
  let parabreak = false;

  while (i < lines.length) {
    const line = lines[i];

    // ── Fenced code block ──────────────────────────────────────────────
    // Ate 3 espacos de indentacao contam como cerca (CommonMark). O Pi indenta a cerca quando ela
    // esta dentro de um item de lista; sem isto o bloco inteiro saia como texto cru com os ``` a
    // mostra. A indentacao da abertura e removida das linhas do codigo, senao o bloco vem torto.
    const fence = /^ {0,3}```/.exec(line);
    if (fence) {
      parabreak = false;
      const indent = fence[0].length - 3;
      const lang = line.slice(fence[0].length).trim();
      const code: string[] = [];
      i++;
      while (i < lines.length && !/^ {0,3}```/.test(lines[i])) {
        code.push(lines[i].slice(0, indent).trim() === '' ? lines[i].slice(indent) : lines[i]);
        i++;
      }
      i++; // pula o ``` de fechamento (se houver)
      const langAttr = lang ? ` class="language-${escapeHtml(lang)}"` : '';
      // Header estilo app do Claude: nome da linguagem (ou "Código") + copiar + expandir.
      // Handlers delegados GLOBAIS (lib/codeActions.svelte.ts, listener no document): valem aqui e
      // em qualquer tela que renderize markdown (PairSheet, ActivitySheet, plano...).
      const rotulo = lang ? escapeHtml(lang) : m.comum_codigo();
      out.push(`<div class="code-block"><div class="code-head"><span class="code-lang">${rotulo}</span><button class="copy-btn" type="button" aria-label="${m.comum_copiar_codigo()}"></button><button class="expand-btn" type="button" aria-label="${m.comum_expandir_codigo()}"></button></div><pre><code${langAttr}>${escapeHtml(code.join('\n'))}</code></pre></div>`);
      continue;
    }

    // ── Tabela GFM: linha com | seguida de separador |---|--- ──────────
    if (line.includes('|') && i + 1 < lines.length && _SEP_RE.test(lines[i + 1])) {
      parabreak = false;
      const head = _cells(line);
      i += 2; // pula header + separador
      const body: string[][] = [];
      while (i < lines.length && lines[i].includes('|') && lines[i].trim() !== '') {
        body.push(_cells(lines[i]));
        i++;
      }
      const th = head.map((c) => `<th>${renderInline(c, opts)}</th>`).join('');
      const rows = body.map((r) => `<tr>${r.map((c) => `<td>${renderInline(c, opts)}</td>`).join('')}</tr>`).join('');
      // Wrapper rolavel: a tabela mantem a largura natural e rola DENTRO da propria box (a pagina
      // continua sem scroll horizontal). Sem isto a tabela espremia e o texto quebrava letra a letra.
      // (Header+caixa estilo code-block foi testado aqui e REPROVADO pelo usuario 2026-08-03: sobre
      // papel de parede a caixa lia como recorte colado — a tabela fica sem caixa de proposito.)
      out.push(`<div class="md-table"><table><thead><tr>${th}</tr></thead><tbody>${rows}</tbody></table></div>`);
      continue;
    }

    // ── Heading ────────────────────────────────────────────────────────
    const h = line.match(/^(#{1,6})\s+(.+)$/);
    if (h) {
      parabreak = false;
      const n = h[1].length;
      out.push(`<h${n}>${renderInline(h[2], opts)}</h${n}>`);
      i++;
      continue;
    }

    // ── Listas (agrupa linhas consecutivas) ────────────────────────────
    const ulm = line.match(/^\s*[-*+]\s+(.+)$/);
    const olm = line.match(/^\s*\d+[.)]\s+(.+)$/);
    if (ulm || olm) {
      parabreak = false;
      const ordered = !!olm;
      const items: string[] = [];
      while (i < lines.length) {
        const match = ordered ? lines[i].match(/^\s*\d+[.)]\s+(.+)$/) : lines[i].match(/^\s*[-*+]\s+(.+)$/);
        if (!match) break;
        items.push(`<li>${renderInline(match[1], opts)}</li>`);
        i++;
      }
      const tag = ordered ? 'ol' : 'ul';
      out.push(`<${tag}>${items.join('')}</${tag}>`);
      continue;
    }

    // ── Blockquote ─────────────────────────────────────────────────────
    const bq = line.match(/^\s*>\s?(.*)$/);
    if (bq) {
      parabreak = false;
      out.push(`<blockquote>${renderInline(bq[1], opts)}</blockquote>`);
      i++;
      continue;
    }

    // ── Vazio / parágrafo ──────────────────────────────────────────────
    if (line.trim() === '') {
      parabreak = true;
    } else {
      out.push(`<p${parabreak ? ' class="para"' : ''}>${renderInline(line, opts)}</p>`);
      parabreak = false;
    }
    i++;
  }

  return out.join('');
}

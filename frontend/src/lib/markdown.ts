/**
 * Lightweight markdown → HTML renderer (no deps). Escape-first (todo texto é escapado antes de virar
 * HTML) -> seguro pra {@html}. Suporta: **bold**, *italic*, `inline code`, [links](http…), fenced
 * code blocks, headings, listas (marcadores - + ou numeradas), tabelas GFM (pipe + separador), links.
 */

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

// Inline: recebe texto JÁ ESCAPADO (escapeHtml não toca em * ` [ ] ( ) — sobrevivem pros regex).
function renderInline(escaped: string): string {
  let text = escaped;
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
  return text;
}

const _SEP_RE = /^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)+\|?\s*$/;   // linha separadora |---|---|

function _cells(line: string): string[] {
  let s = line.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|')) s = s.slice(0, -1);
  return s.split('|').map((c) => c.trim());
}

export function renderMarkdown(input: string): string {
  const lines = input.split('\n');
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
    if (line.startsWith('```')) {
      parabreak = false;
      const lang = line.slice(3).trim();
      const code: string[] = [];
      i++;
      while (i < lines.length && !lines[i].startsWith('```')) {
        code.push(lines[i]);
        i++;
      }
      i++; // pula o ``` de fechamento (se houver)
      const langAttr = lang ? ` class="language-${escapeHtml(lang)}"` : '';
      // Wrapper + botao copiar (handler delegado no AssistantBubble: le o textContent do <pre>).
      out.push(`<div class="code-block"><button class="copy-btn" type="button" aria-label="Copiar código"></button><pre><code${langAttr}>${escapeHtml(code.join('\n'))}</code></pre></div>`);
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
      const th = head.map((c) => `<th>${renderInline(escapeHtml(c))}</th>`).join('');
      const rows = body.map((r) => `<tr>${r.map((c) => `<td>${renderInline(escapeHtml(c))}</td>`).join('')}</tr>`).join('');
      // Wrapper rolavel: a tabela mantem a largura natural e rola DENTRO da propria box (a pagina
      // continua sem scroll horizontal). Sem isto a tabela espremia e o texto quebrava letra a letra.
      out.push(`<div class="md-table"><table><thead><tr>${th}</tr></thead><tbody>${rows}</tbody></table></div>`);
      continue;
    }

    // ── Heading ────────────────────────────────────────────────────────
    const h = line.match(/^(#{1,6})\s+(.+)$/);
    if (h) {
      parabreak = false;
      const n = h[1].length;
      out.push(`<h${n}>${renderInline(escapeHtml(h[2]))}</h${n}>`);
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
        const m = ordered ? lines[i].match(/^\s*\d+[.)]\s+(.+)$/) : lines[i].match(/^\s*[-*+]\s+(.+)$/);
        if (!m) break;
        items.push(`<li>${renderInline(escapeHtml(m[1]))}</li>`);
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
      out.push(`<blockquote>${renderInline(escapeHtml(bq[1]))}</blockquote>`);
      i++;
      continue;
    }

    // ── Vazio / parágrafo ──────────────────────────────────────────────
    if (line.trim() === '') {
      parabreak = true;
    } else {
      out.push(`<p${parabreak ? ' class="para"' : ''}>${renderInline(escapeHtml(line))}</p>`);
      parabreak = false;
    }
    i++;
  }

  return out.join('');
}

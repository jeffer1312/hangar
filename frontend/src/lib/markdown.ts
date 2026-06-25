/**
 * Lightweight markdown → HTML renderer (no deps).
 * Supports: **bold**, *italic*, `inline code`, fenced code blocks, line breaks.
 */

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function renderInline(text: string): string {
  // Bold
  text = text.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
  // Italic
  text = text.replace(/(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g, '<em>$1</em>');
  // Inline code
  text = text.replace(/`([^`]+)`/g, (_, code) => `<code>${escapeHtml(code)}</code>`);
  return text;
}

export function renderMarkdown(input: string): string {
  const lines = input.split('\n');
  const out: string[] = [];
  let inCode = false;
  let codeLang = '';
  let codeLines: string[] = [];

  for (const line of lines) {
    if (!inCode && line.startsWith('```')) {
      inCode = true;
      codeLang = line.slice(3).trim();
      codeLines = [];
      continue;
    }
    if (inCode) {
      if (line.startsWith('```')) {
        inCode = false;
        const langAttr = codeLang ? ` class="language-${escapeHtml(codeLang)}"` : '';
        out.push(`<pre><code${langAttr}>${escapeHtml(codeLines.join('\n'))}</code></pre>`);
        codeLines = [];
        codeLang = '';
      } else {
        codeLines.push(line);
      }
      continue;
    }

    if (line.trim() === '') {
      out.push('<br>');
    } else {
      out.push(`<p>${renderInline(escapeHtml(line))}</p>`);
    }
  }

  // Close unclosed code block
  if (inCode && codeLines.length) {
    out.push(`<pre><code>${escapeHtml(codeLines.join('\n'))}</code></pre>`);
  }

  return out.join('');
}

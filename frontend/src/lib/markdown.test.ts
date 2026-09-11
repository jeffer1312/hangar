import { describe, expect, it, beforeEach } from 'vitest';
import { renderMarkdown } from './markdown';
import { svgIcone } from '@hangar/core';
import { overwriteGetLocale } from '../paraglide/runtime';

beforeEach(() => overwriteGetLocale(() => 'pt'));

describe('citações de arquivo inline', () => {
  it('aceita arquivo sem extensão no destino explícito do link', () => {
    const html = renderMarkdown('[scripts/hangar-send](/home/jefferson/Projetos/hangar/scripts/hangar-send:454) sempre chama `/input`.', { fileLinks: true });
    expect(html).toContain('data-file-path="/home/jefferson/Projetos/hangar/scripts/hangar-send"');
    expect(html).toContain('data-file-line="454"');
    expect(html).toContain('<span class="file-citation-name">hangar-send</span>');
    expect(html).toContain('<code>/input</code>');
    expect(html).not.toContain('[scripts/hangar-send]');
    expect(renderMarkdown('[LICENSE](LICENSE)', { fileLinks: true })).toContain('data-file-path="LICENSE"');
  });
  it('mantém o ícone do script e o clique de arquivo, sem link externo', () => {
    const html = renderMarkdown('Abra `ecc-review-reminder.sh:8`.', { fileLinks: true });
    expect(html).toContain('data-file-path="ecc-review-reminder.sh"');
    expect(html).toContain('data-file-line="8"');
    expect(html).toContain(svgIcone('ecc-review-reminder.sh', false));
    expect(html).not.toContain('<a ');
    expect(html).not.toContain('target=');
    expect(renderMarkdown('Extensão `.sh`.', { fileLinks: true })).toBe('<p>Extensão <code>.sh</code>.</p>');
  });
  it('troca link local por chip no mesmo parágrafo, com ícone e linha', () => {
    const html = renderMarkdown('Veja [pqueue.py:324](/home/jefferson/Projetos/hangar/backend/app/pqueue.py:324) aqui.', { fileLinks: true });
    expect(html).toMatch(/^<p>Veja <button /);
    expect(html).toContain('data-file-path="/home/jefferson/Projetos/hangar/backend/app/pqueue.py"');
    expect(html).toContain('data-file-line="324"');
    expect(html).toContain('<svg');
    expect(html).toContain('<span class="file-citation-name">pqueue.py</span><span class="file-citation-line">:324</span></button> aqui.</p>');
    expect(html).not.toContain('[pqueue');
  });

  it('preserva cada ocorrência, caminhos entre crases e links com espaços', () => {
    const html = renderMarkdown('src/main.ts:2 e `src/main.ts:8` e [arquivo](</home/meu projeto/main.ts:9>)', { fileLinks: true });
    expect(html.match(/class="file-citation"/g)).toHaveLength(3);
    expect(html).toContain('data-file-line="2"');
    expect(html).toContain('data-file-line="8"');
    expect(html).toContain('data-file-path="/home/meu projeto/main.ts"');
    expect(renderMarkdown('Veja src/main.ts.', { fileLinks: true })).toContain('</button>.</p>');
  });

  it('não converte blocos de código, comandos entre crases nem URLs externas', () => {
    const html = renderMarkdown('```sh\ncat src/main.ts\n```\n`cat src/main.ts` e [site](https://example.com/src/main.ts)', { fileLinks: true });
    expect(html).not.toContain('file-citation');
    expect(html).toContain('<code>cat src/main.ts</code>');
    expect(html).toContain('href="https://example.com/src/main.ts"');
    expect(renderMarkdown('src/main.ts')).not.toContain('file-citation');
  });

  it('escapa caminhos e recusa protocolos executáveis', () => {
    const html = renderMarkdown('[arquivo](</tmp/a&<b>.py:4>) [x](javascript:alert(1)) <script>alert(1)</script>', { fileLinks: true });
    expect(html).not.toContain('<script>');
    expect(html).not.toContain('href="javascript:');
    expect(html).not.toContain('data-file-path="javascript:');
  });
});


describe('renderMarkdown — cercas de código', () => {
  it('renderiza cerca indentada dentro de item de lista', () => {
    // O Pi escreve assim quando o bloco está dentro de uma lista numerada. Antes disto o bloco
    // inteiro saía como texto cru, com os ``` à mostra na bolha do celular.
    const md = '3. Procure o sufixo:\n   ```text\n   meta-llama/llama-3.2-3b:free\n   ```\n';
    const html = renderMarkdown(md);
    expect(html).toContain('<pre><code class="language-text">');
    expect(html).toContain('meta-llama/llama-3.2-3b:free');
    expect(html).not.toContain('```');
  });

  it('remove a indentação da abertura das linhas do código', () => {
    const md = '1. exemplo:\n   ```json\n   {\n     "a": 1\n   }\n   ```\n';
    const html = renderMarkdown(md);
    expect(html).toContain('{\n  &quot;a&quot;: 1\n}');
  });

  it('mantém a cerca na coluna zero funcionando igual', () => {
    const md = '```js\nconst a = 1;\n```\n';
    const html = renderMarkdown(md);
    expect(html).toContain('<pre><code class="language-js">const a = 1;</code></pre>');
  });

  it('bloco de código sai com header: linguagem + copiar + expandir', () => {
    // Contrato do header (estilo app do Claude iOS): o handler global de code-actions depende
    // dessas classes pra copiar/expandir — se o markup mudar, os botoes morrem em toda tela.
    const html = renderMarkdown('```yaml\nrule: x\n```\n');
    expect(html).toContain('<div class="code-block"><div class="code-head">');
    expect(html).toContain('<span class="code-lang">yaml</span>');
    expect(html).toContain('<button class="copy-btn"');
    expect(html).toContain('<button class="expand-btn"');
  });

  it('cerca sem linguagem usa o rótulo genérico "Código"', () => {
    const html = renderMarkdown('```\nabc\n```\n');
    expect(html).toContain('<span class="code-lang">Código</span>');
    expect(html).toContain('<pre><code>abc</code></pre>');
  });
});

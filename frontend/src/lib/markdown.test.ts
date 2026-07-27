import { describe, expect, it } from 'vitest';
import { renderMarkdown } from './markdown';

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
});

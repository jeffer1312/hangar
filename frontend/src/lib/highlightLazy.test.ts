// @vitest-environment happy-dom
import { describe, expect, it, vi, beforeEach } from 'vitest';

// O modulo real puxa o Shiki inteiro; aqui so interessa SE a fachada chega nele.
const chamadas = { blocos: 0, linhas: 0, diff: 0 };
vi.mock('./highlight', () => ({
  highlightCodeBlocks: async () => { chamadas.blocos++; },
  highlightCodeLines: async () => { chamadas.linhas++; return null; },
  highlightDiff: async () => { chamadas.diff++; return []; },
}));

import { highlightCodeBlocks, highlightCodeLines } from './highlightLazy';

function raiz(html: string): HTMLElement {
  const el = document.createElement('div');
  el.innerHTML = html;
  return el;
}

describe('highlightLazy', () => {
  beforeEach(() => {
    chamadas.blocos = 0; chamadas.linhas = 0; chamadas.diff = 0;
  });

  it('nao chega no Shiki quando a mensagem nao tem bloco de codigo', async () => {
    // O AssistantBubble chama isto a cada versao de CADA mensagem. Sem o guarda, uma conversa
    // inteira sem um unico bloco de codigo carregava os 205KB do chunk de realce.
    await highlightCodeBlocks(raiz('<p>texto puro</p><code>inline</code><pre>sem language-</pre>'));
    expect(chamadas.blocos).toBe(0);
  });

  it('chega no Shiki quando ha bloco de codigo com linguagem', async () => {
    await highlightCodeBlocks(raiz('<pre><code class="language-ts">const a = 1;</code></pre>'));
    expect(chamadas.blocos).toBe(1);
  });

  it('as outras funcoes nao tem guarda — quem chama ja sabe que tem conteudo', async () => {
    await highlightCodeLines(['const a = 1;'], 'a.ts');
    expect(chamadas.linhas).toBe(1);
  });
});

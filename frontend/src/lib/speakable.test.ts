// @vitest-environment happy-dom
// Unico arquivo da suite com DOM de verdade (o resto e node puro, ver vitest.config.ts): textoFalavel
// manipula <pre> aninhado com cloneNode/querySelectorAll/replaceWith, e stub manual reimplementaria
// DOM em vez de testa-lo.
import { describe, it, expect } from 'vitest';
import { textoFalavel } from './speakable';
import { renderMarkdown } from './markdown';

function montar(html: string): HTMLElement {
  const d = document.createElement('div');
  d.innerHTML = html;
  return d;
}

describe('textoFalavel', () => {
  it('troca bloco de codigo pelo marcador', () => {
    const el = montar('<p>antes</p><pre><code>const x = 1;</code></pre><p>depois</p>');
    const t = textoFalavel(el);
    expect(t).toContain('antes');
    expect(t).toContain('trecho de código omitido');
    expect(t).toContain('depois');
    expect(t).not.toContain('const x = 1');
  });

  it('mantem codigo em linha, que e curto e faz parte da frase', () => {
    const el = montar('<p>use o <code>api.ts</code> aqui</p>');
    expect(textoFalavel(el)).toContain('api.ts');
  });

  it('nao duplica o marcador em blocos vizinhos', () => {
    const el = montar('<pre>a</pre><pre>b</pre>');
    const t = textoFalavel(el);
    expect(t.match(/trecho de código omitido/g)?.length).toBe(2);
  });

  it('nao altera o DOM original', () => {
    const el = montar('<pre><code>segredo</code></pre>');
    textoFalavel(el);
    expect(el.querySelector('pre')?.textContent).toBe('segredo');
  });

  it('titulo, paragrafo e lista renderizados pelo renderMarkdown de verdade saem falaveis, sem palavras coladas', () => {
    const html = renderMarkdown('## Passo 1\n\nFazer X no arquivo.\n\n- item um\n- item dois\n');
    const el = montar(html);
    // Prova de que o audio sai falavel: nao "Passo 1Fazer X no arquivo.item umitem dois".
    expect(textoFalavel(el)).toBe('Passo 1\nFazer X no arquivo.\nitem um\nitem dois');
  });
});

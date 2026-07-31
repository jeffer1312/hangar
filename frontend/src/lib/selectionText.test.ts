// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest';
import { falavelDaSelecao } from './selectionText';

function selecionarTudo(html: string): Selection | null {
  document.body.innerHTML = `<div id="alvo">${html}</div>`;
  const alvo = document.getElementById('alvo')!;
  const r = document.createRange();
  r.selectNodeContents(alvo);
  const s = window.getSelection();
  s?.removeAllRanges();
  s?.addRange(r);
  return s;
}

describe('falavelDaSelecao', () => {
  it('devolve vazio sem selecao', () => {
    expect(falavelDaSelecao(null)).toBe('');
  });

  it('devolve vazio quando a selecao esta colapsada', () => {
    document.body.innerHTML = '<p>texto</p>';
    const s = window.getSelection();
    s?.removeAllRanges();
    expect(falavelDaSelecao(s)).toBe('');
  });

  it('troca bloco de codigo pelo marcador, como o botao da bolha', () => {
    const s = selecionarTudo('<p>antes</p><pre>const x = 1;</pre><p>depois</p>');
    const t = falavelDaSelecao(s);
    expect(t).toContain('antes');
    expect(t).toContain('trecho de código omitido');
    expect(t).not.toContain('const x = 1');
  });

  it('selecao cruzando dois paragrafos vira uma string so, com quebra entre eles', () => {
    const s = selecionarTudo('<p>um</p><p>dois</p>');
    const t = falavelDaSelecao(s);
    // Igualdade exata, nao toContain: "umdois" tambem conteria 'um' e 'dois', e e exatamente o
    // bug (palavras coladas) que este teste existe pra pegar.
    expect(t).toBe('um\ndois');
  });
});

// @vitest-environment happy-dom
import { describe, it, expect, vi } from 'vitest';
import { rolagemSoAoClicar } from './rolagemSoAoClicar';

function montar(ponteiroFino: boolean) {
  vi.stubGlobal('matchMedia', (q: string) => ({ matches: ponteiroFino && q.includes('fine') }));
  const pai = document.createElement('div');
  const bloco = document.createElement('div');
  pai.appendChild(bloco);
  document.body.appendChild(pai);
  const colapsar = vi.fn();
  pai.addEventListener('click', colapsar);
  const acao = rolagemSoAoClicar(bloco);
  return { bloco, colapsar, acao };
}

describe('rolagemSoAoClicar', () => {
  it('clique libera a rolagem sem colapsar o card em volta (ponteiro fino)', () => {
    const { bloco, colapsar } = montar(true);
    expect(bloco.classList.contains('rolagem-travada')).toBe(true);
    bloco.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(bloco.classList.contains('rolagem-travada')).toBe(false);
    expect(colapsar).not.toHaveBeenCalled();
  });

  it('no toque o clique continua subindo (a rolagem ja e nativa)', () => {
    const { bloco, colapsar } = montar(false);
    bloco.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    expect(colapsar).toHaveBeenCalledTimes(1);
  });
});

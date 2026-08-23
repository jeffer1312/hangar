import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { criarPanePoller, type PaneSnapshot } from './usePanePoll';

// Conta quantas vezes o TEXTO mudou (o diff-gate existe pra isso não subir a cada tick).
function espiao() {
  const vistos: PaneSnapshot[] = [];
  let ultimo = '';
  let mudancas = 0;
  return {
    vistos,
    onSnapshot: (s: PaneSnapshot) => {
      vistos.push(s);
      if (s.text !== ultimo) { ultimo = s.text; mudancas++; }
    },
    get mudancas() { return mudancas; },
    get atual() { return vistos[vistos.length - 1]; },
  };
}

describe('criarPanePoller — porte de TerminalMirror.svelte:75-100', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('primeiro tick pinta', async () => {
    const getPane = vi.fn(async () => ({ text: 'ola', scrollback: 10 }));
    const e = espiao();
    const p = criarPanePoller(getPane, 'sess', 200, e.onSnapshot);
    p.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(e.atual.text).toBe('ola');
    expect(e.atual.scrollback).toBe(10);
    expect(e.atual.err).toBe('');
    p.stop();
  });

  it('texto igual não re-renderiza (contador de publicações)', async () => {
    let ret = 'mesmo';
    const getPane = vi.fn(async () => ({ text: ret, scrollback: 0 }));
    const e = espiao();
    const p = criarPanePoller(getPane, 'sess', 200, e.onSnapshot);
    p.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(e.mudancas).toBe(1);
    expect(e.vistos.length).toBe(1);
    await vi.advanceTimersByTimeAsync(450);
    expect(getPane).toHaveBeenCalledTimes(2);
    expect(e.mudancas).toBe(1);
    expect(e.vistos.length).toBe(1);   // tick sem mudança não publica nada
    ret = 'outro';
    await vi.advanceTimersByTimeAsync(450);
    expect(e.mudancas).toBe(2);
    p.stop();
  });

  it('stop cancela o próximo tick', async () => {
    const getPane = vi.fn(async () => ({ text: 'x', scrollback: 0 }));
    const e = espiao();
    const p = criarPanePoller(getPane, 'sess', 200, e.onSnapshot);
    p.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(getPane).toHaveBeenCalledTimes(1);
    p.stop();
    await vi.advanceTimersByTimeAsync(1000);
    expect(getPane).toHaveBeenCalledTimes(1);
  });

  it('resposta que chega DEPOIS do stop não publica (alive)', async () => {
    let resolver: ((v: { text: string; scrollback: number }) => void) | undefined;
    const getPane = vi.fn(() => new Promise<{ text: string; scrollback: number }>((r) => { resolver = r; }));
    const e = espiao();
    const p = criarPanePoller(getPane, 'sess', 200, e.onSnapshot);
    p.start();
    await vi.advanceTimersByTimeAsync(0);
    p.stop();
    resolver!({ text: 'chegou tarde', scrollback: 3 });
    await vi.advanceTimersByTimeAsync(0);
    expect(e.vistos.length).toBe(0);
  });

  it('erro vira err e o poll continua (finally)', async () => {
    let fail = true;
    const getPane = vi.fn(async () => {
      if (fail) throw new Error('boom');
      return { text: 'ok', scrollback: 0 };
    });
    const e = espiao();
    const p = criarPanePoller(getPane, 'sess', 200, e.onSnapshot);
    p.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(e.atual.err).toBe('boom');
    expect(e.atual.text).toBe('');
    fail = false;
    await vi.advanceTimersByTimeAsync(450);
    expect(e.atual.err).toBe('');
    expect(e.atual.text).toBe('ok');
    p.stop();
  });

  it('congelado: tick não troca o texto, refresh (ação do usuário) troca', async () => {
    let ret = 'um';
    const getPane = vi.fn(async () => ({ text: ret, scrollback: 0 }));
    const e = espiao();
    const p = criarPanePoller(getPane, 'sess', 200, e.onSnapshot);
    p.start();
    await vi.advanceTimersByTimeAsync(0);
    p.setAtBottom(false);
    ret = 'dois';
    await vi.advanceTimersByTimeAsync(450);
    expect(e.atual.text).toBe('um');       // poll congelado
    expect(e.atual.pending).toBe(true);
    await p.refresh();                      // ação deliberada passa por cima
    expect(e.atual.text).toBe('dois');
    expect(e.atual.pending).toBe(false);
    p.stop();
  });
});

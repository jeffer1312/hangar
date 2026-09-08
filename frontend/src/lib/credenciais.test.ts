// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { comTeto } from './credenciais';

const originalAny = AbortSignal.any;

afterEach(() => {
  AbortSignal.any = originalAny;
  vi.useRealTimers();
});

describe('comTeto', () => {
  it('sem AbortSignal.any (Safari < 17.4) ainda aborta pelo teto de tempo', async () => {
    // @ts-expect-error simula navegador antigo
    AbortSignal.any = undefined;
    const sinal = comTeto(new AbortController().signal, 30);
    expect(sinal.aborted).toBe(false);
    await new Promise((r) => setTimeout(r, 80));  // AbortSignal.timeout do happy-dom não obedece fake timers
    expect(sinal.aborted).toBe(true);
  });

  it('sem AbortSignal.any ainda aborta pelo sinal do chamador', () => {
    // @ts-expect-error simula navegador antigo
    AbortSignal.any = undefined;
    const controle = new AbortController();
    const sinal = comTeto(controle.signal, 8000);
    controle.abort('trocou de servidor');
    expect(sinal.aborted).toBe(true);
    expect(sinal.reason).toBe('trocou de servidor');
  });

  it('sem sinal do chamador devolve só o teto', () => {
    expect(comTeto(undefined, 8000)).toBeInstanceOf(AbortSignal);
  });
});

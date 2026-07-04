import { describe, it, expect } from 'vitest';
import { parseStatusLine } from './statusline';

describe('parseStatusLine — uso de contexto', () => {
  it('deriva ctxPct do 2º par (usado/janela) quando há métrica de contexto', () => {
    const s = parseStatusLine('💬 20k/1k 40k/200k');
    expect(s?.ctxUsed).toBe(40_000);
    expect(s?.ctxTotal).toBe(200_000);
    expect(s?.ctxPct).toBe(20);
  });

  it('NÃO deriva contexto quando só há o par in/out (sessão zerada pós /clear)', () => {
    // in>out não pode virar 100% falso: sem par de janela, ctxPct fica indefinido.
    const s = parseStatusLine('💬 20k/1k');
    expect(s?.ctxPct).toBeUndefined();
    expect(s?.ctxUsed).toBeUndefined();
  });
});

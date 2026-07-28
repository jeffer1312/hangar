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

describe('parseStatusLine — statusline do Pi', () => {
  // Linha real capturada da sessão `jefferson` (extensão rich-status-line.ts do Pi).
  const PI = '🤖 cline-pass/kimi-k3 (high) │ 📁 jefferson │ 📟 jefferson │ 💬 sessão 251kin/10kout · cache 2M · total 2.3M ctx 97k/1M │ ⚡5h:9% 📅7d:4% 🗓30d:2% │ 💵 $1.29 │ ⏱ 3h4m │ 🕐 22:40';

  it('lê o contexto do marcador "ctx x/y"', () => {
    // Antes: o par do turno vinha grudado em letras (251kin/10kout), não contava como par, e a
    // regra dos >=2 pares descartava o único par existente -> sessão Pi sem anel de contexto.
    const f = parseStatusLine(PI)!;
    expect(f.ctxUsed).toBe(97000);
    expect(f.ctxTotal).toBe(1000000);
    expect(Math.round(f.ctxPct!)).toBe(10);
  });

  it('lê modelo, esforço, custo e limites da linha do Pi', () => {
    const f = parseStatusLine(PI)!;
    expect(f.model).toBe('cline-pass/kimi-k3');
    expect(f.effort).toBe('high');
    expect(f.costUsd).toBe(1.29);
    expect(f.fiveHourPct).toBe(9);
    expect(f.weeklyPct).toBe(4);
    expect(f.sessionTime).toBe('3h4m');
  });

  it('não muda a linha do Claude, que não tem "ctx"', () => {
    const CLAUDE = '🤖 Opus5 (high✦) │ 📁 claude-cockpit [main*] │ 💬 474k/220 470k/1M │ 💵 $169.89';
    const f = parseStatusLine(CLAUDE)!;
    expect(f.ctxUsed).toBe(470000);
    expect(f.ctxTotal).toBe(1000000);
  });
});

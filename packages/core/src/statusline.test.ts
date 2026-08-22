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

  it('separa os tokens do TURNO (1º par) do par de contexto', () => {
    const s = parseStatusLine('💬 271k/590 270k/1M')!;
    expect(s.turnIn).toBe(271_000);
    expect(s.turnOut).toBe(590);
    expect(s.ctxUsed).toBe(270_000);
  });

  it('com um par só, ele é do turno e NÃO vira contexto nem turno duplicado', () => {
    // O par único já é lido como "sem contexto"; ele também não pode virar turno inventado a
    // partir de uma leitura de janela.
    const s = parseStatusLine('💬 20k/1k')!;
    expect(s.turnIn).toBeUndefined();
    expect(s.turnOut).toBeUndefined();
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
    expect(f.monthlyPct).toBe(2);
    expect(f.sessionTime).toBe('3h4m');
  });

  it('não muda a linha do Claude, que não tem "ctx"', () => {
    const CLAUDE = '🤖 Opus5 (high✦) │ 📁 hangar [main*] │ 💬 474k/220 470k/1M │ 💵 $169.89';
    const f = parseStatusLine(CLAUDE)!;
    expect(f.ctxUsed).toBe(470000);
    expect(f.ctxTotal).toBe(1000000);
  });
});

describe('setas de reset das janelas de uso', () => {
  // Linha REAL da sessão Pi/Kimi (2026-07-30): a seta é ↻, não o ↺ do statusline do Claude.
  // Só o ↺ era aceito -> a sessão mostrava "5h 51%" sem nenhum horário de volta, com a
  // porcentagem certa do lado: parecia limite sem reset em vez de campo perdido no parse.
  const KIMI = '🤖 k3-256k (high) │ 📁 pi │ 💬 sessão 28kin/4kout · cache 453k · total 484k ctx 28k/262k'
    + ' │ ⚡5h:51% ↻54m 📅7d:10% ↻6d19h │ 💵 $0.00 │ ⏱ 2h32m │ 🕐 19:04';

  it('lê o reset escrito com ↻ (Pi/Kimi)', () => {
    const f = parseStatusLine(KIMI)!;
    expect(f.fiveHourPct).toBe(51);
    expect(f.fiveHourReset).toBe('54m');
    expect(f.weeklyPct).toBe(10);
    expect(f.weeklyReset).toBe('6d19h');
  });

  it('continua lendo o reset escrito com ↺ (Claude)', () => {
    const f = parseStatusLine('🤖 Opus5 │ ⚡5h:46% ↺34m 📅7d:57% ↺sab 18h │ 💵 $1.00')!;
    expect(f.fiveHourReset).toBe('34m');
    expect(f.weeklyReset).toBe('sab 18h');
  });
});

describe('parseStatusLine — statusline do Kimi Code', () => {
  // Linha REAL publicada por ~/.kimi-code/statusline.js (2026-08-12, v0.34.0). O stdin do Kimi
  // nao traz in/out do turno nem custo (assinatura de valor fixo), entao o contexto vem como
  // par rotulado SOZINHO e o ⏱ vai grudado no relogio, como na linha do Claude.
  const KIMI_CLI = '🤖 K3 (high✦) │ 📁 hangar [main*] │ 💬 ctx 480k/1M │ ⚡5h:3% ↺50m │ 📅7d:33% ↺seg 14h·5d6h │ 🕐 08:09 ⏱ 15h13m';

  it('lê contexto, modelo+esforço, quota e tempo de sessão', () => {
    const f = parseStatusLine(KIMI_CLI)!;
    expect(f.model).toBe('K3');
    expect(f.effort).toBe('high');
    expect(f.ctxUsed).toBe(480_000);
    expect(f.ctxTotal).toBe(1_000_000);
    expect(Math.round(f.ctxPct!)).toBe(48);
    expect(f.turnIn).toBeUndefined();   // o Kimi não informa tokens do turno — nada inventado
    expect(f.fiveHourPct).toBe(3);
    expect(f.fiveHourReset).toBe('50m');
    expect(f.weeklyPct).toBe(33);
    expect(f.weeklyReset).toBe('seg 14h·5d6h');
    expect(f.sessionTime).toBe('15h13m');
    expect(f.repo).toBe('hangar');
    expect(f.branch).toBe('main');
    expect(f.dirty).toBe(true);
  });
});

describe('janelas de uso separadas so por espaco', () => {
  // No Pi as tres janelas vem coladas ("⚡5h … 📅7d … 🗓30d …") sem `│` entre elas. A classe do
  // reset semanal so cortava em │ e 🕐, entao com reset presente ela engolia o segmento mensal:
  // weeklyReset saia "6d19h 🗓30d:2%" e ia assim pra tela.
  const TRES = '🤖 k3 │ ⚡5h:51% ↻54m 📅7d:10% ↻6d19h 🗓30d:2% │ 💵 $0.00 │ 🕐 19:04';

  it('cada reset para no proximo emoji de janela', () => {
    const f = parseStatusLine(TRES)!;
    expect(f.fiveHourReset).toBe('54m');
    expect(f.weeklyReset).toBe('6d19h');
    expect(f.weeklyPct).toBe(10);
    expect(f.monthlyPct).toBe(2);
    expect(f.monthlyReset).toBeUndefined();   // 🗓30d:2% nao tem seta
  });
});

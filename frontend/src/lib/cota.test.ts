// Faixa de cota do rodapé do desktop: módulo puro que decide o que cada conta desenha. Nenhuma
// rede aqui — recebe o CotaConta[] de /api/cotas e devolve o quê mostrar.
import { describe, it, expect, beforeEach } from 'vitest';
import { overwriteGetLocale } from '../paraglide/runtime';
import { faixaDeCota, nivelDePct, faltaPara, diaDoReset, janelaLonga, VELHA_APOS_S, piorJanela } from './cota';
import type { CotaConta } from './contaEstado';

function lida(label: string, cinco: number, sete: number, extra: Partial<CotaConta> = {}): CotaConta {
  return {
    id: `claude:/home/u/${label}`, label, provedor: 'claude', ativa: false, estado: 'lida',
    janelas: [
      { rotulo: '5h', pct: cinco, reset_ts: 2_000 },
      { rotulo: '7d', pct: sete, reset_ts: 200_000 },
    ],
    ts: 1_000, idade_s: 5, ...extra,
  };
}

function semLeitura(label: string, estado: CotaConta['estado'] = 'expirada'): CotaConta {
  return {
    id: `claude:/home/u/${label}`, label, provedor: 'claude', ativa: false, estado,
    janelas: [], ts: null, idade_s: null, motivo: 'token-expirado',
  };
}

describe('faixaDeCota — a regra de aparecer', () => {
  it('sem conta nenhuma -> não aparece (null)', () => {
    expect(faixaDeCota([])).toBeNull();
  });
  it('uma conta só -> APARECE: a faixa é móvel do app, não aviso que pisca', () => {
    // Regra antiga era >=2 contas legíveis, e por isso a tira sumia do rodapé sozinha.
    const r = faixaDeCota([lida('jefferson', 64, 83)])!;
    expect(r).toHaveLength(1);
  });
  it('cada conta traz o SEU número (o bug que a rota nova fecha)', () => {
    const r = faixaDeCota([lida('default', 13, 22), lida('200-01', 26, 87)])!;
    expect(r.map((c) => c.janelas.map((j) => j.pct))).toEqual([[13, 22], [26, 87]]);
  });
  it('conta sem leitura aparece NOMEADA e vazia — nunca como zero', () => {
    const r = faixaDeCota([lida('default', 13, 22), semLeitura('jefferson')])!;
    expect(r).toHaveLength(2);
    expect(r[1].label).toBe('jefferson');
    expect(r[1].janelas).toEqual([]);
    expect(r[1].estado).toBe('expirada');
  });
  it("estado 'lida' sem nenhuma janela cai no balde de quem não leu", () => {
    const r = faixaDeCota([{ ...lida('x', 1, 2), janelas: [] }])!;
    expect(r[0].estado).toBe('indisponivel');
  });
  it('janela com pct não-numérico é descartada, o resto da conta fica', () => {
    const c = lida('x', 10, 20);
    c.janelas = [{ rotulo: '5h', pct: NaN }, { rotulo: '7d', pct: 20 }];
    const r = faixaDeCota([c])!;
    expect(r[0].janelas.map((j) => j.rotulo)).toEqual(['7d']);
  });
  it('rótulo da janela vem do provedor, não de uma constante', () => {
    const c = lida('kimi', 0, 0);
    c.janelas = [{ rotulo: '90min', pct: 5 }];
    expect(faixaDeCota([c])![0].janelas[0].rotulo).toBe('90min');
  });
});

describe('faixaDeCota — cor e idade', () => {
  it('cor só acima de 80%; acima de 90% é cheio', () => {
    expect(nivelDePct(64)).toBe('normal');
    expect(nivelDePct(80)).toBe('normal');
    expect(nivelDePct(83)).toBe('alerta');
    expect(nivelDePct(96)).toBe('cheio');
  });
  it('leitura velha marca a conta e mantém a idade (dado velho parece velho)', () => {
    const r = faixaDeCota([
      lida('fresca', 1, 2, { idade_s: 30 }),
      lida('velha', 1, 2, { idade_s: VELHA_APOS_S + 1 }),
    ])!;
    expect(r[0].velha).toBe(false);
    expect(r[1].velha).toBe(true);
    expect(r[1].idade_s).toBe(VELHA_APOS_S + 1);
  });
  it('conta-base do app vem marcada (é a que uma sessão nova vai gastar)', () => {
    const r = faixaDeCota([lida('default', 1, 2, { ativa: true }), lida('outra', 1, 2)])!;
    expect(r.map((c) => c.ativa)).toEqual([true, false]);
  });
});

describe('faltaPara — quanto falta pro reset', () => {
  const agora = 1_000_000;
  it('minutos abaixo de uma hora', () => {
    expect(faltaPara(agora + 35 * 60, agora)).toBe('35m');
  });
  it('horas com minutos preenchidos em dois dígitos', () => {
    expect(faltaPara(agora + 80 * 60, agora)).toBe('1h20');
    expect(faltaPara(agora + 120 * 60, agora)).toBe('2h');
  });
  it('acima de um dia', () => {
    expect(faltaPara(agora + 50 * 3600, agora)).toBe('2d2h');
  });
  it('reset ausente ou já passado não desenha nada', () => {
    expect(faltaPara(null, agora)).toBe('');
    expect(faltaPara(agora - 10, agora)).toBe('');
  });
});

describe('janela longa — que dia volta', () => {
  // O nome do dia sai do Intl no idioma do app (mesmo precedente do fmt.test.ts).
  beforeEach(() => overwriteGetLocale(() => 'pt'));
  const agora = Date.parse('2026-08-18T10:00:00-03:00') / 1000;   // terça
  it('reset a mais de um dia é janela longa; menos que isso, não', () => {
    expect(janelaLonga(agora + 3 * 86400, agora)).toBe(true);
    expect(janelaLonga(agora + 6 * 3600, agora)).toBe(false);
    expect(janelaLonga(null, agora)).toBe(false);
  });
  it('mostra dia da semana e hora, no idioma do app', () => {
    // sábado, 18h — o formato que responde "que dia a semana vira".
    const sabado = Date.parse('2026-08-22T18:00:00-03:00') / 1000;
    expect(diaDoReset(sabado, agora)).toBe('sáb 18h');
  });
  it('em inglês o dia sai em inglês — é data formatada, não chave de tradução', () => {
    overwriteGetLocale(() => 'en');
    expect(diaDoReset(Date.parse('2026-08-22T18:00:00-03:00') / 1000, agora)).toBe('Sat 18h');
  });
  it('reset ausente ou já passado não desenha nada', () => {
    expect(diaDoReset(null, agora)).toBe('');
    expect(diaDoReset(agora - 10, agora)).toBe('');
  });
});

describe('piorJanela — o smart da pílula do topo', () => {
  it('devolve a janela MAIS cheia entre todas as contas, com dona e janela', () => {
    const r = piorJanela(faixaDeCota([lida('default', 13, 22), lida('200-01', 26, 87)]));
    expect(r?.conta.label).toBe('200-01');
    expect(r?.janela.rotulo).toBe('7d');
    expect(r?.janela.pct).toBe(87);
  });

  it('em empate ganha a janela CURTA — a 5h derruba a sessão antes da 7d', () => {
    const r = piorJanela(faixaDeCota([lida('default', 50, 50)]));
    expect(r?.janela.rotulo).toBe('5h');
  });

  it('sem conta ou sem janela nenhuma: null (a pílula não inventa número)', () => {
    expect(piorJanela(faixaDeCota([]))).toBeNull();
    expect(piorJanela(faixaDeCota([semLeitura('jefferson')]))).toBeNull();
  });
});

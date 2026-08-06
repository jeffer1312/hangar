import { describe, expect, it } from 'vitest';
import { brutos, contaInflada, segundaDe, serieComparada, totaisComparados, valorDe } from './comparar';
import { filtrar, somar } from './cubo';
import type { ComboLocal } from './types';

const c = (o: Partial<ComboLocal>): ComboLocal => ({
  dia: '2026-08-05', provider: 'p', source: 'claude', project: '/r', model: 'm',
  servidor: 'srv-a', subagente: false, sessions: 1,
  input: 0, output: 0, cache_write: 0, cache_read: 0,
  cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0, ...o,
});

describe('segundaDe', () => {
  it('joga qualquer dia na segunda daquela semana', () => {
    expect(segundaDe('2026-08-05')).toBe('2026-08-03'); // quarta -> segunda
    expect(segundaDe('2026-08-03')).toBe('2026-08-03'); // já é segunda
    expect(segundaDe('2026-08-09')).toBe('2026-08-03'); // domingo fecha a MESMA semana
  });

  it('atravessa a virada do mês', () => {
    expect(segundaDe('2026-09-01')).toBe('2026-08-31');
  });
});

describe('serieComparada', () => {
  const dados = [
    c({ provider: 'anthropic:u', dia: '2026-08-03', cost: 10, input: 100 }),
    c({ provider: 'moonshotai', dia: '2026-08-03', cost: 1, input: 500 }),
    c({ provider: 'anthropic:u', dia: '2026-08-05', cost: 20, input: 200 }),
    c({ provider: 'openai', dia: '2026-08-05', cost: 99, input: 9 }), // fora das chaves
  ];

  it('um valor por entidade em cada ponto, na ORDEM das chaves', () => {
    const s = serieComparada(dados, 'provider', ['anthropic:u', 'moonshotai'], 'custo', false);
    expect(s.map((p) => p.x)).toEqual(['2026-08-03', '2026-08-04', '2026-08-05']);
    expect(s.map((p) => p.valores)).toEqual([[10, 1], [0, 0], [20, 0]]);
  });

  it('dia parado vira zero no eixo, não some', () => {
    // Mesma razão do fillDayGaps (costs.ts:249): sem o dia vazio, dois pontos separados por uma
    // semana ficam colados e o eixo mente sobre o ritmo.
    const s = serieComparada(
      [c({ provider: 'a', dia: '2026-08-01', cost: 1 }), c({ provider: 'a', dia: '2026-08-08', cost: 2 })],
      'provider', ['a'], 'custo', false);
    expect(s).toHaveLength(8);
    expect(s[0].valores[0]).toBe(1);
    expect(s[3].valores[0]).toBe(0);
    expect(s[7].valores[0]).toBe(2);
  });

  it('ignora quem não foi marcado', () => {
    const s = serieComparada(dados, 'provider', ['anthropic:u'], 'custo', false);
    expect(s.reduce((t, p) => t + p.valores[0], 0)).toBe(30); // os 99 do openai ficam de fora
  });

  it('tokens somam os quatro tipos', () => {
    const t = [c({ provider: 'a', input: 1, output: 2, cache_write: 4, cache_read: 8 })];
    expect(serieComparada(t, 'provider', ['a'], 'tokens', false)[0].valores[0]).toBe(15);
  });

  it('semanal junta os dias na segunda', () => {
    const s = serieComparada(dados, 'provider', ['anthropic:u', 'moonshotai'], 'custo', true);
    expect(s).toEqual([{ x: '2026-08-03', valores: [30, 1] }]);
  });

  it('sem chave nenhuma devolve série vazia', () => {
    expect(serieComparada(dados, 'provider', [], 'custo', false)).toEqual([]);
  });
});

describe('totaisComparados', () => {
  const dados = [
    c({ source: 'claude', cost: 10, input: 3 }),
    c({ source: 'pi', cost: 4, input: 1 }),
    c({ source: 'claude', cost: 6, input: 2 }),
    c({ source: 'codex', cost: 1000, input: 50 }), // FORA das chaves comparadas
  ];

  it('um balde por chave, na ordem pedida, mesmo sem dado', () => {
    const t = totaisComparados(dados, 'source', ['pi', 'claude', 'nada']);
    expect(t.map((b) => b.key)).toEqual(['pi', 'claude', 'nada']);
    expect(t.map((b) => b.cost)).toEqual([4, 16, 0]);
  });

  it('cada balde é exatamente o recorte daquela chave — nem cria, nem rouba do vizinho', () => {
    // A comparação com o TOTAL não provaria nada: se as chaves cobrissem 100% dos dados, somar
    // tudo em qualquer balde daria o mesmo número. Por isso `codex` fica de fora das chaves e a
    // asserção é balde a balde, contra o mesmo recorte calculado pelo cubo.
    const t = totaisComparados(dados, 'source', ['claude', 'pi']);
    for (const b of t) {
      const esperado = somar(filtrar(dados, { source: b.key }));
      expect(b.cost).toBe(esperado.cost);
      expect(b.input).toBe(esperado.input);
    }
    expect(t.reduce((s, b) => s + b.cost, 0)).toBe(20); // os 1000 do codex NÃO entraram
  });
});

describe('valorDe / brutos', () => {
  it('lê a métrica pedida do balde', () => {
    const b = totaisComparados([c({ source: 'pi', cost: 7, input: 1, output: 2 })], 'source', ['pi'])[0];
    expect(valorDe(b, 'custo')).toBe(7);
    expect(valorDe(b, 'tokens')).toBe(3);
    expect(brutos(b)).toBe(3);
  });
});

describe('contaInflada', () => {
  it('acusa modelo sem tarifa carimbado numa conta Anthropic', () => {
    // costs_sources.py:100-105: o provedor sai do CATÁLOGO de preços, não do id. Modelo que o
    // catálogo não conhece cai na conta Anthropic — inflando justamente o lado que o painel
    // compara. Medido em 06/08/2026: não acontece hoje, mas é o furo do dado.
    const combos = [
      c({ provider: 'anthropic:u', source: 'claude', model: 'modelo-de-motor', input: 10 }),
      c({ provider: 'anthropic:u', source: 'claude', model: 'claude-opus-5', input: 10 }),
    ];
    expect(contaInflada(combos, ['modelo-de-motor'])).toEqual(['modelo-de-motor']);
  });

  it('não acusa Pi nem Codex: eles trazem o provedor no próprio log', () => {
    const combos = [c({ provider: 'openrouter', source: 'pi', model: 'free' })];
    expect(contaInflada(combos, ['free'])).toEqual([]);
  });

  it('sem modelo sem tarifa, não acusa nada', () => {
    expect(contaInflada([c({ provider: 'anthropic:u', source: 'claude' })], [])).toEqual([]);
  });
});

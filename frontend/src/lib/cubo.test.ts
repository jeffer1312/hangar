import { describe, expect, it } from 'vitest';
import { agruparPor, aplicar, filtrar, somar } from './cubo';
import type { ComboRow } from './types';

const c = (o: Partial<ComboRow>): ComboRow => ({
  dia: '2026-07-01', provider: 'p', source: 'claude', project: '/r', model: 'm',
  subagente: false, sessions: 1, input: 0, output: 0, cache_write: 0, cache_read: 0,
  cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0, ...o,
});

describe('cubo', () => {
  const dados = [
    c({ project: '/web', source: 'claude', provider: 'anthropic:u', cost: 100, input: 10 }),
    c({ project: '/web', source: 'pi', provider: 'moonshotai', cost: 20, input: 2 }),
    c({ project: '/api', source: 'claude', provider: 'anthropic:u', cost: 7, input: 1 }),
    c({ project: '/web', source: 'claude', provider: 'anthropic:u', cost: 13, subagente: true }),
  ];

  it('cruza DOIS filtros — o que os agrupamentos marginais não respondiam', () => {
    const r = somar(filtrar(dados, { project: '/web', source: 'pi' }));
    expect(r.cost).toBe(20);
    expect(r.input).toBe(2);
  });

  it('separa o gasto de subagente', () => {
    // 13,7% do volume real desta máquina, invisível antes desta fase
    expect(somar(filtrar(dados, { subagente: true })).cost).toBe(13);
    expect(somar(filtrar(dados, { subagente: false })).cost).toBe(127);
  });

  it('filtro vazio devolve tudo, inclusive subagente', () => {
    expect(somar(filtrar(dados, {})).cost).toBe(140);
  });

  it('agrupar por qualquer dimensão bate com o total', () => {
    // a invariante que a fase 1 perdeu ao apagar um teste: fatiar não cria nem some
    for (const dim of ['dia', 'provider', 'source', 'project', 'model'] as const) {
      const soma = agruparPor(dados, dim).reduce((t, b) => t + b.cost, 0);
      expect(soma).toBeCloseTo(140, 6);
    }
  });

  it('combinação sem resultado devolve zero, não NaN', () => {
    const r = somar(filtrar(dados, { project: '/nao-existe' }));
    expect(r.cost).toBe(0);
    expect(Number.isNaN(r.input)).toBe(false);
  });

  it('sem detalhamento o recorte é ÚNICO — rótulo e número não podem divergir', () => {
    // Com dois cortes guardados, a tela sem `combos` mostraria o número de UM (é só o que os
    // `by_*` sabem responder) debaixo de um rótulo que anuncia os DOIS. O último clique vence,
    // como a tela fazia antes de existir cruzamento.
    const f = aplicar(aplicar({}, 'provider', 'anthropic:u', false), 'project', '/web', false);
    expect(f.provider).toBeFalsy();
    expect(f.project).toBe('/web');
  });

  it('com detalhamento os cortes combinam', () => {
    const f = aplicar(aplicar({}, 'provider', 'anthropic:u', true), 'project', '/web', true);
    expect(somar(filtrar(dados, f)).cost).toBe(113); // 100 da conversa + 13 do subagente
    expect(f.provider).toBe('anthropic:u');
  });

  it('campo ausente de servidor antigo não vira NaN', () => {
    const parcial = [{ dia: 'd', provider: 'p', source: 's', project: '/x', model: 'm',
                       cost: 5 } as unknown as ComboRow];
    const r = somar(parcial);
    expect(r.cost).toBe(5);
    expect(Number.isNaN(r.input)).toBe(false);
  });
});

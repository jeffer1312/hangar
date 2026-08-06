import { describe, it, expect } from 'vitest';
import {
  mergeReports, fillDayGaps, tarifasPorModelo, custoDesconhecido, precoParcial, partirOcultos,
  custoSemCacheDe, equivalenteDe, isFree,
} from './costs';
import type { ComboRow } from './types';
import type { CostBucket, CostReport, DimBucket, RateInfo } from './types';

function bucket(key: string, cost: number): CostBucket {
  return { key, sessions: 1, input: 0, output: 0, cache_read: 0, cache_write: 0, cost };
}

const vazio = () => ({
  totals: { key: 'totals', sessions: 0, input: 0, output: 0, cache_write: 0, cache_read: 0,
            cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0 },
  by_day: [], by_provider: [], by_source: [], by_project: [], by_model: [], by_kind: [],
  rates: [], sem_tarifa: [], custo_sem_cache: 0, applied: { period: '30d' }, usd_brl: null,
});

describe('mergeReports', () => {
  it('soma o mesmo provedor entre máquinas', () => {
    // A assinatura da Kimi é UMA só, gaste ela do desktop ou da VPS.
    const a = { ...vazio(), by_provider: [{ ...vazio().totals, key: 'kimi-coding', cost: 10, sessions: 1 }] };
    const b = { ...vazio(), by_provider: [{ ...vazio().totals, key: 'kimi-coding', cost: 5, sessions: 2 }] };
    const m = mergeReports([{ report: a }, { report: b }], '30d');
    expect(m.report.by_provider).toHaveLength(1);
    expect(m.report.by_provider[0].cost).toBe(15);
    expect(m.report.by_provider[0].sessions).toBe(3);
  });

  it('rótulo da conta sobrevive ao servidor que não o manda', () => {
    // A chave é o uuid (é ela que soma entre máquinas), mas quem se lê é o e-mail. Um servidor
    // da malha em versão antiga responde sem `label`, e sem o `??` ele apagaria o nome que a
    // máquina nova já tinha resolvido — a linha de topo voltava a ser o uuid cru.
    const chave = 'anthropic:758a9521-e2ef';
    const novo = { ...vazio(), by_provider: [{ ...vazio().totals, key: chave, label: 'eu@x.com', cost: 10 }] };
    const velho = { ...vazio(), by_provider: [{ ...vazio().totals, key: chave, cost: 4 }] };
    const m = mergeReports([{ report: velho }, { report: novo }], '30d');
    expect(m.report.by_provider[0].label).toBe('eu@x.com');
    expect(m.report.by_provider[0].cost).toBe(14);
  });

  it('servidor que não ecoou o período vira parcial, nunca somado', () => {
    // FastAPI ignora query param desconhecido: backend antigo recebe ?period=7d e devolve TUDO.
    // Somar isso com 7 dias de outra máquina e chamar de "7 dias" é mentira.
    const novo = { ...vazio(), applied: { period: '7d' } };
    const velho = { ...vazio(), applied: undefined, totals: { ...vazio().totals, cost: 999 } };
    const m = mergeReports([{ report: novo }, { report: velho }], '7d');
    expect(m.partial).toBe(true);
    expect(m.mismatched).toHaveLength(1);
    expect(m.report.totals.cost).toBe(0);
  });

  it('servidor sem os campos novos não vira NaN', () => {
    // O fixture PRECISA ecoar o período: sem `applied` ele cai na recusa e volta antes do
    // somarBucket, e aí `totals.cost === 0` seria verdade mesmo sem um único `?? 0` — o teste
    // passaria por acidente, provando nada. Aqui ele ENTRA na soma faltando campo.
    const semNada = { applied: { period: 'all' }, usd_brl: null };
    const meioBucket = {
      applied: { period: 'all' },
      totals: { key: 'totals', cost: 5 },              // sem os tokens nem os cost_*
      by_provider: [{ key: 'kimi-coding', cost: 2 }],  // idem
    } as unknown as Partial<CostReport>;

    const m = mergeReports([{ report: semNada }, { report: meioBucket }], 'all');
    expect(m.report.totals.cost).toBe(5);
    expect(m.report.totals.input).toBe(0);
    expect(Number.isNaN(m.report.totals.input)).toBe(false);
    expect(Number.isNaN(m.report.totals.cost_cache_read)).toBe(false);
    expect(m.report.by_provider[0].cache_read).toBe(0);
  });

  it('rótulo do servidor recusado sai do merge, não do índice', () => {
    // `#2` é acoplamento posicional: o chamador filtra a lista antes e o índice passa a apontar
    // pra máquina errada, com o tipo `string[]` intacto.
    const velho = { ...vazio(), applied: undefined };
    const m = mergeReports([{ report: velho, label: 'vps' }, { report: vazio() }], '30d');
    expect(m.mismatched).toEqual(['vps']);
  });

  it('a cotação do servidor recusado ainda vale', () => {
    // USD/BRL não depende de período: se a única máquina com cotação for a desatualizada, perder
    // o R$ da malha inteira é dano colateral gratuito.
    const velho = { ...vazio(), applied: undefined, usd_brl: 5.4 };
    const m = mergeReports([{ report: velho }, { report: vazio() }], '30d');
    expect(m.partial).toBe(true);
    expect(m.report.usd_brl).toBe(5.4);
  });

  it('soma equivalente_cobrado entre as máquinas', () => {
    const a = { ...vazio(), equivalente_cobrado: 3_000_000 };
    const b = { ...vazio(), equivalente_cobrado: 1_500_000 };
    expect(mergeReports([{ report: a }, { report: b }], '30d').report.equivalente_cobrado)
      .toBe(4_500_000);
  });

  it('rates com o mesmo modelo em provedores diferentes não colidem', () => {
    const tarifa = (provider: string, input: number) => ({
      model: 'glm-5.2', provider, input, output: 1, cache_read: 0, cache_write: 0, origin: 'override',
    });
    const a = { ...vazio(), rates: [tarifa('zai', 2)] };
    const b = { ...vazio(), rates: [tarifa('kimi', 9)] };
    const m = mergeReports([{ report: a }, { report: b }], '30d');
    expect(m.report.rates).toHaveLength(2);
    expect(m.report.rates.map((t) => t.provider).sort()).toEqual(['kimi', 'zai']);
  });

  describe('anterior', () => {
    const comAnterior = (cost: number, ant: number | null) => ({
      ...vazio(),
      totals: { ...vazio().totals, cost },
      anterior: ant === null ? null : { ...vazio().totals, key: 'anterior', cost: ant },
    });

    it('soma quando TODOS os servidores somados mandaram o deles', () => {
      const m = mergeReports([{ report: comAnterior(100, 40) }, { report: comAnterior(100, 60) }], '30d');
      expect(m.report.anterior?.cost).toBe(100);
    });

    it('some quando só ALGUNS mandaram — comparação parcial mente pra cima', () => {
      // totals = 200 (as duas máquinas) contra anterior = 90 (uma só) daria "+122%", que é falso.
      const m = mergeReports([{ report: comAnterior(100, 90) }, { report: comAnterior(100, null) }], '30d');
      expect(m.report.totals.cost).toBe(200);
      expect(m.report.anterior).toBeNull();
    });

    it('null quando ninguém mandou', () => {
      expect(mergeReports([{ report: vazio() }], '30d').report.anterior).toBeNull();
    });
  });

  it('servidor que falhou marca parcial sem derrubar os outros', () => {
    const ok = { ...vazio(), totals: { ...vazio().totals, cost: 7 } };
    const m = mergeReports([{ report: ok }, { report: null }], '30d');
    expect(m.partial).toBe(true);
    expect(m.report.totals.cost).toBe(7);
  });

  it('caído e fora-de-período são listas separadas, cada um com seu nome', () => {
    // Acontecendo os dois juntos, uma lista só não dá: o aviso da tela precisa dizer QUAL máquina
    // caiu e QUAL respondeu o período errado. Antes, com um `mismatched` preenchido, a máquina
    // caída sumia atrás de um "algum servidor não respondeu" que não nomeava ninguém.
    const m = mergeReports([
      { report: null, label: 'vps' },
      { report: { ...vazio(), applied: undefined }, label: 'notebook' },
      { report: vazio(), label: 'desktop' },
    ], '30d');
    expect(m.failed).toEqual(['vps']);
    expect(m.mismatched).toEqual(['notebook']);
    expect(m.partial).toBe(true);
  });

  it('ordena by_day desc por data e by_model desc por custo', () => {
    // Ordem é contrato, não estética: o ranking divide pela MAIOR barra e o gráfico lê `by_day`
    // de trás pra frente pra desenhar o eixo do tempo. `juntarDim` devolve um Map (ordem de
    // inserção, ou seja, a do primeiro servidor que respondeu), então quem ordena é o merge.
    const dia = (key: string, cost: number) => ({ ...vazio().totals, key, cost });
    const a = {
      ...vazio(),
      by_day: [dia('2026-07-01', 1), dia('2026-07-03', 5)],
      by_model: [dia('opus', 2), dia('haiku', 9)],
    };
    const b = {
      ...vazio(),
      by_day: [dia('2026-07-02', 3)],
      by_model: [dia('sonnet', 5)],
    };
    const m = mergeReports([{ report: a }, { report: b }], '30d');
    expect(m.report.by_day.map((x) => x.key)).toEqual(['2026-07-03', '2026-07-02', '2026-07-01']);
    expect(m.report.by_model.map((x) => x.key)).toEqual(['haiku', 'sonnet', 'opus']);
  });

  it('empate de custo desempata pelo nome, pra ordem não depender do servidor', () => {
    const emp = (key: string) => ({ ...vazio().totals, key, cost: 4 });
    const m = mergeReports([{ report: { ...vazio(), by_project: [emp('zeta'), emp('alfa')] } }], '30d');
    expect(m.report.by_project.map((x) => x.key)).toEqual(['alfa', 'zeta']);
  });
});

describe('tarifasPorModelo', () => {
  const tarifa = (provider: string, model: string, input: number): RateInfo => ({
    model, provider, input, output: 1, cache_read: 0, cache_write: 0, origin: 'models.dev',
  });

  it('modelo com uma tarifa só devolve a tarifa', () => {
    const m = tarifasPorModelo([tarifa('anthropic', 'claude-opus-4', 15)]);
    expect(m.get('claude-opus-4')?.input).toBe(15);
  });

  it('mesmo modelo em dois provedores: conhecido, mas SEM tarifa única', () => {
    // O custo da linha soma os dois provedores; exibir o preço de um ao lado dele seria dizer que
    // aquele número saiu daquela tarifa. `has` continua true — o custo é real, só o preço é que
    // não tem resposta única.
    const m = tarifasPorModelo([tarifa('zai', 'glm-5.2', 2), tarifa('kimi', 'glm-5.2', 9)]);
    expect(m.has('glm-5.2')).toBe(true);
    expect(m.get('glm-5.2')).toBeNull();
  });

  it('modelo sem tarifa nenhuma não entra no mapa — é "não sei", não "de graça"', () => {
    const m = tarifasPorModelo([tarifa('anthropic', 'claude-opus-4', 15)]);
    expect(m.has('claude-sonnet-4')).toBe(false);
  });
});

describe('custoDesconhecido', () => {
  const balde = (p: Partial<DimBucket>): DimBucket => ({
    key: 'x', sessions: 1, input: 0, output: 0, cache_write: 0, cache_read: 0,
    cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0, ...p,
  });

  it('vale para os QUATRO cortes, porque pergunta ao balde e não à dimensão', () => {
    // Um provedor, uma fonte ou um projeto cujos modelos sejam todos desconhecidos cai no mesmo
    // buraco que o modelo sem tarifa: custo 0 com volume positivo. A regra amarrada em
    // `dim === 'model'` só pegava o quarto caso.
    for (const key of ['kimi-coding', 'codex', '/home/jefferson/x', 'claude-sonnet-4']) {
      expect(custoDesconhecido(balde({ key, input: 70, cache_read: 266_000 }))).toBe(true);
    }
  });

  it('volume com custo é conhecido', () => {
    expect(custoDesconhecido(balde({ input: 1000, cost: 0.03 }))).toBe(false);
  });

  it('balde sem volume nenhum não é "sem tarifa" — é dia parado, e US$ 0,00 ali é verdade', () => {
    // O gráfico preenche buraco de data com dia zerado; marcá-lo como desconhecido trocaria um
    // zero honesto por um traço.
    expect(custoDesconhecido(balde({ sessions: 0 }))).toBe(false);
  });
});

describe('isFree', () => {
  it('reconhece tier grátis em qualquer grafia que o log grava', () => {
    expect(isFree('free')).toBe(true);
    expect(isFree('kimi-k3-free')).toBe(true);
    expect(isFree('moonshotai/kimi-k3-free')).toBe(true);
    expect(isFree('nvidia/nemotron-nano-12b-v2-vl:free')).toBe(true);
    expect(isFree('gpt-5.6-luna:free')).toBe(true);
    // o Pi gruda o nível de thinking depois do :free — a regex que exigia fim-de-string perdia
    expect(isFree('kimi-k3-free:high')).toBe(true);
  });

  it('não confunde com modelo pago', () => {
    expect(isFree('kimi-k3')).toBe(false);
    expect(isFree('claude-sonnet-4')).toBe(false);
    expect(isFree('freestyle')).toBe(false);
  });
});

describe('precoParcial', () => {
  it('modelo que UM servidor da malha não tarifou sai marcado', () => {
    // O balde mesclado tem volume dos dois servidores e custo de um só: `tarifas.has()` diz true
    // (o servidor novo mandou a tarifa) e `custoDesconhecido()` diz false (o custo não é zero).
    // Sem esta terceira pergunta a linha mostra preço subestimado sem marca nenhuma.
    expect(precoParcial('kimi-k3', true, ['kimi-k3'])).toBe(true);
  });

  it('modelo tarifado em toda a malha não é parcial', () => {
    expect(precoParcial('claude-opus-5', true, ['kimi-k3'])).toBe(false);
  });

  it('modelo sem tarifa em lugar nenhum não é "parcial" — é "sem tarifa"', () => {
    // Já tem marca própria e o custo dele é traço; marcar de parcial diria que existe preço.
    expect(precoParcial('kimi-k3', false, ['kimi-k3'])).toBe(false);
  });
});

describe('partirOcultos', () => {
  const balde = (key: string, cost: number): DimBucket => ({
    key, sessions: 1, input: 10, output: 2, cache_write: 0, cache_read: 100,
    cost, cost_input: cost, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0,
  });
  const lista = [balde('/repo/a', 90), balde('/repo/b', 30), balde('/repo/c', 5)];

  it('esconder NÃO muda total nenhum', () => {
    // O candidato número um a "o total não bate": o × da lista tira o projeto da VISTA, nunca da
    // conta — a soma dos visíveis com os escondidos tem que continuar sendo a lista inteira, e o
    // `report.totals`, que a tela lê direto do servidor, nem passa por aqui.
    const soma = (l: DimBucket[]) => l.reduce((t, b) => t + b.cost, 0);
    const inteiro = soma(lista);
    for (const ocultos of [new Set<string>(), new Set(['/repo/a']),
                           new Set(['/repo/a', '/repo/c']),
                           new Set(['/repo/a', '/repo/b', '/repo/c'])]) {
      const p = partirOcultos(lista, ocultos);
      expect(soma(p.visiveis) + soma(p.escondidos)).toBe(inteiro);
      expect(p.visiveis.length + p.escondidos.length).toBe(lista.length);
    }
  });

  it('a régua escala pelos VISÍVEIS', () => {
    // Com o pico preso num projeto escondido, todas as barras da tela ficariam curtas por causa
    // de algo que ninguém vê.
    expect(partirOcultos(lista, new Set()).pico).toBe(90);
    expect(partirOcultos(lista, new Set(['/repo/a'])).pico).toBe(30);
  });

  it('esconder tudo não vira divisão por zero', () => {
    // `Math.max()` de lista vazia é -Infinity, e a largura da barra viraria NaN%.
    expect(partirOcultos(lista, new Set(['/repo/a', '/repo/b', '/repo/c'])).pico).toBe(1);
    expect(partirOcultos([], new Set()).pico).toBe(1);
  });

  it('preserva a ordem que veio do servidor', () => {
    expect(partirOcultos(lista, new Set(['/repo/b'])).visiveis.map((b) => b.key))
      .toEqual(['/repo/a', '/repo/c']);
  });
});

describe('custoSemCacheDe / equivalenteDe', () => {
  const combo = (extra: Partial<ComboRow>): ComboRow => ({
    dia: '2026-08-06', provider: 'deepseek', source: 'pi', project: '/x',
    model: 'deepseek-v4-flash', subagente: false, sessions: 1,
    input: 1_000_000, output: 1_000_000, cache_write: 0, cache_read: 1_000_000, ...extra,
  } as ComboRow);
  const tarifas = new Map([['deepseek-v4-flash', {
    model: 'deepseek-v4-flash', provider: 'deepseek', input: 0.14, output: 0.28,
    cache_read: 0.0028, cache_write: 0.14, origin: 'models.dev',
  }]]);

  it('espelha a aritmética do costs.py num recorte', () => {
    // sem cache: (1Mi in + 1Mi cr) * 0.14 + 1Mi out * 0.28 = 0.56
    expect(custoSemCacheDe([combo({})], tarifas)).toBeCloseTo(0.56);
    // equivalente-input: 1Mi + 1Mi*(0.28/0.14) + 1Mi*(0.0028/0.14) = 1 + 2 + 0.02 = 3.02Mi
    expect(equivalenteDe([combo({})], tarifas)).toBeCloseTo(3_020_000);
  });

  it('linha sem tarifa não entra na conta', () => {
    const sem = combo({ model: 'kimi-k3-free', cache_write: 500_000 });
    expect(custoSemCacheDe([sem], new Map())).toBe(0);
    expect(equivalenteDe([sem], new Map())).toBe(0);
  });

  it('ignora tarifa sem preço de input (sem régua pra converter)', () => {
    const semInput = new Map([['x', {
      model: 'x', provider: 'p', input: 0, output: 2, cache_read: 1, cache_write: 1, origin: 'm',
    }]]);
    expect(equivalenteDe([combo({ model: 'x' })], semInput)).toBe(0);
  });
});

describe('fillDayGaps', () => {
  it('preenche buracos com dias zerados', () => {
    const out = fillDayGaps([bucket('2026-07-22', 3), bucket('2026-07-18', 1)]);
    expect(out.map((b) => b.key)).toEqual([
      '2026-07-22', '2026-07-21', '2026-07-20', '2026-07-19', '2026-07-18',
    ]);
    expect(out[1].cost).toBe(0);
    expect(out[1].sessions).toBe(0);
  });

  it('não mexe em lista contínua nem em lista de 1 item', () => {
    const cont = [bucket('2026-07-22', 3), bucket('2026-07-21', 1)];
    expect(fillDayGaps(cont)).toEqual(cont);
    const one = [bucket('2026-07-22', 3)];
    expect(fillDayGaps(one)).toBe(one);
  });

  it('atravessa virada de mês', () => {
    const out = fillDayGaps([bucket('2026-07-02', 1), bucket('2026-06-29', 1)]);
    expect(out.map((b) => b.key)).toEqual(['2026-07-02', '2026-07-01', '2026-06-30', '2026-06-29']);
  });
});

// @vitest-environment happy-dom
// A pílula de cota do topo: mostra a pior janela (smart) e o clique abre o popover agrupado por
// provider. Mock na fronteira (listarCotas), como no QuotaStrip.test.ts — nada de rede aqui.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import QuotaPill from './QuotaPill.svelte';
import * as contaEstadoLib from '../lib/contaEstado';
import { quotaFeed } from '../lib/quotaFeed.svelte';
import { quotaBarra } from '../lib/quotaBarra.svelte';
import type { CotaConta } from '../lib/contaEstado';

vi.mock('../lib/contaEstado', async (importOriginal) => {
  const real = await importOriginal<typeof import('../lib/contaEstado')>();
  return { ...real, listarCotas: vi.fn() };
});
const cotaMock = vi.mocked(contaEstadoLib);

function lida(label: string, cinco: number, sete: number, provedor: CotaConta['provedor'] = 'claude'): CotaConta {
  return {
    id: `${provedor}:/home/u/${label}`, label, provedor, ativa: false, estado: 'lida',
    janelas: [
      { rotulo: '5h', pct: cinco, reset_ts: Date.now() / 1000 + 80 * 60 },
      { rotulo: '7d', pct: sete, reset_ts: Date.now() / 1000 + 3 * 86400 },
    ],
    ts: Date.now() / 1000, idade_s: 5,
  };
}

function montar(contas: CotaConta[], contaAtiva: string | null = null) {
  cotaMock.listarCotas.mockResolvedValue(contas);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(QuotaPill, { target: el, props: { serverKey: 'srv-1', contaAtiva, onIrParaContas: vi.fn() } });
  return { el, comp: comp as never };
}

beforeEach(() => {
  vi.clearAllMocks();
  quotaFeed.resetParaTeste();
  // A faixa aberta por padrão num teste anterior não pode vazar — a preferência é global.
  if (quotaBarra.aberta) quotaBarra.alternar();
});
afterEach(() => { document.body.innerHTML = ''; });

describe('QuotaPill', () => {
  it('mostra a pior janela entre todas as contas (o smart), com rótulo e dona', async () => {
    const t = montar([lida('default', 13, 22), lida('200-01', 26, 87)]);
    await tick(); await tick();
    const pill = t.el.querySelector('.quota-pill')!;
    expect(pill.querySelector('.qp-num')!.textContent).toBe('87%');
    expect(pill.querySelector('.qp-quem')!.textContent).toContain('7d');
    expect(pill.querySelector('.qp-quem')!.textContent).toContain('200-01');
    unmount(t.comp);
  });

  it('sem conta nenhuma a pílula não existe (a regra de aparecer é da faixa)', async () => {
    const t = montar([]);
    await tick(); await tick();
    expect(t.el.querySelector('.quota-pill')).toBeNull();
    unmount(t.comp);
  });

  it('com contaAtiva a pílula mostra a conta DA SESSÃO, mesmo não sendo a pior', async () => {
    const contas = [lida('default', 13, 22), lida('200-01', 26, 87)];
    const t = montar(contas, contas[0].id); // ativa = default (22%), pior geral = 200-01 (87%)
    await tick(); await tick();
    const pill = t.el.querySelector('.quota-pill')!;
    expect(pill.querySelector('.qp-num')!.textContent).toBe('22%');
    expect(pill.querySelector('.qp-quem')!.textContent).toContain('default');
    unmount(t.comp);
  });

  it('o clique abre o popover agrupado por provider, com barra por janela', async () => {
    const t = montar([lida('default', 13, 22), lida('kimi-coding', 40, 64, 'kimi')]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.quota-pill')!.click();
    await tick();
    // O popover sai por portal pro <body> — não é filho do mount.
    const pop = document.querySelector('.qp-pop')!;
    expect(pop).not.toBeNull();
    const provs = [...pop.querySelectorAll('.qp-prov-nome')].map((p) => p.textContent);
    expect(provs).toEqual(['Claude', 'Kimi']);
    expect(pop.querySelectorAll('.qp-barra').length).toBe(4); // 2 contas × 2 janelas
    unmount(t.comp);
  });

  it('conta opencode vira grupo próprio com glifo — sem quebrar o popover (regressão: quebrou em produção)', async () => {
    const t = montar([lida('default', 13, 22), lida('DeepSeek · opencode direto', 40, 93, 'opencode' as CotaConta['provedor'])]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.quota-pill')!.click();
    await tick();
    const pop = document.querySelector('.qp-pop');
    expect(pop).not.toBeNull();
    const provs = [...pop!.querySelectorAll('.qp-prov-nome')].map((p) => p.textContent);
    expect(provs).toEqual(['Claude', 'OpenCode']);
    unmount(t.comp);
  });

  it('o toggle do rodapé abre e fecha a faixa do rodapé (modo expandido)', async () => {
    const t = montar([lida('default', 13, 22)]);
    await tick(); await tick();
    expect(quotaBarra.aberta).toBe(false);
    t.el.querySelector<HTMLButtonElement>('.quota-pill')!.click();
    await tick();
    document.querySelector<HTMLButtonElement>('.qp-toggle')!.click();
    expect(quotaBarra.aberta).toBe(true);
    unmount(t.comp);
  });
});

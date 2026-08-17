// @vitest-environment happy-dom
// Faixa de cota do rodapé do desktop (Task 9): aparece só com >=2 contas legíveis, desenha uma
// linha por conta (janelas 5h/7d, cor acima de 80%, idade na leitura velha), link leva à aba
// Contas por callback — o DesktopShell desvia para a rota (?config=contas), nunca importando o
// componente ContasSettings (contrato de posse do lote).
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import QuotaStrip from './QuotaStrip.svelte';
import * as m from '../paraglide/messages';
import * as contaEstadoLib from '../lib/contaEstado';
import type { ContaEstado } from '../lib/contaEstado';

vi.mock('../lib/contaEstado', async (importOriginal) => {
  const real = await importOriginal<typeof import('../lib/contaEstado')>();
  return { ...real, listarEstadosDeConta: vi.fn() };
});

const estadoMock = vi.mocked(contaEstadoLib);

const ALEATORIO = 60 + Math.floor(Math.random() * 39); // % arbitrária abaixo de 100

// Conta 'lida' com a linha crua da statusline (mesmo shape do mock da Task: ⚡5h/📅7d).
function lida(label: string, linha: string, ts = Date.now() / 1000, idade_s = 5): ContaEstado {
  return {
    path: `/home/u/${label}`, label, active: false,
    login: { estado: 'ok', loggedIn: true },
    limite: { estado: 'lido', linha, ts, idade_s },
  };
}
const COM_JANELAS = (label: string, cinco: string, sete: string, idade_s?: number, ts = Date.now() / 1000) =>
  lida(label, `🤖 Opus (high✦) │ ⚡5h:${cinco}% 📅7d:${sete}% │ 💵 $1.20`, ts, idade_s);
const SEM_LEITURA: ContaEstado = {
  path: '/home/u/nova', label: 'nova', active: false,
  login: { estado: 'ok', loggedIn: false },
  limite: { estado: 'sem_leitura' },
};

function montar(contas: ContaEstado[], onIrParaContas = vi.fn()) {
  estadoMock.listarEstadosDeConta.mockResolvedValue(contas);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(QuotaStrip, { target: el, props: { serverKey: 'srv-1', onIrParaContas } });
  return { el, comp: comp as never, onIrParaContas };
}

beforeEach(() => { vi.clearAllMocks(); });
afterEach(() => { document.body.innerHTML = ''; });

describe('QuotaStrip — a regra de aparecer', () => {
  it('não está no DOM com uma conta só (o número dela já está na statusline da sessão)', async () => {
    const t = montar([COM_JANELAS('jefferson', '64', '83')]);
    await tick(); await tick();
    expect(t.el.querySelector('.quota-faixa')).toBeNull();
    unmount(t.comp);
  });
  it('não está no DOM com nenhuma conta legível (só sem leitura)', async () => {
    const t = montar([SEM_LEITURA]);
    await tick(); await tick();
    expect(t.el.querySelector('.quota-faixa')).toBeNull();
    unmount(t.comp);
  });
  it('com duas, mostra as duas linhas com nome e janelas', async () => {
    const t = montar([
      COM_JANELAS('jefferson', '64', '83'),
      COM_JANELAS('claude-200-1', '5', '58'),
    ]);
    await tick(); await tick();
    const faixa = t.el.querySelector('.quota-faixa')!;
    expect(faixa).not.toBeNull();
    const contas = [...faixa.querySelectorAll('.quota-conta')];
    expect(contas).toHaveLength(2);
    const nomes = [...faixa.querySelectorAll('.quota-nome')].map((n) => n.textContent);
    expect(nomes).toEqual(['jefferson', 'claude-200-1']);
    // janela 5h do jefferson: 64%
    const par5h = contas[0].querySelectorAll('.quota-par')[0];
    expect(par5h.querySelector('.quota-rot')!.textContent).toBe('5h');
    expect(par5h.querySelector('.quota-num')!.textContent).toContain('64');
    unmount(t.comp);
  });
  it('a conta sem leitura não vira linha na faixa', async () => {
    const t = montar([
      COM_JANELAS('jefferson', '64', '83'),
      SEM_LEITURA,
      COM_JANELAS('claude-200-1', '5', '58'),
    ]);
    await tick(); await tick();
    const contas = [...t.el.querySelectorAll('.quota-conta')];
    expect(contas).toHaveLength(2);
    expect(contas.map((c) => c.querySelector('.quota-nome')!.textContent))
      .toEqual(['jefferson', 'claude-200-1']);
    unmount(t.comp);
  });
});

describe('QuotaStrip — cor e idade', () => {
  it('cor acima de 80% na barra e no número', async () => {
    const t = montar([
      COM_JANELAS('jefferson', '64', '83'),
      COM_JANELAS('outra', '96', '5'),
    ]);
    await tick(); await tick();
    const faixa = t.el.querySelector('.quota-faixa')!;
    const contas = [...faixa.querySelectorAll('.quota-conta')];
    const par7dJeff = contas[0].querySelectorAll('.quota-par')[1];
    expect(par7dJeff.querySelector('.quota-barra i')!.getAttribute('class')).toContain('alerta');
    expect(par7dJeff.querySelector('.quota-num')!.getAttribute('class')).toContain('alerta');
    const par5hOutra = contas[1].querySelectorAll('.quota-par')[0];
    expect(par5hOutra.querySelector('.quota-barra i')!.getAttribute('class')).toContain('cheio');
    unmount(t.comp);
  });
  it('a conta de leitura velha esmaece e carrega a idade (dado velho parece velho)', async () => {
    // ts = 2 h atrás (bem acima de VELHA_APOS_S = 600 s) — velha. O componente re-deriva a
    // idade do ts (o backend manda o par ts/idade_s coerente: idade_s = agora - ts), então o
    // cenário fabrica o ts velho, não o idade_s.
    const t = montar([
      COM_JANELAS('jefferson', '64', '83'),
      COM_JANELAS('velha', '5', '58', undefined, Date.now() / 1000 - 2 * 3600),
    ]);
    await tick(); await tick();
    const contas = [...t.el.querySelectorAll('.quota-conta')];
    const velha = contas.find((c) => c.querySelector('.quota-nome')!.textContent === 'velha')!;
    expect(velha.getAttribute('class')).toContain('velha');
    expect(velha.querySelector('.quota-idade')!.textContent).toContain(m.cota_idade({ n: '2 h' }));
    // A fresca não carrega idade.
    const fresca = contas.find((c) => c.querySelector('.quota-nome')!.textContent === 'jefferson')!;
    expect(fresca.getAttribute('class')).not.toContain('velha');
    expect(fresca.querySelector('.quota-idade')).toBeNull();
    unmount(t.comp);
  });
});

describe('QuotaStrip — o link para a aba Contas', () => {
  it('clique no link chama o callback (o DesktopShell desvia para a rota ?config=contas)', async () => {
    const onIrParaContas = vi.fn();
    const t = montar([
      COM_JANELAS('jefferson', '64', '83'),
      COM_JANELAS('claude-200-1', '5', '58'),
    ], onIrParaContas);
    await tick(); await tick();
    const link = t.el.querySelector<HTMLButtonElement>('.quota-link')!;
    expect(link).not.toBeNull();
    expect(link.textContent).toBe(m.contas_titulo());
    link.click();
    expect(onIrParaContas).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });
});
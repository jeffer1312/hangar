// @vitest-environment happy-dom
// Aba Contas: a lista lê a fonte única (/api/conta-estado via lib/contaEstado), criar/apagar
// reusam as rotas de sempre (lib/api), conta deslogada NUNCA some da lista.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ContasSettings from './ContasSettings.svelte';
import * as m from '../../paraglide/messages';
import * as contaEstadoLib from '../../lib/contaEstado';
import * as apiLib from '../../lib/api';
import type { ContaEstado } from '../../lib/contaEstado';

vi.mock('../../lib/contaEstado', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/contaEstado')>();
  return { ...real, listarEstadosDeConta: vi.fn() };
});
vi.mock('../../lib/api', () => ({
  criarConta: vi.fn(async () => ({ path: '/x', label: 'x', active: false })),
  apagarConta: vi.fn(async () => {}),
}));

const estadoMock = vi.mocked(contaEstadoLib);
const apiMock = vi.mocked(apiLib);

const LOGADA: ContaEstado = {
  path: '/home/u/.claude-jefferson', label: 'jefferson', active: true,
  login: { estado: 'ok', loggedIn: true, email: 'jefferson@example.com', plano: 'max' },
  limite: { estado: 'lido', linha: '⚡5h:64% 📅7d:83%', ts: 1, idade_s: 5 },
};
const DESLOGADA: ContaEstado = {
  path: '/home/u/.claude-testes', label: 'testes', active: false,
  login: { estado: 'ok', loggedIn: false },
  limite: { estado: 'sem_leitura' },
};

function montar(contas: ContaEstado[]) {
  estadoMock.listarEstadosDeConta.mockResolvedValue(contas);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ContasSettings, { target: el });
  return { el, comp: comp as never };
}

beforeEach(() => { vi.clearAllMocks(); });

describe('ContasSettings — a lista', () => {
  it('mostra conta com em uso e e-mail (a frase do plano ficou de fora por decisão do árbitro — Task de ajuste serial quando o Lote A mergear)', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    const linha = t.el.querySelector<HTMLElement>('.ct-linha')!;
    expect(linha.querySelector('.ct-nome')!.textContent).toBe('jefferson');
    expect(linha.querySelector('.ct-emuso')!.textContent).toBe(m.contas_em_uso());
    expect(linha.querySelector('.ct-sub')!.textContent).toContain('jefferson@example.com');
    expect(linha.querySelector('.ct-cota')!.textContent).toContain(m.cota_lido_agora());
    unmount(t.comp);
  });

  it('conta deslogada CONTINUA na lista, com Entrar (inerte) e estado de leitura explícito', async () => {
    const t = montar([LOGADA, DESLOGADA]);
    await tick(); await tick();
    const linhas = t.el.querySelectorAll<HTMLElement>('.ct-linha');
    expect(linhas.length).toBe(2);
    const fora = linhas[1];
    expect(fora.textContent).toContain('testes');
    expect(fora.textContent).toContain(m.contas_nao_conectada());
    expect(fora.textContent).toContain(m.contas_sem_leitura());
    expect(fora.querySelector('.ct-acao.primaria')!.textContent).toBe(m.contas_entrar());
    unmount(t.comp);
  });

  it('limite velho aparece com a idade e sem parecer fresco', async () => {
    const t = montar([{ ...LOGADA, active: false, limite: { estado: 'lido', linha: 'x', ts: 1, idade_s: 7200 } }]);
    await tick(); await tick();
    const cota = t.el.querySelector<HTMLElement>('.ct-cota')!;
    expect(cota.textContent).toContain(m.cota_ultima_leitura({ n: '2 h' }));
    expect(cota.classList.contains('velha')).toBe(true);
    unmount(t.comp);
  });
});

describe('ContasSettings — criar e apagar reusam as rotas de sempre', () => {
  it('criar: campo inline + criarConta + recarrega a lista', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-btn')!.click();   // + Nova conta
    await tick();
    const input = t.el.querySelector<HTMLInputElement>('.ct-campo')!;
    input.value = 'minha-conta';
    input.dispatchEvent(new Event('input'));
    await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-rodape .ct-btn:nth-of-type(2)')!.click(); // criar
    await tick(); await tick();
    expect(apiMock.criarConta).toHaveBeenCalledWith('minha-conta');
    expect(estadoMock.listarEstadosDeConta).toHaveBeenCalledTimes(2);   // montagem + pós-criar
    unmount(t.comp);
  });

  it('apagar: kebab → menu → confirmação → apagarConta com o NOME (label) da conta', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-kebab')!.click();
    await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-menu-item')!.click();
    await tick();
    expect(t.el.querySelector('.ct-confirma')!.textContent).toContain(m.criar_apagar_fim());
    t.el.querySelector<HTMLButtonElement>('.ct-confirma-btn.perigo')!.click();
    await tick(); await tick();
    expect(apiMock.apagarConta).toHaveBeenCalledWith('jefferson');
    expect(estadoMock.listarEstadosDeConta).toHaveBeenCalledTimes(2);
    unmount(t.comp);
  });
});
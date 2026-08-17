// @vitest-environment happy-dom
// Task 7 — o fluxo de Entrar numa conta pelo app (sem terminal), por cima da lista da Task 4.
// O estado da tentativa vive no componente; o cliente lib/loginConta é mockado (a tela nunca
// fala com a rede real no teste).
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ContasSettings from './ContasSettings.svelte';
import * as m from '../../paraglide/messages';
import * as contaEstadoLib from '../../lib/contaEstado';
import * as loginLib from '../../lib/loginConta';
import { mensagemDeErro } from '../../lib/errosApi';
import type { ContaEstado } from '../../lib/contaEstado';

vi.mock('../../lib/contaEstado', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/contaEstado')>();
  return { ...real, listarEstadosDeConta: vi.fn() };
});
vi.mock('../../lib/loginConta', () => ({
  iniciarLogin: vi.fn(async () => ({ ok: true })),
  passoLogin: vi.fn(async () => ({ etapa: 'aguardando', url: 'https://claude.com/cai/oauth/authorize' })),
  confirmarLogin: vi.fn(async () => ({ ok: true, email: 'u@example.com', plano: 'max' })),
  cancelarLogin: vi.fn(async () => ({ ok: true })),
}));

const estadoMock = vi.mocked(contaEstadoLib);
const loginMock = vi.mocked(loginLib);

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

describe('ContasSettings — o botão Entrar (Task 7)', () => {
  it('mostra os quatro passos do mock estado 2 quando começa o login', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(loginMock.iniciarLogin).toHaveBeenCalledWith('testes');
    expect(t.el.querySelector('.ct-login')).not.toBeNull();
    const passos = [...t.el.querySelectorAll<HTMLElement>('.ct-passo')];
    expect(passos.length).toBe(4);
    expect(passos[0].textContent).toContain(m.contas_passo1());
    expect(passos[1].textContent).toContain(m.contas_passo2());
    expect(passos[2].textContent).toContain(m.contas_passo3());
    expect(passos[3].textContent).toContain(m.contas_passo4());
    // O link de autorização (quando o poll devolve a URL) é um <a> de verdade.
    const link = t.el.querySelector<HTMLAnchorElement>('.ct-link')!;
    expect(link).not.toBeNull();
    expect(link.getAttribute('href')).toContain('https://claude.com/cai/oauth/authorize');
    unmount(t.comp);
  });

  it('confirmar o código chama confirmarLogin e recarrega a lista', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const input = t.el.querySelector<HTMLInputElement>('.ct-campo')!;
    input.value = 'CODE-123';
    input.dispatchEvent(new Event('input'));
    await tick();
    // O botão Confirmar código fica no SEGUNDO .ct-rodape (o primeiro é o da lista).
    const rodapeLogin = [...t.el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('.ct-btn.primario')!.click();
    await tick(); await tick();
    expect(loginMock.confirmarLogin).toHaveBeenCalledWith('testes', 'CODE-123');
    // O pós-login recarrega a lista (o poll do passo não a recarrega — só o /passo).
    expect(estadoMock.listarEstadosDeConta.mock.calls.length).toBe(2);
    unmount(t.comp);
  });

  it('confirmar com campo vazio não chama a API', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const confirmar = t.el.querySelector<HTMLButtonElement>('.ct-rodape .ct-btn.primario')!;
    expect(confirmar.disabled).toBe(true);
    expect(loginMock.confirmarLogin).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('cancelar chama cancelarLogin, limpa o painel e reabilita o Entrar', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    // O rodapé do login é o SEGUNDO .ct-rodape (o primeiro é o rodapé da lista, "+ Nova conta").
    const rodapeLogin = [...t.el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('button:not(.primario)')!.click();
    await tick(); await tick();
    expect(loginMock.cancelarLogin).toHaveBeenCalledWith('testes');
    expect(t.el.querySelector('.ct-login')).toBeNull();
    expect(t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.disabled).toBe(false);
    unmount(t.comp);
  });

  it('erro do servidor (409 envelope) vira mensagem traduzida no aviso', async () => {
    // O backend manda {code, params, msg} (mensagens.py); o cliente deve devolver a MENSAGEM
    // traduzida pelo id, nunca o texto cru do servidor.
    loginMock.iniciarLogin.mockRejectedValueOnce(
      Object.assign(new Error(mensagemDeErro('erro_login_ja_em_curso')!), { status: 409 }));
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(t.el.querySelector('.ct-login')).toBeNull();
    expect(t.el.querySelector<HTMLElement>('.ct-aviso.erro')!.textContent)
      .toContain(m.erro_login_ja_em_curso());
    unmount(t.comp);
  });

  it('erro de rede nao aparece como Failed to fetch cru', async () => {
    loginMock.iniciarLogin.mockRejectedValueOnce(new Error('Failed to fetch'));
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const aviso = t.el.querySelector<HTMLElement>('.ct-aviso.erro')!;
    expect(aviso.textContent).toContain(m.falha_conexao());
    expect(aviso.textContent).not.toContain('Failed to fetch');
    unmount(t.comp);
  });
});

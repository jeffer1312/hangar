// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import AdicionarMaquina from './AdicionarMaquina.svelte';
import * as m from '../../paraglide/messages';

const getConfigForServer = vi.fn();
// (...args) explícito: addServer real recebe (baseUrl, token) — sem isso o TS infere um mock
// de zero parâmetros e o wrapper `(...a) => addServer(...a)` do vi.mock abaixo não compila
// ("spread argument must ... be passed to a rest parameter").
const addServer = vi.fn((..._args: unknown[]) => ({ id: 'srv-n', existed: false }));
vi.mock('../../lib/api', () => ({ getConfigForServer: (...a: unknown[]) => getConfigForServer(...a) }));
vi.mock('../../lib/auth', () => ({ addServer: (...a: unknown[]) => addServer(...a) }));
vi.mock('../QrScanner.svelte', () => ({ default: () => {} }));

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const onFechar = vi.fn();
  const comp = mount(AdicionarMaquina, { target: el, props: { onFechar } });
  const campo = (rot: string) => document.body.querySelector<HTMLInputElement>(`input[aria-label="${rot}"]`)!;
  const botao = (rot: string) => [...document.body.querySelectorAll('button')].find((b) => b.textContent?.trim() === rot)!;
  return { el, comp, onFechar, campo, botao };
}
function digitar(el: HTMLInputElement, v: string) { el.value = v; el.dispatchEvent(new Event('input', { bubbles: true })); }

beforeEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = '';
  Object.defineProperty(window, 'location', { value: { reload: vi.fn() }, writable: true });
});

describe('AdicionarMaquina', () => {
  it('link inteiro colado no endereço separa o token para o campo dele', async () => {
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), 'http://192.168.0.10:8765/?token=abc');
    t.campo(m.maquinas_add_endereco()).dispatchEvent(new Event('blur'));
    await tick();
    expect(t.campo(m.maquinas_add_endereco()).value).toBe('http://192.168.0.10:8765');
    expect(t.campo(m.sessao_token()).value).toBe('abc');
    unmount(t.comp);
  });

  it('testa no endereço normalizado e só grava depois de responder', async () => {
    getConfigForServer.mockResolvedValue({ campos: {}, somente_leitura: {} });
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    t.botao(m.maquinas_add_testar()).click();
    await tick(); await tick();
    expect(getConfigForServer).toHaveBeenCalledWith(expect.objectContaining({ baseUrl: 'http://192.168.0.10:8765', token: 'abc' }));
    expect(addServer).toHaveBeenCalledWith('http://192.168.0.10:8765', 'abc');
    expect(window.location.reload).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('servidor que não responde: erro visível, nada gravado, diálogo aberto', async () => {
    getConfigForServer.mockRejectedValue(new Error('401: token'));
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), 'casa.ts.net');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    t.botao(m.maquinas_add_testar()).click();
    // 'casa.ts.net' tem alternativa (é FQDN, mesma dedução da porta padrão do teste "nome com
    // ponto"): duas chamadas em sequência, mesmos 3 ticks daquele teste (2 awaits + o flush do
    // erro) — 2 ticks não bastam pra estabilizar aqui.
    await tick(); await tick(); await tick();
    expect(addServer).not.toHaveBeenCalled();
    expect(document.body.querySelector('[role="alert"]')?.textContent).toContain('401');
    expect(t.onFechar).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('sem token não testa e diz o que falta', async () => {
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    await tick();
    t.botao(m.maquinas_add_testar()).click();
    await tick();
    expect(getConfigForServer).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain(m.maquinas_add_erro_token());
    unmount(t.comp);
  });

  it('nome com ponto: https falhou, tenta http na porta padrão e grava o que respondeu', async () => {
    getConfigForServer
      .mockRejectedValueOnce(new Error('fetch failed'))
      .mockResolvedValueOnce({ campos: {}, somente_leitura: {} });
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), 'notebook.casa.lan');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    t.botao(m.maquinas_add_testar()).click();
    await tick(); await tick(); await tick();
    expect(getConfigForServer).toHaveBeenCalledTimes(2);
    expect(getConfigForServer.mock.calls[0][0]).toMatchObject({ baseUrl: 'https://notebook.casa.lan' });
    expect(getConfigForServer.mock.calls[1][0]).toMatchObject({ baseUrl: 'http://notebook.casa.lan:8765' });
    expect(addServer).toHaveBeenCalledWith('http://notebook.casa.lan:8765', 'abc');
    unmount(t.comp);
  });

  it('fechar durante o teste é recusado: o erro tardio não morre num componente desmontado', async () => {
    let rejeitar!: (e: Error) => void;
    getConfigForServer.mockReturnValue(new Promise((_, rej) => { rejeitar = rej; }));
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    t.botao(m.maquinas_add_testar()).click();
    await tick();
    document.body.querySelector('[role="dialog"]')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await tick();
    expect(t.onFechar).not.toHaveBeenCalled();
    rejeitar(new Error('401: token'));
    await tick(); await tick();
    expect(document.body.querySelector('[role="alert"]')?.textContent).toContain('401');
    unmount(t.comp);
  });
});

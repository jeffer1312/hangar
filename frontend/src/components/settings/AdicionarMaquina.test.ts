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
const getIdentificador = vi.fn();
const registrarPeerDoisLados = vi.fn();
vi.mock('../../lib/api', () => ({ getConfigForServer: (...a: unknown[]) => getConfigForServer(...a) }));
vi.mock('../../lib/auth', () => ({ addServer: (...a: unknown[]) => addServer(...a) }));
vi.mock('../QrScanner.svelte', () => ({ default: () => {} }));
vi.mock('../../lib/peers', () => ({ getIdentificador: (...a: unknown[]) => getIdentificador(...a) }));
vi.mock('../../lib/registrarPeerDoisLados', () => ({ registrarPeerDoisLados: (...a: unknown[]) => registrarPeerDoisLados(...a) }));

function montar(props: Record<string, unknown> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const onFechar = vi.fn();
  const comp = mount(AdicionarMaquina, { target: el, props: { onFechar, ...props } });
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

  it('resposta HTTP não tenta a alternativa: erro é o da 1ª chamada, sem 2ª tentativa', async () => {
    // 'casa.ts.net' tem alternativa (é FQDN — mesma dedução do teste "nome com ponto"), mas quem
    // respondeu 401 já é um servidor de verdade: tentar a alternativa trocaria essa mensagem por
    // um "fetch failed" da porta que ninguém abriu.
    getConfigForServer.mockRejectedValue(new Error('401: token'));
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), 'casa.ts.net');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    t.botao(m.maquinas_add_testar()).click();
    await tick(); await tick();
    expect(getConfigForServer).toHaveBeenCalledTimes(1);
    expect(addServer).not.toHaveBeenCalled();
    expect(document.body.querySelector('[role="alert"]')?.textContent).toContain('401');
    expect(t.onFechar).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('as duas tentativas falham por rede: a mensagem junta as duas origens', async () => {
    getConfigForServer
      .mockRejectedValueOnce(new Error('fetch failed'))
      .mockRejectedValueOnce(new Error('ECONNREFUSED'));
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), 'casa.ts.net');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    t.botao(m.maquinas_add_testar()).click();
    await tick(); await tick(); await tick();
    expect(getConfigForServer).toHaveBeenCalledTimes(2);
    const alerta = document.body.querySelector('[role="alert"]')?.textContent ?? '';
    expect(alerta).toContain('fetch failed');
    expect(alerta).toContain('ECONNREFUSED');
    expect(addServer).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('link colado com token quebrado mostra erro de token, não de endereço', async () => {
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), 'http://192.168.0.10:8765/?token=');
    await tick();
    t.botao(m.maquinas_add_testar()).click();
    await tick();
    expect(getConfigForServer).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain(m.maquinas_add_erro_token());
    unmount(t.comp);
  });

  it('guard de duplo envio: clique repetido e Enter no endereço não abrem 2ª chamada', async () => {
    getConfigForServer.mockReturnValue(new Promise(() => {}));
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    const botaoTestar = t.botao(m.maquinas_add_testar());
    botaoTestar.click();
    await tick();
    botaoTestar.click();
    t.campo(m.maquinas_add_endereco()).dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(getConfigForServer).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('retry: novo clique tenta de novo, e digitar limpa o erro anterior', async () => {
    getConfigForServer
      .mockRejectedValueOnce(new Error('401: token'))
      .mockResolvedValueOnce({ campos: {}, somente_leitura: {} });
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    const botaoTestar = t.botao(m.maquinas_add_testar());
    botaoTestar.click();
    await tick(); await tick();
    expect(document.body.querySelector('[role="alert"]')?.textContent).toContain('401');
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    await tick();
    expect(document.body.querySelector('[role="alert"]')).toBeNull();
    botaoTestar.click();
    await tick(); await tick();
    expect(getConfigForServer).toHaveBeenCalledTimes(2);
    expect(addServer).toHaveBeenCalledWith('http://192.168.0.10:8765', 'abc');
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

describe('AdicionarMaquina — servidores se falam', () => {
  const ALVO = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };

  it('com a caixa marcada, registra o peer nas duas pontas antes de gravar no navegador', async () => {
    getConfigForServer.mockResolvedValue({ campos: {}, somente_leitura: {} });
    getIdentificador.mockResolvedValue({ identificador: 'notebook' });
    registrarPeerDoisLados.mockResolvedValue({ ok: true, id: 'notebook', base_url: 'http://192.168.0.10:8765', lados: [] });
    const t = montar({ apiTarget: ALVO, podeFalar: true });
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    const caixa = document.body.querySelector<HTMLInputElement>('input.am-falar')!;
    expect(caixa.checked).toBe(false);   // nasce desmarcada: é uma pergunta, não um pressuposto
    caixa.click();
    t.botao(m.maquinas_add_testar()).click();
    await tick(); await tick(); await tick(); await tick();
    expect(getIdentificador).toHaveBeenCalledWith(expect.objectContaining({ baseUrl: 'http://192.168.0.10:8765', token: 'abc' }));
    expect(registrarPeerDoisLados).toHaveBeenCalledWith(ALVO, { id: 'notebook', base_url: 'http://192.168.0.10:8765', token: 'abc' });
    expect(addServer).toHaveBeenCalledWith('http://192.168.0.10:8765', 'abc');
    unmount(t.comp);
  });

  it('caixa desmarcada (o padrão): grava no navegador sem registrar peer', async () => {
    getConfigForServer.mockResolvedValue({ campos: {}, somente_leitura: {} });
    const t = montar({ apiTarget: ALVO, podeFalar: true });
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    t.botao(m.maquinas_add_testar()).click();
    await tick(); await tick(); await tick();
    expect(registrarPeerDoisLados).not.toHaveBeenCalled();
    expect(addServer).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('sem podeFalar a caixa não existe e nada de peer acontece', async () => {
    getConfigForServer.mockResolvedValue({ campos: {}, somente_leitura: {} });
    const t = montar();
    expect(document.body.querySelector('input.am-falar')).toBeNull();
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    t.botao(m.maquinas_add_testar()).click();
    await tick(); await tick(); await tick();
    expect(getIdentificador).not.toHaveBeenCalled();
    expect(addServer).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('registro do peer falhando não impede gravar no navegador', async () => {
    getConfigForServer.mockResolvedValue({ campos: {}, somente_leitura: {} });
    getIdentificador.mockResolvedValue({ identificador: 'notebook' });
    registrarPeerDoisLados.mockRejectedValue(new Error('500: x'));
    const t = montar({ apiTarget: ALVO, podeFalar: true });
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    document.body.querySelector<HTMLInputElement>('input.am-falar')!.click();
    t.botao(m.maquinas_add_testar()).click();
    await tick(); await tick(); await tick(); await tick();
    expect(addServer).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('enderecoInicial preenche o campo', () => {
    const t = montar({ enderecoInicial: 'https://vps.exemplo.com' });
    expect(t.campo(m.maquinas_add_endereco()).value).toBe('https://vps.exemplo.com');
    unmount(t.comp);
  });
});

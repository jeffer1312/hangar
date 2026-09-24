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
const updateServer = vi.fn((..._args: unknown[]) => true);
const renameServer = vi.fn((..._args: unknown[]) => true);
const setServerDisabled = vi.fn((..._args: unknown[]) => true);
const getIdentificador = vi.fn();
const registrarPeerDoisLados = vi.fn();
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()), getConfigForServer: (...a: unknown[]) => getConfigForServer(...a) }));
vi.mock('../../lib/auth', () => ({
  addServer: (...a: unknown[]) => addServer(...a),
  updateServer: (...a: unknown[]) => updateServer(...a),
  renameServer: (...a: unknown[]) => renameServer(...a),
  setServerDisabled: (...a: unknown[]) => setServerDisabled(...a),
}));
vi.mock('../QrScanner.svelte', () => ({ default: () => {} }));
vi.mock('../../lib/peers', () => ({ getIdentificador: (...a: unknown[]) => getIdentificador(...a) }));
vi.mock('../../lib/registrarPeerDoisLados', () => ({ registrarPeerDoisLados: (...a: unknown[]) => registrarPeerDoisLados(...a) }));

function montar(props: Record<string, unknown> = {}) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const onFechar = vi.fn();
  const onAdicionada = vi.fn();
  const comp = mount(AdicionarMaquina, { target: el, props: { onFechar, onAdicionada, ...props } });
  const campo = (rot: string) => document.body.querySelector<HTMLInputElement>(`input[aria-label="${rot}"]`)!;
  const botao = (rot: string) => [...document.body.querySelectorAll('button')].find((b) => b.textContent?.trim() === rot)!;
  return { el, comp, onFechar, onAdicionada, campo, botao };
}
function digitar(el: HTMLInputElement, v: string) { el.value = v; el.dispatchEvent(new Event('input', { bubbles: true })); }
const esperar = async (n = 4) => { for (let i = 0; i < n; i++) await tick(); };

// Passo 1 inteiro: preenche, testa e espera o "Respondeu".
async function testarAte(t: ReturnType<typeof montar>, endereco = '192.168.0.10', token = 'abc') {
  digitar(t.campo(m.maquinas_add_endereco()), endereco);
  digitar(t.campo(m.sessao_token()), token);
  await tick();
  t.botao(m.servidores_add_testar()).click();
  await esperar();
}

beforeEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = '';
  getConfigForServer.mockResolvedValue({ campos: {}, somente_leitura: {} });
  getIdentificador.mockResolvedValue({ identificador: '' });
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

  it('Testar mede o endereço normalizado e lê quem é; só Adicionar grava, com o nome sugerido', async () => {
    getIdentificador.mockResolvedValue({ identificador: 'notebook' });
    const t = montar();
    await testarAte(t);
    expect(getConfigForServer).toHaveBeenCalledWith(expect.objectContaining({ baseUrl: 'http://192.168.0.10:8765', token: 'abc' }), 20000);
    expect(getIdentificador).toHaveBeenCalledWith(expect.objectContaining({ baseUrl: 'http://192.168.0.10:8765', token: 'abc' }));
    expect(addServer).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain(m.servidores_add_respondeu({ id: 'notebook' }));
    expect(t.campo(m.servidores_add_nome()).value).toBe('notebook');
    expect(t.botao(m.servidores_add_testar())).toBeUndefined();
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(addServer).toHaveBeenCalledWith('http://192.168.0.10:8765', 'abc', 'notebook', { ativar: false });
    expect(t.onAdicionada).toHaveBeenCalled();
    expect(t.onFechar).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('o nome é editável antes de adicionar; sem identificador vem do endereço', async () => {
    const t = montar();
    await testarAte(t, 'https://mac.ts.net');
    expect(document.body.textContent).toContain(m.servidores_add_respondeu_sem_id());
    expect(t.campo(m.servidores_add_nome()).value).toBe('mac');
    digitar(t.campo(m.servidores_add_nome()), 'Meu Mac');
    await tick();
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(addServer).toHaveBeenCalledWith('https://mac.ts.net', 'abc', 'Meu Mac', { ativar: false });
    unmount(t.comp);
  });

  it('falha ao ler o identificador aparece, e ainda dá para adicionar', async () => {
    getIdentificador.mockRejectedValue(new Error('500: x'));
    const t = montar();
    await testarAte(t, 'https://mac.ts.net');
    expect(document.body.textContent).toContain(m.servidores_add_id_falhou({ erro: '500: x' }));
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(addServer).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('mexer no endereço ou no token volta ao passo de testar', async () => {
    const t = montar();
    await testarAte(t);
    expect(t.botao(m.servidores_add_adicionar())).toBeDefined();
    digitar(t.campo(m.sessao_token()), 'outro');
    await tick();
    expect(t.botao(m.servidores_add_adicionar())).toBeUndefined();
    expect(t.campo(m.servidores_add_nome())).toBeNull();
    t.botao(m.servidores_add_testar()).click();
    await esperar();
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.11');
    await tick();
    expect(t.botao(m.servidores_add_testar())).toBeDefined();
    expect(addServer).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('máquina que este aparelho já tem: "Usar este endereço" troca o dela, sem linha nova', async () => {
    getIdentificador.mockResolvedValue({ identificador: 'notebook' });
    const antigo = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://velho', token: 'tv', disabled: true };
    const t = montar({ conhecidas: [{ identificador: 'notebook', server: antigo }], podeFalar: true });
    await testarAte(t);
    expect(document.body.textContent).toContain(m.servidores_add_repetida({ nome: 'Notebook', endereco: 'http://velho' }));
    expect(t.campo(m.servidores_add_nome()).value).toBe('Notebook');
    // repetida não oferece as caixas: a entrada que existe já decide
    expect(document.body.querySelector('input.am-falar, input.am-acompanhar')).toBeNull();
    t.botao(m.servidores_add_usar_endereco()).click();
    await esperar();
    expect(updateServer).toHaveBeenCalledWith('srv-b', { baseUrl: 'http://192.168.0.10:8765', token: 'abc' });
    expect(setServerDisabled).toHaveBeenCalledWith('srv-b', false);
    expect(renameServer).not.toHaveBeenCalled();
    expect(addServer).not.toHaveBeenCalled();
    expect(t.onFechar).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('repetida com outro nome digitado renomeia; ligada não mexe no disabled', async () => {
    getIdentificador.mockResolvedValue({ identificador: 'notebook' });
    const antigo = { id: 'srv-b', label: 'Notebook', baseUrl: 'http://velho', token: 'tv' };
    const t = montar({ conhecidas: [{ identificador: 'notebook', server: antigo }] });
    await testarAte(t);
    digitar(t.campo(m.servidores_add_nome()), 'NB');
    await tick();
    t.botao(m.servidores_add_usar_endereco()).click();
    await esperar();
    expect(renameServer).toHaveBeenCalledWith('srv-b', 'NB');
    expect(setServerDisabled).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('título e faixa dizem onde grava: só este aparelho, ou a lista sincronizada', () => {
    const t = montar({ esteNome: 'Casa' });
    expect(document.body.textContent).toContain(m.servidores_adicionar_aparelho());
    expect(document.body.textContent).toContain(m.servidores_add_faixa({ este: 'Casa' }));
    unmount(t.comp);
    document.body.innerHTML = '';
    const t2 = montar({ sincronizada: true });
    expect(document.body.textContent).toContain(m.servidores_adicionar_lista());
    expect(document.body.textContent).toContain(m.servidores_add_faixa_sync());
    unmount(t2.comp);
  });

  it('resposta HTTP não tenta a alternativa: erro é o da 1ª chamada, sem 2ª tentativa', async () => {
    // 'casa.ts.net' tem alternativa (é FQDN — mesma dedução do teste "nome com ponto"), mas quem
    // respondeu 401 já é um servidor de verdade: tentar a alternativa trocaria essa mensagem por
    // um "fetch failed" da porta que ninguém abriu.
    getConfigForServer.mockRejectedValue(new Error('401: token'));
    const t = montar();
    await testarAte(t, 'casa.ts.net');
    expect(getConfigForServer).toHaveBeenCalledTimes(1);
    expect(getIdentificador).not.toHaveBeenCalled();
    expect(document.body.querySelector('[role="alert"]')?.textContent).toContain('401');
    expect(t.botao(m.servidores_add_adicionar())).toBeUndefined();
    expect(t.onFechar).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('as duas tentativas falham por rede: a mensagem junta as duas origens', async () => {
    getConfigForServer
      .mockRejectedValueOnce(new Error('fetch failed'))
      .mockRejectedValueOnce(new Error('ECONNREFUSED'));
    const t = montar();
    await testarAte(t, 'casa.ts.net');
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
    t.botao(m.servidores_add_testar()).click();
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
    const botaoTestar = t.botao(m.servidores_add_testar());
    botaoTestar.click();
    await tick();
    botaoTestar.click();
    t.campo(m.maquinas_add_endereco()).dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await tick();
    expect(getConfigForServer).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('Enter testa no passo 1 e adiciona no passo 2', async () => {
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    t.campo(m.sessao_token()).dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await esperar();
    expect(getConfigForServer).toHaveBeenCalledTimes(1);
    expect(addServer).not.toHaveBeenCalled();
    t.campo(m.sessao_token()).dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await esperar();
    expect(getConfigForServer).toHaveBeenCalledTimes(1);
    expect(addServer).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('retry: novo clique tenta de novo, e digitar limpa o erro anterior', async () => {
    getConfigForServer
      .mockRejectedValueOnce(new Error('401: token'))
      .mockResolvedValueOnce({ campos: {}, somente_leitura: {} });
    const t = montar();
    await testarAte(t);
    expect(document.body.querySelector('[role="alert"]')?.textContent).toContain('401');
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    await tick();
    expect(document.body.querySelector('[role="alert"]')).toBeNull();
    t.botao(m.servidores_add_testar()).click();
    await esperar();
    expect(getConfigForServer).toHaveBeenCalledTimes(2);
    expect(t.botao(m.servidores_add_adicionar())).toBeDefined();
    unmount(t.comp);
  });

  it('sem token não testa e diz o que falta', async () => {
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    await tick();
    t.botao(m.servidores_add_testar()).click();
    await tick();
    expect(getConfigForServer).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain(m.maquinas_add_erro_token());
    unmount(t.comp);
  });

  it('nome com ponto: https falhou, tenta http na porta padrão e grava o que respondeu', async () => {
    getConfigForServer
      .mockRejectedValueOnce(new Error('fetch failed'))
      .mockResolvedValueOnce({ campos: {}, somente_leitura: {} });
    getIdentificador.mockResolvedValue({ identificador: 'notebook' });
    const t = montar();
    await testarAte(t, 'notebook.casa.lan');
    expect(getConfigForServer).toHaveBeenCalledTimes(2);
    expect(getConfigForServer.mock.calls[0][0]).toMatchObject({ baseUrl: 'https://notebook.casa.lan' });
    expect(getConfigForServer.mock.calls[1][0]).toMatchObject({ baseUrl: 'http://notebook.casa.lan:8765' });
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(addServer).toHaveBeenCalledWith('http://notebook.casa.lan:8765', 'abc', 'notebook', { ativar: false });
    unmount(t.comp);
  });

  it('fechar durante o teste é recusado: o erro tardio não morre num componente desmontado', async () => {
    let rejeitar!: (e: Error) => void;
    getConfigForServer.mockReturnValue(new Promise((_, rej) => { rejeitar = rej; }));
    const t = montar();
    digitar(t.campo(m.maquinas_add_endereco()), '192.168.0.10');
    digitar(t.campo(m.sessao_token()), 'abc');
    await tick();
    t.botao(m.servidores_add_testar()).click();
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
    getIdentificador.mockResolvedValue({ identificador: 'notebook' });
    registrarPeerDoisLados.mockResolvedValue({ ok: true, id: 'notebook', base_url: 'http://192.168.0.10:8765', lados: [] });
    const t = montar({ apiTarget: ALVO, podeFalar: true, esteNome: 'Casa' });
    const caixa = document.body.querySelector<HTMLInputElement>('input.am-falar')!;
    expect(caixa.checked).toBe(false);   // nasce desmarcada: é uma pergunta, não um pressuposto
    expect(document.body.textContent).toContain(m.servidores_rec_titulo({ este: 'Casa', nome: m.servidores_esta_maquina() }));
    caixa.click();
    await testarAte(t);
    // depois do teste o rótulo já diz o nome dela
    expect(document.body.textContent).toContain(m.servidores_rec_titulo({ este: 'Casa', nome: 'notebook' }));
    expect(registrarPeerDoisLados).not.toHaveBeenCalled();
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(registrarPeerDoisLados).toHaveBeenCalledWith(ALVO, { id: 'notebook', base_url: 'http://192.168.0.10:8765', token: 'abc' });
    expect(addServer).toHaveBeenCalledWith('http://192.168.0.10:8765', 'abc', 'notebook', { ativar: false });
    unmount(t.comp);
  });

  it('caixa desmarcada (o padrão): grava no navegador sem registrar peer', async () => {
    const t = montar({ apiTarget: ALVO, podeFalar: true });
    await testarAte(t);
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(registrarPeerDoisLados).not.toHaveBeenCalled();
    expect(addServer).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('sem podeFalar a caixa não existe e nada de peer acontece', async () => {
    const t = montar();
    expect(document.body.querySelector('input.am-falar')).toBeNull();
    await testarAte(t);
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(registrarPeerDoisLados).not.toHaveBeenCalled();
    expect(addServer).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('registro do peer falhando não impede gravar no navegador', async () => {
    getIdentificador.mockResolvedValue({ identificador: 'notebook' });
    registrarPeerDoisLados.mockRejectedValue(new Error('500: x'));
    const t = montar({ apiTarget: ALVO, podeFalar: true });
    document.body.querySelector<HTMLInputElement>('input.am-falar')!.click();
    await testarAte(t);
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(addServer).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('só recado: registra o peer, não grava no navegador e fecha', async () => {
    getIdentificador.mockResolvedValue({ identificador: 'notebook' });
    registrarPeerDoisLados.mockResolvedValue({ ok: true, id: 'notebook', base_url: 'http://192.168.0.10:8765', lados: [], meu_endereco: '' });
    const t = montar({ apiTarget: ALVO, podeFalar: true });
    document.body.querySelector<HTMLInputElement>('input.am-acompanhar')!.click();
    document.body.querySelector<HTMLInputElement>('input.am-falar')!.click();
    await testarAte(t);
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(registrarPeerDoisLados).toHaveBeenCalled();
    expect(addServer).not.toHaveBeenCalled();
    expect(t.onFechar).toHaveBeenCalled();
    unmount(t.comp);
  });

  it('só recado: falha no registro aparece como erro em vez de fechar', async () => {
    getIdentificador.mockResolvedValue({ identificador: 'notebook' });
    registrarPeerDoisLados.mockRejectedValue(new Error('500: x'));
    const t = montar({ apiTarget: ALVO, podeFalar: true });
    document.body.querySelector<HTMLInputElement>('input.am-acompanhar')!.click();
    document.body.querySelector<HTMLInputElement>('input.am-falar')!.click();
    await testarAte(t);
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(document.body.querySelector('#am-erro')?.textContent).toContain('500: x');
    expect(addServer).not.toHaveBeenCalled();
    expect(t.onFechar).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('só recado sem identificador dela: erro próprio, nada gravado', async () => {
    const t = montar({ apiTarget: ALVO, podeFalar: true });
    document.body.querySelector<HTMLInputElement>('input.am-acompanhar')!.click();
    document.body.querySelector<HTMLInputElement>('input.am-falar')!.click();
    await testarAte(t);
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(document.body.querySelector('#am-erro')?.textContent).toBe(m.maquinas_add_erro_sem_identificador());
    expect(registrarPeerDoisLados).not.toHaveBeenCalled();
    expect(addServer).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('as duas caixas desligadas recusam ao adicionar, sem gravar nada', async () => {
    const t = montar({ apiTarget: ALVO, podeFalar: true });
    document.body.querySelector<HTMLInputElement>('input.am-acompanhar')!.click();
    await testarAte(t);
    t.botao(m.servidores_add_adicionar()).click();
    await esperar();
    expect(document.body.querySelector('#am-erro')?.textContent).toBe(m.maquinas_add_erro_nenhum());
    expect(addServer).not.toHaveBeenCalled();
    expect(registrarPeerDoisLados).not.toHaveBeenCalled();
    unmount(t.comp);
  });
});

describe('AdicionarMaquina — busca no Tailscale', () => {
  const busca = (over: Record<string, unknown> = {}) => ({ itens: null, buscando: false, erro: '', podeBuscar: true, onBuscar: vi.fn(), ...over });

  it('só busca quando pedem, e nunca sem servidor escolhido', () => {
    const b = busca();
    const t = montar({ busca: b });
    t.botao(m.maquinas_buscar_tailscale()).click();
    expect(b.onBuscar).toHaveBeenCalledTimes(1);
    unmount(t.comp);
    const t2 = montar({ busca: busca({ podeBuscar: false }) });
    expect(t2.botao(m.maquinas_buscar_tailscale()).disabled).toBe(true);
    expect(document.body.textContent).toContain(m.maquinas_buscar_sem_maquina());
    unmount(t2.comp);
  });

  it('achado preenche o endereço e deixa só o token; busca vazia diz que não achou', async () => {
    const t = montar({ busca: busca({ itens: [{ nome: 'mac', base_url: 'https://mac.ts.net', hosts: [] }] }) });
    t.botao(m.maquinas_adicionar()).click();
    await tick();
    expect(t.campo(m.maquinas_add_endereco()).value).toBe('https://mac.ts.net');
    unmount(t.comp);
    const t2 = montar({ busca: busca({ itens: [] }) });
    expect(document.body.textContent).toContain(m.maquinas_buscar_nada());
    unmount(t2.comp);
  });
});

// @vitest-environment happy-dom
// Round 1 da 4b: a tela Servidores NÃO chama store.carregar (zero GET /api/config) — o controller
// da tela é o ServidoresSettings; as outras telas seguem carregando o config do alvo.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import SettingsModal from './SettingsModal.svelte';
import * as api from '../../lib/api';
import * as auth from '../../lib/auth';
import * as m from '../../paraglide/messages';
import type { Server } from '../../lib/auth';
import type { TelaConfig } from '../../lib/configRoute';

vi.mock('../../lib/api', () => ({
  getConfig: vi.fn(),
  getConfigForServer: vi.fn(),
  patchConfig: vi.fn(),
  patchConfigForServer: vi.fn(),
  getPushSettings: vi.fn(),
  getPushSettingsForServer: vi.fn(),
  setQuietHours: vi.fn(),
  setQuietHoursForServer: vi.fn(),
  criarConta: vi.fn(async () => ({ path: '/x', label: 'x', active: false })),
  apagarConta: vi.fn(async () => {}),
}));
// A aba Contas busca a lista única /api/credenciais — sem este mock, montar a aba
// faria fetch real no teste. formatarIntervalo segue real (puro; mantido via importOriginal).
vi.mock('../../lib/contaEstado', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/contaEstado')>();
  return { ...real, listarEstadosDeConta: vi.fn(async () => []) };
});
vi.mock('../../lib/credenciais', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/credenciais')>();
  return { ...real, listarCredenciais: vi.fn(async () => []), definirApelido: vi.fn() };
});
vi.mock('../../lib/auth', () => ({
  serverColor: () => '#fff',
  serverIdentidade: vi.fn(() => 'global'),
  listServers: vi.fn(),
  getActiveId: vi.fn(),
  selectServer: vi.fn(),
  renameServer: vi.fn(),
  updateServer: vi.fn(() => true),
  removeServer: vi.fn(),
  addServer: vi.fn(),
  validarPareamento: vi.fn(),
  clearCredentials: vi.fn(),
  onServersChanged: vi.fn(() => () => {}),
}));
vi.mock('../../lib/sessionsStore.svelte', () => ({
  sessionsStore: { refreshServers: vi.fn(), reconnect: vi.fn() },
}));
vi.mock('../../lib/push', () => ({ enablePush: vi.fn(), pushSupported: () => true }));

const apiMock = vi.mocked(api);
const authMock = vi.mocked(auth);
const SRV = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' };

beforeEach(() => { vi.clearAllMocks(); });   // contagens de chamada não vazam entre testes

// Os stubs de matchMedia são globais (desktop/mobile por teste); sem restaurar, o último stub
// vazaria pra qualquer describe futuro adicionado depois (achado da revisão).
const matchMediaOriginal = window.matchMedia;
afterEach(() => { window.matchMedia = matchMediaOriginal; });

function montar(tela: TelaConfig, alvo: Server | null = null, identidade = alvo ? `id:${alvo.id}` : 'global',
                extra: { servidores?: Server[]; resolvedServer?: Server | null;
                         nomeAlvo?: string | null; onPickServer?: (id: string) => void } = {}) {
  authMock.listServers.mockReturnValue((extra.servidores ?? [SRV]) as never);
  authMock.getActiveId.mockReturnValue(SRV.id);
  apiMock.getConfig.mockResolvedValue({ campos: {}, somente_leitura: {} } as never);
  apiMock.getConfigForServer.mockResolvedValue({ campos: {}, somente_leitura: {} } as never);
  apiMock.getPushSettings.mockReturnValue(new Promise(() => {}));
  const el = document.createElement('div');
  document.body.appendChild(el);
  const onIrPara = vi.fn();
  const comp = mount(SettingsModal, {
    target: el,
    props: {
      tela, alvo, identidade, nomeAlvo: extra.nomeAlvo ?? null, semServidor: !alvo,
      resolvedServer: extra.resolvedServer ?? null, onPickServer: extra.onPickServer,
      onIrPara, onVoltar: vi.fn(), onFechar: vi.fn(),
    },
  });
  return { el, comp: comp as never, onIrPara };
}

// Desktop: o modal vira o split com a navegação lateral (.st-nav). O happy-dom não sabe o que é
// viewport — o stub responde true para o corte de 820px e o componente monta o lado desktop.
function stubDesktop() {
  window.matchMedia = ((query: string) => ({
    get matches() { return true; },
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as never;
}

// O stub é GLOBAL e não se desfaz sozinho: um teste desktop antes de um mobile deixaria o
// matchMedia respondendo true pra sempre (e o móvel montaria o split). O móvel re-stuba false.
function stubMobile() {
  window.matchMedia = ((query: string) => ({
    get matches() { return false; },
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as never;
}

describe('SettingsModal — GET config por tela', () => {
  it('tela servidores: zero GET config', async () => {
    const t = montar('servidores');
    await Promise.resolve();
    expect(apiMock.getConfig).not.toHaveBeenCalled();
    expect(apiMock.getConfigForServer).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('outras telas continuam carregando o config do alvo', async () => {
    const t = montar('anexos', SRV as Server);
    await Promise.resolve();
    expect(apiMock.getConfigForServer).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('desktop: Acesso e Contas aparecem na navegação do grupo do servidor, e clicar em cada uma troca a tela', async () => {
    stubDesktop();
    // Com alvo: semServidor=false — é o estado do mock, com as abas do servidor habilitadas.
    const t = montar('servidores', SRV);
    await tick();
    // O BottomSheet teleporta pro <body> (use:portal) — o conteúdo não fica dentro de t.el.
    const itens = [...document.querySelectorAll<HTMLButtonElement>('.st-nav-item')];
    // textContent traz ícone + rótulo juntos ('📶Access'); o rótulo do paraglide resolve para o
    // baseLocale 'en' no teste (sem setLocale), então o esperado vem de m.*(), nunca literal.
    const rotulos = itens.map((b) => b.textContent?.trim() ?? '');
    const acha = (rot: string) => rotulos.findIndex((r) => r.includes(rot));
    expect(acha(m.acesso_titulo())).toBeGreaterThanOrEqual(0);
    expect(acha(m.contas_titulo())).toBeGreaterThanOrEqual(0);
    // Grupo do servidor: Acesso e Contas vêm ANTES de Servidores (que já era a primeira do grupo).
    const servidores = acha(m.config_modal_servidores());
    expect(servidores).toBeGreaterThanOrEqual(0);
    expect(acha(m.acesso_titulo())).toBeLessThan(servidores);
    expect(acha(m.contas_titulo())).toBeLessThan(servidores);
    // Clicar em cada uma troca a tela (o dono da rota é o App, que recebe o id via onIrPara).
    itens[acha(m.acesso_titulo())].click();
    expect(t.onIrPara).toHaveBeenCalledWith('acesso');
    itens[acha(m.contas_titulo())].click();
    expect(t.onIrPara).toHaveBeenCalledWith('contas');
    unmount(t.comp);
  });

  it('acesso virou tela real: a seção de endereços aparece e o stub sai', async () => {
    // A Task 3 substitui o stub de Acesso pela tela de endereços. O que prova que a
    // substituição aconteceu é o marcador da tela REAL (a seção de endereços) e a
    // AUSÊNCIA do stub — não a tela inteira, que depende do fetch do alvo (pendente
    // no happy-dom, que aborta o fetch no teardown).
    stubDesktop();
    const t = montar('acesso');
    await tick();
    expect(document.body.textContent).toContain(m.acesso_secao_enderecos());
    expect(document.body.textContent).not.toContain(m.comum_em_construcao());
    unmount(t.comp);
    // Contas também virou tela real — a Task 4 mergeou e a prova dela é o teste
    // 'aba Contas mostra a lista da fonte única' logo abaixo.
  });

  it('aba Contas mostra a lista da fonte única (Task 4 ligou a tela)', async () => {
    stubDesktop();
    const lista = [{
      id: 'claude:/home/u/.claude-a', tipo: 'claude', nome: 'a', nome_natural: 'a', ativa: true,
      path: '/home/u/.claude-a', usos: [],
      login: { estado: 'ok', loggedIn: true, email: 'a@example.com', plano: 'max' },
      cota: { estado: 'sem_credencial', janelas: [] },
    }];
    const contaEstado = await import('../../lib/credenciais');
    vi.mocked(contaEstado.listarCredenciais).mockResolvedValue(lista as never);
    const t = montar('contas');
    await tick(); await tick();
    // Sem ?srv= o alvo é null: a aba segue pelo caminho global (self-heal de 401).
    expect(vi.mocked(contaEstado.listarCredenciais)).toHaveBeenCalledWith(null);
    expect(document.body.textContent).toContain(m.contas_secao_lista());
    expect(document.body.textContent).toContain(m.contas_em_uso());
    expect(document.body.textContent).toContain('a@example.com');
    unmount(t.comp);
  });

  it('aba Contas recebe o alvo do ?srv= e lista do servidor escolhido (parecer)', async () => {
    stubDesktop();
    const contaEstado = await import('../../lib/credenciais');
    vi.mocked(contaEstado.listarCredenciais).mockResolvedValue([] as never);
    const t = montar('contas', SRV as Server);
    await tick(); await tick();
    // O SettingsModal repassa o alvo que o App resolveu (targetConfig): com ?srv=B a lista
    // tem de sair para B — é o defeito do bloqueador da revisão final.
    expect(vi.mocked(contaEstado.listarCredenciais)).toHaveBeenCalledWith(SRV);
    unmount(t.comp);
  });

  it('identidade composta chega ao store: mesmo id com identidade diferente dispara o próprio GET', async () => {
    // O store observa a identidade (JSON id+label+baseUrl+token), não o id. Montar o MESMO alvo.id
    // com identidades distintas (base/token rotacionados) precisa gerar UM GET por identidade — se
    // o efeito dependesse só do id, o segundo carregaria do cache errado ou nem carregaria.
    const t1 = montar('anexos', SRV as Server, 'id:srv-a::v1');
    await Promise.resolve();
    expect(apiMock.getConfigForServer).toHaveBeenCalledTimes(1);
    unmount(t1.comp);
    const t2 = montar('anexos', SRV as Server, 'id:srv-a::v2');
    await Promise.resolve();
    expect(apiMock.getConfigForServer).toHaveBeenCalledTimes(2);
    unmount(t2.comp);
  });
});

describe('SettingsModal — seletor de servidor do grupo', () => {
  // Pedido recorrente do usuário (19/08/2026): o rótulo "Servidor · X" dizia o alvo mas não
  // trocava. Com 2+ servidores e a porta onPickServer ligada, o rótulo vira select — e trocar
  // nele permanece NA TELA (o App reabre a tela atual com o ?srv= novo).
  const DOIS = [SRV, { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'y' }] as Server[];

  it('desktop: o rótulo do grupo vira select com o alvo marcado, e trocar chama onPickServer', async () => {
    stubDesktop();
    const onPickServer = vi.fn();
    const t = montar('contas', SRV as Server, 'id:srv-a',
      { servidores: DOIS, resolvedServer: SRV as Server, onPickServer });
    await tick(); await tick();
    // O BottomSheet teleporta pro <body> — o select do grupo do servidor está lá.
    const sel = document.querySelector<HTMLSelectElement>('.st-secao-sel .srv-sel');
    expect(sel).not.toBeNull();
    expect(sel!.value).toBe('srv-a');
    // O rótulo do grupo continua dizendo O QUE é ("Servidor"), o select diz QUAL.
    expect(sel!.parentElement!.textContent).toContain(m.lista_agrupar_servidor());

    sel!.value = 'srv-b';
    sel!.dispatchEvent(new Event('change', { bubbles: true }));
    expect(onPickServer).toHaveBeenCalledWith('srv-b');
    unmount(t.comp);
  });

  it('com UM servidor não há escolha a fazer — fica o rótulo estático de sempre', async () => {
    stubDesktop();
    const t = montar('contas', SRV as Server, 'id:srv-a',
      { servidores: [SRV as Server], resolvedServer: SRV as Server, onPickServer: vi.fn() });
    await tick(); await tick();
    expect(document.querySelector('.srv-sel')).toBeNull();
    expect(document.querySelector('.st-secao-sel')).toBeNull();
    unmount(t.comp);
  });

  it('celular: o "em X" do sub-cabeçalho vira o select nas telas de servidor', async () => {
    stubMobile();
    const onPickServer = vi.fn();
    const t = montar('contas', SRV as Server, 'id:srv-a',
      { servidores: DOIS, resolvedServer: SRV as Server, nomeAlvo: 'A', onPickServer });
    await tick(); await tick();
    const sub = document.querySelector<HTMLElement>('.st-sub');
    expect(sub).not.toBeNull();
    expect(sub!.querySelector('.srv-sel')).not.toBeNull();
    expect((sub!.querySelector('.srv-sel') as HTMLSelectElement).value).toBe('srv-a');
    unmount(t.comp);
  });

  it('celular, raiz: o rótulo do grupo do servidor também vira select', async () => {
    stubMobile();
    const t = montar('root', SRV as Server, 'id:srv-a',
      { servidores: DOIS, resolvedServer: SRV as Server, onPickServer: vi.fn() });
    await tick(); await tick();
    expect(document.querySelector('.st-secao-sel .srv-sel')).not.toBeNull();
    unmount(t.comp);
  });
});

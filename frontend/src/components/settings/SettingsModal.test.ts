// @vitest-environment happy-dom
// Round 1 da 4b: a tela Servidores NÃO chama store.carregar (zero GET /api/config) — o controller
// da tela é o ServidoresSettings; as outras telas seguem carregando o config do alvo.
import { describe, it, expect, vi, beforeEach } from 'vitest';
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
// A aba Contas (Task 4) buscou a fonte única /api/conta-estado — sem este mock, montar a aba
// faria fetch real no teste. formatarIntervalo segue real (puro; mantido via importOriginal).
vi.mock('../../lib/contaEstado', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/contaEstado')>();
  return { ...real, listarEstadosDeConta: vi.fn(async () => []) };
});
vi.mock('../../lib/auth', () => ({
  serverColor: () => '#fff',
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

function montar(tela: TelaConfig, alvo: Server | null = null, identidade = alvo ? `id:${alvo.id}` : 'global') {
  authMock.listServers.mockReturnValue([SRV as never]);
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
      tela, alvo, identidade, nomeAlvo: null, semServidor: !alvo,
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

  it('aba Acesso ainda abre em construção (Task 3 liga nesta árvore — até lá é o placeholder do prefactor)', async () => {
    stubDesktop();
    const t = montar('acesso');
    await tick();
    expect(document.body.textContent).toContain(m.comum_em_construcao());
    unmount(t.comp);
  });

  it('aba Contas mostra a lista da fonte única (Task 4 ligou a tela)', async () => {
    stubDesktop();
    const lista = [{
      path: '/home/u/.claude-a', label: 'a', active: true,
      login: { estado: 'ok', loggedIn: true, email: 'a@example.com', plano: 'max' },
      limite: { estado: 'sem_leitura' },
    }];
    const contaEstado = await import('../../lib/contaEstado');
    vi.mocked(contaEstado.listarEstadosDeConta).mockResolvedValue(lista as never);
    const t = montar('contas');
    await tick(); await tick();
    expect(document.body.textContent).toContain(m.contas_secao_lista());
    expect(document.body.textContent).toContain(m.contas_em_uso());
    expect(document.body.textContent).toContain('a@example.com');
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

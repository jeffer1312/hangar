// @vitest-environment happy-dom
// Round 1 da 4b: a tela Máquinas NÃO chama store.carregar (zero GET /api/config) — o controller
// da tela é o MaquinasSettings; as outras telas seguem carregando o config do alvo.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import SettingsModal from './SettingsModal.svelte';
import * as api from '@hangar/core';
import * as auth from '../../lib/auth';
import * as m from '../../paraglide/messages';
import type { Server } from '../../lib/auth';
import type { TelaConfig } from '../../lib/configRoute';
import { ENTRADAS } from './BuscaConfig.svelte';
import { criarProps } from './props-reativas.svelte';
import { clienteQuery } from '../../lib/queries';

vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
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
  // Montar a aba Contas passa a CHAMAR getEngines pela queryFn (é o que dá modelo ao card da
  // chave); sem estes o vitest acusa "No export is defined on the mock" só na chamada.
  getEngines: vi.fn(async () => ({ motores: {}, arquivo_corrompido: false, arquivo_caminho: '' })),
  getEnginesForServer: vi.fn(async () => ({ motores: {}, arquivo_corrompido: false, arquivo_caminho: '' })),
  engineModelos: vi.fn(async () => ({ modelos: [] })),
  engineModelosForServer: vi.fn(async () => ({ modelos: [] })),
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
vi.mock('../../lib/peers', () => ({
  getIdentificador: vi.fn(async () => ({ identificador: '' })),
  setIdentificador: vi.fn(),
  listarPeers: vi.fn(async () => []),
  gravarPeer: vi.fn(),
  removerPeer: vi.fn(),
  removerPeerDoisLados: vi.fn(),
  checkPeer: vi.fn(async () => ({ estado: 'ok' })),
}));
vi.mock('../../lib/alcance', () => ({
  alcanceDoServidor: vi.fn(() => new Promise(() => {})),
  pareamentoDoServidor: vi.fn(),
  fraseDeEstado: () => '',
}));

const apiMock = vi.mocked(api);
const authMock = vi.mocked(auth);
const SRV = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' };

// Limpa também o cache das queries: ele é um singleton de módulo que sobrevive à desmontagem (é o
// que faz reabrir uma aba ser instantâneo), então sem isto o caso seguinte monta e é servido pelo
// dado do anterior em vez de chamar o próprio mock.
beforeEach(() => { vi.clearAllMocks(); clienteQuery.clear(); });   // contagens de chamada não vazam entre testes

// Os stubs de matchMedia são globais (desktop/mobile por teste); sem restaurar, o último stub
// vazaria pra qualquer describe futuro adicionado depois (achado da revisão).
const matchMediaOriginal = window.matchMedia;
// O BottomSheet teleporta pro <body> e o `el` de cada montagem fica lá: sem limpar, um teste que
// CONTA elementos soma os das montagens anteriores, e um que espera `null` acha o resíduo do
// vizinho. `unmount` tira o componente, não os nós que o teste criou.
afterEach(() => { window.matchMedia = matchMediaOriginal; document.body.innerHTML = ''; });

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
  it('tela maquinas: zero GET config', async () => {
    const t = montar('maquinas');
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

  it('desktop: Máquinas e Contas aparecem na navegação do grupo do servidor, e clicar em cada uma troca a tela', async () => {
    stubDesktop();
    // Com alvo: semServidor=false — é o estado do mock, com as abas do servidor habilitadas.
    const t = montar('maquinas', SRV);
    await tick();
    // O BottomSheet teleporta pro <body> (use:portal) — o conteúdo não fica dentro de t.el.
    const itens = [...document.querySelectorAll<HTMLButtonElement>('.st-nav-item')];
    // textContent traz ícone + rótulo juntos ('📶Access'); o rótulo do paraglide resolve para o
    // baseLocale 'en' no teste (sem setLocale), então o esperado vem de m.*(), nunca literal.
    const rotulos = itens.map((b) => b.textContent?.trim() ?? '');
    const acha = (rot: string) => rotulos.findIndex((r) => r.includes(rot));
    expect(acha(m.maquinas_titulo())).toBeGreaterThanOrEqual(0);
    expect(acha(m.contas_modelos_titulo())).toBeGreaterThanOrEqual(0);
    // Grupo do servidor: Máquinas vem antes de Contas (já era a primeira do grupo).
    expect(acha(m.maquinas_titulo())).toBeLessThan(acha(m.contas_modelos_titulo()));
    // Clicar em cada uma troca a tela (o dono da rota é o App, que recebe o id via onIrPara).
    itens[acha(m.maquinas_titulo())].click();
    expect(t.onIrPara).toHaveBeenCalledWith('maquinas');
    itens[acha(m.contas_modelos_titulo())].click();
    expect(t.onIrPara).toHaveBeenCalledWith('contas');
    unmount(t.comp);
  });

  it('acesso mora dentro de Máquinas: a seção de endereços aparece quando há alvo', async () => {
    stubDesktop();
    const t = montar('maquinas', SRV, undefined, { resolvedServer: SRV as Server });
    await tick();
    expect(document.body.textContent).toContain(m.acesso_secao_enderecos());
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
    expect(sel!.closest('.st-secao-sel')!.textContent).toContain(m.lista_agrupar_servidor());
    // Com duas máquinas não há motivo nenhum na tela: o apagado é o outro caso.
    expect(sel!.disabled).toBe(false);
    expect(document.body.textContent).not.toContain(m.config_modal_uma_maquina());
    // O estado habilitado é rótulo e select LADO A LADO. O que os separa é a classe: `so-uma`
    // liga a quebra de linha, que só o apagado quer (a segunda linha dele é o motivo). Sem esta
    // asserção, ligar a quebra para todo mundo — foi o que a rodada 1 fez — passa despercebido:
    // o jsdom não tem geometria pra flagrar o select descendo de linha.
    expect(sel!.closest('.st-secao-sel')!.classList.contains('so-uma')).toBe(false);

    sel!.value = 'srv-b';
    sel!.dispatchEvent(new Event('change', { bubbles: true }));
    expect(onPickServer).toHaveBeenCalledWith('srv-b');
    unmount(t.comp);
  });

  it('desktop, UMA máquina: o seletor continua na tela, apagado e com o motivo à vista', async () => {
    stubDesktop();
    const t = montar('contas', SRV as Server, 'id:srv-a',
      { servidores: [SRV as Server], resolvedServer: SRV as Server, onPickServer: vi.fn() });
    await tick(); await tick();
    const sel = document.querySelector<HTMLSelectElement>('.st-secao-sel .srv-sel');
    expect(sel).not.toBeNull();
    expect(sel!.disabled).toBe(true);
    // O motivo é TEXTO visível ao lado, nunca title/tooltip: toque não tem hover.
    expect(sel!.closest('.st-secao-sel')!.textContent).toContain(m.config_modal_uma_maquina());
    // É este estado — e só ele — que quebra a linha para caber o motivo.
    expect(sel!.closest('.st-secao-sel')!.classList.contains('so-uma')).toBe(true);
    unmount(t.comp);
  });

  it('celular, sub-cabeçalho e raiz com UMA máquina: seletor apagado com o motivo nos dois pontos', async () => {
    stubMobile();
    const um = { servidores: [SRV as Server], resolvedServer: SRV as Server,
                 nomeAlvo: 'A', onPickServer: vi.fn() };
    const sub = montar('contas', SRV as Server, 'id:srv-a', um);
    await tick(); await tick();
    const noSub = document.querySelector<HTMLSelectElement>('.st-sub .srv-sel');
    expect(noSub).not.toBeNull();
    expect(noSub!.disabled).toBe(true);
    expect(document.querySelector('.st-sub')!.textContent).toContain(m.config_modal_uma_maquina());
    unmount(sub.comp);
    document.body.innerHTML = '';

    const raiz = montar('root', SRV as Server, 'id:srv-a', um);
    await tick(); await tick();
    const naRaiz = document.querySelector<HTMLSelectElement>('.st-secao-sel .srv-sel');
    expect(naRaiz).not.toBeNull();
    expect(naRaiz!.disabled).toBe(true);
    expect(document.querySelector('.st-secao-sel')!.textContent).toContain(m.config_modal_uma_maquina());
    unmount(raiz.comp);
  });

  it('sem porta de troca (onPickServer ausente) o seletor não existe — não há para onde trocar', async () => {
    stubDesktop();
    const t = montar('contas', SRV as Server, 'id:srv-a',
      { servidores: DOIS, resolvedServer: SRV as Server });
    await tick(); await tick();
    expect(document.querySelector('.srv-sel')).toBeNull();
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

  it('a legenda do grupo Servidor é do MODAL: aparece uma vez nas telas do grupo, some nas do app', async () => {
    stubDesktop();
    // Contas nunca mostrou a legenda (ela morava no ServerSettings, que só serve Notificações,
    // Anexos e Avançado) — e é o caso que prova que ela passou a ser do grupo inteiro.
    const contas = montar('contas', SRV as Server, 'id:srv-a', { resolvedServer: SRV as Server });
    await tick(); await tick();
    expect(document.body.textContent).toContain(m.config_server_valem({ etiqueta: m.config_escopo_servidor() }));
    unmount(contas.comp);
    document.body.innerHTML = '';

    // Anexos passa pelo ServerSettings e Voz pelo VozSettings — os dois mostravam a legenda no
    // próprio cabeçalho. Se a linha tivesse ficado lá, apareceria DUAS vezes.
    for (const tela of ['anexos', 'voz'] as TelaConfig[]) {
      const t = montar(tela, SRV as Server, 'id:srv-a', { resolvedServer: SRV as Server });
      await tick(); await tick();
      const quantas = [...document.querySelectorAll('p')]
        .filter((p) => p.textContent?.trim() === m.config_server_valem({ etiqueta: m.config_escopo_servidor() })).length;
      expect(quantas, tela).toBe(1);
      unmount(t.comp);
      document.body.innerHTML = '';
    }

    // Geral é do aparelho, não de uma máquina: a legenda seria mentira ali.
    const geral = montar('geral', SRV as Server, 'id:srv-a', { resolvedServer: SRV as Server });
    await tick(); await tick();
    expect(document.body.textContent).not.toContain(m.config_server_valem({ etiqueta: m.config_escopo_servidor() }));
    unmount(geral.comp);
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

describe('SettingsModal — a busca', () => {
  // A lista de telas NÃO vem de uma constante compartilhada com a busca (isso seria circular): ela
  // é lida da raiz RENDERIZADA, clicando cada linha pra descobrir a rota. Tirar uma entrada da
  // busca, ou acrescentar uma tela ao modal sem entrada, derruba este teste.
  it('toda tela da raiz tem pelo menos uma entrada na busca', async () => {
    stubMobile();
    const t = montar('root', SRV as Server, 'id:srv-a', { resolvedServer: SRV as Server });
    await tick(); await tick();
    const linhas = [...document.querySelectorAll<HTMLButtonElement>('.st-cartao > button')];
    expect(linhas.length).toBeGreaterThan(0);
    for (const linha of linhas) linha.click();
    const telas = t.onIrPara.mock.calls.map((c) => c[0] as TelaConfig);
    expect(telas.length).toBe(linhas.length);
    for (const tela of telas) {
      expect(ENTRADAS.filter((e) => e.tela === tela).length, `tela sem entrada na busca: ${tela}`)
        .toBeGreaterThan(0);
    }
    unmount(t.comp);
  });

  // Escolher um resultado destrói o botão que tinha o foco. No celular o <h2> recebe o foco; no
  // desktop não existe <h2>, e sem um alvo o foco cai no <body> (leitor de tela mudo, Tab do zero).
  it('desktop: a troca de tela deixa o foco no conteúdo, não no body', async () => {
    stubDesktop();
    const t = montar('voz', SRV as Server, 'id:srv-a', { resolvedServer: SRV as Server });
    await tick(); await tick();
    expect(document.activeElement).toBe(document.querySelector('.st-conteudo'));
    unmount(t.comp);
  });

  // O caso que o $effect de troca de tela NÃO cobre: o resultado é da tela já aberta, a rota não
  // muda, e a lista (com o botão focado dentro) é apagada assim mesmo. É o caminho mais comum no
  // desktop, onde o modal abre na Aparência.
  it('desktop: escolher um resultado da tela JÁ ABERTA não joga o foco no body', async () => {
    stubDesktop();
    // `tela` precisa ser reativo: é o App quem troca a rota, e aqui o teste faz esse papel.
    const props = criarProps({ tela: 'diario' as TelaConfig });
    authMock.listServers.mockReturnValue([SRV] as never);
    authMock.getActiveId.mockReturnValue(SRV.id);
    apiMock.getConfigForServer.mockResolvedValue({ campos: {}, somente_leitura: {} } as never);
    apiMock.getPushSettings.mockReturnValue(new Promise(() => {}));
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(SettingsModal, {
      target: el,
      props: {
        get tela() { return props.tela; },
        alvo: SRV as Server, identidade: 'id:srv-a', nomeAlvo: null, semServidor: false,
        resolvedServer: SRV as Server,
        onIrPara: (t: TelaConfig) => { props.tela = t; },
        onVoltar: vi.fn(), onFechar: vi.fn(),
      },
    });
    await tick(); await tick();

    const campo = document.querySelector('.st-nav .bc input') as HTMLInputElement;
    campo.value = m.config_diag_baixar();
    campo.dispatchEvent(new Event('input'));
    await tick();
    const item = document.querySelector('.bc-item') as HTMLButtonElement;
    expect(item.getAttribute('aria-label')).toContain(m.config_diag_baixar());
    item.focus();
    item.click();
    await tick(); await tick();

    // A rota não mudou — é exatamente a condição em que o $effect não re-executa.
    expect(props.tela).toBe('diario');
    expect(document.activeElement).toBe(document.querySelector('.st-conteudo'));
    unmount(comp as never);
  });

  it('o campo aparece no topo da navegação no desktop e no topo da raiz no celular', async () => {
    stubDesktop();
    // Tela 'voz' de propósito: a navegação lateral é a mesma em qualquer tela, e montar a Aparência
    // aqui puxaria o ThemeToggle, que busca a paleta do desktop e exige mais mocks do que este
    // caso precisa.
    const desktop = montar('voz', SRV as Server, 'id:srv-a', { resolvedServer: SRV as Server });
    await tick(); await tick();
    expect(document.querySelector('.st-nav > .bc')).not.toBeNull();
    unmount(desktop.comp);
    document.body.innerHTML = '';

    stubMobile();
    const celular = montar('root', SRV as Server, 'id:srv-a', { resolvedServer: SRV as Server });
    await tick(); await tick();
    expect(document.querySelectorAll('.bc')).toHaveLength(1);
    unmount(celular.comp);
  });
});

describe('SettingsModal — a regra do escopo dita uma vez', () => {
  // UMA frase por tela, nas duas larguras, e uma frase DIFERENTE por tipo de tela. Duas legendas
  // coladas davam conta de dois tipos e mentiam no terceiro: em Máquinas a frase do aparelho ficava
  // sozinha sobre uma lista em que o ✕ apaga o peer no SERVIDOR.
  const CASOS: [TelaConfig, 'aparelho' | 'servidor' | 'mista'][] = [
    ['geral', 'aparelho'],
    ['voz', 'servidor'],
    ['maquinas', 'mista'],
  ];

  for (const [tela, tipo] of CASOS) {
    it(`${tela}: uma única legenda de escopo, a do tipo "${tipo}", nas duas larguras`, async () => {
      for (const largura of ['desktop', 'celular'] as const) {
        if (largura === 'desktop') stubDesktop(); else stubMobile();
        const t = montar(tela, SRV as Server, 'id:srv-a', { resolvedServer: SRV as Server });
        await tick(); await tick();

        const legendas = document.querySelectorAll('.st-sem-etiqueta, .st-valem');
        expect(legendas.length, `${tela}/${largura}`).toBe(1);
        const texto = legendas[0].textContent?.trim();

        if (tipo === 'aparelho') {
          expect(texto, `${tela}/${largura}`).toBe(m.config_escopo_sem_etiqueta());
        } else if (tipo === 'servidor') {
          expect(texto, `${tela}/${largura}`).toBe(m.config_server_valem({ etiqueta: m.config_escopo_servidor() }));
        } else {
          // A frase da tela mista nomeia a etiqueta e diz o que a etiqueta faz aqui: vai para o
          // servidor. Sem isto, "sem etiqueta vale só neste aparelho" cobriria o ✕ que remove o peer.
          expect(texto, `${tela}/${largura}`).toBe(m.config_escopo_mista({ etiqueta: m.config_escopo_servidor() }));
          expect(texto, `${tela}/${largura}`).toContain(m.config_escopo_servidor());
        }

        unmount(t.comp);
        document.body.innerHTML = '';
      }
    });
  }
});

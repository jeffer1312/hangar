// @vitest-environment happy-dom
// A Sidebar monta uma árvore pesada; os componentes de trabalho (sheets, menus, git, loop) viram
// stubs. Cobertura do fluxo de adicionar servidor (que vivia aqui desde a round 5) migrou pra
// ServidoresSettings na Task 4b/4c — ela mora em ServidoresSettings.test.ts.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick, createRawSnippet } from 'svelte';
import Sidebar from './Sidebar.svelte';
import * as auth from '../lib/auth';
import { abrirConfig } from '../lib/configNav';

// Snippet criado DENTRO de cada factory: vi.mock é hoisted; function declaration é hoisted também
// (const top-level cairia em TDZ quando a factory rodasse).
function stubDe() { return { default: createRawSnippet(() => ({ render: () => '<div />' })) }; }

// O store mockado é objeto PLAIN (sem reatividade Svelte): os deriveds do Sidebar leem as arrays
// uma vez, no mount. Por isso os testes preenchem as arrays ANTES de montar — vi.hoisted pra o
// teste alcançar a MESMA referência que a factory do mock retornou.
const storeState = vi.hoisted(() => ({
  byServer: [] as unknown[],
  rows: [] as unknown[],
  servers: [] as unknown[],
}));

vi.mock('../lib/api', () => ({
  createSession: vi.fn(), deleteSession: vi.fn(), renameSession: vi.fn(),
  gitAction: vi.fn(), checkoutBranch: vi.fn(), resumeSession: vi.fn(),
  broadcast: vi.fn(), getHistoryTailForServer: vi.fn(async () => []),
  // Imports do SessionContextMenu REAL (não stubado): o onMount chama getPushSettings.
  getPushSettings: vi.fn(async () => ({ muted: [] })),
  setSessionMute: vi.fn(), getBranches: vi.fn(), openEditor: vi.fn(),
  setThenLink: vi.fn(), clearThenLink: vi.fn(),
}));
vi.mock('../lib/auth', () => ({
  getActiveId: vi.fn(() => null),
  selectServer: vi.fn(() => true),
  serverColor: () => '#fff',
  withServer: vi.fn(async (_id: string, fn: () => Promise<unknown>) => fn()),
}));
vi.mock('../lib/sessionsStore.svelte', () => ({
  sessionsStore: {
    retain: vi.fn(), release: vi.fn(),
    markDeleting: vi.fn(), unmarkDeleting: vi.fn(),
    byServer: storeState.byServer, rows: storeState.rows, servers: storeState.servers,
    loading: false,
  },
}));
vi.mock('../lib/format', () => ({
  stateLabels: {}, stateColors: {}, countAwaiting: () => 0,
  groupSelectedByServer: () => [], initials: (n: string) => n.slice(0, 2),
  projectKey: () => '', projectLabel: () => '', effectiveGroupBy: () => 'server',
  fmtWhen: () => '', sortSessions: (s: unknown[]) => s, latestAssistantEvent: () => null,
  // Mesmo shape do real: itens do cluster são {session} (ou {kind:'header',...}); o template lê
  // item.session — sessão crua no lugar certo quebraria na chave do each.
  clusterByPair: (s: unknown[]) => s.map((x) => ({ session: x })),
  untrackedReason: () => '', providerName: () => 'claude',
  providerTag: () => null,
}));
vi.mock('../lib/badge', () => ({ updateBadge: vi.fn() }));
vi.mock('../lib/loop', () => ({ loopBadge: () => null, LOOP_TONE_COLOR: {} }));
vi.mock('../lib/plan', () => ({ planBadge: () => null }));
vi.mock('../lib/sidebarPrefs.svelte', () => ({ sidebarPrefs: { height: 'content' } }));
vi.mock('../lib/configNav', () => ({ abrirConfig: vi.fn() }));

vi.mock('./CreateSessionSheet.svelte', stubDe);
// SessionContextMenu NÃO é stubado: o teste clica no botão Renomear REAL do menu (mesmo caminho
// que as abas usam — bridge -> openMenu -> Rename). Stub de raw snippet não repassava o clique
// pro listener do setup no happy-dom (o nó é substituído no flush do Svelte).
vi.mock('./Git.svelte', stubDe);
vi.mock('./LoopSheet.svelte', stubDe);
vi.mock('./SessionSwitcherSheet.svelte', stubDe);
vi.mock('./HoverPreview.svelte', stubDe);
vi.mock('./PlanBar.svelte', stubDe);
vi.mock('./WorkspaceNav.svelte', stubDe);

const authMock = vi.mocked(auth);
const navMock = vi.mocked(abrirConfig);

// sidebarPin é o módulo REAL (só o store de sessões é mockado): o pin decide se a <aside> monta.
// beforeEach garante sidebar EXPANDIDA por padrão — o teste do recolhido seta e limpa o dele.
import { sidebarPin } from '../lib/sidebarPin.svelte';
import { sidebarBridge } from '../lib/sidebarBridge';
import * as api from '../lib/api';
import type { AggSession } from '../lib/types';

beforeEach(() => {
  vi.clearAllMocks();   // contagens de chamada não vazam entre testes
  sidebarPin.setUser(false);   // pin do usuário: expandido (o persistido '0' não vaza do teste anterior)
});

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(Sidebar, {
    target: el,
    props: {
      currentSession: null,
      onSelect: vi.fn(),
      onCompare: vi.fn(),
      boardActive: false,
      canvasActive: false,
      view: 'chat',
      onSelectView: vi.fn(),
      onOpenCommand: vi.fn(),
      onCollapsedChange: vi.fn(),
    },
  });
  return { el, comp: comp as never };
}

describe('Sidebar — engrenagem abre Configurações (Task 4c)', () => {
  it('nome acessível e aria-haspopup corretos; clique chama abrirConfig("root", srv ativo) uma vez', async () => {
    authMock.getActiveId.mockReturnValue('srv-b');
    const t = montar();
    await tick();
    const gear = document.querySelector<HTMLButtonElement>('.acct-btn');
    expect(gear).not.toBeNull();
    expect(gear?.getAttribute('aria-label')).toBe('Configurações e servidor');
    expect(gear?.getAttribute('aria-haspopup')).toBe('dialog');
    gear!.focus();
    expect(document.activeElement).toBe(gear);
    gear!.click();
    await tick();
    expect(navMock).toHaveBeenCalledTimes(1);
    expect(navMock).toHaveBeenCalledWith('root', 'srv-b');
    unmount(t.comp);
  });
});

describe('Sidebar — renomear com a sidebar recolhida (round 7)', () => {
  // Uma sessão pra linha existir no store; a sessão entra ANTES do mount (o mock do store é plain,
  // sem reatividade — os deriveds leem uma vez).
  function comUmaSessao() {
    // servers NÃO pode ficar vazio: o derived `groups` retorna [] cedo quando servers.length === 0
    storeState.servers.length = 0;
    storeState.servers.push({ id: 'srv-a', label: 'Servidor A', baseUrl: 'http://a', token: 'x' });
    storeState.byServer.length = 0;
    storeState.byServer.push({
      server: { id: 'srv-a', label: 'Servidor A' },
      sessions: [{ name: 'sess-1', serverId: 'srv-a', state: 'idle' }],
      error: null, loaded: true,
    });
  }
  function abrirMenuDaAba() {
    // Caminho REAL das abas: sidebarBridge.openSessionMenu -> openMenu do Sidebar -> menu monta.
    sidebarBridge.openSessionMenu(
      new MouseEvent('contextmenu', { clientX: 5, clientY: 5 }),
      { name: 'sess-1', serverId: 'srv-a' } as unknown as AggSession,
      'srv-a',
    );
  }

  it('RECOLHIDA: bridge + Rename do menu abrem diálogo acessível e renomeiam de verdade', async () => {
    comUmaSessao();
    const t = montar();
    await tick();
    // override do board/canvas DEPOIS do mount: o $effect do Sidebar (overviewActive) roda setForced
    // no mount e limparia o override pré-montado — no app real o board já chega no mount com o
    // override ligado, o que o teste reproduz aqui na sequência.
    sidebarPin.setForced(true);
    await tick();
    expect(document.querySelector('.sidebar')).toBeNull();   // aside realmente ausente
    abrirMenuDaAba();
    await tick();
    // Botão REAL do SessionContextMenu (não stubado)
    const renameBtn = [...document.querySelectorAll<HTMLButtonElement>('.ctx-menu button')]
      .find((b) => b.textContent?.trim() === 'Renomear');
    expect(renameBtn).not.toBeNull();
    renameBtn!.click();
    await tick();
    // SEM input inline (não existe fora da aside): o diálogo é o único caminho
    expect(document.querySelector('.sess-edit')).toBeNull();
    const dialog = document.querySelector<HTMLElement>('.confirm-card');
    expect(dialog).not.toBeNull();
    const input = document.querySelector<HTMLInputElement>('input[aria-label="Novo nome da sessão"]');
    expect(input).not.toBeNull();
    // conclui o rename: digita e confirma
    input!.value = 'sess-novo';
    input!.dispatchEvent(new Event('input'));
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-primary')!.click();
    await tick();
    expect(api.renameSession).toHaveBeenCalledWith('sess-1', 'sess-novo');
    expect(document.querySelector('.confirm-card')).toBeNull();   // fechou sozinho após confirmar
    sidebarPin.setForced(null);
    unmount(t.comp);
  });

  it('EXPANDIDA: Rename do menu segue editando INLINE (sem diálogo)', async () => {
    comUmaSessao();
    const t = montar();
    await tick();
    expect(document.querySelector('.sidebar')).not.toBeNull();
    abrirMenuDaAba();
    await tick();
    const renameBtn = [...document.querySelectorAll<HTMLButtonElement>('.ctx-menu button')]
      .find((b) => b.textContent?.trim() === 'Renomear');
    expect(renameBtn).not.toBeNull();
    renameBtn!.click();
    await tick();
    expect(document.querySelector('.confirm-card')).toBeNull();   // nenhum diálogo
    expect(document.querySelector('.sess-edit')).not.toBeNull();  // input inline na linha
    unmount(t.comp);
  });
});

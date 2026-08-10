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

vi.mock('../lib/api', () => ({
  createSession: vi.fn(), deleteSession: vi.fn(), renameSession: vi.fn(),
  gitAction: vi.fn(), checkoutBranch: vi.fn(), resumeSession: vi.fn(),
  broadcast: vi.fn(), getHistoryTailForServer: vi.fn(async () => []),
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
    byServer: [], rows: [], servers: [], loading: false,
  },
}));
vi.mock('../lib/format', () => ({
  stateLabels: {}, stateColors: {}, countAwaiting: () => 0,
  groupSelectedByServer: () => [], initials: (n: string) => n.slice(0, 2),
  projectKey: () => '', projectLabel: () => '', effectiveGroupBy: () => 'server',
  fmtWhen: () => '', sortSessions: (s: unknown[]) => s, latestAssistantEvent: () => null,
  clusterByPair: (s: unknown[]) => s, untrackedReason: () => '', providerName: () => 'claude',
  providerTag: () => null,
}));
vi.mock('../lib/badge', () => ({ updateBadge: vi.fn() }));
vi.mock('../lib/loop', () => ({ loopBadge: () => null, LOOP_TONE_COLOR: {} }));
vi.mock('../lib/plan', () => ({ planBadge: () => null }));
vi.mock('../lib/sidebarPrefs.svelte', () => ({ sidebarPrefs: { height: 'content' } }));
vi.mock('../lib/configNav', () => ({ abrirConfig: vi.fn() }));

vi.mock('./CreateSessionSheet.svelte', stubDe);
vi.mock('./SessionContextMenu.svelte', stubDe);
vi.mock('./Git.svelte', stubDe);
vi.mock('./LoopSheet.svelte', stubDe);
vi.mock('./SessionSwitcherSheet.svelte', stubDe);
vi.mock('./HoverPreview.svelte', stubDe);
vi.mock('./PlanBar.svelte', stubDe);
vi.mock('./WorkspaceNav.svelte', stubDe);

const authMock = vi.mocked(auth);
const navMock = vi.mocked(abrirConfig);

beforeEach(() => { vi.clearAllMocks(); });   // contagens de chamada não vazam entre testes

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

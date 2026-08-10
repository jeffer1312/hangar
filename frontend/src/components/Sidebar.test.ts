// @vitest-environment happy-dom
// Round 5: addBusy na transação de servidor — botão Add desabilitado durante o await, uma tentativa
// por clique, erro/retry visível. O Sidebar monta uma árvore pesada; os componentes de trabalho
// (sheets, menus, git, loop) viram stubs — o que importa aqui é o fluxo de adicionar servidor.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount, tick, createRawSnippet } from 'svelte';
import Sidebar from './Sidebar.svelte';
import * as auth from '../lib/auth';

// Snippet criado DENTRO de cada factory: vi.mock é hoisted; function declaration é hoisted também
// (const top-level cairia em TDZ quando a factory rodasse).
function stubDe() { return { default: createRawSnippet(() => ({ render: () => '<div />' })) }; }

vi.mock('../lib/api', () => ({
  createSession: vi.fn(), deleteSession: vi.fn(), renameSession: vi.fn(),
  gitAction: vi.fn(), checkoutBranch: vi.fn(), resumeSession: vi.fn(),
  broadcast: vi.fn(), getHistoryTailForServer: vi.fn(async () => []),
  getSessions: vi.fn(async () => []),
}));
vi.mock('../lib/auth', () => ({
  listServers: vi.fn(() => []),
  getActiveId: vi.fn(() => null),
  selectServer: vi.fn(() => true),
  removeServer: vi.fn(),
  addServerWithRollback: vi.fn(async () => ({ id: 'srv-x', succeeded: true })),
  renameServer: vi.fn(),
  updateServer: vi.fn(() => true),
  serverColor: () => '#fff',
  validarPareamento: vi.fn(),
  withServer: vi.fn(async (_id: string, fn: () => Promise<unknown>) => fn()),
  onServersChanged: vi.fn(() => () => {}),
  snapshotRemocao: vi.fn(() => null),
  removalStillMatches: vi.fn(() => null),
}));
vi.mock('../lib/sessionsStore.svelte', () => ({
  sessionsStore: {
    retain: vi.fn(), release: vi.fn(), refreshServers: vi.fn(), reconnect: vi.fn(),
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
vi.mock('./QrScanner.svelte', stubDe);
vi.mock('./Git.svelte', stubDe);
vi.mock('./LoopSheet.svelte', stubDe);
vi.mock('./SessionSwitcherSheet.svelte', stubDe);
vi.mock('./HoverPreview.svelte', stubDe);
vi.mock('./PlanBar.svelte', stubDe);
vi.mock('./WorkspaceNav.svelte', stubDe);
vi.mock('./AccountMenu.svelte');   // __mocks__/AccountMenu.svelte (porta do onAddServer)

const authMock = vi.mocked(auth);

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(Sidebar, {
    target: el,
    props: {
      currentSession: null,
      onSelect: vi.fn(),
      onCompare: vi.fn(),
      onLogout: vi.fn(),
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

describe('Sidebar — addBusy na transação de servidor (round 5)', () => {
  it('botão Add desabilitado durante o await; uma tentativa por clique; erro visível com retry', async () => {
    authMock.validarPareamento.mockReturnValue({ base: 'http://host:8765', token: 'abc' });
    let rejectAdd!: (e: Error) => void;
    authMock.addServerWithRollback.mockReturnValueOnce(new Promise((_res, rej) => { rejectAdd = rej; }));
    const t = montar();
    await tick();
    // o AccountMenu stub expõe o onAddServer — único caminho pro diálogo de add do Sidebar
    document.querySelector<HTMLButtonElement>('[data-testid="stub-add"]')!.click();
    await tick(); await tick();
    const input = document.querySelector<HTMLInputElement>('.add-srv-input')!;
    input.value = 'http://host:8765/?token=abc';
    input.dispatchEvent(new Event('input'));
    await tick();   // re-render do disabled (bind:value) antes do clique
    const addBtn = document.querySelector<HTMLButtonElement>('.confirm-card .c-primary')!;
    addBtn.click();
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(1);
    expect(addBtn.disabled).toBe(true);   // Add bloqueado durante o await
    addBtn.click();                       // segundo clique não abre outra tentativa
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(1);
    rejectAdd(new Error('servidor fora do ar'));
    await tick(); await tick();
    expect(addBtn.disabled).toBe(false);  // finally libera o botão
    const err = document.querySelector<HTMLElement>('#sb-add-err');
    expect(err?.innerText).toContain('servidor fora do ar');
    expect(err?.getAttribute('role')).toBe('alert');
    // retry: nova tentativa roda
    authMock.addServerWithRollback.mockReturnValueOnce(new Promise(() => {}));
    addBtn.click();
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(2);
    unmount(t.comp);
  });
});

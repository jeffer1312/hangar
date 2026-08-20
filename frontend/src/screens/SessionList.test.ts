// @vitest-environment happy-dom
// Round 2 (A1): erro TARDIO no mobile — fechar o modal de adicionar servidor enquanto o probe
// (addServerWithRollback) está pendente não pode engolir o erro: o modal reabre com role=alert,
// mesmo caminho do QR e do desktop. Caminho REAL: hamburger -> drawer (AccountMenu) ->
// "Adicionar servidor" -> modal -> submit -> Esc -> rejeição tardia.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick, createRawSnippet } from 'svelte';
import SessionList from './SessionList.svelte';
import * as auth from '../lib/auth';
import * as api from '../lib/api';

function stubDe() { return { default: createRawSnippet(() => ({ render: () => '<div />' })) }; }

vi.mock('../lib/api', () => ({
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  isTimeoutError: vi.fn(() => false),
  isAbortError: vi.fn(() => false),
  getSessions: vi.fn(async () => []),
  createSession: vi.fn(), deleteSession: vi.fn(), renameSession: vi.fn(),
  resumeSession: vi.fn(), broadcast: vi.fn(),
  // Imports do AccountMenu REAL (não stubado): onMount do SessionContextMenu não roda aqui, mas o
  // PushQuiet/ServerManager montam no drawer — api de push/sessão precisa existir.
  getPushSettings: vi.fn(async () => ({ muted: [] })),
  setSessionMute: vi.fn(), getBranches: vi.fn(), openEditor: vi.fn(),
  setThenLink: vi.fn(), clearThenLink: vi.fn(),
}));
vi.mock('../lib/auth', () => ({
  listServers: vi.fn(() => []),
  getActiveId: vi.fn(() => null),
  selectServer: vi.fn(() => true),
  removeServer: vi.fn(),
  addServerWithRollback: vi.fn(),
  renameServer: vi.fn(),
  updateServer: vi.fn(() => true),
  serverColor: () => '#fff',
  validarPareamento: vi.fn(),
  onServersChanged: vi.fn(() => () => {}),
  snapshotRemocao: vi.fn(() => null),
  removalStillMatches: vi.fn(() => null),
}));
vi.mock('../lib/sessionsStore.svelte', () => ({
  sessionsStore: {
    retain: vi.fn(), release: vi.fn(), refreshServers: vi.fn(),
    rows: [], byServer: [], servers: [], loading: false,
  },
}));
vi.mock('../lib/format', () => ({
  countAwaiting: () => 0, groupSelectedByServer: () => [],
  initials: (n: string) => n.slice(0, 2), projectKey: () => '',
  projectLabel: () => '', sortSessions: (s: unknown[]) => s,
  // Mesmo shape do real: itens do cluster são {session} (ou {kind:'header',...}).
  clusterByPair: (s: unknown[]) => s.map((x) => ({ session: x })),
}));
vi.mock('../lib/badge', () => ({ updateBadge: vi.fn() }));
vi.mock('../lib/push', () => ({ enablePush: vi.fn(), pushSupported: () => false }));
vi.mock('../lib/configNav', () => ({ abrirConfig: vi.fn() }));
vi.mock('../lib/vaultPush.svelte', () => ({ vaultPush: { estado: 'idle', detalhe: '', clear: vi.fn() } }));

vi.mock('../components/SessionCard.svelte', stubDe);
vi.mock('../components/CreateSessionSheet.svelte', stubDe);
vi.mock('../components/QrScanner.svelte', stubDe);
vi.mock('../components/BottomSheet.svelte', stubDe);
vi.mock('../components/ConfirmSheet.svelte', stubDe);
vi.mock('../components/Git.svelte', stubDe);
vi.mock('../components/LoopSheet.svelte', stubDe);
vi.mock('../components/AttentionFeed.svelte', stubDe);
vi.mock('../components/SessionSwitcherSheet.svelte', stubDe);

const authMock = vi.mocked(auth);

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(SessionList, {
    target: el,
    props: { onNavigateToChat: vi.fn(), onCompare: vi.fn(), onLogout: vi.fn() },
  });
  return { el, comp: comp as never };
}

beforeEach(() => {
  vi.clearAllMocks();
  document.body.innerHTML = '';   // modais portam pro <body>
});

describe('SessionList — erro tardio do add não some (round 2)', () => {
  it('fechar o modal durante a transação reabre com role=alert quando o probe rejeita', async () => {
    authMock.validarPareamento.mockReturnValue({ base: 'http://a:1', token: 't' });
    let rejectAdd!: (e: Error) => void;
    authMock.addServerWithRollback.mockReturnValueOnce(
      new Promise((_res, rej) => { rejectAdd = rej; }),
    );
    const t = montar();
    await tick();
    // Caminho real: hamburger -> drawer -> "Adicionar servidor"
    t.el.querySelector<HTMLButtonElement>('.sl-ham')!.click();
    await tick();
    t.el.querySelector<HTMLButtonElement>('.sm-item')!.click();
    await tick(); await tick();
    const modal = document.querySelector<HTMLElement>('.modal-dialog');
    expect(modal).not.toBeNull();
    // Preenche e submete (probe fica pendente)
    const url = document.querySelector<HTMLInputElement>('#add-url')!;
    url.value = 'http://a:1';
    url.dispatchEvent(new Event('input'));
    const token = document.querySelector<HTMLInputElement>('#add-token')!;
    token.value = 't';
    token.dispatchEvent(new Event('input'));
    await tick();
    // happy-dom não sintetiza submit no clique do botão: dispara o evento no form (handler real)
    document.querySelector<HTMLFormElement>('.add-form')!
      .dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await tick();
    expect(authMock.addServerWithRollback).toHaveBeenCalledTimes(1);
    // Usuário fecha o modal (Esc) com a transação ainda pendente
    document.querySelector<HTMLElement>('.modal-dialog')!
      .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await tick();
    expect(document.querySelector('.modal-dialog')).toBeNull();
    // Probe rejeita DEPOIS do fechamento: erro visível, modal reaberto
    rejectAdd(new Error('servidor fora do ar'));
    await tick(); await tick();
    expect(document.querySelector('.modal-dialog')).not.toBeNull();
    const err = document.querySelector<HTMLElement>('#sl-add-err');
    expect(err?.innerText).toContain('servidor fora do ar');
    expect(err?.getAttribute('role')).toBe('alert');
    unmount(t.comp);
  });
});

// @vitest-environment happy-dom
// Seção "Ocultas (N)" da lista do celular. O modelo da lista e o store viram dublês planos (lidos
// uma vez no mount); o SessionCard vira um botão com o nome, para contar onde cada linha caiu.
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import * as m from '../paraglide/messages';

const state = vi.hoisted(() => ({ rows: [] as Record<string, unknown>[], filterText: '' }));

vi.mock('../lib/sessionsStore.svelte', () => {
  const server = { id: 'a', label: 'Casa', baseUrl: 'http://a.test', token: 't' };
  return { sessionsStore: {
    get rows() { return state.rows; },
    byServer: [{ server, sessions: [], error: null, loaded: true }],
    servers: [server], loading: false, identities: new Map(),
    retain: vi.fn(), release: vi.fn(), buscarAgora: vi.fn(), refreshServers: vi.fn(), reconnect: vi.fn(),
    takePairLeft: () => null, markDeleting: vi.fn(), unmarkDeleting: vi.fn(),
  } };
});
vi.mock('../lib/sessionListModel.svelte', () => ({
  pairCodigo: (s: string) => s, pairResto: () => '',
  createSessionListModel: () => ({
    get flatRows() { return state.rows; },
    get groups() { return [{ id: 'all', label: '', color: null, sessions: state.rows }]; },
    cluster: (rows: unknown[]) => rows.map((session) => ({ kind: 'row', session, gid: null })),
    groupMode: 'none', groupBy: 'none', collapsed: new Set(), selected: new Set(), selectMode: false,
    showFilter: false, get filterText() { return state.filterText; }, awaitingTotal: 0, confirmDel: null, gitSheet: null,
    resumeCandidates: null, resumeError: '', resumeBusy: false,
    broadcastBusy: false, broadcastDisabled: true, broadcastIsSlash: false, broadcastMsg: '', broadcastText: '',
    compareDisabled: true,
    mount: () => () => {}, groupBadge: () => null, memberMachine: () => null, pairMembers: () => [],
    pairLabel: (_g: string, l: string) => l, pairSeal: () => null,
    toggleGroup: vi.fn(), toggleSelected: vi.fn(), toggleSelectMode: vi.fn(), selectGroupForBroadcast: vi.fn(),
    open: vi.fn(), requestDelete: vi.fn(), doDelete: vi.fn(), rename: vi.fn(), resume: vi.fn(),
    openGit: vi.fn(), closeGit: vi.fn(), openCompare: vi.fn(), sendBroadcast: vi.fn(), setGroupBy: vi.fn(),
  }),
}));
vi.mock('../components/SessionCard.svelte', async () => {
  const { createRawSnippet } = await import('svelte');
  return { default: createRawSnippet((input) => {
    const props = input as unknown as { session: { name: string } };
    return { render: () => `<button class="card-stub">${props.session.name}</button>` };
  }) };
});
function stub() {
  return import('svelte').then(({ createRawSnippet }) => ({ default: createRawSnippet(() => ({ render: () => '<div></div>' })) }));
}
vi.mock('../components/AccountMenu.svelte', stub);
vi.mock('../components/AttentionFeed.svelte', stub);
vi.mock('../components/CreateSessionSheet.svelte', stub);
vi.mock('../components/ConfirmSheet.svelte', stub);
vi.mock('../components/ConversationList.svelte', stub);
vi.mock('../components/Git.svelte', stub);
vi.mock('../components/HangarRunning.svelte', stub);
vi.mock('../components/KeyringNotice.svelte', stub);
vi.mock('../components/LiveVoiceButton.svelte', stub);
vi.mock('../components/PairAcceptSheet.svelte', stub);
vi.mock('../components/PairSheet.svelte', stub);
vi.mock('../components/SessionSwitcherSheet.svelte', stub);
vi.mock('../components/ShareSessionSheet.svelte', stub);
vi.mock('../lib/liveVoiceStore.svelte', () => ({ liveVoiceStore: { registerActions: () => () => {} } }));
vi.mock('../lib/quotaFeed.svelte', () => ({ quotaFeed: { retain: vi.fn(), release: vi.fn(), setServidor: vi.fn(), contas: [] } }));
vi.mock('../lib/sessionOrganization.svelte', () => ({ sessionOrganization: { mode: 'sessions' } }));
vi.mock('../lib/badge', () => ({ updateBadge: vi.fn() }));
vi.mock('../lib/configNav', () => ({ abrirConfig: vi.fn() }));
vi.mock('../lib/auth', () => ({
  listAllServers: () => [], getActiveId: () => 'a', selectServer: vi.fn(), removeServer: vi.fn(), renameServer: vi.fn(),
  updateServer: vi.fn(), onServersChanged: () => () => {}, snapshotRemocao: vi.fn(), removalStillMatches: vi.fn(),
  listOwnServers: () => [], withServer: vi.fn(), serverColor: () => '#fff',
}));

import SessionList from './SessionList.svelte';
import { hiddenSection } from '../lib/hiddenSection.svelte';

const row = (name: string, hidden = false) => ({
  name, serverId: 'a', serverLabel: 'Casa', state: 'idle', provider: 'claude', cwd: '/x', tracked: true,
  ...(hidden ? { hidden_from_list: true } : {}),
});

let component: ReturnType<typeof mount> | null = null;
async function render() {
  component = mount(SessionList, { target: document.body, props: { onNavigateToChat: vi.fn(), onCompare: vi.fn(), onLogout: vi.fn() } });
  for (let i = 0; i < 6; i++) await tick();
}
const cards = () => [...document.querySelectorAll('.card-stub')].map((b) => b.textContent);
const sectionHead = () => document.querySelector<HTMLButtonElement>('.hidden-section .group-head');

beforeEach(() => {
  localStorage.clear();
  hiddenSection.open = false;
  state.filterText = '';
});
afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

it('a oculta sai da lista e a seção aparece recolhida com a contagem', async () => {
  state.rows = [row('visivel'), row('oculta', true)];
  await render();
  expect(cards()).toEqual(['visivel']);
  expect(sectionHead()?.textContent).toContain(m.session_hidden_group({ n: 1 }));
  expect(sectionHead()?.getAttribute('aria-expanded')).toBe('false');
});

it('abrir a seção mostra a oculta e grava o aberto no aparelho', async () => {
  state.rows = [row('visivel'), row('oculta', true)];
  await render();
  sectionHead()!.click();
  await tick();
  expect(cards()).toEqual(['visivel', 'oculta']);
  expect(localStorage.getItem('cp_hidden_section_open')).toBe('1');
});

it('aberta antes (atalho de Configurações) já monta aberta', async () => {
  hiddenSection.open = true;
  state.rows = [row('oculta', true)];
  await render();
  expect(cards()).toEqual(['oculta']);
});

it('buscando, a seção abre sem mudar a preferência gravada', async () => {
  // A busca casou só a oculta: o modelo já devolve só ela.
  state.filterText = 'ocu';
  state.rows = [row('oculta', true)];
  await render();
  expect(cards()).toEqual(['oculta']);
  expect(sectionHead()?.getAttribute('aria-expanded')).toBe('true');
  expect(hiddenSection.open).toBe(false);
  expect(localStorage.getItem('cp_hidden_section_open')).not.toBe('1');
});

it('sem oculta não há seção', async () => {
  state.rows = [row('visivel')];
  await render();
  expect(sectionHead()).toBeNull();
});

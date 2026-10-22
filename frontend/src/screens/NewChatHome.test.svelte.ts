// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createRawSnippet, tick } from 'svelte';
import { createClassComponent } from 'svelte/legacy';
import type { AggSession } from '@hangar/core';
import { homeSessionsView } from '../lib/homeSessions';
import { overwriteGetLocale } from '../paraglide/runtime';

let rows = $state<AggSession[]>([]);
const servers = ['a', 'b'].map((id) => ({ id, label: id, baseUrl: `https://${id}.test`, token: 't' }));
const Empty = createRawSnippet(() => ({ render: () => '<span></span>' }));
vi.mock('../components/HomeUsage.svelte', () => ({ default: Empty }));
vi.mock('../components/FolderGitPill.svelte', () => ({ default: Empty }));
vi.mock('../components/BottomSheet.svelte', () => ({ default: Empty }));
vi.mock('../components/FolderScanner.svelte', () => ({ default: Empty }));
vi.mock('../components/CreateSessionSheet.svelte', () => ({ default: Empty }));
vi.mock('../components/newchat/QuietPill.svelte', () => ({ default: Empty }));
vi.mock('../components/newchat/AccountPill.svelte', () => ({ default: Empty }));
vi.mock('../components/newchat/ModelPill.svelte', () => ({ default: Empty }));
vi.mock('../components/newchat/NewChatComposer.svelte', () => ({ default: Empty }));
vi.mock('../components/newchat/NewChatAttach.svelte', () => ({ default: Empty }));
vi.mock('../components/newchat/NewChatMic.svelte', () => ({ default: Empty }));
vi.mock('../components/SessionCard.svelte', () => ({ default: createRawSnippet((input) => {
  const props = input as unknown as { session: AggSession; onClick: () => void };
  return {
    render: () => `<button class="session-stub">${props.session.name}</button>`,
    setup: (el) => { el.addEventListener('click', () => props.onClick()); },
  };
}) }));
vi.mock('../lib/sessionsStore.svelte', () => ({ sessionsStore: {
  get rows() { return rows; },
  get byServer() { return servers.map((server) => ({ server, loaded: true, error: null })); },
  loading: false,
} }));
vi.mock('../lib/sessionListModel.svelte', () => ({ createSessionListModel: (options: { onOpen: (name: string) => void }) => ({
  mount: () => () => {}, confirmDel: null,
  open: (session: AggSession) => { options.onOpen(session.name); return true; },
}) }));
const draft = {
  servers, serverObj: servers[0], server: 'a', attachments: [], worktrees: [], branches: null,
  init: () => {}, switchFromExhausted: () => {}, syncReserve: () => {}, releaseReserve: () => {},
};
vi.mock('../lib/newChatDraft.svelte', () => ({ createNewChatDraft: () => draft }));
vi.mock('../lib/quotaFeed.svelte', () => ({ quotaFeed: { contas: [] } }));
vi.mock('../lib/homeScreen.svelte', () => ({ homeScreen: { pref: 'sessions' } }));
vi.mock('../lib/auth', () => ({ selectServer: vi.fn(), getActiveId: () => 'a', listOwnServers: () => servers }));

const { default: NewChatHome } = await import('./NewChatHome.svelte');
const sessions = Array.from({ length: 8 }, (_, i) => ({
  name: `ready-${i}`, state: 'idle', serverId: i % 2 ? 'b' : 'a', serverLabel: i % 2 ? 'b' : 'a',
  serverColor: '#fff', last_activity: 1000 - i,
})) as AggSession[];

let target: HTMLElement;
let component: { $destroy(): void } | null = null;
let limit = 900;
let frames: Map<number, FrameRequestCallback>;
let frameId = 0;
const opened = vi.fn();

async function paint() {
  await tick();
  const callbacks = [...frames.values()];
  frames.clear();
  for (const callback of callbacks) callback(0);
  await tick();
}
async function mountHome() {
  component = createClassComponent({ component: NewChatHome, target, props: {
    onOpenList: vi.fn(), onOpenChat: (name: string) => {
      opened(name);
      // O navegador pode emitir scroll(0) antes de desmontar a rota.
      list.scrollTop = 0;
      list.dispatchEvent(new Event('scroll'));
      component!.$destroy(); component = null;
    },
  } });
  const list = target.querySelector<HTMLDivElement>('.list-layer')!;
  let top = 0;
  Object.defineProperties(list, {
    scrollTop: { get: () => top, set: (value: number) => { top = Math.max(0, Math.min(limit, value)); } },
    scrollHeight: { get: () => limit + 100 },
    clientHeight: { value: 100 },
  });
  await paint();
  return list;
}
function scroll(list: HTMLElement, top: number) {
  list.scrollTop = top;
  list.dispatchEvent(new Event('scroll'));
}
async function leaveExpandedHome() {
  rows = sessions;
  const list = await mountHome();
  target.querySelector<HTMLButtonElement>('.more')!.click();
  await paint();
  scroll(list, 640);
  const older = [...target.querySelectorAll<HTMLButtonElement>('.session-stub')].find((b) => b.textContent === 'ready-7')!;
  older.click(); await tick();
}

beforeEach(() => {
  overwriteGetLocale(() => 'pt');
  Object.assign(homeSessionsView, { readyExpanded: false, scrollTop: 0 });
  history.replaceState({}, '', '/');
  target = document.body.appendChild(document.createElement('div'));
  frames = new Map(); limit = 900; opened.mockClear();
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.set(++frameId, callback); return frameId; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
});
afterEach(() => {
  component?.$destroy(); component = null; target.remove(); vi.unstubAllGlobals();
});

it('voltar do chat mantém expansão e restaura rolagem em ondas sem gravar o zero da saída', async () => {
  await leaveExpandedHome();
  expect(opened).toHaveBeenCalledWith('ready-7');
  rows = []; limit = 0;
  const list = await mountHome();
  rows = sessions.slice(0, 2); limit = 140;
  await paint();
  expect(list.scrollTop).toBe(140);
  expect(target.querySelector('.more')?.getAttribute('aria-expanded')).toBe('true');
  rows = sessions; limit = 900;
  await paint();
  expect(list.scrollTop).toBe(640);
  expect(target.querySelectorAll('.session-stub')).toHaveLength(8);
});

it.each(['wheel', 'touchmove', 'keydown'])('gesto %s interrompe restauração pendente e novas ondas não puxam a lista', async (gesture) => {
  await leaveExpandedHome();
  rows = sessions.slice(0, 2); limit = 140;
  const list = await mountHome();
  expect(list.scrollTop).toBe(140);
  list.dispatchEvent(gesture === 'keydown' ? new KeyboardEvent('keydown', { key: 'ArrowDown' }) : new Event(gesture));
  scroll(list, 35);
  rows = sessions; limit = 900;
  await paint();
  expect(list.scrollTop).toBe(35);
});

it('recolher a lista para caber inteira salva zero e não recupera posição antiga após chat e nova onda', async () => {
  rows = sessions;
  const list = await mountHome();
  target.querySelector<HTMLButtonElement>('.more')!.click(); await paint();
  scroll(list, 94);
  target.querySelector<HTMLButtonElement>('.more')!.click();
  limit = 0; await paint();
  scroll(list, 0);
  expect(target.querySelector('.more')?.getAttribute('aria-expanded')).toBe('false');
  target.querySelector<HTMLButtonElement>('.session-stub')!.click(); await tick();

  const returned = await mountHome();
  expect(returned.scrollTop).toBe(0);
  expect(target.querySelector('.more')?.getAttribute('aria-expanded')).toBe('false');
  target.querySelector<HTMLButtonElement>('.more')!.click();
  limit = 900; await paint();
  expect(returned.scrollTop).toBe(0);
  rows = [...sessions, { ...sessions[0], name: 'ready-new' }];
  await paint();
  expect(returned.scrollTop).toBe(0);
});

it('abrir outro chat durante restauração parcial conserva o alvo para o próximo retorno', async () => {
  await leaveExpandedHome();
  rows = sessions.slice(0, 2); limit = 140;
  await mountHome();
  target.querySelector<HTMLButtonElement>('.session-stub')!.click();
  rows = sessions; limit = 900;
  const list = await mountHome();
  expect(list.scrollTop).toBe(640);
});

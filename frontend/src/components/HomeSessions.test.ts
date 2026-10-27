// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { tick } from 'svelte';
import { createClassComponent } from 'svelte/legacy';
import HomeSessions from './HomeSessions.svelte';
import { overwriteGetLocale } from '../paraglide/runtime';

const store = vi.hoisted(() => ({ rows: [] as unknown[], byServer: [] as unknown[], loading: false }));
const model = vi.hoisted(() => ({ open: vi.fn(), requestDelete: vi.fn(), doDelete: vi.fn(), confirmDel: null, mount: () => () => {} }));
vi.mock('../lib/sessionsStore.svelte', () => ({ sessionsStore: store }));
vi.mock('../lib/sessionListModel.svelte', () => ({ createSessionListModel: () => model }));
vi.mock('./SessionCard.svelte', async () => {
  const { createRawSnippet } = await import('svelte');
  return { default: createRawSnippet((input) => {
    const props = input as unknown as { session: { name: string }; serverBadge?: { label: string } | null; onClick: () => void };
    return {
      render: () => `<button class="session-stub">${props.session.name}${props.serverBadge ? ` · ${props.serverBadge.label}` : ''}</button>`,
      setup: (el) => { el.addEventListener('click', () => props.onClick()); },
    };
  }) };
});

const server = (id: string) => ({ id, label: id, baseUrl: `https://${id}.test`, token: 't' });
const row = (name: string, state: string, serverId = 'a', last_activity = 0) => ({ name, state, serverId, serverLabel: serverId, serverColor: '#fff', last_activity });

let target: HTMLElement;
let component: { $destroy(): void } | null = null;
async function mount() {
  component = createClassComponent({ component: HomeSessions, target, props: { onOpenChat: vi.fn() } });
  await tick();
  return target.textContent ?? '';
}
beforeEach(() => {
  overwriteGetLocale(() => 'pt');
  target = document.createElement('div');
  document.body.appendChild(target);
});
afterEach(() => {
  target.querySelector<HTMLButtonElement>('.more[aria-expanded="true"]')?.click();
  component?.$destroy(); component = null; target.remove();
});

it('carregando enquanto nenhuma máquina respondeu', async () => {
  Object.assign(store, { loading: true, rows: [], byServer: [{ server: server('a'), sessions: [], error: null, loaded: false }] });
  expect(await mount()).toContain('Carregando sessões');
});

it('vazio quando as máquinas responderam sem sessão aberta', async () => {
  Object.assign(store, { loading: false, rows: [], byServer: [{ server: server('a'), sessions: [], error: null, loaded: true }] });
  expect(await mount()).toContain('Nenhuma sessão ativa');
});

it('erro por máquina aparece junto das sessões das outras', async () => {
  Object.assign(store, { loading: false, rows: [row('s1', 'working')], byServer: [
    { server: server('a'), sessions: [], error: null, loaded: true },
    { server: server('b'), sessions: [], error: 'offline', loaded: false },
  ] });
  const text = await mount();
  expect(text).toContain('b offline');
  expect(text).toContain('Trabalhando');
});

it('rótulos de servidor ficam acima das linhas em cada estado, sem badge repetido', async () => {
  Object.assign(store, { loading: false,
    rows: ['awaiting_input', 'working', 'idle'].flatMap((state) => [row(`${state}-b`, state, 'b', 300), row(`${state}-a`, state, 'a', 100)]),
    byServer: ['a', 'b'].map((id) => ({ server: server(id), sessions: [], error: null, loaded: true })),
  });
  await mount();
  for (const state of ['Esperando você', 'Trabalhando']) {
    const section = target.querySelector(`section[aria-label="${state}"]`)!;
    expect([...section.querySelectorAll('h3')].map((h) => h.textContent?.trim())).toEqual(['a', 'b']);
    for (const machine of section.querySelectorAll('section')) {
      expect(machine.firstElementChild?.tagName).toBe('H3');
      expect(machine.querySelector('button')?.textContent).not.toContain(' · ');
    }
  }
  const ready = target.querySelector('section[aria-label="Prontas"]')!;
  expect(ready.querySelector('h2')?.textContent).toContain('2');
  expect([...ready.querySelectorAll('h3')].map((h) => h.textContent?.trim())).toEqual(['b']);
  expect(ready.querySelector('.session-stub')?.textContent).toBe('idle-b');
  const more = ready.querySelector<HTMLButtonElement>('.more')!;
  expect(more.textContent).toContain('mais 1 prontas');
  more.click(); await tick();
  expect([...ready.querySelectorAll('h3')].map((h) => h.textContent?.trim())).toEqual(['a', 'b']);
  expect(ready.querySelectorAll('.session-stub')).toHaveLength(2);
});

it('mantém prontas expandidas ao abrir uma sessão e remontar a home', async () => {
  Object.assign(store, { loading: false, rows: [row('p1', 'idle', 'a', 300), row('p2', 'idle', 'b', 100)],
    byServer: ['a', 'b'].map((id) => ({ server: server(id), sessions: [], error: null, loaded: true })),
  });
  await mount();
  target.querySelector<HTMLButtonElement>('.more')!.click(); await tick();
  const older = [...target.querySelectorAll<HTMLButtonElement>('.session-stub')].find((b) => b.textContent === 'p2')!;
  older.click(); await tick();
  expect(model.open).toHaveBeenCalledWith(store.rows[1]);
  component!.$destroy(); component = null;
  await mount();
  expect(target.querySelector('.more')?.getAttribute('aria-expanded')).toBe('true');
  expect([...target.querySelectorAll('.session-stub')].map((b) => b.textContent)).toEqual(['p1', 'p2']);
});

it('chaveiro trancado aparece na tela inicial, por máquina e com campo de senha', async () => {
  Object.assign(store, { loading: false, rows: [row('s1', 'idle')], byServer: [
    { server: server('a'), sessions: [], error: null, loaded: true, keyringLocked: true },
    { server: server('b'), sessions: [], error: null, loaded: true, keyringLocked: false },
  ] });
  const text = await mount();
  expect(text).toContain('a: chaveiro trancado');
  expect(text).not.toContain('b: chaveiro trancado');
  expect(target.querySelectorAll('input[type="password"]')).toHaveLength(1);
});

it('sucesso agrupa e "mais N prontas" expande', async () => {
  Object.assign(store, { loading: false, rows: [row('p1', 'idle'), row('w1', 'awaiting_input'), row('p2', 'idle')],
    byServer: [{ server: server('a'), sessions: [], error: null, loaded: true }] });
  const text = await mount();
  expect(text.indexOf('Esperando você')).toBeLessThan(text.indexOf('Prontas'));
  const more = target.querySelector<HTMLButtonElement>('.more')!;
  expect(more.textContent).toContain('mais 1 prontas');
  more.click(); await tick();
  expect(more.getAttribute('aria-expanded')).toBe('true');
});

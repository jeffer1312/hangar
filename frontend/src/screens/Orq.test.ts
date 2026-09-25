// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import * as api from '@hangar/core';
import * as queries from '../lib/queries';
import Orq from './Orq.svelte';

vi.mock('../lib/queries', () => ({ orqDetalhe: vi.fn(() => ({})), clienteQuery: { fetchQuery: vi.fn() } }));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  getOrqForServer: vi.fn(), getOrqConductorForServer: vi.fn(),
}));

const SRV = { id: 'A', label: 'A', baseUrl: 'https://a.test', token: 'a' };
const WD: api.OrqWatchdog = { alive: true, last_cycle: new Date().toISOString(), since: null, restarts: 0,
  unit: 'vigia-g1', unit_state: 'active', arbiter: 'arb', watching: ['rev1', 'arb'], source: 'heartbeat' };
const EXEC: api.OrqExecucao = { id: '2026-09-23-x', plano: '', branch: 'b', gid: 'g1', inicio: null,
  fim: '2026-09-24T10:00:00-03:00', resultado: 'concluida', tasks: [], voltas: 0, aprovadas_primeira: 0,
  reconstruida: false, watchdog: WD };
const CONDUCTOR: api.OrqConductor = { watchdog: WD, truncated: false, skipped: 0, feed: [
  { id: 'r2', ts: '2026-09-25T10:01:00-03:00', kind: 'alarm', text: 'rev1-parada', task: 1, jev: null },
  { id: 'r1', ts: '2026-09-25T10:00:00-03:00', kind: 'woke', text: 'ok-recebido', task: null, jev: null },
] };

async function flush() { for (let i = 0; i < 15; i++) await tick(); }

async function montar() {
  localStorage.setItem('cp_servers', JSON.stringify([SRV]));
  localStorage.setItem('cp_active', 'A');
  vi.mocked(api.getOrqForServer).mockResolvedValue({ execucoes: [EXEC], fichas: [] });
  vi.mocked(queries.clienteQuery.fetchQuery).mockResolvedValue(EXEC as never);
  vi.mocked(api.getOrqConductorForServer).mockResolvedValue(CONDUCTOR);
  const target = document.body.appendChild(document.createElement('div'));
  const component = mount(Orq, { target, props: {} });
  await flush();
  return { target, component };
}

afterEach(() => { vi.clearAllMocks(); vi.useRealTimers(); localStorage.clear(); document.body.innerHTML = ''; });

it('o chip do card mostra o condutor e leva ao feed já rolado', async () => {
  const rolou = vi.fn();
  Element.prototype.scrollIntoView = rolou;
  const { target, component } = await montar();
  try {
    const chip = target.querySelector('.conductor-chip')!;
    expect(chip.classList.contains('k-alive')).toBe(true);
    expect(chip.textContent).toContain('rev1, arb');
    chip.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    await flush();
    expect(api.getOrqConductorForServer).toHaveBeenCalledWith(expect.objectContaining({ id: 'A' }), EXEC.id);
    expect(target.textContent).toContain('rev1-parada');
    expect(rolou).toHaveBeenCalled();
  } finally { await unmount(component); }
});

it('a revalidação de 20 s não desmonta o feed nem perde o filtro', async () => {
  vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
  const { target, component } = await montar();
  try {
    target.querySelector<HTMLButtonElement>('.exec')!.click();
    await flush();
    target.querySelector<HTMLButtonElement>('[data-kind="alarm"]')!.click();
    await flush();
    const feed = target.querySelector('.feed');
    expect(feed).not.toBeNull();
    vi.advanceTimersByTime(20_000);
    await flush();
    expect(api.getOrqConductorForServer).toHaveBeenCalledTimes(2);
    expect(target.querySelector('.feed')).toBe(feed);
    expect(target.querySelector('[data-kind="alarm"]')!.getAttribute('aria-pressed')).toBe('true');
    expect(target.textContent).not.toContain('ok-recebido');
  } finally { await unmount(component); }
});

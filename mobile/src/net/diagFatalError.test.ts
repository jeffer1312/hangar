import { afterEach, beforeEach, expect, it, vi } from 'vitest';

const disk = vi.hoisted(() => new Map<string, string>());
const state = vi.hoisted(() => ({
  own: false, invite: false,
  changed: undefined as (() => void) | undefined,
  foreground: undefined as ((state: string) => void) | undefined,
}));
vi.mock('../stores/prefs', () => ({ prefs: { getString: (key: string) => disk.get(key), set: (key: string, value: string) => disk.set(key, value) } }));
vi.mock('../stores/servers', () => ({ useServers: {
  getState: () => ({ servers: state.own ? [{ baseUrl: 'https://own.test', token: 'private-token' }] : [],
    active: () => state.own ? { baseUrl: 'https://own.test', token: 'private-token', invite: state.invite } : null }),
  subscribe: (fn: () => void) => { state.changed = fn; },
} }));
vi.mock('react-native', () => ({ Platform: { OS: 'ios' }, AppState: {
  addEventListener: (_: string, fn: (value: string) => void) => { state.foreground = fn; },
} }));
vi.mock('expo-updates', () => ({ updateId: null }));
const report = { timestamp: '2026-10-18T10:26:00.000Z', name: 'TypeError', message: 'Cannot read property of undefined',
  route: '/s/[server]/[name]/activity', stack: 'Activity@main.jsbundle:1:99', componentStack: '',
  metadata: { platform: 'ios', runtime: 'native-2', updateId: 'ota-one' } };

beforeEach(() => {
  vi.resetModules(); vi.useFakeTimers();
  state.own = false; state.invite = false; state.changed = undefined; state.foreground = undefined;
  disk.clear(); disk.set('diagnostics.lastFatalError', JSON.stringify({ report, pending: true }));
});
afterEach(() => { vi.clearAllTimers(); vi.useRealTimers(); vi.restoreAllMocks(); });

it('espera credenciais próprias e confirma o diário depois do ACK do servidor', async () => {
  const fetch = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"gravadas":1}'));
  (await import('./diag')).iniciarDiag();
  await vi.advanceTimersByTimeAsync(0);
  expect(fetch).not.toHaveBeenCalled();
  state.own = true;
  state.changed?.();
  await vi.advanceTimersByTimeAsync(0);
  expect(fetch).toHaveBeenCalledWith('https://own.test/api/diag', expect.anything());
  const body = JSON.parse(fetch.mock.calls[0][1]!.body as string);
  expect(body.eventos).toEqual([expect.objectContaining({ evento: 'js.fatal', ts: report.timestamp,
    detalhe: report.message, pilha: report.stack, tela: report.route })]);
  expect(JSON.stringify(body)).not.toContain('private-token');
  expect(JSON.parse(disk.get('diagnostics.lastFatalError')!).pending).toBe(false);
}, 20000);

it('não envia ao convidado e conserva o pendente em falha HTTP ou ACK inválido', async () => {
  state.own = true; state.invite = true;
  const fetch = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response('{"gravadas":1}'));
  (await import('./diag')).iniciarDiag();
  await vi.advanceTimersByTimeAsync(0);
  expect(fetch).not.toHaveBeenCalled();
  state.invite = false;
  fetch.mockResolvedValueOnce(new Response('{}', { status: 503 }));
  state.changed?.(); await vi.advanceTimersByTimeAsync(0);
  expect(JSON.parse(disk.get('diagnostics.lastFatalError')!).pending).toBe(true);
  fetch.mockResolvedValueOnce(new Response('{"gravadas":0}'));
  state.changed?.(); await vi.advanceTimersByTimeAsync(0);
  expect(JSON.parse(disk.get('diagnostics.lastFatalError')!).pending).toBe(true);
  state.changed?.(); await vi.advanceTimersByTimeAsync(0);
  expect(JSON.parse(disk.get('diagnostics.lastFatalError')!).pending).toBe(false);
}, 20000);

// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CliproxyInstall, Server } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { overwriteGetLocale } from '../../paraglide/runtime';
import { ProxyUpdateCard, useProxyUpdate } from './ProxyUpdate';

const calls = vi.hoisted(() => ({ read: vi.fn(), update: vi.fn(), alert: vi.fn() }));
vi.mock('@hangar/core', async original => ({
  ...await original<typeof import('@hangar/core')>(),
  getCliproxyInstallForServer: calls.read, updateCliproxyForServer: calls.update,
}));
vi.mock('react-native', async original => {
  const rn = await original<typeof import('react-native')>();
  return { ...rn, Alert: { alert: calls.alert } };
});
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
overwriteGetLocale(() => 'pt');

const server: Server = { id: 'b', label: 'B', baseUrl: 'https://b.local', token: 'token-b' };
const install = (patch: Partial<CliproxyInstall> = {}): CliproxyInstall => ({
  state: 'managed', version: 'v8.0.79', latest: 'v8.1.0', update_available: true, running: true, sessions: 0, job: null, ...patch,
});
const job = (step: 'download' | 'service' | 'start', done = false, error: { code: string; detail: string } | null = null) =>
  ({ kind: 'update' as const, step, done, error, outcome: done && !error ? 'updated' as const : null });
const roots: Array<ReturnType<typeof createRoot>> = [];

function Harness() { return createElement(ProxyUpdateCard, { state: useProxyUpdate(server) }); }
async function show() {
  const container = document.createElement('div'); const root = createRoot(container); roots.push(root);
  await act(async () => { root.render(createElement(Harness)); await vi.advanceTimersByTimeAsync(0); });
  return container;
}
async function click(container: HTMLElement, label: string) {
  const target = [...container.querySelectorAll('button')].find(b => b.getAttribute('aria-label') === label);
  expect(target, label).toBeTruthy();
  await act(async () => { target!.click(); await vi.advanceTimersByTimeAsync(0); });
}
async function tick() { await act(async () => { await vi.advanceTimersByTimeAsync(1000); }); }

beforeEach(() => { vi.useFakeTimers(); for (const fn of Object.values(calls)) fn.mockReset(); });
afterEach(async () => {
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  vi.useRealTimers();
});

describe('atualização do CLIProxyAPI no celular', () => {
  it('com sessões no proxy pergunta antes, mostra as etapas e diz quando terminou', async () => {
    calls.read.mockResolvedValue(install());
    const view = await show();
    expect(view.textContent).toContain('CLIProxyAPI v8.1.0');
    expect(view.textContent).toContain('Você está na v8.0.79.');

    calls.update.mockRejectedValueOnce(Object.assign(new Error('x'), { code: 'erro_cliproxy_sessoes_ativas', envelope: { params: { sessions: 2 } } }));
    await click(view, m.mobile_cliproxy_update_button());
    expect(calls.update).toHaveBeenCalledExactlyOnceWith(server, false);
    expect(calls.alert).toHaveBeenCalledOnce();
    const [title, body, buttons] = calls.alert.mock.calls[0];
    expect(title).toBe(m.mobile_cliproxy_update_confirm_title());
    expect(body).toBe(m.mobile_cliproxy_update_confirm_many({ n: '2' }));

    calls.update.mockResolvedValueOnce({ job: job('download') });
    calls.read.mockResolvedValueOnce(install({ job: job('service') }));
    await act(async () => { buttons[1].onPress(); await vi.advanceTimersByTimeAsync(0); });
    expect(calls.update).toHaveBeenLastCalledWith(server, true);
    await tick();
    expect(view.textContent).toContain('Atualizando para a v8.1.0…');
    expect(view.textContent).toContain('Trocar o programa do serviço');

    calls.read.mockResolvedValue(install({ version: 'v8.1.0', update_available: false, job: job('start', true) }));
    await tick();
    expect(view.textContent).toContain('CLIProxyAPI atualizado para a v8.1.0.');
    await act(async () => { await vi.advanceTimersByTimeAsync(4000); });
    expect(view.textContent).toBe('');
  });

  it('só a volta que deu certo diz que o proxy segue funcionando', async () => {
    calls.read.mockResolvedValue(install());
    const view = await show();
    calls.update.mockResolvedValue({ job: job('download') });
    calls.read.mockResolvedValueOnce(install({ job: job('start', true, { code: 'cliproxy_nao_subiu', detail: 'x' }) }));
    await click(view, m.mobile_cliproxy_update_button());
    await tick();
    expect(view.textContent).toContain(m.mobile_cliproxy_update_failed_back({ new: 'v8.1.0', old: 'v8.0.79' }));
    expect(view.querySelector(`button[aria-label="${m.mobile_cliproxy_update_retry()}"]`)).toBeTruthy();

    calls.read.mockResolvedValueOnce(install({ job: job('start', true, { code: 'cliproxy_atualizacao_sem_volta', detail: 'x' }) }));
    await click(view, m.mobile_cliproxy_update_retry());
    await tick();
    expect(view.textContent).toContain(m.mobile_cliproxy_update_failed_lost({ old: 'v8.0.79' }));
    expect(view.textContent).not.toContain('segue funcionando');
  });

  it('proxy de fora, em dia ou leitura que falhou: nada no topo', async () => {
    calls.read.mockResolvedValue(install({ state: 'external', version: null, latest: null, update_available: false }));
    expect((await show()).textContent).toBe('');
    calls.read.mockResolvedValue(install({ update_available: false }));
    expect((await show()).textContent).toBe('');
    calls.read.mockRejectedValue(new Error('rede'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    expect((await show()).textContent).toBe('');
    warn.mockRestore();
  });
});

// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CliproxyInstall, CliproxyJob, Server } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { overwriteGetLocale } from '../../paraglide/runtime';
import { ProxyCard, useProxyUpdate } from './ProxyUpdate';

const calls = vi.hoisted(() => ({ read: vi.fn(), update: vi.fn(), adopt: vi.fn(), alert: vi.fn(), connect: vi.fn() }));
vi.mock('@hangar/core', async original => ({
  ...await original<typeof import('@hangar/core')>(),
  getCliproxyInstallForServer: calls.read, updateCliproxyForServer: calls.update, adoptCliproxyForServer: calls.adopt,
}));
vi.mock('react-native', async original => {
  const rn = await original<typeof import('react-native')>();
  return { ...rn, Alert: { alert: calls.alert } };
});
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
overwriteGetLocale(() => 'pt');

const server: Server = { id: 'b', label: 'B', baseUrl: 'https://b.local', token: 'token-b' };
const install = (patch: Partial<CliproxyInstall> = {}): CliproxyInstall => ({
  state: 'managed', version: 'v8.0.79', latest: 'v8.1.0', update_available: true, running: true, base_url: 'http://127.0.0.1:8317',
  accounts: 3, sessions: 0, job: null, ...patch,
});
const external = (patch: Partial<CliproxyInstall> = {}) =>
  install({ state: 'external', version: 'v8.0.56', latest: 'v8.0.79', update_available: false, handover: { ok: true }, ...patch });
const job = (step: 'download' | 'config' | 'service' | 'start', done = false, error: { code: string; detail: string } | null = null,
  kind: 'update' | 'adopt' = 'update'): CliproxyJob =>
  ({ kind, step, done, error, outcome: done && !error ? (kind === 'adopt' ? 'adopted' : 'updated') : null });
const roots: Array<ReturnType<typeof createRoot>> = [];

function Harness() { return createElement(ProxyCard, { state: useProxyUpdate(server), onConnect: calls.connect }); }
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

describe('CLIProxyAPI em Contas no celular', () => {
  it('do Hangar: com sessões no proxy pergunta antes, mostra as etapas e diz quando terminou', async () => {
    calls.read.mockResolvedValue(install());
    const view = await show();
    expect(view.textContent).toContain('CLIProxyAPI v8.0.79');
    expect(view.textContent).toContain(m.mobile_cliproxy_card_managed());
    expect(view.textContent).toContain('v8.1.0');

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
    expect(view.textContent).not.toContain('CLIProxyAPI atualizado');
  });

  it('só a volta que deu certo diz que o proxy segue funcionando', async () => {
    calls.read.mockResolvedValue(install());
    const view = await show();
    calls.update.mockResolvedValue({ job: job('download') });
    calls.read.mockResolvedValueOnce(install({ job: job('start', true, { code: 'cliproxy_nao_subiu', detail: 'x' }) }));
    await click(view, m.mobile_cliproxy_update_button());
    await tick();
    expect(view.textContent).toContain(m.mobile_cliproxy_update_failed_back({ new: 'v8.1.0', old: 'v8.0.79' }));

    calls.read.mockResolvedValueOnce(install({ job: job('start', true, { code: 'cliproxy_atualizacao_sem_volta', detail: 'x' }) }));
    await click(view, m.mobile_cliproxy_update_retry());
    await tick();
    expect(view.textContent).toContain(m.mobile_cliproxy_update_failed_lost({ old: 'v8.0.79' }));
    expect(view.textContent).not.toContain('segue funcionando');
  });

  it('de fora: passa ao Hangar com as etapas da entrega e diz que as contas continuam', async () => {
    calls.read.mockResolvedValue(external());
    const view = await show();
    expect(view.textContent).toContain('CLIProxyAPI v8.0.56');
    expect(view.textContent).toContain(m.mobile_cliproxy_card_external());
    expect(view.querySelector(`button[aria-label="${m.mobile_cliproxy_update_button()}"]`)).toBeNull();

    calls.adopt.mockResolvedValue({ job: job('download', false, null, 'adopt') });
    calls.read.mockResolvedValueOnce(external({ job: job('config', false, null, 'adopt') }));
    await click(view, m.mobile_cliproxy_adopt_button());
    expect(calls.adopt).toHaveBeenCalledExactlyOnceWith(server, false);
    await tick();
    expect(view.textContent).toContain(m.mobile_cliproxy_adopt_running());
    expect(view.textContent).toContain(m.mobile_cliproxy_adopt_step_config());

    calls.read.mockResolvedValue(install({ update_available: false, latest: 'v8.0.79', job: job('start', true, null, 'adopt') }));
    await tick();
    expect(view.textContent).toContain(m.mobile_cliproxy_adopt_done({ new: 'v8.0.79' }));
    expect(view.textContent).toContain(m.mobile_cliproxy_card_managed());

    await click(view, m.mobile_cliproxy_card_connect());
    expect(calls.connect).toHaveBeenCalledOnce();
  });

  it('entrega cuja tarefa sumiu sem entrar diz que parou, e tocar com outra tarefa em curso mostra o resultado dela', async () => {
    calls.read.mockResolvedValue(external());
    const view = await show();
    calls.adopt.mockResolvedValueOnce({ job: job('download', false, null, 'adopt') });
    await click(view, m.mobile_cliproxy_adopt_button());
    await tick();
    expect(view.textContent).toContain(m.mobile_cliproxy_wizard_err_task_crashed());

    calls.adopt.mockRejectedValueOnce(Object.assign(new Error('x'), { code: 'erro_cliproxy_ocupado' }));
    calls.read.mockResolvedValueOnce(external({ job: job('download', true, { code: 'cliproxy_entrega_outro_config', detail: 'x' }, 'adopt') }));
    await click(view, m.mobile_cliproxy_update_retry());
    await tick();
    expect(view.textContent).toContain(m.mobile_cliproxy_handover_other_config());
  });

  it('de fora que lê outro config: diz o motivo e não oferece a entrega', async () => {
    calls.read.mockResolvedValue(external({ handover: { ok: false, code: 'cliproxy_entrega_outro_config', detail: '/etc/x.yaml' } }));
    const view = await show();
    expect(view.textContent).toContain(m.mobile_cliproxy_handover_other_config());
    expect(view.querySelector(`button[aria-label="${m.mobile_cliproxy_adopt_button()}"]`)).toBeNull();
  });

  it('sem proxy na máquina ou leitura que falhou: nada', async () => {
    calls.read.mockResolvedValue(install({ state: 'absent', version: null, latest: null, update_available: false }));
    expect((await show()).textContent).toBe('');
    calls.read.mockRejectedValue(new Error('rede'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    expect((await show()).textContent).toBe('');
    warn.mockRestore();
  });
});

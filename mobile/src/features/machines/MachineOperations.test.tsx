// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ fetch: vi.fn(), alert: vi.fn() }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(), Alert: { alert: mocks.alert } }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
import { MachineOperations } from './MachineOperations';
import * as m from '../../paraglide/messages';
const server = { id: 'ops-b', label: 'fixture-operations', baseUrl: 'https://operations.test', token: 'secret-b' };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
let root: Root;
let el: HTMLDivElement;
let mounted: boolean;
const postCalls = () => mocks.fetch.mock.calls.filter(([, init]) => init?.method === 'POST');
async function click(label: string) {
  const button = [...el.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label);
  expect(button, label).toBeTruthy();
  await act(async () => button!.click());
}
function confirm() { return mocks.alert.mock.calls.at(-1)![2].find((b: { style?: string }) => b.style !== 'cancel').onPress as () => void; }
const tick = (ms = 2000) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });
beforeEach(() => {
  vi.useFakeTimers(); mocks.fetch.mockReset(); mocks.alert.mockReset();
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', origin: null, onUnauthorized: vi.fn(), createEventSource: vi.fn() });
  vi.stubGlobal('fetch', mocks.fetch);
  el = document.createElement('div'); document.body.appendChild(el); root = createRoot(el); mounted = true;
});
afterEach(async () => { if (mounted) await act(async () => root.unmount()); el.remove(); vi.unstubAllGlobals(); vi.useRealTimers(); });
const render = () => act(async () => root.render(<MachineOperations server={server} />));

describe('operações da máquina explícita', () => {
  it('confirma antes do POST e só conclui a atualização com o pid correto, mostrando progresso e avisos', async () => {
    let reads = 0;
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'POST' ? json({ ok: true, pid: 77 }) : json({ estado: [
      { pid: 88, fase: 'pronto', ok: true },
      { pid: 77, fase: 'rodando', passo: 2, total: 3, texto: 'fixture-progress' },
      { pid: 77, fase: 'pronto', ok: true, reiniciar_manual: true, avisos: ['fixture-warning'] },
    ][Math.min(reads++, 2)] }));
    await render(); await click(m.mobile_machines_update());
    expect(postCalls()).toHaveLength(0);
    const accept = confirm();
    await act(async () => { accept(); accept(); });
    expect(postCalls()).toHaveLength(1);
    expect(postCalls()[0][0]).toBe(`${server.baseUrl}/api/atualizacao/iniciar`);
    await tick(); expect(el.textContent).not.toContain(m.mobile_machines_operation_ready());
    await tick(); expect(el.textContent).toContain('fixture-progress');
    await tick();
    expect(el.textContent).toContain(m.mobile_machines_operation_ready());
    expect(el.textContent).toContain(m.mobile_machines_restart_manual());
    expect(el.textContent).toContain('fixture-warning');
  });

  it('build pendente no CI não vira atualização incompleta: anuncia a conclusão', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'POST' ? json({ ok: true, pid: 77 }) : json({ estado: {
      pid: 77, fase: 'pronto', ok: true, waiting_for_server_build: true, commit_para: '991bb4f123456789',
    } }));
    await render(); await click(m.mobile_machines_update()); await act(async () => confirm()()); await tick();
    expect(el.textContent).toContain(m.mobile_machines_operation_ready());
    expect(el.textContent).not.toContain('991bb4f1');
  });

  it('reinício Windows confirma pela tentativa mesmo com pid diferente do lançador', async () => {
    let reads = 0;
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'POST'
      ? json({ ok: true, pid: 77, request_id: 'fixture-restart' })
      : json({ estado: { pid: 88, fase: 'pronto', ok: true, request_id: reads++ ? 'fixture-restart' : 'fixture-old' } }));
    await render(); await click(m.mobile_machines_restart()); await act(async () => confirm()());
    await tick(); expect(el.textContent).not.toContain(m.mobile_machines_operation_ready());
    await tick(); expect(el.textContent).toContain(m.mobile_machines_operation_ready());
  });

  it('não envia após confirmação atrasada de uma tela desmontada', async () => {
    await render(); await click(m.mobile_machines_restart());
    const accept = confirm();
    await act(async () => root.unmount()); mounted = false;
    await act(async () => accept());
    expect(postCalls()).toHaveLength(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('não trata recusa HTTP como queda de rede esperada no reinício', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'POST' ? json({ ok: true, pid: 77 }) : json({ detail: 'fixture-auth-refused' }, 401));
    await render(); await click(m.mobile_machines_restart()); await act(async () => confirm()()); await tick();
    expect(el.textContent).toContain('401');
    expect(el.textContent).toContain('fixture-auth-refused');
    expect(el.textContent).not.toContain(m.mobile_machines_operation_ready());
  });

  it.each([403, 404, 500, 502, 503, 504])('mantém a espera após HTTP %i dentro do prazo e só conclui com o resultado do mesmo pid', async (status) => {
    let reads = 0;
    mocks.fetch.mockImplementation(async (_url, init) => {
      if (init?.method === 'POST') return json({ ok: true, pid: 77 });
      reads++;
      return reads === 1 ? json({ detail: 'fixture-proxy-restarting' }, status) : json({ estado: { pid: 77, fase: 'pronto', ok: true } });
    });
    await render(); await click(m.mobile_machines_restart()); await act(async () => confirm()());
    await tick();
    expect(reads).toBe(1);
    expect(el.textContent).not.toContain(m.mobile_machines_operation_ready());
    expect((el.querySelector(`[aria-label="${m.mobile_machines_restart()}"]`) as HTMLButtonElement).disabled).toBe(true);
    await tick();
    expect(reads).toBe(2);
    expect(el.textContent).toContain(m.mobile_machines_operation_ready());
  });
});

// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ fetch: vi.fn(), alert: vi.fn() }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  Alert: { alert: mocks.alert },
  TextInput: (p: { accessibilityLabel?: string; value?: string; editable?: boolean; onChangeText?: (t: string) => void }) =>
    React.createElement('input', { 'aria-label': p.accessibilityLabel, disabled: p.editable === false, value: p.value ?? '',
      onChange: (e: { target: { value: string } }) => p.onChangeText?.(e.target.value) }),
}));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
import { DevModeSetting } from './DevModeSetting';
import * as m from '../../paraglide/messages';

const server = { id: 'dev-a', label: 'fixture-dev', baseUrl: 'https://dev-mode.test', token: 'secret-d' };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
const refused = (status: number, code = 'erro_dev_mode_nao_e_checkout') => json({ detail: { code, params: {}, msg: 'fixture-dev-refused' } }, status);
const pkg = { modo: 'pacote', canal: 'beta', checkout: '/x/hangar', estado: {} };
let root: Root;
let el: HTMLDivElement;
const row = () => el.querySelector(`[aria-label="${m.native_dev_mode_title()}"]`) as HTMLButtonElement | null;
const folder = () => el.querySelector(`input[aria-label="${m.native_dev_mode_folder()}"]`) as HTMLInputElement | null;
const posts = () => mocks.fetch.mock.calls.filter(([, init]) => init?.method === 'POST');
const confirm = () => mocks.alert.mock.calls.at(-1)![2].find((b: { style?: string }) => b.style !== 'cancel').onPress as () => void;
const tick = (ms = 2000) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });
const render = () => act(async () => root.render(<DevModeSetting server={server} />));
async function type(value: string) {
  const input = folder()!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}
beforeEach(() => {
  vi.useFakeTimers(); mocks.fetch.mockReset(); mocks.alert.mockReset();
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', origin: null, onUnauthorized: vi.fn(), createEventSource: vi.fn() });
  vi.stubGlobal('fetch', mocks.fetch);
  el = document.createElement('div'); document.body.appendChild(el); root = createRoot(el);
});
afterEach(async () => { await act(async () => root.unmount()); el.remove(); vi.unstubAllGlobals(); vi.useRealTimers(); });

describe('modo desenvolvedor', () => {
  it('mostra o modo, confirma, liga com a pasta e acompanha o executor até o fim', async () => {
    let switched = false;
    let reads = 0;
    mocks.fetch.mockImplementation(async (_url, init) => {
      if (init?.method === 'POST') { switched = true; return json({ ok: true, pid: 55 }); }
      if (!switched) return json(pkg);
      reads++;
      if (reads === 1) return json({ ...pkg, estado: { pid: 55, fase: 'rodando', passo: 4, total: 4, texto: 'fixture-restarting' } });
      return json({ modo: 'desenvolvedor', canal: 'main', checkout: '/y/hangar', estado: { pid: 55, fase: 'pronto', ok: true } });
    });
    await render();
    expect(row()?.textContent).toContain(m.native_dev_mode_package({ channel: 'beta' }));
    expect(folder()?.value).toBe('/x/hangar');
    await type('/y/hangar');
    await act(async () => row()!.click());
    expect(posts()).toHaveLength(0);
    await act(async () => confirm()());
    expect(posts()).toHaveLength(1);
    expect(posts()[0][0]).toBe(`${server.baseUrl}/api/atualizacao/dev-mode`);
    expect(JSON.parse(posts()[0][1].body)).toEqual({ on: true, checkout: '/y/hangar' });
    await tick();
    expect(el.textContent).toContain('fixture-restarting');
    await tick();
    expect(el.textContent).toContain(m.native_dev_mode_done());
    expect(row()?.textContent).toContain(m.native_dev_mode_developer({ folder: '/y/hangar' }));
    expect(folder()).toBeNull();
  });

  it('recusa do servidor mostra o motivo sem acompanhar nada', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'POST' ? refused(409) : json(pkg));
    await render();
    await act(async () => row()!.click());
    await act(async () => confirm()());
    expect(el.textContent).toContain('fixture-dev-refused');
    await tick();
    expect(mocks.fetch.mock.calls.filter(([, init]) => init?.method !== 'POST')).toHaveLength(1);
  });

  it('troca que falhou no executor mostra o erro gravado', async () => {
    let switched = false;
    mocks.fetch.mockImplementation(async (_url, init) => {
      if (init?.method === 'POST') { switched = true; return json({ ok: true, pid: 56 }); }
      return json(switched ? { ...pkg, estado: { pid: 56, fase: 'pronto', ok: false, erro: 'fixture-rolled-back' } } : { ...pkg, modo: 'desenvolvedor' });
    });
    await render();
    expect(folder()).toBeNull();
    await act(async () => row()!.click());
    await act(async () => confirm()());
    expect(JSON.parse(posts()[0][1].body)).toEqual({ on: false });
    await tick();
    expect(el.textContent).toContain(m.native_dev_mode_failed({ reason: 'fixture-rolled-back' }));
  });

  it('sem pasta o interruptor não liga', async () => {
    mocks.fetch.mockImplementation(async () => json({ ...pkg, checkout: null }));
    await render();
    expect(folder()?.value).toBe('');
    expect(row()?.getAttribute('aria-disabled') === 'true' || row()?.disabled).toBe(true);
  });

  it('leitura com erro mostra o motivo e tenta de novo', async () => {
    let reads = 0;
    mocks.fetch.mockImplementation(async () => reads++ ? json(pkg) : refused(503, 'erro_qualquer'));
    await render();
    expect(el.textContent).toContain('fixture-dev-refused');
    await act(async () => (el.querySelector(`[aria-label="${m.mobile_machines_retry()}"]`) as HTMLButtonElement).click());
    expect(row()?.textContent).toContain(m.native_dev_mode_package({ channel: 'beta' }));
  });

  it('servidor que já estava no modo pedido mostra o detalhe da rota', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'POST'
      ? json({ ok: true, nada_a_fazer: true, detalhe: 'fixture-already' }) : json(pkg));
    await render();
    await act(async () => row()!.click());
    await act(async () => confirm()());
    expect(el.textContent).toContain('fixture-already');
  });

  it('o POST da troca espera até 30 s, como o nativo', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'POST'
      ? json({ ok: true, nada_a_fazer: true, detalhe: 'fixture-slow' }) : json(pkg));
    await render();
    // O teto é um `AbortSignal.timeout`, que o relógio falso não adianta.
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    await act(async () => row()!.click());
    await act(async () => confirm()());
    const ceilings = timeout.mock.calls.map(([ms]) => ms);
    expect(ceilings).toContain(30_000);
    expect(ceilings).not.toContain(15_000);
    timeout.mockRestore();
  });

  it('troca para um checkout sem a rota nova: o controle fica, com modo desconhecido, o motivo e o desligar', async () => {
    let switched = false;
    mocks.fetch.mockImplementation(async (_url, init) => {
      if (init?.method === 'POST') { switched = true; return json({ ok: true, pid: 57 }); }
      return json(switched ? { modo: 'desenvolvedor', estado: { pid: 57, fase: 'pronto', ok: true } } : pkg);
    });
    await render();
    await act(async () => row()!.click());
    await act(async () => confirm()());
    await tick();
    expect(row()?.textContent).toContain(m.native_dev_mode_unknown());
    expect(el.textContent).toContain(m.native_dev_mode_unknown_reason());
    expect(folder()).toBeNull();
    await act(async () => row()!.click());
    await act(async () => confirm()());
    expect(posts()).toHaveLength(2);
    expect(JSON.parse(posts()[1][1].body)).toEqual({ on: false });
  });

  it('releitura que falha logo após a troca tenta de novo enquanto o servidor reinicia', async () => {
    let switched = false;
    let reads = 0;
    mocks.fetch.mockImplementation(async (_url, init) => {
      if (init?.method === 'POST') { switched = true; return json({ ok: true, pid: 58 }); }
      if (!switched) return json(pkg);
      reads++;
      if (reads === 1) return json({ ...pkg, estado: { pid: 58, fase: 'pronto', ok: true } });
      if (reads <= 3) return refused(503, 'erro_qualquer');
      return json({ modo: 'desenvolvedor', canal: 'main', checkout: '/x/hangar', estado: {} });
    });
    await render();
    await act(async () => row()!.click());
    await act(async () => confirm()());
    await tick();
    expect(row()?.textContent).not.toContain(m.native_dev_mode_package({ channel: 'beta' }));
    expect(row()?.getAttribute('aria-disabled') === 'true' || row()?.disabled).toBe(true);
    await tick(10_000);
    expect(row()?.textContent).toContain(m.native_dev_mode_developer({ folder: '/x/hangar' }));
    expect(el.textContent).not.toContain('fixture-dev-refused');
  });

  it('releitura que nunca volta mostra o erro sem o modo velho, e o desligar fica', async () => {
    let switched = false;
    let reads = 0;
    mocks.fetch.mockImplementation(async (_url, init) => {
      if (init?.method === 'POST') { switched = true; return json({ ok: true, pid: 59 }); }
      if (!switched) return json(pkg);
      return reads++ ? refused(503, 'erro_qualquer') : json({ ...pkg, estado: { pid: 59, fase: 'pronto', ok: true } });
    });
    await render();
    await act(async () => row()!.click());
    await act(async () => confirm()());
    await tick(30_000);
    expect(el.textContent).toContain('fixture-dev-refused');
    expect(row()?.textContent).not.toContain(m.native_dev_mode_package({ channel: 'beta' }));
    expect(row()?.textContent).toContain(m.native_dev_mode_unknown());
    await act(async () => row()!.click());
    await act(async () => confirm()());
    expect(JSON.parse(posts()[1][1].body)).toEqual({ on: false });
  });

  it.each([
    ['servidor anterior sem canal', () => json({ modo: 'pacote', estado: {} })],
    ['máquina sem migrar', () => json({ versoes: {}, estado: {} })],
    ['rota ausente', () => json({ detail: 'Not Found' }, 404)],
  ])('%s: não mostra a linha', async (_name, response) => {
    mocks.fetch.mockImplementation(async () => response());
    await render();
    expect(row()).toBeNull();
    expect(el.textContent).toBe('');
  });
});

// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ fetch: vi.fn() }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
import { WebUiSetting } from './WebUiSetting';
import * as m from '../../paraglide/messages';

const server = { id: 'web-a', label: 'fixture-web', baseUrl: 'https://web-ui.test', token: 'secret-w' };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
const refused = (status: number) => json({ detail: { code: 'web_ui_unavailable', params: {}, msg: 'fixture-web-ui-refused' } }, status);
let root: Root;
let el: HTMLDivElement;
const row = () => el.querySelector(`[aria-label="${m.native_web_ui_title()}"]`) as HTMLButtonElement | null;
const puts = () => mocks.fetch.mock.calls.filter(([, init]) => init?.method === 'PUT');
const render = () => act(async () => root.render(<WebUiSetting server={server} />));
beforeEach(() => {
  mocks.fetch.mockReset();
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', origin: null, onUnauthorized: vi.fn(), createEventSource: vi.fn() });
  vi.stubGlobal('fetch', mocks.fetch);
  el = document.createElement('div'); document.body.appendChild(el); root = createRoot(el);
});
afterEach(async () => { await act(async () => root.unmount()); el.remove(); vi.unstubAllGlobals(); });

describe('tela web neste servidor', () => {
  it('mostra o valor lido e alterna pelo toque, gravando no servidor escolhido', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => json({ web_ui: init?.method === 'PUT' ? JSON.parse(init.body).web_ui : true }));
    await render();
    expect(row()?.textContent).toContain(m.native_web_ui_on());
    await act(async () => row()!.click());
    expect(puts()).toHaveLength(1);
    expect(puts()[0][0]).toBe(`${server.baseUrl}/api/web-ui`);
    expect(JSON.parse(puts()[0][1].body)).toEqual({ web_ui: false });
    expect(row()?.textContent).toContain(m.native_web_ui_off());
  });

  it('gravação recusada volta ao valor anterior e mostra o motivo do servidor', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'PUT' ? refused(503) : json({ web_ui: true }));
    await render();
    await act(async () => row()!.click());
    expect(row()?.textContent).toContain(m.native_web_ui_on());
    expect(el.textContent).toContain('fixture-web-ui-refused');
  });

  it('leitura com erro mostra o motivo e tenta de novo', async () => {
    let reads = 0;
    mocks.fetch.mockImplementation(async () => reads++ ? json({ web_ui: false }) : refused(503));
    await render();
    expect(el.textContent).toContain('fixture-web-ui-refused');
    expect(row()?.disabled).toBe(true);
    await act(async () => (el.querySelector(`[aria-label="${m.mobile_machines_retry()}"]`) as HTMLButtonElement).click());
    expect(row()?.textContent).toContain(m.native_web_ui_off());
    expect(el.textContent).not.toContain('fixture-web-ui-refused');
  });

  it.each([404, 405])('servidor sem a rota (%i) não mostra a linha nem erro', async (status) => {
    mocks.fetch.mockImplementation(async () => json({ detail: 'Not Found' }, status));
    await render();
    expect(row()).toBeNull();
    expect(el.textContent).toBe('');
  });
});

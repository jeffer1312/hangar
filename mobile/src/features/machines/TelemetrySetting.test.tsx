// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Alert, Linking } from 'react-native';
import { configureApi, getTelemetry } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ fetch: vi.fn(), erro: vi.fn() }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('../../ui/Toast', () => ({ toast: { erro: mocks.erro, ok: vi.fn() } }));
import { TELEMETRY_DOC_URL, TelemetrySetting, askTelemetry } from './TelemetrySetting';
import * as m from '../../paraglide/messages';

const server = { id: 'tel-a', label: 'fixture-a', baseUrl: 'https://tel-a.test', token: 'secret-a' };
const other = { id: 'tel-b', label: 'fixture-b', baseUrl: 'https://tel-b.test', token: 'secret-b' };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
const refused = (status: number, code: string) => json({ detail: { code, params: {}, msg: 'fixture-telemetry-refused' } }, status);
const view = (consent: boolean | null, disabled_by: string | null = null) => json({ consent, disabled_by });
let root: Root;
let el: HTMLDivElement;
const row = () => el.querySelector(`[aria-label="${m.telemetry_setting()}"]`) as HTMLButtonElement | null;
const puts = () => mocks.fetch.mock.calls.filter(([, init]) => init?.method === 'PUT');
const render = () => act(async () => root.render(<TelemetrySetting server={server} />));
beforeEach(() => {
  mocks.fetch.mockReset(); mocks.erro.mockReset();
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', origin: null, onUnauthorized: vi.fn(), createEventSource: vi.fn() });
  vi.stubGlobal('fetch', mocks.fetch);
  el = document.createElement('div'); document.body.appendChild(el); root = createRoot(el);
});
afterEach(async () => { await act(async () => root.unmount()); el.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe('getTelemetry', () => {
  it('404 e 503 telemetry_unavailable viram servidor sem telemetria, sem erro', async () => {
    mocks.fetch.mockImplementation(async () => json({ detail: 'Not Found' }, 404));
    await expect(getTelemetry(server)).resolves.toBeUndefined();
    mocks.fetch.mockImplementation(async () => refused(503, 'telemetry_unavailable'));
    await expect(getTelemetry(server)).resolves.toBeUndefined();
    mocks.fetch.mockImplementation(async () => refused(503, 'other_failure'));
    await expect(getTelemetry(server)).rejects.toMatchObject({ status: 503 });
    mocks.fetch.mockImplementation(async () => view(null, 'env'));
    await expect(getTelemetry(server)).resolves.toEqual({ consent: null, disabled_by: 'env' });
  });

  it('200 com corpo fora da forma é resposta inválida, não "sem telemetria"', async () => {
    mocks.fetch.mockImplementation(async () => json({ consent: 'yes', disabled_by: null }));
    await expect(getTelemetry(server)).rejects.toThrow(m.atualizar_resposta_invalida());
  });
});

describe('linha de telemetria', () => {
  it('sem resposta mostra "Não respondido" e o toque liga nesta máquina', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'PUT' ? view(JSON.parse(init.body).consent) : view(null));
    await render();
    expect(row()?.textContent).toContain(m.auto_report_value_unanswered());
    await act(async () => row()!.click());
    expect(puts()).toHaveLength(1);
    expect(puts()[0][0]).toBe(`${server.baseUrl}/api/telemetry`);
    expect(JSON.parse(puts()[0][1].body)).toEqual({ consent: true });
    expect(row()?.textContent).toContain(m.auto_report_on());
  });

  it('desligada pelo ambiente diz o motivo e não grava', async () => {
    mocks.fetch.mockImplementation(async () => view(true, 'ci'));
    await render();
    expect(el.textContent).toContain(m.telemetry_disabled_env());
    expect(row()?.textContent).toContain(m.auto_report_off());
    expect(row()?.disabled).toBe(true);
    await act(async () => row()!.click());
    expect(puts()).toHaveLength(0);
  });

  it('"O que é enviado" abre a lista no repositório', async () => {
    const open = vi.spyOn(Linking, 'openURL');
    mocks.fetch.mockImplementation(async () => view(false));
    await render();
    await act(async () => (el.querySelector(`[aria-label="${m.telemetry_what_is_sent()}"]`) as HTMLButtonElement).click());
    expect(open).toHaveBeenCalledWith(TELEMETRY_DOC_URL);
  });

  it('falha ao abrir a lista avisa na tela', async () => {
    vi.spyOn(Linking, 'openURL').mockRejectedValue(new Error('fixture-no-handler'));
    mocks.fetch.mockImplementation(async () => view(false));
    await render();
    await act(async () => (el.querySelector(`[aria-label="${m.telemetry_what_is_sent()}"]`) as HTMLButtonElement).click());
    expect(mocks.erro).toHaveBeenCalledWith(m.telemetry_doc_open_failed());
  });

  it.each([404, 503])('servidor sem telemetria (%i) não mostra a linha nem erro', async (status) => {
    mocks.fetch.mockImplementation(async () => status === 404 ? json({ detail: 'Not Found' }, 404) : refused(503, 'telemetry_unavailable'));
    await render();
    expect(row()).toBeNull();
    expect(el.textContent).toBe('');
  });
});

describe('aviso da abertura', () => {
  it('fechar sem tocar não grava nada', async () => {
    const alert = vi.spyOn(Alert, 'alert');
    const asked = askTelemetry([server, other]);
    expect(alert).toHaveBeenCalledTimes(1);
    expect(alert.mock.calls[0][1]).toContain('fixture-a, fixture-b');
    alert.mock.calls[0][3]!.onDismiss!();
    await asked;
    expect(mocks.fetch).not.toHaveBeenCalled();
  });

  it('"O que é enviado" abre a lista e pergunta de novo; "Pode enviar" grava em todas', async () => {
    const alert = vi.spyOn(Alert, 'alert');
    const open = vi.spyOn(Linking, 'openURL');
    mocks.fetch.mockImplementation(async (url) => String(url).startsWith(other.baseUrl)
      ? refused(503, 'telemetry_unavailable') : view(true));
    const asked = askTelemetry([server, other]);
    const press = (text: string) => alert.mock.calls.at(-1)![2]!.find((button) => button.text === text)!.onPress!();
    press(m.telemetry_what_is_sent());
    expect(open).toHaveBeenCalledWith(TELEMETRY_DOC_URL);
    expect(alert).toHaveBeenCalledTimes(2);
    expect(mocks.fetch).not.toHaveBeenCalled();
    press(m.telemetry_yes());
    await asked;
    await vi.waitFor(() => expect(mocks.erro).toHaveBeenCalledWith(m.auto_report_save_failed({ machines: 'fixture-b' })));
    expect(puts().map(([url, init]) => [url, JSON.parse(init.body)])).toEqual([
      [`${server.baseUrl}/api/telemetry`, { consent: true }],
      [`${other.baseUrl}/api/telemetry`, { consent: true }],
    ]);
  });
});

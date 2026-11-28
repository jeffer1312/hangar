// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Alert, Linking } from 'react-native';
import { configureApi, getTelemetry, putTelemetry, telemetryUnanswered } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ fetch: vi.fn(), erro: vi.fn(), saved: vi.fn() }));
vi.mock('../../net/diag', () => ({ onTelemetrySaved: mocks.saved }));
vi.mock('../../stores/servers', () => ({ useServers: { getState: () => ({ servers: [server, { ...other, label: server.label }] }) } }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => <div>{children}</div> }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('../../ui/Toast', () => ({ toast: { erro: mocks.erro, ok: vi.fn() } }));
import { TelemetrySetting } from './TelemetrySetting';
import { LEGACY_TELEMETRY_DOC_URL, TELEMETRY_DOC_URL, askTelemetry } from './startupQuestions';
import * as m from '../../paraglide/messages';

const server = { id: 'tel-a', label: 'fixture-a', baseUrl: 'https://tel-a.test', token: 'secret-a' };
const other = { id: 'tel-b', label: 'fixture-b', baseUrl: 'https://tel-b.test', token: 'secret-b' };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
const refused = (status: number, code: string) => json({ detail: { code, params: {}, msg: 'fixture-telemetry-refused' } }, status);
// Sem escopo = servidor anterior aos escopos; com escopo, `enabled` segue o aceite ligado.
const view = (consent: boolean | null, disabled_by: string | null = null, consent_scope?: number) => json(consent_scope === undefined
  ? { consent, disabled_by } : { consent, disabled_by, consent_scope, enabled: consent === true && disabled_by === null });
const pending = (server: typeof other, consent_scope?: number) => ({ server, shown: { consent: null, disabled_by: null, consent_scope } });
let root: Root;
let el: HTMLDivElement;
const row = () => el.querySelector(`[aria-label="${m.telemetry_setting()}"]`) as HTMLButtonElement | null;
const puts = () => mocks.fetch.mock.calls.filter(([, init]) => init?.method === 'PUT');
const render = () => act(async () => root.render(<TelemetrySetting server={server} />));
// O "sim" do ajuste passa pelo mesmo aviso da abertura.
const confirmYes = async (alert: { mock: { calls: unknown[][] } }) => act(async () => {
  const buttons = alert.mock.calls.at(-1)![2] as { text: string; onPress?: () => void }[];
  buttons.find((button) => button.text === m.telemetry_yes())!.onPress!();
});
beforeEach(() => {
  mocks.fetch.mockReset(); mocks.erro.mockReset(); mocks.saved.mockReset();
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
    mocks.fetch.mockImplementation(async () => view(null, 'env', 3));
    await expect(getTelemetry(server)).resolves.toEqual({ consent: null, disabled_by: 'env', consent_scope: 3, enabled: false });
  });

  it('200 com corpo fora da forma é resposta inválida, não "sem telemetria"', async () => {
    for (const body of [{ consent: 'yes', disabled_by: null }, { consent: null, consent_scope: null }]) {
      mocks.fetch.mockImplementation(async () => json(body));
      await expect(getTelemetry(server)).rejects.toThrow(m.atualizar_resposta_invalida());
    }
  });

  it('o período do retrato só vem com aceite ligado dos escopos 2 e 3', async () => {
    const period = 'a'.repeat(32);
    for (const [consent_scope, expected] of [[2, period], [3, period], [4, undefined]] as const) {
      mocks.fetch.mockImplementation(async () => json({ consent: true, disabled_by: null, consent_scope, enabled: true, snapshot_period: period }));
      expect((await getTelemetry(server))?.snapshot_period).toBe(expected);
    }
    mocks.fetch.mockImplementation(async () => json({ consent: true, disabled_by: null, snapshot_period: period }));
    expect((await getTelemetry(server))?.snapshot_period).toBeUndefined();
  });

  it('não tenta aceite de escopo desconhecido; legado vai sem escopo', async () => {
    mocks.fetch.mockImplementation(async () => refused(409, 'telemetry_consent_scope_mismatch'));
    await expect(putTelemetry(server, true, 4)).rejects.toThrow(m.telemetry_scope_unknown());
    expect(puts()).toHaveLength(0);
    await expect(putTelemetry(server, true, 2)).rejects.toMatchObject({ status: 409 });
    await expect(putTelemetry(server, true, undefined)).rejects.toMatchObject({ status: 409 });
    expect(puts().map(([, init]) => JSON.parse(init.body))).toEqual([{ consent: true, consent_scope: 2 }, { consent: true }]);
  });
});

describe('linha de telemetria', () => {
  it('reabre o mesmo aviso depois de fechar e só liga ao confirmar', async () => {
    const alert = vi.spyOn(Alert, 'alert');
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'PUT' ? view(JSON.parse(init.body).consent, null, 3) : view(null, null, 3));
    await render();
    expect(row()?.textContent).toContain(m.auto_report_value_unanswered());
    await act(async () => row()!.click());
    const body = m.telemetry_b_question_body({ machines: 'fixture-a (tel-a.test)' });
    expect(alert.mock.calls[0][1]).toBe(body);
    expect(puts()).toHaveLength(0);
    await act(async () => alert.mock.calls[0][3]!.onDismiss!());
    expect(puts()).toHaveLength(0);
    await act(async () => row()!.click());
    expect(alert.mock.calls[1][1]).toBe(body);
    await confirmYes(alert);
    expect(puts()).toHaveLength(1);
    expect(puts()[0][0]).toBe(`${server.baseUrl}/api/telemetry`);
    expect(JSON.parse(puts()[0][1].body)).toEqual({ consent: true, consent_scope: 3 });
    expect(row()?.textContent).toContain(m.auto_report_on());
    expect(mocks.saved).toHaveBeenCalledWith(server, expect.objectContaining({ consent: true, consent_scope: 3, enabled: true }));
  });

  it('servidor do escopo 2 recebe o aviso de preferências daquela versão', async () => {
    const alert = vi.spyOn(Alert, 'alert');
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'PUT' ? view(true, null, 2) : view(null, null, 2));
    await render();
    await act(async () => row()!.click());
    expect(alert.mock.calls[0][1]).toBe(m.telemetry_question_body({ machines: 'fixture-a (tel-a.test)' }));
    await confirmYes(alert);
    expect(JSON.parse(puts()[0][1].body)).toEqual({ consent: true, consent_scope: 2 });
  });

  it('"Não enviar" no aviso grava a recusa da máquina ainda sem resposta', async () => {
    const alert = vi.spyOn(Alert, 'alert');
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'PUT' ? view(JSON.parse(init.body).consent, null, 3) : view(null, null, 3));
    await render();
    await act(async () => row()!.click());
    await act(async () => alert.mock.calls[0][2]!.find((button) => button.text === m.telemetry_no())!.onPress!());
    expect(puts().map(([, init]) => JSON.parse(init.body))).toEqual([{ consent: false }]);
    expect(row()?.textContent).toContain(m.auto_report_off());
  });

  it('mesmo com o ambiente desligado, pode revogar o aceite salvo', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => view(init?.method !== 'PUT', 'ci', 3));
    await render();
    expect(el.textContent).toContain(m.telemetry_disabled_env());
    await act(async () => row()!.click());
    expect(puts()).toHaveLength(1);
    expect(JSON.parse(puts()[0][1].body)).toEqual({ consent: false });
  });

  it('aceite salvo que o servidor não está usando mostra o aviso e ainda revoga', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => json({ consent: init?.method !== 'PUT', disabled_by: null, consent_scope: 3, enabled: false }));
    await render();
    expect(el.textContent).toContain(m.telemetry_not_active());
    await act(async () => row()!.click());
    expect(puts().map(([, init]) => JSON.parse(init.body))).toEqual([{ consent: false }]);
  });

  it('usa escopo 3 no ajuste e nunca repete a gravação no formato legado', async () => {
    mocks.fetch.mockImplementation(async (_url, init) => init?.method === 'PUT' ? refused(422, 'telemetry_consent_scope_required') : view(null, null, 3));
    const alert = vi.spyOn(Alert, 'alert');
    await render();
    expect(el.textContent).toContain('400');
    await act(async () => row()!.click());
    expect(alert.mock.calls[0][1]).toBe(m.telemetry_b_question_body({ machines: 'fixture-a (tel-a.test)' }));
    await confirmYes(alert);
    expect(puts()).toHaveLength(1);
    expect(JSON.parse(puts()[0][1].body)).toEqual({ consent: true, consent_scope: 3 });
  });

  it('"O que é enviado" abre a lista no repositório', async () => {
    const open = vi.spyOn(Linking, 'openURL');
    mocks.fetch.mockImplementation(async () => view(false));
    await render();
    await act(async () => (el.querySelector(`[aria-label="${m.telemetry_what_is_sent()}"]`) as HTMLButtonElement).click());
    expect(open).toHaveBeenCalledWith(LEGACY_TELEMETRY_DOC_URL);
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
  it('um aviso por escopo conhecido e nenhum para escopo que o app não conhece', async () => {
    const alert = vi.spyOn(Alert, 'alert');
    const scope2 = { ...other, id: 'tel-c', label: 'fixture-c', baseUrl: 'https://tel-c.test' };
    mocks.fetch.mockImplementation(async (url) => String(url).startsWith(server.baseUrl) ? view(true, null, 3)
      : String(url).startsWith(scope2.baseUrl) ? view(true, null, 2) : view(true));
    const asked = askTelemetry([pending(other), pending(server, 3), pending(scope2, 2), pending({ ...other, label: 'fixture-unknown' }, 4)]);
    const answer = async (n: number) => { alert.mock.calls[n][2]!.find((button) => button.text === m.telemetry_yes())!.onPress!(); await Promise.resolve(); };
    expect(alert.mock.calls[0][1]).toBe(m.telemetry_b_question_body({ machines: 'fixture-a' }));
    await answer(0);
    expect(alert.mock.calls[1][1]).toBe(m.telemetry_question_body({ machines: 'fixture-c' }));
    await answer(1);
    expect(alert.mock.calls[2][1]).toBe(m.telemetry_legacy_question_body({ machines: 'fixture-b' }));
    await answer(2);
    await asked;
    expect(alert).toHaveBeenCalledTimes(3);
    expect(puts().map(([, init]) => JSON.parse(init.body))).toEqual([{ consent: true, consent_scope: 3 }, { consent: true, consent_scope: 2 }, { consent: true }]);
  });

  it('fechar sem tocar não grava nada', async () => {
    const alert = vi.spyOn(Alert, 'alert');
    const asked = askTelemetry([pending(server, 3), pending(other, 3)]);
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
      ? refused(503, 'telemetry_unavailable') : view(true, null, 3));
    const asked = askTelemetry([pending(server, 3), pending(other, 3)]);
    const press = (text: string) => alert.mock.calls.at(-1)![2]!.find((button) => button.text === text)!.onPress!();
    press(m.telemetry_what_is_sent());
    expect(open).toHaveBeenCalledWith(TELEMETRY_DOC_URL);
    expect(alert).toHaveBeenCalledTimes(2);
    expect(mocks.fetch).not.toHaveBeenCalled();
    press(m.telemetry_yes());
    await asked;
    await vi.waitFor(() => expect(mocks.erro).toHaveBeenCalledWith(m.auto_report_save_failed({ machines: 'fixture-b' })));
    expect(puts().map(([url, init]) => [url, JSON.parse(init.body)])).toEqual([
      [`${server.baseUrl}/api/telemetry`, { consent: true, consent_scope: 3 }],
      [`${other.baseUrl}/api/telemetry`, { consent: true, consent_scope: 3 }],
    ]);
    expect(mocks.saved).toHaveBeenCalledWith(server, expect.objectContaining({ consent: true, consent_scope: 3 }));
  });

  it('a lista da abertura só leva máquina de aviso conhecido e sem resposta', async () => {
    const future = { ...other, id: 'tel-c', label: 'fixture-c', baseUrl: 'https://tel-c.test' };
    mocks.fetch.mockImplementation(async (url) => String(url).startsWith(future.baseUrl) ? view(null, null, 4)
      : String(url).startsWith(other.baseUrl) ? view(true, null, 3) : view(null, null, 3));
    const offered = await telemetryUnanswered([server, other, future]);
    expect(offered.map(({ server: s }) => s.id)).toEqual([server.id]);
  });
});

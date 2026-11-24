// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { ServerBucket, WhatsAppDraft, WhatsAppOutboxEvent } from '@hangar/core';
import { OutboxRequestSheet, OutboxRequests, SessionOutboxRequests } from './OutboxRequests';
import { openWhatsAppDraft, useWhatsAppTarget } from './target';
import { useSessions } from '../../stores/sessions';
import * as m from '../../paraglide/messages';

const mocks = vi.hoisted(() => ({ answer: vi.fn(), status: vi.fn(), ok: vi.fn(), erro: vi.fn() }));

vi.mock('@hangar/core', async (original) => ({ ...await original<object>(), answerWhatsAppDraft: mocks.answer, getWhatsApp: mocks.status }));
vi.mock('../machines/parts', () => ({
  MachineSheet: ({ title, children }: { title: string; children: ReactNode }) => createElement('section', { 'aria-label': title }, children),
  Notice: ({ text }: { text: string }) => text ? createElement('p', { role: 'alert' }, text) : null,
}));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {}, deleteItemAsync: async () => {} }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {}, remove: () => {} }) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: mocks.ok, erro: mocks.erro } }));
vi.mock('../pair/identities', () => ({ useIdentities: () => ({ identities: new Map([['a', 'a']]), loading: new Set() }) }));

// Relógio fixo: a tool desiste 570 s depois de created_at (14:09:30), antes do expires_at (14:10:00); às 14:00:48 o
// cartão mostra 8:42.
const NOW = new Date('2026-11-24T14:00:48Z');

const draft = (id: string, extra: Partial<WhatsAppDraft> = {}): WhatsAppDraft => ({
  id, origin: 'a::sessao', chat: '5511987654321@s.whatsapp.net', chat_kind: 'contact', chat_number: '5511987654321',
  chat_name: 'Carlos', chat_name_source: 'contact', recent: [], text: 'nada perdido\u200B', reply_to: '', quoted_text: '',
  file: null, created_at: '2026-11-24T14:00:00Z', expires_at: '2026-11-24T14:10:00Z', state: 'pending', ...extra,
});
const event = (...drafts: WhatsAppDraft[]): WhatsAppOutboxEvent => ({ drafts });
const bucket = (id: string, extra: Partial<ServerBucket> = {}) => ({
  server: { id, label: id, baseUrl: `https://${id}.test`, token: 't' }, sessions: [], error: null, loaded: true, ...extra,
}) as unknown as ServerBucket;

let container: HTMLElement;
let root: Root;
const render = (byServer: ServerBucket[], outbox: Record<string, WhatsAppOutboxEvent>) => act(async () => {
  useSessions.setState({ byServer, whatsappOutbox: outbox });
  root.render(<><OutboxRequests byServer={byServer} /><OutboxRequestSheet /></>);
});
const notice = (label: string, count = 1) => container.querySelector<HTMLElement>(`button[aria-label="${m.whatsapp_draft_requests_title({ label, count })}"]`);
const button = (label: string) => container.querySelector<HTMLElement>(`button[aria-label="${label}"]`);
const sheet = () => container.querySelector(`section[aria-label="${m.whatsapp_draft_title()}"]`);

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(NOW);
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  container = document.createElement('div');
  root = createRoot(container);
  openWhatsAppDraft(null);
  mocks.answer.mockReset().mockResolvedValue({});
  mocks.status.mockReset();
  mocks.ok.mockReset();
  mocks.erro.mockReset();
});

afterEach(async () => {
  await act(async () => { root.unmount(); });
  useSessions.setState({ whatsappOutbox: {}, byServer: [] });
  openWhatsAppDraft(null);
  vi.useRealTimers();
});

it('tocar no aviso da máquina abre a folha com o número, o texto exato e o invisível marcado', async () => {
  await render([bucket('a'), bucket('b')], { a: event(draft('d1')) });
  expect(notice('b')).toBeNull();
  await act(async () => { notice('a')!.click(); });
  expect(useWhatsAppTarget.getState().target).toEqual({ serverId: 'a', draftId: null });
  expect(sheet()).not.toBeNull();
  expect(container.textContent).toContain('5511987654321');
  expect(container.textContent).toContain('nada perdido⟨U+200B⟩');
  expect(container.textContent).toContain(m.whatsapp_draft_text({ count: 13 }));
  expect(container.textContent).toContain('8:42');
});

it('máquina com erro ou ainda carregando não mostra o aviso', async () => {
  await render([bucket('a', { error: 'offline' })], { a: event(draft('d2')) });
  expect(notice('a')).toBeNull();
  await render([bucket('a', { loaded: false })], { a: event(draft('d2')) });
  expect(notice('a')).toBeNull();
});

it('no chat da sessão que pediu, o aviso abre aquele rascunho; em outra sessão, nada', async () => {
  useSessions.setState({ byServer: [bucket('a')], whatsappOutbox: { a: event(draft('d3')) } });
  const banner = m.whatsapp_draft_session_notice({ name: 'Carlos' });
  await act(async () => { root.render(<SessionOutboxRequests serverId="a" name="outra" />); });
  expect(button(banner)).toBeNull();
  await act(async () => { root.render(<><SessionOutboxRequests serverId="a" name="sessao" /><OutboxRequestSheet /></>); });
  await act(async () => { button(banner)!.click(); });
  expect(useWhatsAppTarget.getState().target).toEqual({ serverId: 'a', draftId: 'd3' });
  expect(container.textContent).toContain('5511987654321');
});

it('Enviar responde com via mobile e o cartão some', async () => {
  await render([bucket('a')], { a: event(draft('d4')) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: null }); });
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  expect(mocks.answer).toHaveBeenCalledWith(expect.objectContaining({ id: 'a' }), 'd4', 'send', 'mobile');
  expect(mocks.ok).toHaveBeenCalledWith(m.whatsapp_draft_sending());
  expect(useWhatsAppTarget.getState().target).toBeNull();
  expect(notice('a')).toBeNull();
});

it('Recusar responde decline', async () => {
  await render([bucket('a')], { a: event(draft('d5')) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: 'd5' }); });
  await act(async () => { button(m.whatsapp_draft_decline())!.click(); });
  expect(mocks.answer).toHaveBeenCalledWith(expect.objectContaining({ id: 'a' }), 'd5', 'decline', 'mobile');
  expect(mocks.ok).toHaveBeenCalledWith(m.whatsapp_draft_declined());
});

it('recusa com frase própria deixa o cartão; a trava diz até quando pelo status', async () => {
  await render([bucket('a')], { a: event(draft('d6')) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: 'd6' }); });
  mocks.answer.mockRejectedValueOnce(Object.assign(new Error('429: limite'), { status: 429, code: 'whatsapp_rate_limited' }));
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.whatsapp_draft_err_rate_limited());
  expect(sheet()).not.toBeNull();

  mocks.answer.mockRejectedValueOnce(Object.assign(new Error('409: trava'), { status: 409, code: 'whatsapp_reachout_locked' }));
  mocks.status.mockResolvedValueOnce({ status: { reachout_locked_until: '2026-11-25T12:30:00Z' } });
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  const text = container.querySelector('[role="alert"]')?.textContent ?? '';
  expect(text).not.toBe(m.whatsapp_draft_err_reachout_locked_unknown());
  expect(text).toMatch(/\d{2}:\d{2}/);

  mocks.answer.mockRejectedValueOnce(Object.assign(new Error('409: velho'), { status: 409, code: 'whatsapp_outdated' }));
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.whatsapp_draft_err_outdated());
});

it('expirado tira o cartão e avisa o motivo', async () => {
  await render([bucket('a')], { a: event(draft('d7')) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: null }); });
  mocks.answer.mockRejectedValueOnce(Object.assign(new Error('409: expired'), { status: 409, code: 'whatsapp_expired' }));
  await act(async () => { button(m.whatsapp_draft_send())!.click(); });
  expect(mocks.erro).toHaveBeenCalledWith(m.whatsapp_draft_err_expired());
  expect(useWhatsAppTarget.getState().target).toBeNull();
  expect(notice('a')).toBeNull();
});

it('anexo fora da pasta da sessão ganha a linha de aviso e o nome escolhido pela pessoa, o dela', async () => {
  const file = { name: 'custos.xlsx', size: 88 * 1024, sha256: 'x', source: '/home/j/custos.xlsx', outside_cwd: true };
  await render([bucket('a')], { a: event(draft('d8', { file, chat_name_source: 'push' })) });
  await act(async () => { openWhatsAppDraft({ serverId: 'a', draftId: 'd8' }); });
  expect(container.textContent).toContain('/home/j/custos.xlsx');
  expect(container.textContent).toContain('custos.xlsx · 88 KB');
  expect(container.textContent).toContain(m.whatsapp_draft_outside_cwd());
  expect(container.textContent).toContain(m.whatsapp_draft_push_warning());
});

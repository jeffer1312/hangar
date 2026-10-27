// @vitest-environment happy-dom
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, type UpdateNotes } from '@hangar/core';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ fetch: vi.fn(), store: new Map<string, string>() }));
vi.mock('react-native', async (original) => ({
  ...await original<typeof import('react-native')>(),
  Switch: ({ value, onValueChange, accessibilityLabel }: { value: boolean; onValueChange: (v: boolean) => void; accessibilityLabel: string }) =>
    React.createElement('button', { 'aria-label': accessibilityLabel, 'data-switch': String(value), onClick: () => onValueChange(!value) }),
}));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({
  getString: (k: string) => mocks.store.get(k), set: (k: string, v: string) => { mocks.store.set(k, v); }, remove: (k: string) => { mocks.store.delete(k); },
}) }));
vi.mock('../../chat/AssistantBubble', () => ({ mkMarkdownStyle: () => ({}) }));
vi.mock('react-native-enriched-markdown', () => ({ EnrichedMarkdownText: ({ markdown }: { markdown: string }) => React.createElement('article', null, markdown) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
// O `Notice` vem de machines/parts, que importa a folha e o teclado nativos.
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => children }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children }: { children: React.ReactNode }) => children }));
import { UpdateConfirmSheet } from './UpdateConfirmSheet';
import { readPendingNews, type ConfirmRequest } from './pending';
import * as m from '../../paraglide/messages';

const server = { id: 'srv', label: 'fixture-machine', baseUrl: 'https://notes.test', token: 'fixture-token' };
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
const notes: UpdateNotes = {
  target: 'origin/main',
  warnings: [{ id: 'w1', title: 'fixture-warning-title', text: 'fixture-warning-text' }],
  news: [
    { id: 'n1', title: 'fixture-card', kind: 'new', text: 'fixture-card-text', platforms: 'all', try_it: 'fixture-try', animation: 'unknown' },
    { id: 'n2', title: 'fixture-desktop-only', kind: 'new', text: '', platforms: 'desktop', try_it: null, animation: null },
    { id: 'f1', title: 'fixture-fix', kind: 'fix', text: '', platforms: 'mobile', try_it: null, animation: null },
  ],
};
let root: Root;
let el: HTMLDivElement;
const button = (label: string) => [...el.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label || b.textContent === label) as HTMLButtonElement;
async function open(notesResponse: () => Response) {
  mocks.fetch.mockImplementation(async (url: string) => url.includes('/api/update/notes') ? notesResponse()
    : json({ versao_legivel: { repo: null, backend: 'v1.0', remoto: 'v1.1' } }));
  const request: ConfirmRequest = { server, resolve: vi.fn() };
  const onClose = vi.fn();
  await act(async () => root.render(<UpdateConfirmSheet request={request} onClose={onClose} />));
  await act(async () => { await Promise.resolve(); });
  return { request, onClose };
}
beforeEach(() => {
  mocks.fetch.mockReset(); mocks.store.clear();
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', origin: null, onUnauthorized: vi.fn(), createEventSource: vi.fn() });
  vi.stubGlobal('fetch', mocks.fetch);
  el = document.createElement('div'); document.body.appendChild(el); root = createRoot(el);
});
afterEach(async () => { await act(async () => root.unmount()); el.remove(); vi.unstubAllGlobals(); });

describe('folha "Atualizar?"', () => {
  it('aviso trava o Atualizar até "Entendi"; confirma e guarda cartões e correções do celular', async () => {
    const { request, onClose } = await open(() => json(notes));
    expect(el.textContent).toContain('v1.0 → v1.1');
    expect(el.textContent).toContain('fixture-warning-text');
    expect(el.textContent).toContain('fixture-card');
    expect(el.textContent).toContain(m.update_notes_try_it({ text: 'fixture-try' }));
    expect(el.textContent).not.toContain('fixture-desktop-only');
    expect(el.textContent).toContain(m.update_notes_fixes({ n: '1' }));
    expect(el.textContent).not.toContain('fixture-fix');
    const update = button(m.update_confirm_action());
    expect(update.disabled).toBe(true);
    await act(async () => button(m.update_notes_understood()).click());
    expect(update.disabled).toBe(false);
    await act(async () => update.click());
    expect(request.resolve).toHaveBeenCalledWith(true, { srv: { ids: ['w1'] } });
    expect(onClose).toHaveBeenCalled();
    const saved = readPendingNews(server.id);
    expect([...saved?.cards ?? [], ...saved?.fixes ?? []].map((n) => n.id)).toEqual(['n1', 'f1']);
  });

  it('falha ao ler os avisos mostra uma linha e deixa atualizar', async () => {
    const { request } = await open(() => json({ code: 'update_notes_unavailable' }, 503));
    expect(el.textContent).toContain(m.update_notes_failed());
    await act(async () => button(m.update_confirm_action()).click());
    expect(request.resolve).toHaveBeenCalledWith(true, {});
  });

  it('aviso da migração pede o modo, com o pacote marcado; a escolha volta com os ids', async () => {
    const migration = { id: 'mig', title: 'fixture-migration', text: 'fixture-migration-text', choice: 'mode' as const, mode: null };
    const { request } = await open(() => json({ ...notes, warnings: [migration] }));
    expect(el.textContent).toContain(m.update_choice_title());
    await act(async () => button(m.update_choice_developer()).click());
    await act(async () => button(m.update_notes_understood()).click());
    await act(async () => button(m.update_confirm_action()).click());
    expect(request.resolve).toHaveBeenCalledWith(true, { srv: { ids: ['mig'], mode: 'developer' } });
  });

  it('"Atualizar todas" agrupa os avisos por máquina e diz quem não respondeu', async () => {
    mocks.fetch.mockImplementation(async (url: string) => url.includes('/api/update/notes') ? json({ ...notes, warnings: [] })
      : json({ versao_legivel: { repo: null, backend: 'v1.0', remoto: 'v1.1' } }));
    const other = { ...server, id: 'other', label: 'fixture-other' };
    const down = { ...server, id: 'down', label: 'fixture-down' };
    const migration = { id: 'mig', title: 'fixture-migration', text: '', choice: 'mode' as const, mode: 'package' as const };
    const request: ConfirmRequest = { server, count: 3, resolve: vi.fn(), machines: Promise.resolve({
      run: [{ server, notes: null }, { server: other, notes: { ...notes, warnings: [migration] } }], failed: [down] }) };
    await act(async () => root.render(<UpdateConfirmSheet request={request} onClose={vi.fn()} />));
    await act(async () => { await Promise.resolve(); });
    expect(el.textContent).toContain(m.update_all_warnings_from({ server: 'fixture-other' }));
    expect(el.textContent).not.toContain(m.update_all_warnings_from({ server: server.label }));
    expect(el.textContent).toContain(m.update_all_warnings_failed({ server: 'fixture-down' }));
    await act(async () => button(m.update_notes_understood()).click());
    await act(async () => button(m.update_confirm_action()).click());
    expect(request.resolve).toHaveBeenCalledWith(true, { other: { ids: ['mig'], mode: 'package' } });
  });

  it('servidor antigo (404) mostra só a confirmação; fechar é cancelar', async () => {
    const { request } = await open(() => json({}, 404));
    expect(el.textContent).not.toContain(m.update_notes_failed());
    expect(el.textContent).not.toContain(m.update_notes_important());
    expect(button(m.update_confirm_action()).disabled).toBe(false);
    await act(async () => root.unmount());
    root = createRoot(el);
    expect(request.resolve).toHaveBeenCalledWith(false);
  });
});

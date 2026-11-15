import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { configureApi, type Server } from '@hangar/core';
const data = vi.hoisted(() => new Map<string, string>());
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: (key: string) => data.get(key), set: (key: string, value: string) => data.set(key, value), remove: (key: string) => data.delete(key) }) }));
import { readDraft, writeDraft, type ConversationDraft } from '../../stores/drafts';
import { prepareReviewDraft } from './review-draft';

const server: Server = { id: 'server-a', label: 'Servidor A', baseUrl: 'http://machine.test', token: 'machine-token' };
const original: ConversationDraft = { version: 1, text: 'Rascunho existente.', revision: 4, transcript: 'conversation-1', attachments: [], submission: null };
beforeEach(() => {
  data.clear();
  configureApi({ getBaseUrl: () => 'http://other.test', getToken: () => 'other-token', origin: null, onUnauthorized: () => {}, createEventSource: () => { throw new Error('Sem stream nesta prova.'); } });
});
afterEach(() => vi.unstubAllGlobals());

it('acrescenta comentários ao rascunho da sessão e da máquina escolhidas, sem enviá-los', async () => {
  const calls: { url: string; method: string }[] = [];
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), method: init?.method ?? 'GET' });
    return new Response(JSON.stringify([{ name: 'review-session', provider: 'claude', state: 'idle', jsonl: 'conversation-1' }]), { status: 200 });
  }));
  writeDraft(server.id, 'review-session', original);
  await prepareReviewDraft(server, 'review-session', 'Comentários da revisão.', () => true);
  expect(readDraft(server.id, 'review-session')).toMatchObject({ text: 'Rascunho existente.\n\nComentários da revisão.', revision: 5, transcript: 'conversation-1' });
  expect(calls).toHaveLength(1);
  expect(calls[0]).toEqual({ url: 'http://machine.test/api/sessions', method: 'GET' });
});

it('não altera um rascunho de outra conversa nem um destino que deixou de estar selecionado', async () => {
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify([{ name: 'review-session', provider: 'claude', state: 'idle', jsonl: 'conversation-2' }]), { status: 200 })));
  writeDraft(server.id, 'review-session', original);
  await expect(prepareReviewDraft(server, 'review-session', 'Comentário.', () => true)).rejects.toThrow();
  expect(readDraft(server.id, 'review-session')).toEqual(original);
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify([{ name: 'review-session', provider: 'claude', state: 'idle', jsonl: 'conversation-1' }]), { status: 200 })));
  await expect(prepareReviewDraft(server, 'review-session', 'Comentário.', () => false)).rejects.toThrow();
  expect(readDraft(server.id, 'review-session')).toEqual(original);
});

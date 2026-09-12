/** @vitest-environment happy-dom */
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { configureApi } from '@hangar/core';
import type { AskQuestionPayload } from '@hangar/core';
import { chatStore, _resetChatsForTests } from '../../stores/chat';
import * as m from '../../paraglide/messages';
vi.mock('../../stores/servers', () => ({ useServers: { getState: () => ({
  servers: [{ id: 'srv', baseUrl: 'http://teste' }],
}) } }));

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ back: vi.fn(), replace: vi.fn() }));
vi.mock('expo-router', () => ({
  useLocalSearchParams: () => ({ server: 'srv', name: 'sess' }),
  useRouter: () => ({ canGoBack: () => true, ...mocks }),
}));
vi.mock('react-native-keyboard-controller', () => ({
  KeyboardAvoidingView: ({ children }: { children: React.ReactNode }) => React.createElement('div', null, children),
}));
vi.mock('react-native', async (original) => ({
  ...await original<object>(),
  TextInput: ({ secureTextEntry, value, onChangeText }: {
    secureTextEntry?: boolean; value: string; onChangeText: (value: string) => void;
  }) => React.createElement('input', {
    type: secureTextEntry ? 'password' : 'text', value,
    onInput: (event: React.FormEvent<HTMLInputElement>) => onChangeText(event.currentTarget.value),
  }),
}));

import AskSheet from '../../../app/s/[server]/[name]/ask';

let root: Root;
let container: HTMLDivElement;
const payload: AskQuestionPayload = {
  provider: 'codex', request_id: 41,
  questions: [{ id: 'choice', header: 'Escolha', question: 'Qual?', multiSelect: false,
    options: [{ label: 'Primeira', description: '' }] }],
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{}', { status: 200 })));
  configureApi({
    getBaseUrl: () => 'http://localhost:8765', getToken: () => 'tok', onUnauthorized: () => {},
    origin: null, createEventSource: vi.fn(),
  });
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  _resetChatsForTests();
  vi.unstubAllGlobals();
});

async function open(value = payload) {
  chatStore('srv', 'sess').openAsk(value);
  await act(async () => root.render(React.createElement(AskSheet)));
}

async function click(label: string) {
  const button = [...container.querySelectorAll('button')].find((item) => item.textContent === label);
  expect(button, label).toBeDefined();
  await act(async () => button!.click());
}

test('envia identidades nativas e mantém a pergunta quando RPC falha', async () => {
  vi.stubGlobal('fetch', vi.fn(async () => new Response('{"detail":"indisponível"}', { status: 503 })));
  await open();
  expect(container.textContent).not.toContain(m.askq_digitar_resposta());
  expect(container.textContent).not.toContain(m.askq_conversar_sobre());
  await click('Primeira');
  await click(m.lista_enviar());
  const options = vi.mocked(fetch).mock.calls[0][1];
  expect(JSON.parse(options?.body as string)).toMatchObject({
    request_id: 41, answers: [{ question_id: 'choice', indices: [0] }],
  });
  expect(chatStore('srv', 'sess').use.getState().askOpen).toBe(true);
  expect(mocks.replace).not.toHaveBeenCalled();
  expect(container.textContent).toContain('indisponível');
});

test('troca a pergunta aberta pelo ID novo e protege texto secreto na entrada e revisão', async () => {
  await open();
  await click('Primeira');
  await act(async () => chatStore('srv', 'sess').openAsk({
    provider: 'codex', request_id: 42,
    questions: [{ id: 'secret', header: 'Segredo', question: 'Qual segredo?', multiSelect: false,
      isSecret: true, options: [] }],
  }));
  const input = container.querySelector('input')!;
  expect(input.type).toBe('password');
  await act(async () => {
    input.value = 'valor-secreto';
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await click(m.comum_confirmar());
  expect(container.textContent).toContain('••••••');
  expect(container.textContent).not.toContain('valor-secreto');
  await click(m.lista_enviar());
  expect(JSON.parse(vi.mocked(fetch).mock.calls[0][1]?.body as string)).toMatchObject({
    request_id: 42, answers: [{ question_id: 'secret', value: 'valor-secreto' }],
  });
});

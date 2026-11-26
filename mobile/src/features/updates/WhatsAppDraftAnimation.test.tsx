// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import { fmtBytes } from '@hangar/core';
import * as m from '../../paraglide/messages';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Glass', () => ({ Glass: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('../../ui/MultilineInput', () => ({ MultilineInput: () => null }));
vi.mock('../../ui/RecordingWave', () => ({ RecordingWave: () => null }));
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: () => {}, erro: () => {} } }));
vi.mock('../machines/parts', () => ({
  MachineSheet: ({ children }: { children: ReactNode }) => createElement('section', null, children),
  Notice: ({ text }: { text: string }) => text ? createElement('p', { role: 'alert' }, text) : null,
}));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {}, deleteItemAsync: async () => {} }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {}, remove: () => {} }) }));
vi.mock('../pair/identities', () => ({ useIdentities: () => ({ identities: new Map(), loading: new Set() }) }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import { WhatsAppDraftAnimation, sampleDraft } from './WhatsAppDraftAnimation';

it('movimento reduzido para no cartão aberto: número, texto exato, anexo com caminho e o Enviar', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(WhatsAppDraftAnimation, { width: 340 })));
  try {
    const text = container.textContent ?? '';
    const draft = sampleDraft();
    expect(text).toContain('5500912345678');
    expect(text).toContain(m.whatsapp_draft_news_text());
    expect(text).toContain(`${draft.file!.name} · ${fmtBytes(draft.file!.size)}`);
    expect(text).toContain(draft.file!.source);
    expect(text).toContain(m.whatsapp_draft_send());
    expect(text).toContain('9:12');
    expect(text).not.toContain(m.whatsapp_draft_sending());
  } finally {
    act(() => root.unmount());
  }
});

it('o rascunho de exemplo usa número fictício e o anexo dentro da pasta da sessão', () => {
  const draft = sampleDraft();
  expect(draft.chat_number.slice(2, 4)).toBe('00');
  expect(draft.file?.outside_cwd).toBe(false);
  expect(draft.state).toBe('pending');
});

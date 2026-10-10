// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { describe, expect, it, vi } from 'vitest';
import { TranscriptionProviders } from './TranscriptionProviders';
import type { ServerConfig } from './serverConfig';
import * as m from '../../paraglide/messages';
import { pickFile } from '../../ui/attachmentPicker';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
vi.mock('../../ui/attachmentPicker', () => ({ pickFile: vi.fn() }));
vi.mock('./serverConfig', () => ({ PROVIDERS: 'transcription_providers' }));
vi.mock('react-native', async () => {
  const { createElement } = await import('react');
  const widget = (tag: string) => ({ children, onPress, accessibilityLabel, disabled }: {
    children?: import('react').ReactNode; onPress?: () => void; accessibilityLabel?: string; disabled?: boolean;
  }) => createElement(tag, { onClick: onPress, 'aria-label': accessibilityLabel, disabled }, children);
  return { View: widget('div'), Text: widget('span'), Pressable: widget('button'), TextInput: widget('input'), Linking: { openURL: vi.fn() } };
});
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('./colors', () => ({ useSettingsColors: () => ({ text: '#000', muted: '#555', borderStrong: '#aaa' }) }));
vi.mock('./ServerConfigParts', () => ({
  BlockHead: () => null, Box: ({ children }: { children: import('react').ReactNode }) => children, useInputStyle: () => ({}),
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(), getTranscriptionProvidersStatus: vi.fn(async () => ({ providers: [] })),
}));

describe('seleção de áudio para testar o serviço', () => {
  it('dois toques abrem um seletor e cancelar libera uma nova tentativa', async () => {
    let cancel!: (value: null) => void;
    vi.mocked(pickFile).mockReturnValue(new Promise((resolve) => { cancel = resolve; }));
    const provider = { id: 'p', kind: 'openai', name: '', base_url: '', api_key: '', model: '' };
    const cfg = { server: { id: 's', label: 'fixture', baseUrl: 'https://fixture.test', token: 'fixture' },
      fields: { transcription_providers: { valor: [provider] } }, load: { status: 'ready' }, draft: {},
      current: () => [provider], stage: vi.fn() } as unknown as ServerConfig;
    const target = document.createElement('div'); document.body.appendChild(target);
    const root = createRoot(target);
    await act(async () => { root.render(createElement(TranscriptionProviders, { cfg })); });
    const button = target.querySelector<HTMLButtonElement>(`button[aria-label="${m.native_voice_provider_test_audio()}"]`)!;
    act(() => { button.click(); button.click(); });
    const selections = vi.mocked(pickFile).mock.calls.length;
    await act(async () => { cancel(null); });
    vi.mocked(pickFile).mockResolvedValue(null);
    await act(async () => { button.click(); });
    await act(async () => { root.unmount(); }); target.remove();
    expect(selections).toBe(1);
    expect(pickFile).toHaveBeenCalledTimes(2);
  });
});

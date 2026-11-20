// @vitest-environment happy-dom
import { act, createElement } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import * as m from '../../paraglide/messages';

// O mock padrão do reanimated já responde movimento reduzido.
vi.mock('expo-router', () => ({ useIsFocused: () => true, useRouter: () => ({ push: vi.fn() }) }));
// A folha de ajustes inteira traz módulos nativos; aqui só importa a velocidade que a animação entrega ao controle.
vi.mock('../../chat/SessionSettings', () => ({
  FastControl: ({ fast }: { fast: { tier: string } }) => createElement('div', { 'data-tier': fast.tier }),
}));
// Os rótulos vêm do controle da velocidade, que lê a lista de servidores guardada no aparelho.
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {} }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {} }) }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
(HTMLElement.prototype as HTMLElement & { measureInWindow?: unknown }).measureInWindow =
  (done: (x: number, y: number, w: number, h: number) => void) => done(0, 0, 340, 400);

import { SpeedChoiceAnimation } from './SpeedChoiceAnimation';

it('movimento reduzido para no Ultra Fast escolhido, com a linha do modelo', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(SpeedChoiceAnimation, { width: 340 })));
  try {
    expect(container.querySelector('[data-tier]')?.getAttribute('data-tier')).toBe('ultrafast');
    expect(container.textContent).toContain(`gpt-6.2-sol · ${m.native_ctl_speed_ultrafast()}`);
  } finally {
    act(() => root.unmount());
  }
});

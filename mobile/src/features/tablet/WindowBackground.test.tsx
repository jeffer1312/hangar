// @vitest-environment happy-dom
import { act, createElement, useRef, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import { Platform } from 'react-native';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
vi.mock('expo-blur', () => ({ BlurTargetView: ({ children, ref }: { children: ReactNode; ref: React.Ref<HTMLDivElement> }) =>
  createElement('div', { ref }, children) }));
vi.mock('../../ui/Background', () => ({ Background: () => createElement('div', { 'data-wallpaper': true }) }));
vi.mock('expo-router', () => ({ useRouter: () => ({}), useSegments: () => ['create'], useLocalSearchParams: () => ({}) }));
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: ReactNode }) => children }));
vi.mock('../create/CreateSessionSheet', () => ({ CreateSessionSheet: () => createElement('textarea') }));

import { Screen } from '../../ui/Screen';
import { useBlurTarget } from '../../ui/blurTarget';
import CreateRoute from '../../../app/create';

it('a tela central herda um único fundo e o alvo de vidro da janela sem remontar ao girar', async () => {
  const host = document.createElement('div');
  const root = createRoot(host);
  const targets: unknown[] = [];
  let mounts = 0;
  function Content() {
    const instance = useRef<number | null>(null);
    if (instance.current === null) instance.current = ++mounts;
    targets.push(useBlurTarget());
    return createElement('textarea', { 'data-instance': instance.current });
  }
  function Window({ tablet }: { tablet: boolean }) {
    return createElement(Screen, { background: tablet, edges: [], children:
      createElement(Screen, { children: createElement(Content) }) });
  }
  try {
    await act(async () => root.render(createElement(Window, { tablet: true })));
    const field = host.querySelector('textarea');
    const target = targets.at(-1);
    expect(target).not.toBeNull();
    expect(host.querySelectorAll('[data-wallpaper]')).toHaveLength(1);
    await act(async () => root.render(createElement(Window, { tablet: false })));
    expect(host.querySelectorAll('[data-wallpaper]')).toHaveLength(1);
    expect(host.querySelector('textarea')).toBe(field);
    expect(mounts).toBe(1);
    await act(async () => root.render(createElement(Window, { tablet: true })));
    expect(host.querySelectorAll('[data-wallpaper]')).toHaveLength(1);
    expect(targets.at(-1)).toBe(target);
    expect(host.querySelector('textarea')).toBe(field);
  } finally {
    act(() => root.unmount());
  }
});

it('uma folha independente conserva fundo e alvo de vidro próprios dentro da janela de tablet', async () => {
  const host = document.createElement('div');
  const root = createRoot(host);
  const targets: unknown[] = [];
  function Probe() { targets.push(useBlurTarget()); return null; }
  try {
    await act(async () => root.render(createElement(Screen, { children: [
      createElement(Probe, { key: 'window' }),
      createElement(Screen, { key: 'sheet', independent: true, children: createElement(Probe) }),
    ] })));
    expect(host.querySelectorAll('[data-wallpaper]')).toHaveLength(2);
    expect(targets.at(-1)).not.toBeNull();
    expect(targets.at(-1)).not.toBe(targets.at(-2));
  } finally {
    act(() => root.unmount());
  }
});

it.each(['android', 'ios'] as const)('criação no %s pinta fundo próprio só quando é folha', async platform => {
  const previous = Platform.OS;
  Platform.OS = platform;
  const host = document.createElement('div');
  const root = createRoot(host);
  try {
    await act(async () => root.render(createElement(Screen, { children: createElement(CreateRoute) })));
    expect(host.querySelector('textarea')).not.toBeNull();
    expect(host.querySelectorAll('[data-wallpaper]')).toHaveLength(platform === 'android' ? 1 : 2);
  } finally {
    act(() => root.unmount());
    Platform.OS = previous;
  }
});

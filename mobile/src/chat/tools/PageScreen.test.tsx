// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const route = vi.hoisted(() => ({ params: { server: 'a', name: 'session', id: 'page', title: 'page-title' }, canGoBack: () => false, replace: vi.fn(), screen: vi.fn(), loadSource: vi.fn(async () => '<html>source</html>') }));
vi.mock('expo-router', () => ({ useRouter: () => route, useLocalSearchParams: () => route.params, Stack: { Screen: ({ options }: { options: { headerRight?: () => ReactNode } }) => { route.screen(options); return options.headerRight?.() ?? null; } } }));
vi.mock('expo-router/react-navigation', () => ({ useHeaderHeight: () => 64 }));
vi.mock('react-native-unistyles', async (original) => {
  const module = await original<typeof import('react-native-unistyles')>();
  return { ...module, useUnistyles: () => { const value = module.useUnistyles(); return { ...value, theme: new Proxy(value.theme, {}) }; } };
});
vi.mock('./pageDocument', () => ({ onlyInitial: () => true, usePageDocument: () => ({ state: 'ready', doc: '<html>page</html>', load: vi.fn(), params: {}, loadSource: route.loadSource }), usePageWebView: () => ({ web: null, applyTheme: vi.fn(), touched: vi.fn(), openLink: vi.fn() }) }));
vi.mock('../../ui/Screen', () => ({ Screen: ({ children }: { children: ReactNode }) => children }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
vi.mock('../../features/attachments/mediaCache', () => ({ canShareFile: false, shareTextFile: vi.fn() }));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));
vi.mock('expo-blur', () => ({ BlurView: () => null }));
vi.mock('expo-glass-effect', () => ({ GlassView: () => null, isLiquidGlassAvailable: () => false }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../stores/aparencia', () => ({ useAparencia: (select: (s: unknown) => unknown) => select({ fundo: 'flat', imagemUri: null }) }));
import PageScreen from '../../../app/s/[server]/[name]/page';
import * as m from '../../paraglide/messages';

it('preserva opções e ações da página ao redesenhar sem alteração', async () => {
  const el = document.createElement('div');
  const root = createRoot(el);
  const render = () => act(async () => root.render(createElement(PageScreen)));
  try {
    await render();
    const initial = route.screen.mock.calls.at(-1)![0];
    await render();
    expect(route.screen.mock.calls.at(-1)![0]).toBe(initial);
    await act(async () => [...el.querySelectorAll('button')].find(button => button.textContent === m.page_view_source())!.click());
    const sourceOptions = route.screen.mock.calls.at(-1)![0];
    expect(sourceOptions).not.toBe(initial);
    expect(el.textContent).toContain(m.page_view_page());
    await render();
    expect(route.screen.mock.calls.at(-1)![0]).toBe(sourceOptions);
  } finally {
    await act(async () => root.unmount());
  }
});

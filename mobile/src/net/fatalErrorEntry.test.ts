import { readFileSync } from 'node:fs';
import { createContext, runInContext } from 'node:vm';
import { expect, it, vi } from 'vitest';

it('instala captura antes de carregar rotas e identifica o bundle OTA', () => {
  const order: string[] = [];
  const install = vi.fn(() => { order.push('capture'); });
  const errorUtils = { getGlobalHandler: vi.fn(), setGlobalHandler: vi.fn() };
  const listener = vi.fn();
  const exceptions = { unstable_setExceptionDecorator: vi.fn() };
  const entry = readFileSync(new URL('../../index.js', import.meta.url), 'utf8');
  runInContext(entry, createContext({
    ErrorUtils: errorUtils, RN$registerExceptionListener: listener,
    require: (path: string) => {
      if (path === './src/net/appStart') { order.push('start'); return {}; }
      if (path === 'expo') { order.push('expo'); return {}; }
      if (path === 'react-native') return { Platform: { OS: 'ios' } };
      if (path === 'expo-constants') return { default: { expoConfig: { version: '0.1.0', runtimeVersion: 'native-2' } } };
      if (path === 'expo-updates') return { updateId: 'ota-one', isEmbeddedLaunch: false };
      if (path === './src/net/fatalErrors') return { installFatalErrorCapture: install };
      if (path === 'react-native/Libraries/Core/ExceptionsManager') return { default: exceptions };
      if (path === 'expo-router/entry') { order.push('router'); return {}; }
      throw Error(`unexpected-module:${path}`);
    },
  }));
  expect(order).toEqual(['start', 'expo', 'capture', 'router']);
  expect(install).toHaveBeenCalledWith({ errorUtils, exceptions, registerExceptionListener: listener,
    metadata: { platform: 'ios', version: '0.1.0', runtime: 'native-2', updateId: 'ota-one', embedded: false } });
});

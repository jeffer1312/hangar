import { expect, test } from 'vitest';
import { configureApi, apiEnv, _resetApiEnvForTests } from './apiEnv';

test('sem configurar, lanca erro claro', () => {
  _resetApiEnvForTests();
  expect(() => apiEnv()).toThrow(/configureApi/);
});
test('configurado, devolve o env', () => {
  const env = { getBaseUrl: () => 'http://x', getToken: () => 't', onUnauthorized() {}, origin: null,
    createEventSource: () => ({ addEventListener() {}, removeEventListener() {}, close() {}, onerror: null, onopen: null, readyState: 0 }) };
  configureApi(env);
  expect(apiEnv().getBaseUrl()).toBe('http://x');
});

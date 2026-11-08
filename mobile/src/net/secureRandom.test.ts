import { afterEach, expect, test, vi } from 'vitest';
import { secureRandom } from './secureRandom';

afterEach(() => vi.unstubAllGlobals());

test('junta dois UUIDs do Expo', () => {
  const uuids = ['aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'];
  vi.stubGlobal('expo', { uuidv4: () => uuids.shift() });
  expect(secureRandom()).toBe(`${'a'.repeat(8)}${'a'.repeat(4)}4aaa8aaa${'a'.repeat(12)}${'b'.repeat(8)}${'b'.repeat(4)}4bbb8bbb${'b'.repeat(12)}`);
});

test('sem o gerador do Expo não inventa segredo', () => {
  vi.stubGlobal('expo', undefined);
  expect(secureRandom()).toBeNull();
});

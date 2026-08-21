import { overwriteGetLocale } from './paraglide/runtime';
import { configureLocale, intlLocale, localeAtual } from './i18n';

test('locale injetado via configureLocale', () => {
  expect(localeAtual()).toBe('en'); // --strategy baseLocale
  configureLocale({ getLocale: () => 'pt' });
  expect(localeAtual()).toBe('pt');
  expect(intlLocale()).toBe('pt-BR');
});

test('overwriteGetLocale direto tambem vale (é o que format.test.ts usa)', () => {
  overwriteGetLocale(() => 'en');
  expect(intlLocale()).toBe('en-US');
});

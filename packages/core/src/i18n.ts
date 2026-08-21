import { getLocale, overwriteGetLocale } from './paraglide/runtime';

export const LOCALES = ['en', 'pt'] as const;
export type Locale = (typeof LOCALES)[number];

// Quem sabe o idioma é o app (localStorage no Svelte, expo-localization no RN). O core só pergunta.
export function configureLocale(env: { getLocale: () => Locale }): void {
  overwriteGetLocale(() => env.getLocale());
}

export function localeAtual(): Locale {
  return getLocale() as Locale;
}

export function intlLocale(): string {
  return localeAtual() === 'en' ? 'en-US' : 'pt-BR';
}

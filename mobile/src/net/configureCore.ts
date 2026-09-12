import { configureApi, configureLocale } from '@hangar/core';
import { getLocales } from 'expo-localization';
import { overwriteGetLocale } from '../paraglide/runtime';
import { useServers } from '../stores/servers';
import { useAparencia } from '../stores/aparencia';
import { createEventSource } from './sse';
import { iniciarDiag } from './diag';

export function configureCore() {
  // uma fonte só: o mesmo getLocale alimenta o runtime do core e o do paraglide mobile.
  // Escolha manual em Configurações vence o idioma do aparelho; `system` volta a segui-lo.
  const locale = (): 'pt' | 'en' => {
    const escolhido = useAparencia.getState().idioma;
    if (escolhido !== 'system') return escolhido;
    return getLocales()[0]?.languageCode === 'pt' ? 'pt' : 'en';
  };
  configureApi({
    getBaseUrl: () => useServers.getState().active()?.baseUrl ?? '',
    getToken: () => useServers.getState().active()?.token ?? null,
    onUnauthorized: () => {
      const a = useServers.getState().active();
      if (a) useServers.getState().markInvalid(a.id);
    },
    origin: null,
    createEventSource,
  });
  configureLocale({ getLocale: locale });
  overwriteGetLocale(locale);
  iniciarDiag();
}

import { configureApi, configureLocale } from '@hangar/core';
import { getLocales } from 'expo-localization';
import { overwriteGetLocale } from '../paraglide/runtime';
import { useServers } from '../stores/servers';
import { createEventSource } from './sse';

export function configureCore() {
  // uma fonte só: o mesmo getLocale alimenta o runtime do core e o do paraglide mobile
  const locale = (): 'pt' | 'en' => (getLocales()[0]?.languageCode === 'pt' ? 'pt' : 'en');
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
}

import { configureApi, configureLocale } from '@hangar/core';
import { getLocales } from 'expo-localization';
import { useServers } from '../stores/servers';
import { createEventSource } from './sse';

export function configureCore() {
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
  configureLocale({ getLocale: () => (getLocales()[0]?.languageCode === 'pt' ? 'pt' : 'en') });
}

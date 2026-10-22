import { baseOf, configureApi, configureLocale, definirArmazem } from '@hangar/core';
import { getLocales } from 'expo-localization';
import { overwriteGetLocale } from '../paraglide/runtime';
import { useServers } from '../stores/servers';
import { prefs } from '../stores/prefs';
import { setPairNotice } from '../stores/sessions';
import { useAparencia } from '../stores/aparencia';
import { createEventSource } from './sse';
import { iniciarDiag } from './diag';
import { toast } from '../ui/Toast';

export function configureCore() {
  definirArmazem({
    getItem: (key) => prefs.getString(key) ?? null,
    setItem: (key, value) => prefs.set(key, value),
    removeItem: (key) => { prefs.remove(key); },
  });
  // uma fonte só: o mesmo getLocale alimenta o runtime do core e o do paraglide mobile.
  // Escolha manual em Configurações vence o idioma do aparelho; `system` volta a segui-lo.
  const locale = (): 'pt' | 'en' => {
    const escolhido = useAparencia.getState().idioma;
    if (escolhido !== 'system') return escolhido;
    return getLocales()[0]?.languageCode === 'pt' ? 'pt' : 'en';
  };
  configureApi({
    getBaseUrl: () => {
      const a = useServers.getState().active();
      return a ? baseOf(a) : '';
    },
    rememberLan: (id, lan, source) => useServers.getState().rememberLan(id, lan, source),
    getToken: () => useServers.getState().active()?.token ?? null,
    onSessionWarnings: (warnings) => toast.aviso(warnings.join('\n')),
    onUnauthorized: () => {
      const a = useServers.getState().active();
      if (a) useServers.getState().markInvalid(a.id);
    },
    // Convite de par recusado (401/410) é par encerrado: a entrada fica marcada, nunca é removida.
    onInviteEnded: (id) => {
      const alvo = id ?? useServers.getState().active()?.id;
      if (alvo) useServers.getState().markEnded(alvo);
    },
    origin: null,
    createEventSource,
  });
  setPairNotice((text) => toast.aviso(text));
  configureLocale({ getLocale: locale });
  overwriteGetLocale(locale);
  iniciarDiag();
}

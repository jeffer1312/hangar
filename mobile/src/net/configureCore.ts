import { baseOf, configureApi, configureLocale, definirArmazem } from '@hangar/core';
import { getLocales } from 'expo-localization';
import { Dimensions } from 'react-native';
import { overwriteGetLocale } from '../paraglide/runtime';
import { useServers } from '../stores/servers';
import { prefs } from '../stores/prefs';
import { setPairNotice } from '../stores/sessions';
import { useAparencia } from '../stores/aparencia';
import { useTabletPrefs } from '../stores/tabletPrefs';
import { useHandsFree } from '../stores/handsFree';
import { useDeliveryMode } from '../chat/useQueueDelivery';
import { CONVERSA_PADRAO } from '../theme/aplicarMaterial';
import { TABLET_MIN_SIDE } from '../features/tablet/layout';
import { settingsValues } from './settingsSnapshot';
import { updateDeviceId } from '../features/updates/pending';
import { createEventSource } from './sse';
import { installDemoFetch, isDemoUrl, openDemoEventSource } from './demo';
import { iniciarDiag } from './diag';
import { deviceIdentity, deviceReady, loadDeviceId } from './deviceIdentity';
import { secureRandom } from './secureRandom';
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
    createEventSource: (url, opts) => (isDemoUrl(url) ? openDemoEventSource(url) : createEventSource(url, opts)),
    deviceIdentity,
    deviceReady,
    secureRandom,
  });
  void loadDeviceId();
  setPairNotice((text) => toast.aviso(text));
  configureLocale({ getLocale: locale });
  overwriteGetLocale(locale);
  installDemoFetch();
  iniciarDiag(() => {
    const a = useAparencia.getState();
    const { width, height } = Dimensions.get('window');
    return {
      valores: settingsValues({
        ...a, texto: a.conversa.texto, textoPadrao: CONVERSA_PADRAO.texto,
        tabletBar: Math.min(width, height) >= TABLET_MIN_SIDE ? useTabletPrefs.getState().placement : undefined,
        delivery: useDeliveryMode.getState().mode, handsFree: useHandsFree.getState().on,
      }),
      aparelho: updateDeviceId(),
    };
  });
}

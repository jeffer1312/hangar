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
  // ponytail: NÃO registramos `configureDiag` — o app nativo não grava diário de uso, de propósito.
  // O diário de verdade (fila, envio em lote, captura de erro de JS, plataforma) é da web:
  // `frontend/src/lib/diag.ts` depende de `window`, `navigator` e do `auth` do frontend, e nada
  // disso foi portado. Sem registro, `registrar()` e `novoReq()` do core viram no-op e string vazia
  // — o `apiFetch` funciona igual e o cabeçalho `X-Hangar-Req` sai vazio, que o backend já trata
  // como ausente (`_limpar` em `backend/app/diag.py` descarta campo vazio).
  //
  // Quem for portar a tela de Diário (`frontend/src/components/settings/DiarioSettings.svelte`)
  // para cá: a tela sozinha NÃO basta. Sem um sink registrado aqui ela baixa um arquivo sempre
  // vazio, sem erro nenhum. Escreva o sink primeiro — ele precisa de fila em memória, envio em
  // lote e um gatilho de descarga que sirva pro ciclo de vida do app nativo (o da web usa
  // `pagehide`, que não existe aqui).
}

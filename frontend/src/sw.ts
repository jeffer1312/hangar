/// <reference lib="webworker" />
import { clientsClaim } from 'workbox-core';
import { cleanupOutdatedCaches, precacheAndRoute } from 'workbox-precaching';

declare let self: ServiceWorkerGlobalScope;

// Aplica o build NOVO na hora, sem esperar todas as abas fecharem. Com registerType:'autoUpdate' +
// injectManifest, o vite-plugin-pwa injeta o registro que recarrega — MAS so funciona se o SW chamar
// skipWaiting. Sem isto o SW novo ficava em "waiting" pra sempre num PWA de celular (raramente fecha
// tudo) -> o app servia build velho indefinidamente (o "cache trap": mudanca pushada nao aparecia).
self.skipWaiting();
clientsClaim();

cleanupOutdatedCaches();
precacheAndRoute(self.__WB_MANIFEST);

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

// Web Push: o backend manda {title, body, session, url} quando uma sessao fica awaiting_input.
self.addEventListener('push', (event) => {
  let data: { title?: string; body?: string; session?: string; url?: string } = {};
  try {
    data = event.data?.json() ?? {};
  } catch {
    /* payload nao-JSON: cai no default */
  }
  event.waitUntil(
    self.registration.showNotification(data.title || 'Claude Pocket', {
      body: data.body || 'Aguardando sua resposta',
      tag: data.session, // mesma sessao -> substitui a notif anterior em vez de empilhar
      data: { url: data.url || '/' },
      icon: '/icons/icon-192.png',
      badge: '/icons/icon-192.png',
    }),
  );
});

// Tocar a notif: foca uma janela do app aberta (e tenta deep-link) ou abre uma nova.
self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const url = (event.notification.data as { url?: string } | undefined)?.url || '/';
  event.waitUntil(
    (async () => {
      const wins = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
      for (const c of wins) {
        await c.focus();
        try {
          await c.navigate(url);
        } catch {
          /* navigate pode falhar cross-origin; o foco ja resolve o essencial */
        }
        return;
      }
      await self.clients.openWindow(url);
    })(),
  );
});

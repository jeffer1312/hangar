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

// App-icon badge (feature #13): o payload do push NAO carrega uma contagem agregada (so a sessao
// que acabou de virar awaiting_input). Como as notificacoes ja sao dedupadas por `tag: session`
// (linha abaixo), o Nº DE NOTIFICACOES ATIVAS *e* o nº de sessoes aguardando — sem precisar do app
// aberto pra recontar. setAppBadge/clearAppBadge podem faltar em algum navegador -> feature-detect.
async function syncBadgeFromNotifications(): Promise<void> {
  if (!('setAppBadge' in self.navigator)) return;
  const open = await self.registration.getNotifications();
  const p = open.length > 0 ? self.navigator.setAppBadge(open.length) : self.navigator.clearAppBadge();
  await p.catch(() => {});
}

// Web Push: o backend manda {title, body, session, url, tag?} quando uma sessao fica awaiting_input
// (ou N sessoes coalescidas numa unica notif — feature #5).
self.addEventListener('push', (event) => {
  let data: { title?: string; body?: string; session?: string; url?: string; tag?: string } = {};
  try {
    data = event.data?.json() ?? {};
  } catch {
    /* payload nao-JSON: cai no default */
  }
  event.waitUntil(
    (async () => {
      await self.registration.showNotification(data.title || 'Claude Cockpit', {
        body: data.body || 'Aguardando sua resposta',
        // tag explicito (coalescido: mesma tag CONSTANTE pra N sessoes) sobrepoe o default (a
        // sessao) -> mesma sessao/grupo substitui a notif anterior em vez de empilhar.
        tag: data.tag || data.session,
        data: { url: data.url || '/' },
        icon: '/icons/icon-192.png',
        badge: '/icons/icon-192.png',
      });
      await syncBadgeFromNotifications();
    })(),
  );
});

// Tocar a notif: foca uma janela do app aberta (e tenta deep-link) ou abre uma nova.
self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  const url = (event.notification.data as { url?: string } | undefined)?.url || '/';
  event.waitUntil(
    (async () => {
      await syncBadgeFromNotifications(); // fechou 1 notif -> recontabiliza o badge
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

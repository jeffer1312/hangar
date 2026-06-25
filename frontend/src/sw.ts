/// <reference lib="webworker" />
import { cleanupOutdatedCaches, precacheAndRoute } from 'workbox-precaching';

declare let self: ServiceWorkerGlobalScope;

cleanupOutdatedCaches();
precacheAndRoute(self.__WB_MANIFEST);

// Network-first for /api/*, cache-first for everything else
self.addEventListener('fetch', (event: FetchEvent) => {
  if (event.request.url.includes('/api/')) {
    // Never cache API responses
    event.respondWith(fetch(event.request));
    return;
  }
  // Static assets handled by precacheAndRoute above
});

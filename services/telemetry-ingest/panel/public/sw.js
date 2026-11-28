"use strict";
// Só repassa à rede, sem guardar nada: existe para o painel poder ser instalado, nunca para servir versão antiga.
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));
self.addEventListener("fetch", (e) => e.respondWith(fetch(e.request)));

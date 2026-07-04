// App-icon badge (Badging API): mostra quantas sessões estão aguardando resposta, pra dar pra saber
// sem abrir o app (feature #13). setAppBadge/clearAppBadge não existem em todo navegador -> feature-
// detect (some silencioso onde falta). Chamado do foreground (recompute das sessões agregadas) E do
// service worker no evento 'push' (mesma API, disponível em WorkerNavigator).
export function updateBadge(count: number): void {
  if (!('setAppBadge' in navigator)) return;
  const p = count > 0 ? navigator.setAppBadge(count) : navigator.clearAppBadge();
  p.catch(() => {}); // permissão/plataforma pode recusar em runtime mesmo com o método presente
}

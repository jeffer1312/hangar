// Web Push no celular: liga notificacao de "sessao aguardando". Uma inscricao + uma VAPID
// compartilhada serve TODOS os servidores (single-user controla os 3). Registra a inscricao em cada
// servidor com o label/id locais, pra notif mostrar "Casa · sessao" e linkar certo.
import { listServers } from './auth';
import { getVapidKey, subscribePush } from './api';

function urlBase64ToUint8Array(base64: string): Uint8Array<ArrayBuffer> {
  const padding = '='.repeat((4 - (base64.length % 4)) % 4);
  const b64 = (base64 + padding).replace(/-/g, '+').replace(/_/g, '/');
  const raw = atob(b64);
  // Aloca sobre um ArrayBuffer explicito: Uint8Array<ArrayBuffer> casa com BufferSource (o TS estrito
  // recusa Uint8Array<ArrayBufferLike> em applicationServerKey por causa do ramo SharedArrayBuffer).
  const arr = new Uint8Array(new ArrayBuffer(raw.length));
  for (let i = 0; i < raw.length; i++) arr[i] = raw.charCodeAt(i);
  return arr;
}

export function pushSupported(): boolean {
  return 'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window;
}

// Pede permissao, assina no PushManager com a VAPID compartilhada e registra a inscricao em TODOS os
// servidores. Retorna quantos aceitaram. Lanca em: sem suporte / permissao negada / nenhum servidor
// com push ligado / nenhum aceitou. Servidor offline e pulado (nao derruba os outros).
export async function enablePush(): Promise<number> {
  if (!pushSupported()) {
    throw new Error('Push não suportado aqui — instale o app na tela inicial (iOS 16.4+).');
  }
  const perm = await Notification.requestPermission();
  if (perm !== 'granted') throw new Error('Permissão de notificação negada.');

  const servers = listServers();
  if (servers.length === 0) throw new Error('Nenhum servidor cadastrado.');

  // VAPID compartilhada: pega do 1o servidor que responder com chave. Servidor offline -> tenta o proximo.
  let vapid = '';
  for (const s of servers) {
    try {
      vapid = await getVapidKey(s);
      if (vapid) break;
    } catch {
      /* offline/sem rota: proximo */
    }
  }
  if (!vapid) throw new Error('Nenhum servidor com push configurado (falta CP_VAPID_* no backend).');

  const reg = await navigator.serviceWorker.ready;
  const sub =
    (await reg.pushManager.getSubscription()) ??
    (await reg.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: urlBase64ToUint8Array(vapid),
    }));
  const subJson = sub.toJSON() as PushSubscriptionJSON;

  let ok = 0;
  for (const s of servers) {
    try {
      await subscribePush(s, subJson);
      ok++;
    } catch {
      /* servidor offline / sem push: pula, registra nos que dao */
    }
  }
  if (ok === 0) throw new Error('Nenhum servidor aceitou a inscrição.');
  return ok;
}

import { useEffect } from 'react';
import { Linking, Platform } from 'react-native';
import { router } from 'expo-router';
import Constants from 'expo-constants';
import * as Notifications from 'expo-notifications';
import { isOwnMachine, subscribeExpoPushForServer, type Server } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { openVaultRequest } from '../vault/target';
import { getLocale } from '../../paraglide/runtime';

// Convite (token de convidado), desligado e entrada montada para par externo não recebem push do dono.
const pushable = (s: Server) => isOwnMachine(s) && !s.disabled;

// 'failed' = nenhum servidor aceitou o token (ou nem houve token): a volta ao app tenta de novo.
type PushResult = 'registered' | 'denied' | 'unsupported' | 'failed';
let lastResult: PushResult | null = null;
let inflight: Promise<PushResult> | null = null;
let pending: Server[] | null = null;

// ponytail: só Android; o iOS do SideStore não assina o entitlement de push.
export function registerExpoPush(servers: Server[]): Promise<PushResult> {
  return run(servers, true);
}

/** Volta dos ajustes do sistema: quem negou e depois liberou precisa mandar o token agora. */
export async function retryExpoPushIfDenied(servers: Server[]): Promise<PushResult | null> {
  if (lastResult !== 'denied' && lastResult !== 'failed') return null;
  // Só lê: perguntar a cada volta ao app insistiria com quem já negou.
  return run(servers, false);
}

// Uma rodada por vez: duas juntas perguntariam duas vezes e buscariam dois tokens. Quem chega no
// meio recebe a rodada em curso, e a lista dele é registrada no fim dela, sem perguntar de novo.
function run(servers: Server[], ask: boolean): Promise<PushResult> {
  if (inflight) { pending = servers; return inflight; }
  inflight = cycle(servers, ask)
    .then((r) => (lastResult = r), (e: unknown) => { lastResult = 'failed'; throw e; })
    .finally(() => { inflight = null; pending = null; });
  return inflight;
}

async function cycle(servers: Server[], ask: boolean): Promise<PushResult> {
  let result = await register(servers, ask);
  while (pending && result === 'registered') {
    const next = pending;
    pending = null;
    result = await register(next, false);
  }
  return result;
}

let handlerInstalled = false;
// Sem o handler, o push que chega com o app aberto é descartado; sem som, o Android não mostra o banner.
export function installNotificationHandler(): void {
  if (handlerInstalled) return;
  handlerInstalled = true;
  Notifications.setNotificationHandler({
    handleNotification: async () => ({ shouldShowBanner: true, shouldShowList: true, shouldPlaySound: true, shouldSetBadge: false }),
  });
}

async function register(servers: Server[], ask: boolean): Promise<PushResult> {
  if (Platform.OS !== 'android') return 'unsupported';
  let { status } = await Notifications.getPermissionsAsync();
  if (status !== 'granted' && !ask) return 'denied';
  // O canal precisa existir antes do pedido: no Android 13+ é ele que dispara a pergunta do sistema.
  await Notifications.setNotificationChannelAsync('default', {
    name: 'Hangar',
    importance: Notifications.AndroidImportance.HIGH,
  });
  if (status !== 'granted') ({ status } = await Notifications.requestPermissionsAsync());
  if (status !== 'granted') return 'denied';
  const projectId = Constants.expoConfig?.extra?.eas?.projectId as string | undefined;
  const { data: token } = await Notifications.getExpoPushTokenAsync({ projectId });
  const locale = getLocale();
  // Servidor fora do ar não impede os outros; a falha de cada um já vai ao diário pelo apiFetch.
  const results = await Promise.allSettled(servers.filter(pushable).map((s) =>
    subscribeExpoPushForServer(s, token, locale)));
  // Um servidor recusado precisa cair no retry; só tudo aceito (e ao menos um) é registro.
  return results.length > 0 && results.every((r) => r.status === 'fulfilled') ? 'registered' : 'failed';
}

/** Muda quando o registro precisa ser refeito: servidor entrou ou saiu, foi renomeado, ou o idioma mudou. */
export function pushRegistrationKey(servers: Server[], locale: string): string {
  const parts = servers.filter(pushable).map((s) => `${s.id}\t${s.label}`);
  return parts.length ? [locale, ...parts].join('\n') : '';
}

export function openFromNotification(data: unknown, servers: Server[]): void {
  const d = (data ?? {}) as { server?: unknown; session?: unknown; server_id?: unknown; request_id?: unknown };
  const known = typeof d.server === 'string' && servers.some((s) => s.id === d.server);
  // Pedido do cofre: a folha dele mora na lista, não no chat da sessão que pediu.
  if (typeof d.request_id === 'string' && d.request_id && typeof d.server_id === 'string' && servers.some((s) => s.id === d.server_id)) {
    openVaultRequest({ serverId: d.server_id, requestId: d.request_id });
    router.push('/');
  } else if (known && typeof d.session === 'string' && d.session) {
    router.push({ pathname: '/s/[server]/[name]', params: { server: d.server as string, name: d.session } });
  } else {
    router.push('/');
  }
}

export function handleNotificationResponse(response: Notifications.NotificationResponse, servers: Server[]): void {
  if (response.actionIdentifier !== Notifications.DEFAULT_ACTION_IDENTIFIER) return;
  openFromNotification(response.notification.request.content.data, servers);
  // Sem limpar, o hook devolve a mesma resposta na próxima montagem e abre de novo.
  Notifications.clearLastNotificationResponse();
}

/** Toque na notificação, inclusive com o app fechado: espera a lista de servidores carregar. */
export function useNotificationNavigation(): void {
  const response = Notifications.useLastNotificationResponse();
  const ready = useServers((s) => s.ready);
  useEffect(() => {
    if (ready && response) handleNotificationResponse(response, useServers.getState().servers);
  }, [ready, response]);
}

export function openNotificationSettings(): Promise<void> {
  return Linking.openSettings();
}

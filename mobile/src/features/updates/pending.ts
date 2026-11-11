import { forClient, getArrivedNews, markArrivedSeen, registrarDiag, type PreparedUpdateAll, type Server, type UpdateAcceptance, type UpdateNews } from '@hangar/core';
import { prefs } from '../../stores/prefs';

// Um id por instalação, o mesmo para todos os servidores: cada um guarda até onde este aparelho já viu.
const DEVICE_KEY = 'update.deviceId.v1';
const VALID_DEVICE = /^[A-Za-z0-9_-]{1,64}$/;
let device: string | null = null;

export function updateDeviceId(): string {
  if (device) return device;
  try {
    const saved = prefs.getString(DEVICE_KEY);
    if (saved && VALID_DEVICE.test(saved)) return (device = saved);
  } catch { /* ilegível: gera outro */ }
  device = globalThis.crypto?.randomUUID?.() ?? Array.from({ length: 32 }, () => Math.floor(Math.random() * 16).toString(16)).join('');
  // Sem gravar, a próxima abertura é um aparelho novo para o servidor: só perde o que já tinha chegado.
  try { prefs.set(DEVICE_KEY, device); } catch { /* segue com o id desta abertura */ }
  return device;
}

// A lista local de antes: o servidor passou a guardar o que chegou, então ela só é apagada, sem ler.
const LEGACY_PREFIX = 'update.whatsNew.v1:';
let legacyDropped = false;
export function dropLegacyPendingNews(): void {
  if (legacyDropped) return;
  legacyDropped = true;
  try { for (const key of prefs.getAllKeys()) if (key.startsWith(LEGACY_PREFIX)) prefs.remove(key); }
  catch { /* fica para a próxima abertura */ }
}

export type ShownNews = { cards: UpdateNews[]; fixes: UpdateNews[]; server?: Server; through?: number };

// A rota da folha só recebe o id do servidor: a lista fica aqui até ela abrir.
const shown = new Map<string, ShownNews>();
// `servidor:through` já oferecido nesta abertura do app: abrir, voltar ao primeiro plano e o fim do clique não repetem a folha.
const offered = new Set<string>();

function markSeen(server: Server, through: number): void {
  markArrivedSeen(server, updateDeviceId(), through).catch((e: unknown) => {
    registrarDiag({ evento: 'update.arrived_seen_failed', nivel: 'aviso', codigo: 'update_arrived_seen_failed', detalhe: e instanceof Error ? e.message : String(e) });
  });
}

// Leitura em curso por servidor: dois gatilhos juntos não abrem a folha duas vezes; o segundo espera e lê de novo.
const reading = new Map<string, Promise<boolean>>();

/** Lê o que chegou para este aparelho. `true` = há o que mostrar, e quem chamou abre `whatsNewHref`. */
export async function loadArrivedNews(server: Server): Promise<boolean> {
  const running = reading.get(server.id);
  if (running) { await running.catch(() => false); return loadArrivedNews(server); }
  const read = readArrivedNews(server);
  reading.set(server.id, read);
  try { return await read; }
  finally { if (reading.get(server.id) === read) reading.delete(server.id); }
}

async function readArrivedNews(server: Server): Promise<boolean> {
  // Lista pronta que ninguém abriu (a tela que pediu já tinha saído): este gatilho abre.
  if (shown.has(server.id)) return true;
  let arrived: { news: UpdateNews[]; through: number } | null;
  try { arrived = await getArrivedNews(server, updateDeviceId()); }
  catch (e) {
    registrarDiag({ evento: 'update.arrived_failed', nivel: 'aviso', codigo: 'update_arrived_failed', detalhe: e instanceof Error ? e.message : String(e) });
    return false;
  }
  if (arrived === null) {
    registrarDiag({ evento: 'update.arrived_absent', nivel: 'ok', codigo: 'update_arrived_absent' });
    return false;
  }
  if (!Array.isArray(arrived?.news) || !Number.isSafeInteger(arrived.through)) {
    registrarDiag({ evento: 'update.arrived_failed', nivel: 'aviso', codigo: 'update_arrived_invalid' });
    return false;
  }
  const key = `${server.id}:${arrived.through}`;
  if (offered.has(key)) return false;
  offered.add(key);
  const { cards, fixes } = forClient({ warnings: [], news: arrived.news }, 'mobile');
  if (!cards.length && !fixes.length) { markSeen(server, arrived.through); return false; }
  shown.set(server.id, { cards, fixes, server, through: arrived.through });
  return true;
}

/** Lista que a folha mostra; não apaga, para o React poder chamar duas vezes. */
export const peekShownNews = (serverId: string): ShownNews => shown.get(serverId) ?? { cards: [], fixes: [] };

/** A folha abriu: apaga a lista e marca visto, mesmo que a pessoa feche sem ler. */
export function markShownNews(serverId: string): void {
  const news = shown.get(serverId);
  shown.delete(serverId);
  if (news?.server && news.through !== undefined) markSeen(news.server, news.through);
}

/** Aceite dos avisos lidos, por id de servidor; vazio quando nenhum aviso apareceu. */
export type UpdateAcceptances = Record<string, UpdateAcceptance>;

// A rota da folha não recebe callback: quem pede confirmação deixa o pedido aqui e espera a resposta.
// `machines`: no "Atualizar todas", os avisos de cada máquina, lidos por quem pediu.
export type ConfirmRequest = { server: Server; count?: number; machines?: Promise<PreparedUpdateAll>;
  resolve: (ok: boolean, accept?: UpdateAcceptances) => void };
let request: ConfirmRequest | null = null;

/** `null` = cancelou. */
export function requestUpdateConfirm(server: Server, open: () => void, count?: number, machines?: Promise<PreparedUpdateAll>): Promise<UpdateAcceptances | null> {
  request?.resolve(false);
  return new Promise((resolve) => {
    let done = false;
    request = { server, count, machines, resolve: (ok, accept = {}) => {
      if (done) return; done = true; request = null; resolve(ok ? accept : null);
    } };
    open();
  });
}

export const currentConfirmRequest = () => request;

export const whatsNewHref = (serverId: string) => ({ pathname: '/whats-new', params: { server: serverId } }) as never;

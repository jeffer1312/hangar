// A mensagem enviada viaja do campo até onde fica: a bolha dela no fim da conversa ou a linha dela no painel
// "Na fila". O destino nasce no envio, local; a viagem só acontece quando ele já tem lugar na tela.
import type { View } from 'react-native';
import { create } from 'zustand';

/** Lugar de uma peça dentro da área do chat, contado de baixo: o teclado fecha no envio e leva a área junto, mas
 *  a distância de cada peça até a borda de baixo não muda. */
export interface Box { left: number; bottom: number; width: number; height: number }
interface Rect { x: number; y: number; width: number; height: number }
export type FlightTarget = 'chat' | 'queue';
export interface Flight {
  id: number;
  route: string;
  text: string;
  from: Box;
  to: Box | null;
  target: FlightTarget | null;
  /** O destino é uma bolha esmaecida (sessão trabalhando, envio Direto). */
  dim: boolean;
}

export const FLIGHT_MS = 320;
/** Depois do pouso a cópia fica mais um instante por cima do destino: um quadro com os dois, nunca um sem nenhum. */
const OVERLAP_MS = 50;
/** Sem destino medido dentro disto, a cópia some e a mensagem aparece no lugar, sem viagem. */
const TAKEOFF_WAIT_MS = 600;

/** Texto simples e curto viaja; vazio, longo ou de muitas linhas aparece direto. */
export function flies(text: string): boolean {
  const t = text.trim();
  return t !== '' && Array.from(t).length <= 400 && t.split('\n').length <= 6;
}

/** O mesmo texto, sem contar espaço e quebra de linha. */
export function sameText(a: string, b: string): boolean {
  const flat = (value: string) => value.trim().split(/\s+/).join(' ');
  return flat(a) === flat(b);
}

export function boxIn(rect: Rect, host: Rect): Box {
  return { left: rect.x - host.x, bottom: host.y + host.height - (rect.y + rect.height), width: rect.width, height: rect.height };
}

/** O destino tem tamanho e está inteiro dentro da área: fora dela (conversa rolada para cima) não há viagem. */
export function lands(box: Box, hostHeight: number): boolean {
  return box.width > 0 && box.height > 0 && box.bottom >= 0 && box.bottom + box.height <= hostHeight;
}

export const useSendFlight = create<{ flight: Flight | null }>(() => ({ flight: null }));

type Measurable = Pick<View, 'measureInWindow'>;
const hosts = new Map<string, Measurable>();
let seq = 0;
let timer: ReturnType<typeof setTimeout> | undefined;

export function setFlightHost(route: string, host: Measurable | null): void {
  if (host) hosts.set(route, host);
  else hosts.delete(route);
}

function clear(id: number): void {
  if (useSendFlight.getState().flight?.id === id) useSendFlight.setState({ flight: null });
}

function measure(route: string, node: Measurable, done: (box: Box, hostHeight: number) => void): void {
  const host = hosts.get(route);
  if (!host) return;
  host.measureInWindow((hx, hy, hw, hh) => {
    node.measureInWindow((x, y, width, height) => done(boxIn({ x, y, width, height }, { x: hx, y: hy, width: hw, height: hh }), hh));
  });
}

/** O envio saiu do campo: a cópia nasce sobre ele e espera o destino. */
export function takeoff(route: string, text: string, input: Measurable | null): void {
  if (!input || !flies(text)) return;
  measure(route, input, (from) => {
    const id = ++seq;
    clearTimeout(timer);
    useSendFlight.setState({ flight: { id, route, text, from, to: null, target: null, dim: false } });
    timer = setTimeout(() => clear(id), TAKEOFF_WAIT_MS);
  });
}

/** O destino desta mensagem foi desenhado: a viagem começa, ou é cancelada se ele está fora da tela. */
export function land(route: string, text: string, target: FlightTarget, node: Measurable | null, dim = false): void {
  const flight = useSendFlight.getState().flight;
  if (!node || !flight || flight.to || flight.route !== route || !sameText(flight.text, text)) return;
  measure(route, node, (to, hostHeight) => {
    const current = useSendFlight.getState().flight;
    if (current?.id !== flight.id || current.to) return;
    clearTimeout(timer);
    if (!lands(to, hostHeight)) { clear(flight.id); return; }
    useSendFlight.setState({ flight: { ...current, to, target, dim } });
    timer = setTimeout(() => clear(flight.id), FLIGHT_MS + OVERLAP_MS);
  });
}

/** A mensagem desta peça está em viagem: a peça fica invisível até o pouso. */
export function flying(flight: Flight | null, route: string, text: string): boolean {
  return !!flight && flight.route === route && sameText(flight.text, text);
}

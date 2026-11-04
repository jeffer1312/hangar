// Cliente da rota /api/alcance — por onde UM servidor responde (aba Acesso).
// Padrão de servidor explícito (o mesmo do apiFetchForServer, que mora em api.ts):
// baseUrl + Bearer do Server passado, pra chegar no servidor ALVO da config — não
// obrigatoriamente no ativo. lib/api.ts é fechado neste plano; este módulo nasce novo.

import * as m from '../paraglide/messages';
import { errorDetail, probeServerResponse } from '@hangar/core';
import type { Server } from './auth';

// Os QUATRO estados nomeados da linha. O backend manda ok/falhou/nao_configurado; o
// `testando` é o estado em voo (a tela pinta enquanto a resposta não chega).
export type EstadoEndereco = 'ok' | 'falhou' | 'testando' | 'nao_configurado';
export type TipoEndereco = 'nesta_maquina' | 'rede_local' | 'tailscale' | 'publico';

export interface EnderecoAlcance {
  tipo: TipoEndereco;
  url: string;
  estado: EstadoEndereco;
  tempo_ms: number | null;
  // Motivo nomeado do "por que não" (recusou | timeout | erro) — a Task 8 (peers)
  // pode mostrá-lo; a aba Acesso, não.
  motivo?: string;
}

export interface AlcanceDoServidor {
  loopback: boolean;
  bind: string;
  enderecos: EnderecoAlcance[];
}

export async function alcanceDoServidor(s: Server, init?: RequestInit): Promise<AlcanceDoServidor> {
  const res = await probeServerResponse(s, '/api/alcance', init);
  if (!res.ok) throw Object.assign(new Error(`${res.status}: ${await errorDetail(res)}`), { status: res.status });
  return res.json() as Promise<AlcanceDoServidor>;
}

// ── Pareamento (Task 6, Lote B) ────────────────────────────────────────────────
// O QR vem PRONTO do backend (decisão de plano: o front só tem qr-scanner, que lê e
// não gera). A rota devolve o endereço escolhido + credencial, e o SVG da imagem.

export interface PareamentoDoServidor {
  url: string;
  qr_svg: string;
}

export async function pareamentoDoServidor(s: Server, endereco: TipoEndereco): Promise<PareamentoDoServidor> {
  const res = await probeServerResponse(s, `/api/alcance/pareamento?endereco=${encodeURIComponent(endereco)}`);
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<PareamentoDoServidor>;
}

// Frase de estado POR LINHA, derivada dos mocks (estados 1 e 3): o ok varia conforme o
// tipo (wifi / 4G / nesta máquina), falhou e testando são fixos, "não configurado" é
// neutro de propósito — não estar configurado não é defeito.
export function fraseDeEstado(e: EnderecoAlcance, bindLoopback = ''): string {
  if (e.estado === 'nao_configurado') return m.acesso_publico_sem_valor();
  if (e.estado === 'testando') return m.acesso_testando();
  // Endereço da LAN fechado com o bind em loopback não é defeito, é consequência da escolha da
  // máquina: dizer "não está escutando neste endereço" manda procurar um problema que não existe.
  if (e.estado === 'falhou') {
    return e.tipo === 'rede_local' && bindLoopback
      ? m.acesso_fechado_loopback({ endereco: bindLoopback })
      : m.acesso_falhou_endereco();
  }
  const tempo = `${e.tempo_ms ?? 0} ms`;
  switch (e.tipo) {
    case 'rede_local':
      return m.acesso_ok_wifi({ tempo });
    case 'tailscale':
    case 'publico':
      return m.acesso_ok_4g({ tempo });
    case 'nesta_maquina':
      return m.acesso_ok_local();
  }
}

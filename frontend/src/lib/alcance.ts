// Cliente da rota /api/alcance — por onde UM servidor responde (aba Acesso).
// Padrão de servidor explícito (o mesmo do apiFetchForServer, que mora em api.ts):
// baseUrl + Bearer do Server passado, pra chegar no servidor ALVO da config — não
// obrigatoriamente no ativo. lib/api.ts é fechado neste plano; este módulo nasce novo.

import * as m from '../paraglide/messages';
import { errorDetail } from './api';
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
  const res = await fetch(`${s.baseUrl}/api/alcance`, {
    // Prazo por PADRAO, mesmo do apiFetchForServer: servidor atras de VPN nao recusa a
    // conexao, o socket pendura e a promessa nunca resolve — sem prazo a tela ficaria
    // em "Testando…" para sempre, sem erro nenhum.
    signal: AbortSignal.timeout(8000),
    ...init,
    headers: {
      'Content-Type': 'application/json',
      Authorization: `Bearer ${s.token}`,
      ...(init?.headers ?? {}),
    },
  });
  if (!res.ok) throw new Error(`${res.status}: ${await errorDetail(res)}`);
  return res.json() as Promise<AlcanceDoServidor>;
}

// Frase de estado POR LINHA, derivada dos mocks (estados 1 e 3): o ok varia conforme o
// tipo (wifi / 4G / nesta máquina), falhou e testando são fixos, "não configurado" é
// neutro de propósito — não estar configurado não é defeito.
export function fraseDeEstado(e: EnderecoAlcance): string {
  if (e.estado === 'nao_configurado') return m.acesso_publico_sem_valor();
  if (e.estado === 'testando') return m.acesso_testando();
  if (e.estado === 'falhou') return m.acesso_falhou_endereco();
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
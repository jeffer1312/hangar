// Faixa de cota do rodapé do desktop (Task 9): módulo puro que decide se a faixa aparece e o
// que cada conta desenha, a partir do estado de conta da Task 4 (lib/contaEstado).
//
// Regra de aparecer: duas ou mais contas de que se consiga ler o limite. Com zero ou uma só,
// a faixa some por inteiro — o número de uma conta só já está na statusline da própria sessão.
// Conta sem leitura (limite.estado === 'sem_leitura') não conta para o total e não vira zero.
//
// Cor: só acima de 80%, sempre junto do número (o mock desenha a barra cinza abaixo disso e o
// número sem cor). Acima de 90% é "cheio" (mais grave que "alerta").
//
// Leitura velha: mais de VELHA_APOS_S segundos sem a statusline ser reescrita. O valor escolhido
// é 10 minutos: uma sessão viva reescreve o sidecar a cada render (segundos atrás), então 10 min
// separa "sessão ativa agora" de "não roda há um tempo". A régua do plano é "dado velho parece
// velho" — a conta velha fica esmaecida e carrega a idade ao lado, nunca vira zero nem fresco.
import { parseStatusLine } from './statusline';
import type { ContaEstado } from './contaEstado';

export type NivelCota = 'normal' | 'alerta' | 'cheio';

export interface JanelaCota {
  /** Rótulo da janela como vem da linha ("5h"/"7d") — dado do servidor, não texto de interface. */
  rotulo: string;
  pct: number;
  nivel: NivelCota;
}

export interface ContaCota {
  label: string;
  cincoH: JanelaCota | null;  // null = a linha não trouxe a janela; o par não é desenhado
  seteD: JanelaCota | null;
  velha: boolean;
  idade_s: number | null;
}

export const LIMIAR_ALERTA = 80;
export const LIMIAR_CHEIO = 90;
export const VELHA_APOS_S = 600;

export function nivelDePct(pct: number): NivelCota {
  if (pct > LIMIAR_CHEIO) return 'cheio';
  if (pct > LIMIAR_ALERTA) return 'alerta';
  return 'normal';
}

// Rótulo da janela extraído da própria linha ("⚡5h:64%" → "5h"), com o padrão do app de reserva
// (a linha sempre traz o rótulo junto do emoji; o fallback só cobre tema custom que mude o texto).
function rotuloJanela(linha: string, emoji: string, padrao: string): string {
  const m = linha.match(new RegExp(`${emoji}\\s*(\\d+[hd])`));
  return m ? m[1] : padrao;
}

export function faixaDeCota(contas: ContaEstado[]): ContaCota[] | null {
  const legiveis: ContaCota[] = [];
  for (const c of contas) {
    if (c.limite.estado !== 'lido' || !c.limite.linha) continue;
    const st = parseStatusLine(c.limite.linha);
    if (!st) continue;
    const cincoH = st.fiveHourPct != null
      ? { rotulo: rotuloJanela(c.limite.linha, '⚡', '5h'), pct: st.fiveHourPct, nivel: nivelDePct(st.fiveHourPct) }
      : null;
    const seteD = st.weeklyPct != null
      ? { rotulo: rotuloJanela(c.limite.linha, '📅', '7d'), pct: st.weeklyPct, nivel: nivelDePct(st.weeklyPct) }
      : null;
    // Linha "lida" sem nenhuma janela parseável não é uma leitura de LIMITE — não conta como
    // legível (mesma régua do RateStrip, que exige um pct para o chip existir).
    if (!cincoH && !seteD) continue;
    legiveis.push({
      label: c.label,
      cincoH, seteD,
      velha: c.limite.idade_s != null && c.limite.idade_s > VELHA_APOS_S,
      idade_s: c.limite.idade_s ?? null,
    });
  }
  return legiveis.length >= 2 ? legiveis : null;
}
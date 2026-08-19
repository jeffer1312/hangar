// Faixa de cota do rodapé do desktop: módulo puro que decide se a faixa aparece e o que cada
// conta desenha, a partir de /api/cotas (lib/contaEstado.listarCotas).
//
// A fonte mudou em 18/08/2026 e a razão importa: antes cada conta era parseada da LINHA de
// statusline guardada no sidecar dentro da pasta da conta, o que dava o mesmo número pras três
// (a pasta era um symlink) e nada nenhum pra conta sem sessão aberta. Agora o backend pergunta
// ao provedor com a credencial de cada conta, então aqui não se parseia texto — só se decide
// nível de cor, idade e quem aparece.
//
// Regra de aparecer: qualquer conta conhecida. A faixa é MÓVEL do app de desktop, não um aviso
// que aparece e some conforme a leitura da vez — some do rodapé e o usuário passa a procurar por
// ela. A regra antiga (>=2 contas legíveis) fazia a tira piscar: bastava uma conta perder a
// leitura pra faixa inteira sumir. Sem conta nenhuma ela não existe, e só isso.
//
// Cor: só acima de 80%, sempre junto do número (abaixo disso a barra é cinza e o número sem
// cor). Acima de 90% é "cheio", mais grave que "alerta".
//
// Leitura velha: mais de VELHA_APOS_S sem o backend conseguir renovar. O backend relê a cada
// 5 min, então 10 min significa que as últimas duas tentativas falharam — a conta fica esmaecida
// e carrega a idade ao lado, nunca vira zero nem fresco.
import type { CotaConta, EstadoCota } from './contaEstado';
import { intlLocale } from './locale';

export type NivelCota = 'normal' | 'alerta' | 'cheio';

export interface JanelaExibida {
  /** Rótulo da janela como o provedor o define ("5h"/"7d") — dado do servidor. */
  rotulo: string;
  pct: number;
  nivel: NivelCota;
  /** Epoch do reset, quando o provedor manda. Vira contagem regressiva na tela. */
  resetTs: number | null;
}

export interface ContaCota {
  id: string;
  label: string;
  ativa: boolean;
  estado: EstadoCota;
  janelas: JanelaExibida[];
  velha: boolean;
  idade_s: number | null;
  /** Motivo cru do backend (`sessao-viva`, `login-necessario`, …). É CÓDIGO, não texto de tela:
   *  quem escolhe a frase é `motivoParado` aqui embaixo. */
  motivo: string | null;
}

export const LIMIAR_ALERTA = 80;
export const LIMIAR_CHEIO = 90;
export const VELHA_APOS_S = 600;

export function nivelDePct(pct: number): NivelCota {
  if (pct > LIMIAR_CHEIO) return 'cheio';
  if (pct > LIMIAR_ALERTA) return 'alerta';
  return 'normal';
}

export function faixaDeCota(contas: CotaConta[]): ContaCota[] | null {
  const linhas: ContaCota[] = [];
  for (const c of contas) {
    const janelas = (c.janelas ?? [])
      .filter((j) => typeof j.pct === 'number' && isFinite(j.pct))
      .map((j) => ({
        rotulo: j.rotulo,
        pct: j.pct,
        nivel: nivelDePct(j.pct),
        resetTs: j.reset_ts ?? null,
      }));
    // Estado 'lida' sem nenhuma janela não é leitura de limite: cai no mesmo balde de quem não
    // conseguiu ler (a conta aparece nomeada, sem número — nunca com zero).
    const estado: EstadoCota = c.estado === 'lida' && janelas.length === 0 ? 'indisponivel' : c.estado;
    linhas.push({
      id: c.id,
      label: c.label,
      ativa: !!c.ativa,
      estado,
      janelas,
      velha: c.idade_s != null && c.idade_s > VELHA_APOS_S,
      idade_s: c.idade_s ?? null,
      motivo: c.motivo ?? null,
    });
  }
  return linhas.length > 0 ? linhas : null;
}

/** Contagem regressiva curta até o reset ("1h20", "35m"). Passado/ausente = string vazia. */
export function faltaPara(resetTs: number | null, agora: number): string {
  if (resetTs == null || !isFinite(resetTs)) return '';
  const s = resetTs - agora;
  if (s <= 0) return '';
  const min = Math.floor(s / 60);
  if (min < 60) return `${min}m`;
  const h = Math.floor(min / 60);
  if (h < 24) return min % 60 ? `${h}h${String(min % 60).padStart(2, '0')}` : `${h}h`;
  return `${Math.floor(h / 24)}d${h % 24 ? `${h % 24}h` : ''}`;
}

// Janela LONGA = reset a mais de um dia. A pergunta muda com a escala: numa janela de 5h o que
// importa é "falta quanto"; numa de 7 dias é "que dia volta" — ninguém converte 6h41 de cabeça pra
// descobrir que cai no sábado. Por isso a longa mostra o dia e a curta mostra o tempo.
export function janelaLonga(resetTs: number | null, agora: number): boolean {
  return resetTs != null && isFinite(resetTs) && resetTs - agora > 86400;
}

// Dia e hora do reset, curto. Vazio quando não há reset ou ele já passou.
// É data formatada, não texto de interface — por isso sai do Intl no idioma do app, e não de uma
// chave de tradução (nome de dia da semana não se escreve à mão em duas línguas).
//
// Comentário em `//` de propósito: o extrator do i18nGuard lê bloco `/** */` de várias linhas como
// string de interface e reprova o arquivo. É falso positivo, mas mascarar na lista de exceções
// esconderia o defeito do extrator atrás de uma exceção nomeada — mais barato escrever assim.
export function diaDoReset(resetTs: number | null, agora: number): string {
  if (resetTs == null || !isFinite(resetTs) || resetTs <= agora) return '';
  const d = new Date(resetTs * 1000);
  const dia = new Intl.DateTimeFormat(intlLocale(), { weekday: 'short' })
    .format(d)
    .replace(/\.$/, '');
  return `${dia} ${d.getHours()}h`;
}

/** Texto de uma conta SEM número, escolhido pelo motivo que o backend mandou.
 *
 * "precisa entrar" é login de verdade, e dizer isso quando a credencial só está com o token
 * vencido (mas com refresh vivo) manda o usuário fazer uma coisa que não resolve. Dos motivos
 * do backend (ver backend/app/cotas.py `_tentar_renovar`):
 *
 * - `sessao-viva`: há SESSÃO ABERTA na conta (ou é a conta-base do app) — renovar por fora
 *   rotacionaria o par debaixo do processo vivo. Não tem NADA pra fazer: o CLI da sessão
 *   renova sozinho no próximo turno e o número volta. Mandar "abra uma sessão" aqui é
 *   absurdo — o usuário já está nela (queixa real de 19/08/2026).
 * - `renovacao-falhou`: refresh vivo, a tentativa automática falhou — aí sim a frase que
 *   resolve é "abra uma sessão nela" (o CLI renova ao abrir).
 */
export function motivoSessaoViva(motivo?: string | null): boolean {
  return motivo === 'sessao-viva';
}

export function motivoParado(motivo?: string | null): boolean {
  return motivo === 'renovacao-falhou';
}

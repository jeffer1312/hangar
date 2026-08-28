// Lê o kick-off da passagem de bastão — o recado que a sessão SUCESSORA recebe pela fila durável
// (backend: `bastao.kickoff`) — pra o chat desenhar um cartão em vez de seis linhas de prosa.
//
// Mesmas duas regras do `hangarCmd.ts` e do `orqRecado.ts`: o que não for reconhecido volta `null`
// e cai na bolha de sempre, e o texto cru nunca some — quem o mostra é o componente, num bloco
// fechado.
//
// O cartão REESCREVE o recado em frases curtas em vez de citá-lo. Isso só é honesto enquanto os
// dois dizem a mesma coisa, e quem mexe no `kickoff` não tem como saber que existe um cartão do
// outro lado — então o parser exige os marcadores do que o cartão resume (`MARCAS`). Mudou a
// instrução, isto para de casar e a mensagem volta a aparecer inteira, no texto do servidor.
// Perder o cartão é o preço certo; mostrar instrução velha como oficial, não.

export const PREFIXO_BASTAO = '[hangar: passagem de bastão]';

export type RecadoBastao = {
  /** Sessão de onde o trabalho vem — continua VIVA, só parou de escrever. */
  origem: string;
  /** Caminho do dossiê `.md` gravado no disco desta máquina. */
  dossie: string;
  /** Conta de onde a origem vinha (vazio quando o kick-off não soube dizer). */
  conta: string;
  /** Modelo/esforço de onde a origem vinha. */
  modelo: string;
};

const ORIGEM = /Você continua o trabalho da sessão\s*`([^`]+)`/;
const DOSSIE = /o dossiê em\s*`([^`]+)`/;
const DE = /Ela vinha de\s*(.+?)\s*—/;
const CONTA = /conta\s*`([^`]+)`/;
const MODELO = /modelo\s*`([^`]+)`/;

// As três coisas que o cartão afirma por conta própria: os dois passos (dossiê, depois plano) e os
// dois avisos (a origem continua viva; par/grupo não vêm junto).
const MARCAS = ['Leia o plano', 'continua VIVA', 'NÃO move esses vínculos'];

/** @param texto o recado inteiro, COM o prefixo (é ele que identifica a passagem). */
export function lerRecadoBastao(texto: string): RecadoBastao | null {
  if (!texto.startsWith(PREFIXO_BASTAO)) return null;
  if (!MARCAS.every((marca) => texto.includes(marca))) return null;
  const origem = texto.match(ORIGEM)?.[1];
  const dossie = texto.match(DOSSIE)?.[1];
  // Sem origem ou sem dossiê o cartão perderia justamente o que ele existe pra dar (de quem
  // continuo, o que abro primeiro) — aí a bolha de texto serve melhor.
  if (!origem || !dossie) return null;
  const de = texto.match(DE)?.[1] ?? '';
  return {
    origem,
    dossie,
    conta: de.match(CONTA)?.[1] ?? '',
    modelo: de.match(MODELO)?.[1] ?? '',
  };
}

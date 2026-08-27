// Lê o recado `[painel: orquestração]` que o modal de papéis manda pro árbitro (backend:
// `api._recado_arbitro`) e devolve o que mudou, pro chat desenhar um cartão em vez de um parágrafo
// de doze linhas.
//
// Mesmas duas regras do `hangarCmd.ts`: o que não for reconhecido volta `null` e cai na bolha de
// sempre (cartão que erra a leitura é pior que nenhum cartão), e o texto cru nunca some — quem o
// mostra é o componente, num bloco fechado.
//
// O texto é gerado pelo servidor, em português e com forma fixa: por isso dá pra ler por regex. Ao
// mexer no `_recado_arbitro`, mexa aqui e no teste junto.

export type PapelMudado = {
  papel: string;
  provider: string;
  conta: string;
  /** `-` no texto = não definido; aqui vira string vazia (o cartão omite o chip). */
  modelo: string;
  esforco: string;
};

export type RecadoOrq = {
  papeis: PapelMudado[];
  /** Caminho do `regras-<gid>.md` que o árbitro deve reler. */
  regras: string;
  /** Um dos papéis mudados é o do próprio árbitro → o rito de sucessão vale. */
  sucessao: boolean;
};

const CABECA = /configuração de modelos do grupo mudou no painel:\s*(.+?)\.\s*Releia\s*`([^`]+)`/s;
const PAPEL = /^`([^`]+)`\s*agora é provider\s*`([^`]+)`,\s*conta\s*`([^`]+)`,\s*modelo\s*`([^`]+)`,\s*esforço\s*`([^`]+)`$/;

const vazio = (v: string) => (v === '-' ? '' : v);

/** @param texto corpo do recado JÁ sem o prefixo `[painel: orquestração]` (parsePeerMessage o tira). */
export function lerRecadoOrq(texto: string): RecadoOrq | null {
  const casou = texto.match(CABECA);
  if (!casou) return null;
  const papeis: PapelMudado[] = [];
  for (const parte of casou[1].split(';')) {
    const p = parte.trim().match(PAPEL);
    // Uma linha que não casa derruba o cartão inteiro: meia lista seria pior que a bolha de texto,
    // porque o papel que sumiu é justamente o que ninguém iria aplicar.
    if (!p) return null;
    papeis.push({ papel: p[1], provider: p[2], conta: p[3], modelo: vazio(p[4]), esforco: vazio(p[5]) });
  }
  if (!papeis.length) return null;
  return {
    papeis,
    regras: casou[2],
    sucessao: papeis.some((p) => /^[áa]rbitro$/i.test(p.papel.trim())),
  };
}

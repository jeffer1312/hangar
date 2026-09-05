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

// O cartão não CITA a cauda do recado: ele a reescreve em frases curtas e traduzidas. Isso só é
// honesto enquanto as duas dizem a mesma coisa — e quem mexe no `_recado_arbitro` não tem como
// saber que existe um cartão do outro lado. Então o parser exige os marcadores do que o cartão
// resume: mudou a instrução, isto para de casar e a mensagem volta a aparecer inteira, no texto do
// servidor. Perder o cartão é o preço certo; mostrar instrução velha como oficial, não.
const MARCAS = ['PARADA (idle)', 'TRABALHANDO', 'não reescreva a tabela'];
const MARCA_SUCESSAO = 'Sucessão do árbitro';

const vazio = (v: string) => (v === '-' ? '' : v);

/** @param texto corpo do recado JÁ sem o prefixo `[painel: orquestração]` (parsePeerMessage o tira). */
export function lerRecadoOrq(texto: string): RecadoOrq | null {
  const casou = texto.match(CABECA);
  if (!casou) return null;
  if (!MARCAS.every((marca) => texto.includes(marca))) return null;
  const papeis: PapelMudado[] = [];
  for (const parte of casou[1].split(';')) {
    const p = parte.trim().match(PAPEL);
    // Uma linha que não casa derruba o cartão inteiro: meia lista seria pior que a bolha de texto,
    // porque o papel que sumiu é justamente o que ninguém iria aplicar.
    if (!p) return null;
    papeis.push({ papel: p[1], provider: p[2], conta: p[3], modelo: vazio(p[4]), esforco: vazio(p[5]) });
  }
  if (!papeis.length) return null;
  const sucessao = papeis.some((p) => /^[áa]rbitro$/i.test(p.papel.trim()));
  // O bloco do rito só é desenhado na sucessão — então só ali o marcador dele é exigido.
  if (sucessao && !texto.includes(MARCA_SUCESSAO)) return null;
  return { papeis, regras: casou[2], sucessao };
}

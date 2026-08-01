// Formatação pt-BR num lugar só: vírgula decimal, ponto de milhar. Espalhar `toFixed()` pela
// tela foi o que produziu "19.83 Bi" ao lado de "US$ 17.730,43" — dois significados pro mesmo
// ponto na mesma página.
export type Cur = 'USD' | 'BRL';

export const dec = (n: number, casas: number) =>
  n.toLocaleString('pt-BR', { minimumFractionDigits: casas, maximumFractionDigits: casas });

export function tok(n: number): string {
  if (n >= 1e9) return `${dec(n / 1e9, 2)} Bi`;
  if (n >= 1e6) return `${dec(n / 1e6, 1)} Mi`;
  if (n >= 1e3) return `${dec(n / 1e3, 0)} mil`;
  return dec(n, 0);
}

// Sem cotação, BRL cai em USD: mostrar número convertido por taxa que não temos é pior que
// mostrar o dólar.
const efetiva = (cur: Cur, rate: number | null): Cur => (cur === 'BRL' && rate ? 'BRL' : 'USD');
const valor = (n: number, cur: Cur, rate: number | null) =>
  efetiva(cur, rate) === 'BRL' ? n * (rate as number) : n;

const nf = (cur: Cur, extra: Intl.NumberFormatOptions = {}) =>
  new Intl.NumberFormat('pt-BR', { style: 'currency', currency: cur, ...extra });

// `style: 'currency'` do ICU separa símbolo e número com NBSP (U+00A0), nunca com espaço
// normal. Medido: Intl.NumberFormat('pt-BR',{style:'currency',currency:'USD'}).format(105336.79)
// devolve "US$ 105.336,79", que NÃO é === "US$ 105.336,79". Tem que valer nos TRÊS caminhos
// — o compacto já tratava, o money2 e o ramo abaixo de mil não, e eram os do teste.
const semNbsp = (s: string) => s.replace(/\u00a0/g, ' ');

/** Valor exato, com centavos — tabela, tooltip, qualquer lugar que some ou compare. */
export const money2 = (n: number, cur: Cur, rate: number | null) =>
  semNbsp(nf(efetiva(cur, rate)).format(valor(n, cur, rate)));

/** Grandeza na hora — só nos números grandes do topo. */
export function money(n: number, cur: Cur, rate: number | null): string {
  const c = efetiva(cur, rate);
  const v = valor(n, cur, rate);
  if (Math.abs(v) < 1000) return semNbsp(nf(c).format(v));
  return semNbsp(nf(c, { notation: 'compact', maximumFractionDigits: 1 }).format(v));
}

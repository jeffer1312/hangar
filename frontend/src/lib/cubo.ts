// Filtra e soma o detalhamento cruzado no cliente.
//
// Os quatro agrupamentos do CostReport somam antes de mandar, e depois de somado não dá pra
// separar "quanto daquele projeto foi de tal fonte". Aqui cada linha é uma combinação que
// aconteceu, então qualquer recorte vira uma soma — inclusive dois ou três filtros juntos.
import type { ComboLocal, DimBucket } from '@hangar/core';

export type Dim = 'dia' | 'provider' | 'source' | 'project' | 'model' | 'servidor';

export interface Filtro {
  provider?: string;
  source?: string;
  project?: string;
  model?: string;
  servidor?: string;
  subagente?: boolean;
}

// As quatro dimensões que viram um filtro na tela. 'dia' é do eixo do gráfico, não do recorte.
export type DimFiltro = Exclude<Dim, 'dia'>;

// Escrita no filtro. Com detalhamento os cortes COMBINAM — é a razão de ele existir. Sem ele
// (servidor da malha em versão antiga) o recorte volta a ser de UMA dimensão, porque aí só existem
// os totais marginais dos `by_*`: guardar duas dimensões faria a tela mostrar o número de uma
// debaixo do rótulo das duas ("Recorte: provedor X · projeto Y" com o gasto só do provedor X).
// O último clique vence, que é como a tela se comportava antes de haver cruzamento.
export function aplicar(
  f: Filtro, dim: DimFiltro, valor: string | undefined, cruza: boolean,
): Filtro {
  return { ...(cruza ? f : {}), [dim]: valor };
}

export function filtrar(combos: ComboLocal[], f: Filtro): ComboLocal[] {
  return combos.filter(
    (c) =>
      (!f.provider || c.provider === f.provider) &&
      (!f.source || c.source === f.source) &&
      (!f.project || c.project === f.project) &&
      (!f.model || c.model === f.model) &&
      (!f.servidor || c.servidor === f.servidor) &&
      (f.subagente === undefined || Boolean(c.subagente) === f.subagente),
  );
}

const zero = (key: string): DimBucket => ({
  key, sessions: 0, subagentes: 0, input: 0, output: 0, cache_write: 0, cache_read: 0,
  cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0,
});

// `?? 0` em toda entrada: servidor antigo da malha pode não mandar um campo, e
// `undefined + n` vira NaN, que se espalha e apaga a coluna inteira — inclusive as linhas
// dos servidores que mandaram o dado certo.
function acumular(alvo: DimBucket, c: ComboLocal, sessoes: Set<string>): void {
  // A identidade que o servidor põe em `session_ids` já carrega o flag de subagente
  // (backend/app/costs.py:_somar), então o id de um subagente nunca colide com o da conversa
  // e contar os dois lados pelo flag do combo não perde nem repete ninguém.
  if (c.session_ids) {
    for (const id of c.session_ids) {
      const chave = JSON.stringify([c.servidor, id]);
      if (!sessoes.has(chave)) {
        sessoes.add(chave);
        alvo.sessions += 1;
        if (c.subagente) alvo.subagentes = (alvo.subagentes ?? 0) + 1;
      }
    }
  } else {
    alvo.sessions += c.sessions ?? 0;
    if (c.subagente) alvo.subagentes = (alvo.subagentes ?? 0) + (c.sessions ?? 0);
  }
  alvo.input += c.input ?? 0;
  alvo.output += c.output ?? 0;
  alvo.cache_write += c.cache_write ?? 0;
  alvo.cache_read += c.cache_read ?? 0;
  alvo.cost += c.cost ?? 0;
  alvo.cost_input += c.cost_input ?? 0;
  alvo.cost_output += c.cost_output ?? 0;
  alvo.cost_cache_write += c.cost_cache_write ?? 0;
  alvo.cost_cache_read += c.cost_cache_read ?? 0;
}

export function somar(combos: ComboLocal[]): DimBucket {
  const t = zero('totals');
  const sessoes = new Set<string>();
  for (const c of combos) acumular(t, c, sessoes);
  return t;
}

export function agruparPor(combos: ComboLocal[], dim: Dim): DimBucket[] {
  const m = new Map<string, DimBucket>();
  const sessoes = new Map<string, Set<string>>();
  for (const c of combos) {
    const k = c[dim];
    let b = m.get(k);
    if (!b) { b = zero(k); m.set(k, b); sessoes.set(k, new Set()); }
    acumular(b, c, sessoes.get(k)!);
  }
  return [...m.values()].sort((a, b) =>
    dim === 'dia' ? b.key.localeCompare(a.key) : b.cost - a.cost || a.key.localeCompare(b.key));
}

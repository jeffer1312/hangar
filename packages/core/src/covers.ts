// Regra de casamento eco<->fila do dedup cruzado do Chat (Chat.svelte): quando o transcript
// grava o prompt real, a bolha sintetica "queued-" de mesmo texto sai da tela. `covers(a, b)`:
// `a` cobre `b` se forem iguais, `b` for uma linha de `a`, as legendas (sem o marcador de
// anexo) forem iguais — ou `b` for PREFIXO de uma linha de `a`.
//
// O prefixo existe porque o eco chega com SUFIXO — medido em 18/08/2026: a fila digitou
// "Vamos fazer ate as 23 com o Deepseek…", o transcript gravou a mesma linha com "… eu tinha
// mandado isso" no fim. Sem ele a bolha ficava marcada "não chegou — reenvie" para sempre
// sobre uma mensagem que CHEGOU. Piso de comprimento: "ok"/"sim"/"1" não podem confirmar
// frase alheia que começa igual.
//
// COBRIR NAO BASTA: com DUAS bolhas na fila, uma linha do transcript pode cobrir as duas, e
// quem sai da tela tem de ser a DONA dela. Espelha `_dono_do_prefixo`/`reservadas` do
// `backend/app/pqueue.py`: a linha pertence a quem a reivindica de forma MAIS ESPECIFICA
// (igual > legenda > prefixo; entre prefixos, o mais longo). Sem isso, com X="Vamos fazer"
// (que NAO chegou) e Y="Vamos fazer ate as 23..." (que chegou), o front apagava a bolha de X
// — a que carrega o aviso "não chegou" — e deixava a de Y pendente: as duas marcas invertidas,
// o mesmo defeito que o backend fechou em 18/08/2026 (parecer G2 rev2), vivo na metade que o
// usuario ve. A regra mora AQUI, num lugar so, porque ela ja existia em dois (backend e front)
// e os dois ja divergiram uma vez.
const PREFIXO_MIN = 8;

// Especificidade da reivindicacao de `fila` sobre a linha `real`. -1 = nao cobre.
const EXATO = 3;
const LEGENDA = 2;
const PREFIXO = 1;

// Legenda canonica: sem o marcador "📎 imagem:/arquivo: `path`" + o "—" que liga. Mesma
// normalização do _cap do Chat.svelte — mantida em cópia porque o Chat usa a dele em mais
// lugares e mexer neles não é o escopo desta correção.
function _cap(text: string): string {
  const i = text.search(/(?:\s*—\s*)?📎\s*(?:imagem|arquivo):/u);
  return (i >= 0 ? text.slice(0, i) : text).trim();
}

export function especificidade(real: string, fila: string): number {
  const at = real.trim(), bt = fila.trim();
  if (at === bt) return EXATO;
  if (at.split('\n').some((ln) => ln.trim() === bt)) return EXATO;
  // Msg com imagem: eco/fila carrega "📎 imagem: `path`", o transcript grava so a legenda ->
  // casa pela legenda canonica (senao a bolha com foto fica pendente eterna).
  const ac = _cap(at), bc = _cap(bt);
  if (!!bc && ac === bc) return LEGENDA;
  if (bt.length >= PREFIXO_MIN && at.split('\n').some((ln) => ln.trim().startsWith(bt))) return PREFIXO;
  return -1;
}

export function covers(a: string, b: string): boolean {
  return especificidade(a, b) >= 0;
}

// Indice da entrada de `filas` que e DONA da linha `real`, ou -1 se nenhuma a cobre. Empate de
// especificidade E de tamanho fica com a PRIMEIRA: com duas "ok" na fila e uma real commitada,
// a 2a continua pendente e visivel (comportamento antigo, de proposito).
export function donoDaLinha(real: string, filas: string[]): number {
  let vencedor = -1, melhor = -1, tamanho = -1;
  for (let i = 0; i < filas.length; i++) {
    const e = especificidade(real, filas[i]);
    if (e < 0) continue;
    const t = filas[i].trim().length;
    if (e > melhor || (e === melhor && t > tamanho)) { vencedor = i; melhor = e; tamanho = t; }
  }
  return vencedor;
}

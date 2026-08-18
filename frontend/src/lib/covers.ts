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
const PREFIXO_MIN = 8;

// Legenda canonica: sem o marcador "📎 imagem:/arquivo: `path`" + o "—" que liga. Mesma
// normalização do _cap do Chat.svelte — mantida em cópia porque o Chat usa a dele em mais
// lugares e mexer neles não é o escopo desta correção.
function _cap(text: string): string {
  const i = text.search(/(?:\s*—\s*)?📎\s*(?:imagem|arquivo):/u);
  return (i >= 0 ? text.slice(0, i) : text).trim();
}

export function covers(a: string, b: string): boolean {
  const at = a.trim(), bt = b.trim();
  if (at === bt || at.split('\n').some((ln) => ln.trim() === bt)) return true;
  // Msg com imagem: eco/fila carrega "📎 imagem: `path`", o transcript grava so a legenda ->
  // casa pela legenda canonica (senao a bolha com foto fica pendente eterna).
  const ac = _cap(a), bc = _cap(b);
  if (!!bc && ac === bc) return true;
  return bt.length >= PREFIXO_MIN && at.split('\n').some((ln) => ln.trim().startsWith(bt));
}

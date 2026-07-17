// Posições iniciais dos cards do canvas livre (#/canvas). Puro (testável no vitest node): o
// componente Canvas passa o layout salvo + as linhas novas e recebe onde nasce cada card.
// Convenções: 1 coluna por servidor (ordem de serverOrder), card novo empilha abaixo do mais
// fundo que intersecta a coluna; pareados (mesmo gid) nascem consecutivos.
export interface CardBox { x: number; y: number; w: number; h: number }
export type CanvasLayout = Record<string, CardBox>;

export const CARD_W = 320;
// 380: altura padrão precisa mostrar conversa DE VERDADE (300 mal cabia header+sub+2 linhas+composer).
export const CARD_H = 380;
export const GAP = 16;
export const PAD = 24;

export function placeNew(
  layout: CanvasLayout,
  rows: { key: string; serverId: string; pairGid: string | null }[],
  serverOrder: string[],
): CanvasLayout {
  const fresh = rows.filter((r) => !layout[r.key]);
  if (fresh.length === 0) return {};

  // Pareados consecutivos: reordena por (servidor, gid na 1ª aparição), estável no resto.
  const gidOrder = new Map<string, number>();
  for (const r of fresh) if (r.pairGid && !gidOrder.has(r.pairGid)) gidOrder.set(r.pairGid, gidOrder.size);
  const ordered = [...fresh].sort((a, b) => {
    const sa = serverOrder.indexOf(a.serverId), sb = serverOrder.indexOf(b.serverId);
    if (sa !== sb) return sa - sb;
    const ga = a.pairGid ? gidOrder.get(a.pairGid)! : Number.MAX_SAFE_INTEGER;
    const gb = b.pairGid ? gidOrder.get(b.pairGid)! : Number.MAX_SAFE_INTEGER;
    return ga - gb;
  });

  const out: CanvasLayout = {};
  const colX = (serverId: string) => {
    const i = Math.max(0, serverOrder.indexOf(serverId));   // desconhecido -> coluna 0
    return PAD + i * (CARD_W + GAP);
  };
  // Fundo da coluna: maior y+h entre caixas (salvas OU recém-colocadas) que intersectam a faixa
  // [x, x+CARD_W). ponytail: varredura O(n) por card — layout tem dezenas de entradas, não milhares.
  const bottom = (x: number) => {
    let max = PAD - GAP;
    for (const box of [...Object.values(layout), ...Object.values(out)]) {
      if (box.x < x + CARD_W && box.x + box.w > x) max = Math.max(max, box.y + box.h);
    }
    return max;
  };
  for (const r of ordered) {
    const x = colX(r.serverId);
    out[r.key] = { x, y: bottom(x) + GAP, w: CARD_W, h: CARD_H };
  }
  return out;
}

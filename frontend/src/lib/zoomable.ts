// Zoom no visualizador de imagem.
//
// O `maximum-scale=1` do viewport (index.html) desliga o pinch nativo no PWA instalado, então abrir
// a foto em tela cheia não dava pra aproximar: dava pra ver, não pra ler. Em vez de mexer no
// viewport (que devolveria o pinch no app INTEIRO, inclusive zoom acidental no chat), o gesto é
// implementado só aqui — determinístico, sem depender de como o iOS trata o viewport em standalone.
//
// Pinch com dois dedos, duplo-toque pra alternar, arrastar quando ampliado.

export const MAX_SCALE = 6;
const TAP_MS = 300;      // janela do duplo-toque
const TAP_PX = 30;       // tolerância de deslocamento entre os dois toques
const DOUBLE_TAP_SCALE = 2.5;

/** Escala presa entre 1 (imagem inteira) e o teto. */
export function clampScale(s: number, max = MAX_SCALE): number {
  return Math.min(max, Math.max(1, s));
}

/**
 * Limite de deslocamento pra imagem não sair da tela: com origem no centro, a metade que "sobra"
 * ao ampliar é (escala-1)/2 de cada lado. Em escala 1 o limite é 0 — a imagem fica travada no meio.
 */
export function clampPan(tx: number, ty: number, scale: number, w: number, h: number) {
  const maxX = Math.max(0, ((scale - 1) * w) / 2);
  const maxY = Math.max(0, ((scale - 1) * h) / 2);
  // `+ 0` mata o zero NEGATIVO que sai do Math.max quando o limite é 0 — senão vira
  // "translate(-0px)" no estilo.
  return {
    x: Math.min(maxX, Math.max(-maxX, tx)) + 0,
    y: Math.min(maxY, Math.max(-maxY, ty)) + 0,
  };
}

/**
 * Novo deslocamento pra manter FIXO o ponto sob os dedos ao mudar a escala.
 * `p` é o ponto na tela relativo ao centro do elemento.
 */
export function panParaAncorar(
  p: { x: number; y: number },
  t: { x: number; y: number },
  de: number,
  para: number,
) {
  const v = { x: (p.x - t.x) / de, y: (p.y - t.y) / de };   // ponto em coordenadas da imagem
  return { x: p.x - para * v.x, y: p.y - para * v.y };
}

interface Opts {
  /** Avisa quando um gesto está em curso — o overlay usa pra não fechar num arrasto/pinch. */
  onGesture?: (ativo: boolean) => void;
}

export function zoomable(node: HTMLElement, opts: Opts = {}) {
  let scale = 1;
  let t = { x: 0, y: 0 };
  const pointers = new Map<number, { x: number; y: number }>();

  // Estado do gesto em curso
  let baseScale = 1;
  let baseT = { x: 0, y: 0 };
  let baseDist = 0;
  let baseMid = { x: 0, y: 0 };     // ponto médio dos DOIS dedos (só no pinch)
  let inicioArrasto = { x: 0, y: 0 };  // ponto do dedo único (só no arrasto)
  let arrastou = false;
  let ultimoTap = 0;
  let ultimoTapPos = { x: 0, y: 0 };

  node.style.touchAction = 'none';
  node.style.willChange = 'transform';

  const centro = () => {
    const r = node.getBoundingClientRect();
    // getBoundingClientRect vem TRANSFORMADO: desconta a escala E o deslocamento pra achar o centro
    // de repouso. Sem descontar o deslocamento, a âncora do pinch andava junto com a imagem e um
    // pinch simétrico no centro empurrava a foto pro lado (visto no teste: 80px de deriva).
    return {
      x: r.left + r.width / 2 - t.x,
      y: r.top + r.height / 2 - t.y,
      w: r.width / scale,
      h: r.height / scale,
    };
  };

  function aplicar() {
    node.style.transform = scale === 1 ? '' : `translate(${t.x}px, ${t.y}px) scale(${scale})`;
    node.style.cursor = scale > 1 ? 'grab' : '';
  }

  function irPara(novaEscala: number, ponto?: { x: number; y: number }) {
    const c = centro();
    const alvo = clampScale(novaEscala);
    if (ponto) {
      const p = { x: ponto.x - c.x, y: ponto.y - c.y };
      t = panParaAncorar(p, t, scale, alvo);
    }
    scale = alvo;
    t = scale === 1 ? { x: 0, y: 0 } : clampPan(t.x, t.y, scale, c.w, c.h);
    node.style.transition = 'transform 200ms var(--ease-out)';
    aplicar();
    setTimeout(() => (node.style.transition = ''), 220);
  }

  function down(e: PointerEvent) {
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    // A captura e um bonus (segue o dedo fora do elemento), NAO um pre-requisito: ela lanca quando
    // o ponteiro ja foi liberado, e a excecao abortava o resto do handler — o segundo dedo do pinch
    // nunca registrava a distancia base e o gesto inteiro morria calado.
    try {
      node.setPointerCapture?.(e.pointerId);
    } catch {
      /* segue sem captura */
    }
    arrastou = false;
    baseScale = scale;
    baseT = { ...t };
    if (pointers.size === 1) inicioArrasto = { x: e.clientX, y: e.clientY };
    if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      baseDist = Math.hypot(a.x - b.x, a.y - b.y);
      baseMid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      opts.onGesture?.(true);
    }
  }

  function move(e: PointerEvent) {
    if (!pointers.has(e.pointerId)) return;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    const c = centro();

    if (pointers.size >= 2) {
      const [a, b] = [...pointers.values()];
      const dist = Math.hypot(a.x - b.x, a.y - b.y);
      if (!baseDist) return;
      const alvo = clampScale(baseScale * (dist / baseDist));
      const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
      const p = { x: baseMid.x - c.x, y: baseMid.y - c.y };
      const ancorado = panParaAncorar(p, baseT, baseScale, alvo);
      // Acompanha também o arrasto do par de dedos.
      const arrasto = { x: mid.x - baseMid.x, y: mid.y - baseMid.y };
      scale = alvo;
      t = clampPan(ancorado.x + arrasto.x, ancorado.y + arrasto.y, scale, c.w, c.h);
      arrastou = true;
      opts.onGesture?.(true);
      aplicar();
      return;
    }

    // Um dedo só arrasta quando há o que arrastar (ampliado). Em escala 1 o toque fica livre
    // pro overlay fechar, que é o comportamento de sempre.
    if (scale > 1) {
      const delta = { x: e.clientX - inicioArrasto.x, y: e.clientY - inicioArrasto.y };
      t = clampPan(baseT.x + delta.x, baseT.y + delta.y, scale, c.w, c.h);
      if (Math.abs(delta.x) > 4 || Math.abs(delta.y) > 4) {
        arrastou = true;
        opts.onGesture?.(true);
      }
      aplicar();
    }
  }

  function up(e: PointerEvent) {
    pointers.delete(e.pointerId);
    if (pointers.size > 0) return;
    baseDist = 0;
    baseMid = { x: 0, y: 0 };

    // 50ms cobre o clique que o browser dispara logo depois do pointerup (é assim que o overlay
    // sabe que não deve fechar no fim de um arrasto).
    let atrasoLiberar = 50;
    if (!arrastou) {
      const agora = Date.now();
      const perto =
        Math.abs(e.clientX - ultimoTapPos.x) < TAP_PX &&
        Math.abs(e.clientY - ultimoTapPos.y) < TAP_PX;
      if (agora - ultimoTap < TAP_MS && perto) {
        // Duplo-toque: alterna entre inteira e ampliada, ancorando no ponto tocado.
        irPara(scale > 1 ? 1 : DOUBLE_TAP_SCALE, { x: e.clientX, y: e.clientY });
        ultimoTap = 0;
        opts.onGesture?.(true);
        atrasoLiberar = 350;      // engole o clique do segundo toque
      } else {
        ultimoTap = agora;
        ultimoTapPos = { x: e.clientX, y: e.clientY };
      }
    }
    // SEMPRE libera. Antes, só liberava quando houve arrasto ou duplo-toque: encostar dois dedos
    // sem mover (pinch acidental) prendia a trava pra sempre e a imagem NUNCA mais fechava —
    // sem Escape no overlay, só recarregando o app.
    setTimeout(() => opts.onGesture?.(false), atrasoLiberar);
  }

  node.addEventListener('pointerdown', down);
  node.addEventListener('pointermove', move);
  node.addEventListener('pointerup', up);
  node.addEventListener('pointercancel', up);

  return {
    destroy() {
      node.removeEventListener('pointerdown', down);
      node.removeEventListener('pointermove', move);
      node.removeEventListener('pointerup', up);
      node.removeEventListener('pointercancel', up);
    },
  };
}

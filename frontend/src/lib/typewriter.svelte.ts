/**
 * Máquina de escrever da prévia ao vivo: desacopla o RITMO de chegada (o hook do Claude entrega
 * ~um parágrafo por vez; Pi/Codex, rajadas de tokens) do ritmo de EXIBIÇÃO — a mesma ideia do
 * smoothStream do Vercel AI SDK, só que no cliente, porque aqui o backend não muda (a entrega
 * SSE segue full-replace a cada ~150ms).
 *
 * Regras, na ordem em que importam:
 *  - Só anima quando o texto novo ESTENDE o que já está na tela (é o caminho do sidecar, que
 *    acumula deltas). Troca de conteúdo (pane oscilando, mensagem nova) vira snap imediato —
 *    "digitar de novo" um texto que a pessoa já leu é pior que o pulo que se quer esconder.
 *  - Ritmo adaptativo: piso de 160 chars/s (legível e vivo), e nunca deixa o atraso passar de
 *    ~1.2s — o próximo pedaço chega em ~1s (medido no hook), então a bolha alcança antes dele
 *    e não fica eternamente atrás numa resposta longa.
 *  - `prefers-reduced-motion` = snap sempre (o typewriter É movimento).
 *  - Passo de ~33ms (30fps), não por frame: cada avanço re-renderiza o markdown da prévia
 *    inteira, e 60×/s disso no celular é custo sem ganho visível.
 */
import { untrack } from 'svelte';

const PISO_CHARS_S = 160;
const ATRASO_MAX_S = 1.2;
const PASSO_MS = 33;

export class Typewriter {
  alvo = $state('');
  mostrado = $state(0);
  #raf = 0;
  #tPrev = 0;
  #prazo = 0;   // instante em que o backlog atual precisa estar todo na tela
  #snap: boolean;

  constructor(snap?: boolean) {
    // matchMedia lido UMA vez: quem troca a preferência no meio de uma prévia viva é caso raro
    // demais pra pagar um listener por bolha.
    this.#snap = snap ?? (typeof matchMedia !== 'undefined'
      && matchMedia('(prefers-reduced-motion: reduce)').matches);
  }

  /** Texto atualmente revelado — o que a bolha renderiza. */
  get texto(): string {
    return this.alvo.slice(0, this.mostrado);
  }

  /** Novo texto completo (full-replace, como chega do SSE). */
  set(texto: string) {
    // untrack no corpo INTEIRO: set() roda dentro de um $effect e escreve alvo/mostrado — mas
    // tambem LE os dois (this.texto, o Math.min, o if). Sem untrack o efeito chamador vira
    // dependente do que a propria animacao muta, e passa a rodar de novo a cada chunk (medido
    // na review com flushSync: 2x por troca) e a cada frame do tick.
    untrack(() => {
      const estende = texto.startsWith(this.texto);
      this.alvo = texto;
      if (this.#snap || !estende) {
        this.mostrado = texto.length;
        return;
      }
      this.mostrado = Math.min(this.mostrado, texto.length);
      if (this.mostrado < texto.length) {
        // Pedaço novo renova o prazo: o backlog TODO (o que sobrou + o que chegou) tem 1.2s pra
        // aparecer. Sem prazo, o ritmo proporcional ao que falta decai exponencialmente e o rabo
        // da mensagem se arrasta no piso pra sempre (pego pelo teste de backlog grande).
        if (this.#raf) {
          this.#prazo = this.#tPrev + ATRASO_MAX_S * 1000;   // animando: relogio ja conhecido
        } else {
          // Base do relogio vem do 1º tick, NUNCA de performance.now() aqui: o rAF manda o seu
          // proprio timestamp, e misturar os dois relogios (ou o do teste, que anda na mao)
          // dava dt negativo e a animacao nascia morta — flaky medido na review.
          this.#tPrev = NaN;
          this.#agendar();
        }
      }
    });
  }

  // rAF guardado num só lugar: em teste (node, sem requestAnimationFrame) os passos vêm de
  // chamadas diretas a tick(), e um rAF solto aqui viraria ReferenceError.
  #agendar() {
    if (typeof requestAnimationFrame !== 'undefined') {
      this.#raf = requestAnimationFrame((t) => this.tick(t));
    }
  }

  /** Um passo da animação. Público só pra teste — em produção quem chama é o rAF. */
  tick(agora: number) {
    this.#raf = 0;
    const falta = this.alvo.length - this.mostrado;
    if (falta <= 0) return;
    if (Number.isNaN(this.#tPrev)) {
      // 1º tick de uma animacao nova: fixa a base do relogio e o prazo no MESMO relogio dos
      // proximos frames, e so avanca a partir do seguinte.
      this.#tPrev = agora;
      this.#prazo = agora + ATRASO_MAX_S * 1000;
      this.#agendar();
      return;
    }
    const dt = (agora - this.#tPrev) / 1000;
    if (dt * 1000 < PASSO_MS) {
      this.#agendar();
      return;
    }
    this.#tPrev = agora;
    const restante = Math.max((this.#prazo - agora) / 1000, 0.05);
    const ritmo = Math.max(PISO_CHARS_S, falta / restante);
    this.mostrado = Math.min(this.alvo.length, this.mostrado + Math.max(1, Math.round(ritmo * dt)));
    if (this.mostrado < this.alvo.length) this.#agendar();
  }

  /** Desmontagem da bolha: nenhum frame pendente pode sobreviver ao componente. */
  parar() {
    if (this.#raf) cancelAnimationFrame(this.#raf);
    this.#raf = 0;
  }
}

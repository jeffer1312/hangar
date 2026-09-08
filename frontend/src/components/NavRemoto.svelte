<script lang="ts">
  // O navegador embutido da sessão, visto e mexido daqui. Não é cópia nem espelho: é o MESMO
  // navegador que o agente dirige — o toque daqui e o clique dele chegam no mesmo lugar.
  //
  // Duas escolhas que definem o uso no celular:
  //  * arrastar ROLA a página (vira roda do mouse), não arrasta o ponteiro. Sem isso não dá pra
  //    passear numa página comprida com o dedo, que é o gesto que a pessoa já tem na mão.
  //  * o toque curto vira clique só quando o dedo quase não andou — senão todo fim de rolagem
  //    clicava no link que estivesse embaixo.
  import { onDestroy } from 'svelte';
  import * as m from '../paraglide/messages';
  import { navUrl } from '../lib/navRemoto';
  import SegmentedPicker from './SegmentedPicker.svelte';

  interface Props {
    sessionName: string;
    /** Fecha a conexão sem desmontar o componente (folha fechada). */
    ativo?: boolean;
  }
  let { sessionName, ativo = true }: Props = $props();

  let ws: WebSocket | null = null;
  let quadro = $state('');              // data: URI do último quadro
  let largura = $state(1280);
  let altura = $state(800);
  let lento = $state(false);            // quadro veio de print (painel fechado no desktop)
  let url = $state('');
  let estado = $state<'ligando' | 'ligado' | 'caiu'>('ligando');
  let erro = $state('');
  let modo = $state<'desktop' | 'mobile'>('desktop');
  let telaEl: HTMLDivElement | null = $state(null);
  let tecladoEl: HTMLTextAreaElement | null = $state(null);

  function ligar() {
    desligar();
    estado = 'ligando';
    erro = '';
    const s = new WebSocket(navUrl(sessionName));
    ws = s;
    s.onopen = () => (estado = 'ligado');
    // O motivo do backend é o que separa "esta sessão não tem navegador aberto" de queda de rede —
    // sem ele, problema de configuração e cabo solto viravam a mesma frase genérica.
    s.onclose = (e) => {
      if (ws !== s) return;
      estado = 'caiu';
      if (e?.reason) erro = e.reason;
    };
    s.onmessage = (e) => {
      let msg: Record<string, unknown>;
      try { msg = JSON.parse(String(e.data)); } catch { return; }
      if (msg.t === 'q') {
        quadro = `data:image/jpeg;base64,${msg.d}`;
        largura = Number(msg.w) || largura;
        altura = Number(msg.h) || altura;
        lento = !!msg.lento;
      } else if (msg.t === 'url') {
        url = String(msg.url ?? '');
      } else if (msg.t === 'layout') {
        // A resposta vem do shell e pode ser recusa — quem manda no rótulo é o que ele respondeu,
        // não o que a gente pediu.
        const r = String(msg.resposta ?? '');
        if (r.startsWith('layout:')) modo = msg.modo === 'mobile' ? 'mobile' : 'desktop';
        // "verbo desconhecido" aqui só quer dizer uma coisa: o app desktop está rodando a versão
        // anterior a este verbo, e o verbo mora no processo principal do Electron. Repassar o texto
        // cru mandava a pessoa procurar defeito onde não há.
        else if (r.includes('verbo desconhecido')) erro = m.nav_remoto_reabrir_app();
        else erro = r;
      } else if (msg.t === 'erro') {
        erro = String(msg.m ?? '');
      }
    };
  }

  function desligar() {
    if (!ws) return;
    const s = ws;
    ws = null;
    s.onclose = null;
    s.close();
  }

  function manda(o: Record<string, unknown>) {
    if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(o));
  }

  $effect(() => {
    if (ativo && sessionName) ligar();
    else desligar();
  });
  onDestroy(desligar);

  // ── ponteiro ────────────────────────────────────────────────────────────────
  // Fração do quadro, não pixel: quem renderiza aqui não sabe o tamanho do viewport do outro lado,
  // e ele muda quando o layout troca.
  function fracao(ev: PointerEvent): { x: number; y: number } {
    const r = (ev.currentTarget as HTMLElement).getBoundingClientRect();
    return { x: (ev.clientX - r.left) / r.width, y: (ev.clientY - r.top) / r.height };
  }

  const ARRASTO_MIN = 8;                // px na tela daqui, não do outro lado
  let inicio: { x: number; y: number; fx: number; fy: number } | null = null;
  let ultimo: { x: number; y: number } | null = null;
  let rolou = false;

  function aoDescer(ev: PointerEvent) {
    const f = fracao(ev);
    inicio = { x: ev.clientX, y: ev.clientY, fx: f.x, fy: f.y };
    ultimo = { x: ev.clientX, y: ev.clientY };
    rolou = false;
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
  }

  function aoMover(ev: PointerEvent) {
    if (!inicio || !ultimo) return;
    const dx = ev.clientX - ultimo.x;
    const dy = ev.clientY - ultimo.y;
    if (!rolou && Math.hypot(ev.clientX - inicio.x, ev.clientY - inicio.y) < ARRASTO_MIN) return;
    rolou = true;
    ultimo = { x: ev.clientX, y: ev.clientY };
    const f = fracao(ev);
    manda({ t: 'w', x: f.x, y: f.y, dx: -dx, dy: -dy });
  }

  // Cancelamento não é toque concluído: o navegador desistiu do gesto (perdeu a captura, gesto do
  // sistema, foco mudou). Tratar igual ao `pointerup` clicava sozinho na página do outro lado.
  function aoCancelar() {
    inicio = null;
    ultimo = null;
  }

  function aoSubir(ev: PointerEvent) {
    if (inicio && !rolou) {
      manda({ t: 'm', acao: 'press', x: inicio.fx, y: inicio.fy });
      manda({ t: 'm', acao: 'release', x: inicio.fx, y: inicio.fy });
      tecladoEl?.focus();               // teclado do celular sobe junto do toque, como num site
    }
    inicio = null;
    ultimo = null;
  }

  function aoRodar(ev: WheelEvent) {
    const r = (ev.currentTarget as HTMLElement).getBoundingClientRect();
    manda({ t: 'w', x: (ev.clientX - r.left) / r.width, y: (ev.clientY - r.top) / r.height,
            dx: ev.deltaX, dy: ev.deltaY });
  }

  // ── teclado ─────────────────────────────────────────────────────────────────
  // Uma textarea invisível recebe o que o teclado do sistema produzir: texto vai por `insertText`
  // (um comando por trecho, acento incluído) e tecla nomeada vai por evento de tecla.
  const NOMEADAS = new Set(['Enter', 'Backspace', 'Tab', 'Escape', 'ArrowUp', 'ArrowDown',
                            'ArrowLeft', 'ArrowRight', 'Home', 'End', 'PageUp', 'PageDown']);
  function aoTeclar(ev: KeyboardEvent) {
    if (NOMEADAS.has(ev.key)) {
      ev.preventDefault();
      manda({ t: 'k', key: ev.key });
    }
  }
  function aoEntrarTexto(ev: Event) {
    const el = ev.currentTarget as HTMLTextAreaElement;
    if (el.value) manda({ t: 'txt', s: el.value });
    el.value = '';
  }

  function trocarModo(novo: 'desktop' | 'mobile') {
    manda({ t: 'layout', modo: novo });
  }
</script>

<div class="nr">
  <div class="nr-bar">
    <span class="nr-url" title={url}>{url || m.nav_remoto_sem_url()}</span>
    <SegmentedPicker
      value={modo}
      ariaLabel={m.nav_remoto_layout()}
      options={[
        { v: 'desktop', label: m.nav_remoto_desktop(), aria: m.nav_remoto_desktop() },
        { v: 'mobile', label: m.nav_remoto_celular(), aria: m.nav_remoto_celular() },
      ]}
      onPick={trocarModo}
    />
  </div>

  {#if erro}
    <p class="nr-erro" role="alert">{erro}</p>
  {/if}

  <div
    class="nr-tela"
    bind:this={telaEl}
    data-gesto-proprio
    style="aspect-ratio: {largura} / {altura};"
    role="application"
    aria-label={m.nav_remoto_tela()}
    tabindex="-1"
    onpointerdown={aoDescer}
    onpointermove={aoMover}
    onpointerup={aoSubir}
    onpointercancel={aoCancelar}
    onwheel={aoRodar}
  >
    {#if quadro}
      <img src={quadro} alt="" draggable="false" />
    {:else}
      <p class="nr-vazio">{estado === 'caiu' ? m.nav_remoto_caiu() : m.nav_remoto_ligando()}</p>
    {/if}
  </div>

  <div class="nr-pe">
    {#if lento}<span class="nr-aviso">{m.nav_remoto_lento()}</span>{/if}
    {#if estado === 'caiu'}
      <button class="nr-religar" onclick={ligar}>{m.nav_remoto_religar()}</button>
    {/if}
  </div>

  <!-- Fora da tela, não `hidden`: um campo escondido de verdade não recebe foco, e sem foco o
       teclado do celular não sobe. -->
  <textarea
    class="nr-teclado"
    bind:this={tecladoEl}
    onkeydown={aoTeclar}
    oninput={aoEntrarTexto}
    aria-label={m.nav_remoto_teclado()}
    autocapitalize="off"
    autocomplete="off"
    spellcheck="false"
  ></textarea>
</div>

<style>
  .nr { display: flex; flex-direction: column; gap: var(--space-2); }
  .nr-bar { display: flex; align-items: center; gap: var(--space-2); }
  .nr-url {
    flex: 1; min-width: 0; font-size: var(--text-xs); color: var(--text-muted);
    font-family: var(--font-mono); white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .nr-tela {
    position: relative; width: 100%; overflow: hidden; touch-action: none;
    background: var(--surface-inset); border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md); display: grid; place-items: center;
    /* Teto de altura: com o navegador em layout de celular o quadro é mais alto que largo, e o
       `aspect-ratio` sozinho empurrava os controles pra fora da folha. */
    max-height: 62vh;
  }
  .nr-tela img {
    width: 100%; height: 100%; object-fit: contain; display: block; user-select: none;
    -webkit-user-drag: none;   /* iOS: arrastar a imagem virava "salvar foto" no meio da rolagem */
  }
  .nr-vazio { font-size: var(--text-sm); color: var(--text-muted); }
  .nr-erro { font-size: var(--text-xs); color: var(--error); }
  .nr-pe { display: flex; align-items: center; gap: var(--space-2); min-height: 20px; }
  .nr-aviso { font-size: var(--text-xs); color: var(--text-muted); }
  .nr-religar {
    min-height: 0; height: 28px; padding: 0 var(--space-3); border-radius: var(--radius-md);
    background: var(--surface-raised); color: var(--text-secondary); font-size: var(--text-xs);
  }
  .nr-teclado {
    position: absolute; left: -9999px; width: 1px; height: 1px; opacity: 0;
  }
</style>

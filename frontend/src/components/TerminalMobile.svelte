<script lang="ts">
  // Terminal DE VERDADE no celular: o mesmo PTY por WebSocket que o painel do desktop usa
  // (app/termsock.py roda `tmux attach`, o backend nao interpreta nada), com a casca que o espelho
  // ja tinha — voltar, tamanho de fonte, campo de texto e barra de teclas de resgate.
  //
  // Por que nao reusar o TerminalPanel: ele e uma FAIXA do rodape do desktop (alca de arrastar,
  // maximizar, abas attach/shell, botao de terminal nativo) e nada disso existe no celular; o que
  // valia reusar — montar o xterm com o tema e a fonte do app — mora em lib/xterm.ts e e o mesmo
  // codigo nos dois.
  //
  // O TerminalMirror (capture-pane a cada 450ms, texto cru) CONTINUA existindo: e o caminho quando o
  // servidor nao tem `pty` (Windows), onde este aqui abriria morto.
  import ModalDialog from './ModalDialog.svelte';
  import { TermSocket, termUrlForServer, sessionExistsOnServer } from '../lib/term';
  import { novoTerminal, temaDe } from '../lib/xterm';
  import { listServers, getActiveId } from '../lib/auth';
  import type { Terminal } from '@xterm/xterm';
  import type { FitAddon } from '@xterm/addon-fit';
  import * as m from '../paraglide/messages';

  interface Props {
    open: boolean;
    sessionName: string;
    onClose: () => void;
  }
  let { open, sessionName, onClose }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let caiu = $state(false);
  // Cano aberto de verdade (onopen do WebSocket), nao "socket criado": entre um e outro ha a janela
  // do handshake, e nela toda tecla da barra sai no vazio — sem eco, sem erro, sem nada.
  let pronto = $state(false);
  let motivo = $state<string | null>(null);
  let erro = $state<string | null>(null);
  let geracao = $state(0);          // incrementar reconecta de verdade
  let sock: TermSocket | null = null;
  let term: Terminal | null = null;
  let fit: FitAddon | null = null;
  let ro: ResizeObserver | null = null;
  let mo: MutationObserver | null = null;
  const enc = new TextEncoder();

  // ── Tamanho da fonte ─────────────────────────────────────────────────────────
  // No celular ela nao e so legibilidade: o tmux redimensiona a janela pro tamanho DESTE cliente
  // (window-size=latest), entao a fonte decide quantas COLUNAS a TUI vai ter enquanto o terminal
  // estiver aberto. Persistida — reajustar a cada abertura seria atrito puro.
  const FONT_MIN = 6, FONT_MAX = 18, FONT_KEY = 'cp_term_font';
  let fontPx = $state(
    (() => {
      // try/catch na LEITURA tambem: com cookies bloqueados no Safari o proprio acesso a
      // propriedade `localStorage` lanca SecurityError, e isso derrubaria a inicializacao do
      // componente inteiro, nao so a persistencia.
      try {
        const v = Number(localStorage?.getItem(FONT_KEY));
        if (Number.isFinite(v) && v >= FONT_MIN && v <= FONT_MAX) return v;
      } catch { /* storage bloqueado: cai no default */ }
      return 9;
    })(),
  );
  function bumpFont(d: number) {
    fontPx = Math.min(FONT_MAX, Math.max(FONT_MIN, fontPx + d));
    // ponytail: preferencia nao persistida e perda aceitavel (modo privado/quota); so nao pode ser
    // 100% muda.
    try { localStorage.setItem(FONT_KEY, String(fontPx)); }
    catch (e) { console.warn('cp: tamanho de fonte do terminal nao persistiu', e); }
  }

  // Fonte NAO remonta o terminal: troca a opcao, refaz o fit e avisa o tmux do tamanho novo. Um
  // efeito de montagem que dependesse de `fontPx` fecharia e reabriria o socket a cada toque em A+,
  // e cada reconexao repinta a TUI inteira.
  $effect(() => {
    const px = fontPx;
    if (!term || !fit) return;
    term.options.fontSize = px;
    fit.fit();
    sock?.resize(term.cols, term.rows);
  });

  // Servidor da sessao. No celular a rota ja aponta o ATIVO pra sessao aberta (App.applyRouteServer)
  // — o mesmo servidor que o resto do Chat usa nos fetches desta tela.
  function servidorAtivo() {
    const id = getActiveId();
    return listServers().find((s) => s.id === id) ?? null;
  }

  $effect(() => {
    const alvo = sessionName;
    void geracao;
    if (!open || !host) return;
    const hostEl = host;
    let vivo = true;
    caiu = false;
    motivo = null;
    erro = null;

    (async () => {
      const srv = servidorAtivo();
      if (!srv) {
        caiu = true;
        erro = m.servidor_nao_existe();
        return;
      }
      // Probe antes do socket: o backend recusa sessao inexistente FECHANDO ANTES do accept, e isso
      // chega no navegador como 1006 mudo — sem o probe, "sessao que nao existe" e "rede caiu" viram
      // a mesma frase. Servidor fora do ar NAO e "sessao nao encontrada": deixa a conexao tentar.
      let existe = true;
      try {
        existe = await sessionExistsOnServer(srv, alvo);
      } catch {
        existe = true;
      }
      if (!vivo) return;
      if (!existe) {
        caiu = true;
        erro = m.erro_sessao_inexistente();
        return;
      }

      const [{ Terminal }, { FitAddon }] = await Promise.all([
        import('@xterm/xterm'),
        import('@xterm/addon-fit'),
      ]);
      await import('@xterm/xterm/css/xterm.css');
      if (!vivo) return;

      const r = novoTerminal(hostEl, Terminal, FitAddon, fontPx);
      term = r.term; fit = r.fit;
      const t = r.term;   // copia local nao-nula: `term` e reatribuido e zerado no cleanup

      mo = new MutationObserver(() => { t.options.theme = temaDe(hostEl); });
      mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });

      sock = new TermSocket(termUrlForServer(srv, alvo, t.cols, t.rows), {
        data: (b) => { t.write(b); agendarBuscaDeUrl(); },
        open: () => { if (vivo) pronto = true; },
        // `vivo`, nao incondicional: o close() dispara onclose ASSINCRONO, e sem a guarda o "caiu"
        // de um socket ja descartado aterrissava na proxima conexao (ou num componente destruido).
        close: (motivoFechamento) => {
          if (vivo) { caiu = true; pronto = false; motivo = motivoFechamento ?? null; }
        },
      });
      // Digitar e DIRETO no xterm: tocar na tela ja levanta o teclado do sistema (o xterm foca
      // sozinho a <textarea> escondida dele) e cada tecla vai como byte pro PTY. A barra de baixo
      // fica so com o que teclado de celular nao tem (Esc, Tab, setas, PgUp/PgDn).
      t.onData((d: string) => { sock?.send(enc.encode(d)); agendarBuscaDeUrl(); });

      // { passive: false } exige addEventListener na mao: o `ontouchmove` do template nasce PASSIVO
      // (o navegador ignora o preventDefault e ainda avisa no console), e sem o preventDefault o
      // Safari leva o arrasto pro bounce da pagina em vez de deixar a TUI paginar.
      hostEl.addEventListener('touchstart', toqueInicio, { passive: true });
      hostEl.addEventListener('touchmove', toqueMove, { passive: false });

      ro = new ResizeObserver(() => { r.fit.fit(); sock?.resize(t.cols, t.rows); });
      ro.observe(hostEl);
    })().catch((e) => {
      // Falha do import DINAMICO (pedaco 404 depois de um deploy com a tela aberta; o dev server do
      // Vite servindo modulo vazio, documentado no CLAUDE.md) ou do proprio mount. Sem isto virava
      // rejeicao nao tratada: tela preta pra sempre, sem nem o botao de reconectar.
      if (!vivo) return;
      caiu = true;
      motivo = e instanceof Error ? m.term_falha_carregar_msg({ msg: e.message })
                                  : m.term_falha_carregar();
    });

    return () => {
      vivo = false;
      pronto = false;
      hostEl.removeEventListener('touchstart', toqueInicio);
      hostEl.removeEventListener('touchmove', toqueMove);
      ro?.disconnect(); ro = null;
      mo?.disconnect(); mo = null;
      sock?.close(); sock = null;
      term?.dispose(); term = null;
      fit = null;
      paneUrl = null;
      clearTimeout(urlTimer);
    };
  });

  // ── Link na tela ─────────────────────────────────────────────────────────────
  // Herdado do espelho, e por um motivo concreto: no login por OAuth a URL aparece DENTRO da TUI, e
  // copiar um endereco gigante de uma fonte de 9px com o dedo era o atrito. O xterm nao faz link
  // clicavel sozinho (isso e um addon a parte), entao a tela varre o proprio buffer.
  let paneUrl = $state<string | null>(null);
  let urlTimer: ReturnType<typeof setTimeout> | undefined;
  function agendarBuscaDeUrl() {
    // Coalescido: `data` chega token a token, e varrer o buffer a cada quadro seria varrer a tela
    // inteira dezenas de vezes por segundo.
    clearTimeout(urlTimer);
    urlTimer = setTimeout(procurarUrl, 600);
  }
  function procurarUrl() {
    const t = term;
    if (!t) return;
    const buf = t.buffer.active;
    let texto = '';
    for (let i = 0; i < t.rows; i++) {
      const linha = buf.getLine(buf.viewportY + i);
      if (!linha) continue;
      // `true` = junta a linha logica quebrada pelo wrap: sem isso a URL cortada no fim da linha
      // virava dois pedacos e nenhum deles abria.
      texto += linha.translateToString(true) + '\n';
    }
    paneUrl = texto.match(/https?:\/\/\S+/)?.[0] ?? null;
  }

  // ── Entrada ──────────────────────────────────────────────────────────────────
  // Sequencias CRUAS pro PTY, nao os nomes de tecla do /term-input: quem esta do outro lado e o
  // `tmux attach`, que parseia a entrada como um terminal de verdade e reemite pro programa no modo
  // que ELE espera (inclusive cursor-keys em modo aplicacao). E o mesmo byte que uma tecla fisica
  // mandaria.
  const SEQ = {
    Escape: '\x1b', Tab: '\t', Enter: '\r',
    Up: '\x1b[A', Down: '\x1b[B', Right: '\x1b[C', Left: '\x1b[D',
    PageUp: '\x1b[5~', PageDown: '\x1b[6~',
  } as const;

  function tecla(nome: keyof typeof SEQ) {
    // Sem cano aberto nao adianta mandar: `TermSocket.send` e no-op de proposito e a barra nao tem
    // eco nenhum pra denunciar (o eco de um terminal vem do PTY). Os botoes ja ficam desabilitados
    // fora do ar; isto e a guarda do caminho programatico.
    if (!sock?.aberto) return;
    sock.send(enc.encode(SEQ[nome]));
    agendarBuscaDeUrl();
  }

  // ── Rolar com o dedo ─────────────────────────────────────────────────────────
  // Arrastar nao rolava NADA, e a causa nao e o gesto: numa TUI de tela alternada (Claude Code,
  // vim, less) o buffer do xterm nao tem historico — medido, `scrollHeight == clientHeight` (690 =
  // 690) com a conversa inteira acima. Quem guarda o passado ali e o PROGRAMA, e ele so devolve
  // com PageUp/PageDown (o mesmo motivo pelo qual os botoes ⇞/⇟ existem desde o espelho).
  // Entao o arrasto vira pagina: cada terco da altura visivel = um PageUp/PageDown.
  // Com buffer NORMAL (shell comum, que tem scrollback de verdade) nao mexemos em nada — ali o
  // proprio viewport do xterm rola nativo, com a inercia do sistema.
  let toqueY = 0;
  let acumulado = 0;
  const emTuiSemHistorico = () => term?.buffer.active.type === 'alternate';

  function toqueInicio(e: TouchEvent) {
    toqueY = e.touches[0]?.clientY ?? 0;
    acumulado = 0;
  }

  function toqueMove(e: TouchEvent) {
    if (!emTuiSemHistorico() || !host) return;
    const y = e.touches[0]?.clientY;
    if (y == null) return;
    acumulado += y - toqueY;
    toqueY = y;
    // preventDefault: sem ele o Safari leva o gesto pro bounce da pagina inteira enquanto a TUI
    // rola por baixo — por isso o listener e registrado com { passive: false }, la no efeito.
    e.preventDefault();
    const passo = Math.max(40, host.clientHeight / 3);
    while (acumulado >= passo) { tecla('PageUp'); acumulado -= passo; }
    while (acumulado <= -passo) { tecla('PageDown'); acumulado += passo; }
  }

  // Botao de barra de ferramentas nao pode levar o foco junto: no celular, tirar o foco da textarea
  // escondida do xterm ABAIXA o teclado — apertar uma seta no meio de uma frase custava dois toques
  // a mais pra voltar a digitar.
  function naoRoubarFoco(e: PointerEvent) {
    e.preventDefault();
  }
</script>

<ModalDialog {open} ariaLabel={m.term_overlay_aria()} onClose={onClose} className="tx-dialog">
  <div class="tx-wrap">
    <header class="tx-head">
      <button class="tx-back" onclick={onClose} aria-label={m.term_voltar_chat()}>
        <span class="tx-back-arrow">←</span> {m.term_voltar_chat()}
      </button>
      <div class="tx-head-right">
        <div class="tx-font" role="group" aria-label={m.term_fonte_tamanho()}>
          <button class="tx-fontbtn" onclick={() => bumpFont(-1)} disabled={fontPx <= FONT_MIN}
                  aria-label={m.term_fonte_diminuir()}>A−</button>
          <span class="tx-fontval">{fontPx}</span>
          <button class="tx-fontbtn" onclick={() => bumpFont(1)} disabled={fontPx >= FONT_MAX}
                  aria-label={m.term_fonte_aumentar()}>A+</button>
        </div>
        <span class="tx-title">{sessionName}</span>
      </div>
    </header>

    <div class="tx-screen" bind:this={host}></div>

    {#if caiu}
      <div class="tx-caiu" role="alert">
        <span>⚠ {erro ?? motivo ?? m.term_desconectado()}</span>
        {#if !erro}
          <button class="tx-key" onclick={() => geracao++}>{m.term_reconectar_btn()}</button>
        {/if}
      </div>
    {:else if !pronto}
      <!-- Janela do handshake: sem isto a barra de teclas parecia funcionar e nao mandava nada. -->
      <div class="tx-conectando" role="status">{m.comum_carregando()}</div>
    {/if}

    {#if paneUrl}
      <a class="tx-link" href={paneUrl} target="_blank" rel="noopener noreferrer">{m.term_abrir_link()}</a>
    {/if}

    <!-- Sem campo de texto: quem digita e o proprio terminal (tocar nele levanta o teclado do
         sistema). A barra abaixo tem so o que teclado de celular nao tem.
         `onpointerdown` com preventDefault em cada botao: sem isso o botao ROUBA o foco da textarea
         do xterm e o teclado do celular abaixa a cada seta apertada. -->
    <nav class="tx-keys" aria-label={m.term_teclas_resgate()}>
      <span class="tx-keys-hint">{m.term_resgate()}</span>
      <button class="tx-key" disabled={!pronto} onpointerdown={naoRoubarFoco} onclick={() => tecla('Escape')}>Esc</button>
      <button aria-label="Tab" class="tx-key" disabled={!pronto} onpointerdown={naoRoubarFoco} onclick={() => tecla('Tab')}>⇥</button>
      <button aria-label={m.term_rolar_cima_aria()} class="tx-key" disabled={!pronto} onpointerdown={naoRoubarFoco} onclick={() => tecla('PageUp')} title={m.term_rolar_cima()}>⇞</button>
      <button aria-label={m.term_rolar_baixo_aria()} class="tx-key" disabled={!pronto} onpointerdown={naoRoubarFoco} onclick={() => tecla('PageDown')} title={m.term_rolar_baixo()}>⇟</button>
      <div class="tx-arrows">
        <button aria-label={m.term_seta_esquerda()} class="tx-key" disabled={!pronto} onpointerdown={naoRoubarFoco} onclick={() => tecla('Left')}>←</button>
        <button aria-label={m.term_seta_cima()} class="tx-key" disabled={!pronto} onpointerdown={naoRoubarFoco} onclick={() => tecla('Up')}>↑</button>
        <button aria-label={m.term_seta_baixo()} class="tx-key" disabled={!pronto} onpointerdown={naoRoubarFoco} onclick={() => tecla('Down')}>↓</button>
        <button aria-label={m.term_seta_direita()} class="tx-key" disabled={!pronto} onpointerdown={naoRoubarFoco} onclick={() => tecla('Right')}>→</button>
      </div>
      <button aria-label="Enter" class="tx-key tx-enter" disabled={!pronto} onpointerdown={naoRoubarFoco} onclick={() => tecla('Enter')}>⏎</button>
    </nav>
  </div>
</ModalDialog>

<style>
  /* Overlay fullscreen (NAO bottom sheet): o terminal precisa da tela toda. */
  :global(.tx-dialog) { width: 100%; max-width: 100%; height: 100%; max-height: 100%; padding: 0; border: 0; border-radius: 0; }
  .tx-wrap { height: 100%; display: flex; flex-direction: column; background: var(--bg-base); }
  .tx-head {
    flex-shrink: 0; display: flex; align-items: center; justify-content: space-between;
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border-subtle);
    padding-top: max(var(--space-2), env(safe-area-inset-top));
  }
  .tx-head-right { display: flex; align-items: center; gap: var(--space-3); min-width: 0; }
  .tx-title { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 40vw; }
  .tx-font { display: flex; align-items: center; gap: var(--space-1); }
  .tx-fontbtn {
    min-width: 30px; height: 28px;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md, 8px);
    background: var(--surface-raised); color: var(--text-secondary);
    font-size: var(--text-xs); font-family: var(--font-mono); -webkit-tap-highlight-color: transparent;
  }
  .tx-fontbtn:disabled { opacity: 0.4; }
  .tx-fontval { font-family: var(--font-mono); font-size: 10px; color: var(--text-muted); min-width: 14px; text-align: center; }
  .tx-back {
    display: inline-flex; align-items: center; gap: var(--space-1);
    background: var(--accent-dim); border: 1px solid var(--accent); color: var(--accent);
    font-size: var(--text-sm); font-weight: 600;
    padding: var(--space-1) var(--space-3); border-radius: var(--radius-full, 999px);
    -webkit-tap-highlight-color: transparent;
  }
  .tx-back-arrow { font-size: var(--text-base); line-height: 1; }

  /* Superficie do terminal. Sem backdrop-filter/transform: no iOS eles promovem camada e pintam
     preto no scroll (regra do CLAUDE.md). O canvas do xterm e transparente, entao quem da o fundo
     e esta caixa — e ela segue o veu do papel de parede como qualquer campo do app. */
  .tx-screen { flex: 1; min-height: 0; overflow: hidden; background: var(--surface-inset); padding: var(--space-1); }
  .tx-screen :global(.xterm) { height: 100%; }

  .tx-caiu {
    flex-shrink: 0; display: flex; align-items: center; justify-content: space-between; gap: var(--space-2);
    padding: var(--space-2) var(--space-3);
    background: color-mix(in srgb, var(--error) 16%, transparent);
    color: var(--text-primary); font-size: var(--text-sm);
  }
  /* Mesma faixa do "caiu", em tom neutro: e informacao de espera, nao erro. */
  .tx-conectando {
    flex-shrink: 0; padding: var(--space-2) var(--space-3);
    background: var(--surface-raised); color: var(--text-muted); font-size: var(--text-xs);
  }
  .tx-link {
    flex-shrink: 0; padding: var(--space-2) var(--space-3);
    color: var(--accent); font-size: var(--text-sm); text-decoration: none;
    border-top: 1px solid var(--border-subtle);
  }

  .tx-keys {
    flex-shrink: 0; display: flex; align-items: center; gap: var(--space-2); flex-wrap: wrap;
    padding: var(--space-2) var(--space-3);
    padding-bottom: max(var(--space-2), env(safe-area-inset-bottom));
    border-top: 1px solid var(--border-subtle);
  }
  .tx-keys-hint { font-size: 10px; letter-spacing: 0.04em; text-transform: uppercase; color: var(--text-muted); }
  .tx-arrows { display: flex; gap: var(--space-1); margin-left: auto; }
  .tx-key {
    min-width: 44px; min-height: 40px; padding: 0 var(--space-2);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md, 8px);
    background: var(--surface-raised); color: var(--text-primary);
    font-family: var(--font-mono); font-size: var(--text-sm); -webkit-tap-highlight-color: transparent;
  }
  .tx-key:disabled { opacity: 0.4; }
  .tx-enter { border-color: var(--accent); color: var(--accent); }
  button:focus-visible, a:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>

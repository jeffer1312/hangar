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
        // `vivo`, nao incondicional: o close() dispara onclose ASSINCRONO, e sem a guarda o "caiu"
        // de um socket ja descartado aterrissava na proxima conexao (ou num componente destruido).
        close: (motivoFechamento) => { if (vivo) { caiu = true; motivo = motivoFechamento ?? null; } },
      });
      // Digitar direto no xterm continua valendo (quem tem teclado bluetooth, ou toca na tela e usa
      // o teclado do sistema); o campo e a barra de teclas embaixo sao o caminho de dedo.
      t.onData((d: string) => sock?.send(enc.encode(d)));

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
    sock?.send(enc.encode(SEQ[nome]));
    agendarBuscaDeUrl();
  }

  let draft = $state('');
  function enviar(comEnter: boolean) {
    const valor = draft;
    if (!valor || !sock) return;
    // Texto e Enter no MESMO envio: dois sends separados abriam janela pra a TUI processar a linha
    // antes do texto inteiro chegar.
    sock.send(enc.encode(comEnter ? valor + '\r' : valor));
    draft = '';
    agendarBuscaDeUrl();
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
    {/if}

    {#if paneUrl}
      <a class="tx-link" href={paneUrl} target="_blank" rel="noopener noreferrer">{m.term_abrir_link()}</a>
    {/if}

    <form class="tx-compose" onsubmit={(e) => { e.preventDefault(); enviar(true); }}>
      <input
        class="tx-input"
        bind:value={draft}
        placeholder={m.term_digitar()}
        aria-label={m.term_texto_para()}
        autocapitalize="off"
        autocorrect="off"
        spellcheck="false"
        enterkeyhint="send"
      />
      <!-- Enviar SEM Enter: num picker/filtro o Enter submeteria antes da hora. -->
      <button class="tx-key" type="button" aria-label={m.term_enviar_sem_enter()}
              disabled={!draft} onclick={() => enviar(false)}
              title={m.term_enviar_sem_enter_curto()}>↵̸</button>
      <button class="tx-key tx-enter" type="submit" disabled={!draft}
              title={m.term_enviar_com_enter()}>{m.term_envia()}</button>
    </form>

    <nav class="tx-keys" aria-label={m.term_teclas_resgate()}>
      <span class="tx-keys-hint">{m.term_resgate()}</span>
      <button class="tx-key" onclick={() => tecla('Escape')}>Esc</button>
      <button aria-label="Tab" class="tx-key" onclick={() => tecla('Tab')}>⇥</button>
      <button aria-label={m.term_rolar_cima_aria()} class="tx-key" onclick={() => tecla('PageUp')} title={m.term_rolar_cima()}>⇞</button>
      <button aria-label={m.term_rolar_baixo_aria()} class="tx-key" onclick={() => tecla('PageDown')} title={m.term_rolar_baixo()}>⇟</button>
      <div class="tx-arrows">
        <button aria-label={m.term_seta_esquerda()} class="tx-key" onclick={() => tecla('Left')}>←</button>
        <button aria-label={m.term_seta_cima()} class="tx-key" onclick={() => tecla('Up')}>↑</button>
        <button aria-label={m.term_seta_baixo()} class="tx-key" onclick={() => tecla('Down')}>↓</button>
        <button aria-label={m.term_seta_direita()} class="tx-key" onclick={() => tecla('Right')}>→</button>
      </div>
      <button aria-label="Enter" class="tx-key tx-enter" onclick={() => tecla('Enter')}>⏎</button>
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
  .tx-link {
    flex-shrink: 0; padding: var(--space-2) var(--space-3);
    color: var(--accent); font-size: var(--text-sm); text-decoration: none;
    border-top: 1px solid var(--border-subtle);
  }

  .tx-compose { flex-shrink: 0; display: flex; align-items: center; gap: var(--space-2); padding: var(--space-2) var(--space-3); border-top: 1px solid var(--border-subtle); }
  /* 16px: abaixo disso o iOS da zoom ao focar e a tela inteira sai do lugar. */
  .tx-input {
    flex: 1; min-width: 0; min-height: 40px; padding: 0 var(--space-3);
    background: var(--surface-inset); border: 1px solid var(--border-subtle); border-radius: var(--radius-md, 8px);
    color: var(--text-primary); font-family: var(--font-mono); font-size: 16px; outline: none;
  }
  .tx-input:focus-visible { border-color: var(--accent); }

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

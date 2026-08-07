<script lang="ts">
  import { TermSocket, termUrl } from '../lib/term';
  import { openShell, openNativeTerminal } from '../lib/api';

  interface Props { sessionName: string; connKey: string; open: boolean; onClose: () => void; }
  let { sessionName, connKey, open, onClose }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let secEl = $state<HTMLElement | null>(null);
  let maximizado = $state(false);
  let caiu = $state(false);
  let geracao = $state(0);          // incrementar reconecta de verdade
  let sock: TermSocket | null = null;
  let term: any = null;
  let fit: any = null;
  let ro: ResizeObserver | null = null;
  let mo: MutationObserver | null = null;   // UM so -- cobre as duas abas (attach e shell)
  let alturaArrastada = '';   // altura que "resize: vertical" grava inline (guardada pra repor ao desmaximizar)

  // ── Segunda aba: shell escondido (Task 6, Step 7) ───────────────────────────────────────────────
  // A sessao tmux e a mesma que o backend cria/reata em POST /shell (separada e ESCONDIDA das tres
  // views do app -- so o painel e o terminal nativo alcancam). So entra depois do primeiro clique na
  // aba, pra nao gastar POST+fork de tmux em quem nunca abre o shell.
  let abaAtiva = $state<'attach' | 'shell'>('attach');
  let shellVisitada = false;                     // trava contra reclique repetindo o POST /shell
  let shellNome = $state<string | null>(null);   // "term-<nome>" devolvido pelo backend -- e o ALVO
  let shellErro = $state<string | null>(null);
  let shellCarregando = $state(false);
  let hostShell = $state<HTMLDivElement | null>(null);
  let caiuShell = $state(false);
  let geracaoShell = $state(0);
  let sockShell: TermSocket | null = null;
  let termShell: any = null;
  let fitShell: any = null;
  let roShell: ResizeObserver | null = null;

  // ── Terminal nativo (item da v1, pedido explicito do dono do plano) ────────────────────────────
  // Aviso do 503 (sem emulador no PATH, ou o emulador morreu logo apos abrir): SOBREVIVE ao fechar
  // do painel de proposito -- o painel fecha ANTES do POST sair (ver abrirTerminalNativo), entao
  // qualquer erro so chega DEPOIS que a `<section>` (que so existe com `open`) ja sumiu do DOM. Por
  // isso este aviso mora FORA do bloco `{#if open}` no template. Sem toast global no app (nao existe
  // um; `window.alert()` foi descartado -- ver o comentario de EnginesSettings.svelte sobre nao usar
  // dialogo nativo, quebra o tema), entao e um aviso local mesmo, auto-some.
  let nativeErro = $state<string | null>(null);
  let nativeErroTimer: ReturnType<typeof setTimeout> | undefined;

  // Alvo do terminal nativo = a aba ATIVA no momento do clique (attach ou shell), o mesmo tmux
  // session name que o painel embutido estaria usando ali. Fecha o painel ANTES de pedir a janela:
  // e a regra que evita dois clientes (o xterm embutido + a janela nativa) com tamanhos diferentes
  // brigando pelo `window-size=latest` da MESMA sessao tmux (mesmo motivo do comentario em
  // termsock.py sobre "um painel por sessao").
  function abrirTerminalNativo() {
    const alvo = abaAtiva === 'attach' ? sessionName : (shellNome ?? sessionName);
    onClose();
    openNativeTerminal(alvo).catch((e) => {
      clearTimeout(nativeErroTimer);
      // Mensagem do backend (503 = sem emulador no PATH, ou o emulador morreu logo apos abrir) --
      // mostrada como veio, nao engolida.
      nativeErro = e instanceof Error ? e.message : 'falha ao abrir o terminal nativo';
      nativeErroTimer = setTimeout(() => { nativeErro = null; }, 8000);
    });
  }

  // Fecha (o X) reseta o maximizado: sem isto o painel reabria maximizado por acidente, herdando
  // estado da vez anterior.
  $effect(() => { if (!open) maximizado = false; });

  // Troca de sessao (outro Chat pediu o painel, MESMO mount) ou fechar o painel: a aba do shell era
  // de outra sessao (ou de uma rodada anterior) -- esquece, senao o usuario digitaria no shell ERRADO
  // com o rotulo da sessao nova por cima. Reabrir chama POST /shell de novo, que e idempotente (reata
  // a MESMA sessao tmux) -- quem preserva a tela e o tmux, nao este estado.
  $effect(() => {
    void sessionName; void connKey; void open;
    abaAtiva = 'attach';
    shellVisitada = false;
    shellNome = null;
    shellErro = null;
    // Sem isto, trocar de sessao com o POST /shell ainda em voo deixava a sessao NOVA nascendo com
    // shellCarregando===true pra sempre (o `finally` do abrirAbaShell so zera se `alvo===sessionName`,
    // e a sessao mudou). Sem consequencia visivel hoje (nada le isto antes do 1o clique), mas e
    // estado mentindo.
    shellCarregando = false;
  });

  // UX de troca de aba: joga o foco pro terminal que ficou visivel, senao o usuario precisa clicar
  // dentro pra digitar depois de trocar. So dispara na troca -- nao precisa ler `term`/`termShell`
  // como dependencia (nao sao $state; o valor lido AQUI, no disparo, ja e o atual).
  $effect(() => { (abaAtiva === 'attach' ? term : termShell)?.focus(); });

  function toggleMax() {
    if (secEl) {
      if (!maximizado) {
        // Vai maximizar: `resize: vertical` grava `height` INLINE ao arrastar a borda, e inline vence
        // o `height: auto` de `.tp.max` no cascade -- sem zerar, maximizar depois de arrastar nao
        // maximizava. Guarda o valor antes de zerar pra poder repor.
        alturaArrastada = secEl.style.height;
        secEl.style.height = '';
      } else {
        // Volta do maximizado: repoe a altura arrastada -- sem isto, desmaximizar sempre caia nos
        // 320px do CSS, perdendo o ajuste manual do usuario.
        secEl.style.height = alturaArrastada;
      }
    }
    maximizado = !maximizado;
  }

  // Cores do xterm a partir dos tokens do app. `color`, propriedade REAL (nao custom property): o
  // browser sempre entrega ela RESOLVIDA, mesmo quando --text-primary e um color-mix() com var()
  // aninhado (app.css:323, o boost de texto sobre papel de parede) -- ler a CUSTOM PROPERTY direto
  // devolveria a string CRUA com var() por dentro (e assim que a spec de CSS Custom Properties define
  // o computed value delas: sem substituir var() aninhado), o xterm rejeitava calado e caia no branco
  // padrao. `body` ja seta `color: var(--text-primary)` (app.css) e `host` herda -- de graca, sem
  // elemento nem estilo extra. --accent, ao contrario, e hex LITERAL nas duas paletas do app.css (sem
  // var() aninhado), entao ler a custom property direto e seguro ali.
  function lerTema(el: HTMLElement) {
    const cs = getComputedStyle(el);
    const fg = cs.color || '#d2cbcd';
    const cursor = cs.getPropertyValue('--accent').trim() || fg;
    return { fg, cursor };
  }

  // Constroi o Terminal+FitAddon com o mesmo tema/fonte das duas abas -- fatorado so pra nao duplicar
  // o comentario do 'rgba(0, 0, 0, 0)' (achado caro, ver abaixo) numa segunda copia que pode divergir.
  function novoTerminal(hostEl: HTMLDivElement, TerminalCls: any, FitAddonCls: any) {
    // getComputedStyle, nao `var(--font-mono)` cru: o renderer canvas monta
    // `ctx.font = \`${size}px ${family}\``, onde var() e invalido e ignorado calado -> metrica de
    // glifo errada e grade desalinhada.
    const mono = getComputedStyle(hostEl).getPropertyValue('--font-mono').trim() || 'monospace';
    const t = new TerminalCls({
      fontFamily: mono, fontSize: 12, convertEol: false,
      allowTransparency: true,
      // 'rgba(0, 0, 0, 0)', NAO 'transparent': o parser de cor do xterm 6.0.0 (Color.ts) so casa
      // hex/rgb()/rgba() -- 'transparent' cai no caminho do canvas e LANCA (alfa != 255 e
      // rejeitado), o ThemeService ENGOLE a excecao calado e devolve o fallback #000000 opaco por
      // cima do --surface-inset do .tp-screen (regra de vidro do CLAUDE.md). Medido no pacote
      // instalado -- nao aparece nenhum erro no console, so o retangulo preto.
      theme: { background: 'rgba(0, 0, 0, 0)', ...lerTema(hostEl) },
    });
    const f = new FitAddonCls();
    t.loadAddon(f);
    t.open(hostEl);
    f.fit();
    return { term: t, fit: f };
  }

  // Garante o MutationObserver compartilhado (troca de tema claro/escuro com o painel ABERTO):
  // qualquer uma das duas abas que montar primeiro o cria; a outra reusa. O fundo acompanha sozinho
  // (e CSS, --surface-inset por baixo do canvas transparente), mas foreground/cursor sao retrato do
  // MOUNT -- sem isto, escuro->claro deixava o texto quase-branco sobre fundo claro ate fechar e
  // reabrir. Cobre as DUAS instancias (Task 6, Step 7): checa cada uma antes de tocar.
  function garantirObserverDeTema() {
    if (mo) return;
    mo = new MutationObserver(() => {
      if (term && host) term.options.theme = { background: 'rgba(0, 0, 0, 0)', ...lerTema(host) };
      if (termShell && hostShell) {
        termShell.options.theme = { background: 'rgba(0, 0, 0, 0)', ...lerTema(hostShell) };
      }
    });
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
  }

  $effect(() => {
    // Os tres lidos AQUI, sincronos: se `sessionName` so fosse lido dentro do callback async (depois
    // do await), o Svelte nao o rastrearia e trocar de sessao no sidebar deixaria o terminal preso
    // na anterior. `geracao` e o que faz o botao de reconectar funcionar.
    const alvo = sessionName;
    // connKey (server-aware, "servidor::nome"): so o nome nao bastava — trocar de servidor com uma
    // sessao homonima na tela nao mudava `sessionName`, e o socket ficava conectado no servidor
    // VELHO (termUrl usa o servidor ATIVO no instante da conexao, nao um serverId explicito).
    void connKey;
    void geracao;
    if (!open || !host) return;
    let vivo = true;
    caiu = false;

    (async () => {
      // Import DINAMICO: xterm so entra no bundle de quem abre o terminal. E feature desktop-only na
      // v1 — o PWA do celular nao pode pagar o download.
      const [{ Terminal }, { FitAddon }] = await Promise.all([
        import('@xterm/xterm'),
        import('@xterm/addon-fit'),
      ]);
      await import('@xterm/xterm/css/xterm.css');
      if (!vivo || !host) return;

      const r = novoTerminal(host, Terminal, FitAddon);
      term = r.term; fit = r.fit;

      garantirObserverDeTema();

      const enc = new TextEncoder();
      sock = new TermSocket(termUrl(alvo, term.cols, term.rows), {
        data: (b) => term.write(b),
        // `vivo`, nao incondicional: TermSocket.close() dispara onclose ASSINCRONO. Ao trocar de
        // sessao, o cleanup fecha o socket velho -> o efeito novo zera `caiu` -> DEPOIS chega o
        // onclose do socket velho -> sem a guarda, `caiu = true` aterrissava na sessao ERRADA (ou
        // num componente ja destruido, se foi o painel que fechou).
        close: () => { if (vivo) caiu = true; },
      });
      term.onData((d: string) => sock?.send(enc.encode(d)));

      ro = new ResizeObserver(() => { fit?.fit(); sock?.resize(term.cols, term.rows); });
      ro.observe(host);
      // I3: o $effect por `abaAtiva` sozinho erra a ESTREIA desta aba -- no primeiro clique em
      // "Shell", `abaAtiva` muda ANTES do POST /shell sair, `term` ainda e null ali, e quando o
      // terminal enfim monta (agora) o efeito ja rodou e nao roda de novo. Foca aqui tambem, so se
      // esta aba seguir sendo a visivel no instante em que o mount terminou.
      if (abaAtiva === 'attach') term.focus();
    })();

    return () => {
      vivo = false;
      ro?.disconnect(); ro = null;
      sock?.close(); sock = null;
      term?.dispose(); term = null;
      mo?.disconnect(); mo = null;
    };
  });

  // Clique na aba Shell: so dispara o POST na PRIMEIRA vez (shellVisitada trava reclique). Idempotente
  // no backend (reata a mesma sessao), mas repetir aqui so gastaria rede a toa.
  async function abrirAbaShell() {
    abaAtiva = 'shell';
    if (shellVisitada) return;
    shellVisitada = true;
    shellCarregando = true;
    shellErro = null;
    const alvo = sessionName;   // captura ANTES do await, mesma cautela do efeito principal
    try {
      const r = await openShell(alvo);
      if (alvo !== sessionName) return;   // sessao trocou enquanto esperava -- descarta resposta velha
      shellNome = r.shell;
    } catch (e) {
      if (alvo !== sessionName) return;
      // Mensagem do backend (409 = colisao de nome, 404/500 = tmux recusou) -- mostrada como veio,
      // nao engolida.
      shellErro = e instanceof Error ? e.message : 'falha ao abrir o shell';
      // Destrava o clique seguinte: um erro TRANSITORIO (409 na janela de corrida com um `tmux
      // new-session` do proprio usuario, 500 do tmux, rede) nao pode deixar a aba morta ate fechar
      // e reabrir o painel inteiro -- sem isto o gesto natural (clicar em "Shell" de novo) caia no
      // guard do topo desta funcao e nao fazia nada.
      shellVisitada = false;
    } finally {
      if (alvo === sessionName) shellCarregando = false;
    }
  }

  // Mesmo desenho do efeito do attach, mirando `shellNome` (o "term-<nome>" da sessao escondida) em
  // vez de `sessionName`. So roda depois que `abrirAbaShell` resolve o POST -- e o que faz a aba
  // shell ser um mount so por rodada (trocar de volta pra "attach" e voltar NAO reexecuta isto,
  // porque `abaAtiva` nao entra nas dependencias: o socket da aba inativa continua aberto).
  $effect(() => {
    const alvo = shellNome;
    void geracaoShell;
    if (!open || !alvo || !hostShell) return;
    let vivo = true;
    caiuShell = false;

    (async () => {
      const [{ Terminal }, { FitAddon }] = await Promise.all([
        import('@xterm/xterm'),
        import('@xterm/addon-fit'),
      ]);
      await import('@xterm/xterm/css/xterm.css');
      if (!vivo || !hostShell) return;

      const r = novoTerminal(hostShell, Terminal, FitAddon);
      termShell = r.term; fitShell = r.fit;

      garantirObserverDeTema();

      const enc = new TextEncoder();
      sockShell = new TermSocket(termUrl(alvo, termShell.cols, termShell.rows), {
        data: (b) => termShell.write(b),
        close: () => { if (vivo) caiuShell = true; },
      });
      termShell.onData((d: string) => sockShell?.send(enc.encode(d)));

      roShell = new ResizeObserver(() => { fitShell?.fit(); sockShell?.resize(termShell.cols, termShell.rows); });
      roShell.observe(hostShell);
      // Mesmo motivo do efeito do attach acima: a estreia desta aba so foca aqui.
      if (abaAtiva === 'shell') termShell.focus();
    })();

    return () => {
      vivo = false;
      roShell?.disconnect(); roShell = null;
      sockShell?.close(); sockShell = null;
      termShell?.dispose(); termShell = null;
      mo?.disconnect(); mo = null;
    };
  });
</script>

{#if open}
  <!-- UM mount, DUAS abas (attach + shell) e DOIS tamanhos (normal/max): mover ou remontar
       qualquer um dos dois derrubaria xterm.js e socket. Trocar de aba so alterna qual `.tp-screen`
       fica visivel -- visibility, nao display:none, pra manter a caixa (ResizeObserver/fit
       continuam medindo certo mesmo com a aba escondida). -->
  <!-- svelte-ignore a11y_no_static_element_interactions (onkeydown so pra parar propagacao; quem
       captura teclado de verdade e o textarea do proprio xterm, ja nativamente interativo) -->
  <section class="tp" class:max={maximizado} bind:this={secEl}
           onkeydown={(e) => {
             // BOLHA, nao captura: o xterm registra o keydown no proprio textarea (descendente desta
             // section) e trata a tecla PRIMEIRO na fase de captura -- se a gente parasse na captura,
             // o evento nunca chegava nele (digitar/setas/Ctrl+C morriam mudos).
             // Esc vai pro AGENTE, nao fecha o painel: e a tecla que interrompe o Claude Code na TUI,
             // e um terminal onde Esc fecha a janela em vez de chegar no Claude perde a razao de
             // existir (decisao do dono do plano). So o botao X fecha.
             // stopPropagation em QUALQUER tecla (Esc incluso) so pra nao vazar pros atalhos globais
             // do Chat (svelte:window onkeydown) enquanto o usuario digita aqui dentro.
             e.stopPropagation();
           }}>
    <header class="tp-bar">
      <div class="tp-abas" role="tablist">
        <button class="tp-aba" class:sel={abaAtiva === 'attach'} role="tab" aria-selected={abaAtiva === 'attach'}
                onclick={() => (abaAtiva = 'attach')}>{sessionName}</button>
        <button class="tp-aba" class:sel={abaAtiva === 'shell'} role="tab" aria-selected={abaAtiva === 'shell'}
                onclick={abrirAbaShell}>Shell</button>
      </div>
      {#if (abaAtiva === 'attach' ? caiu : caiuShell)}
        <button class="tp-recon" onclick={() => (abaAtiva === 'attach' ? geracao++ : geracaoShell++)}>
          desconectado · reconectar
        </button>
      {/if}
      <button onclick={abrirTerminalNativo} aria-label="Abrir terminal nativo"
              title="Abrir janela do terminal do sistema, já anexada (fecha este painel)">↗</button>
      <button onclick={toggleMax} aria-label="Maximizar">⤢</button>
      <button onclick={onClose} aria-label="Fechar">✕</button>
    </header>
    <div class="tp-screens">
      <div class="tp-screen" class:hidden={abaAtiva !== 'attach'} bind:this={host}></div>
      {#if shellNome}
        <div class="tp-screen" class:hidden={abaAtiva !== 'shell'} bind:this={hostShell}></div>
      {:else if shellErro}
        <div class="tp-screen tp-status" class:hidden={abaAtiva !== 'shell'}>
          <p class="tp-erro">Shell: {shellErro}</p>
        </div>
      {:else if shellCarregando}
        <div class="tp-screen tp-status" class:hidden={abaAtiva !== 'shell'}>
          <p class="tp-msg">abrindo shell…</p>
        </div>
      {/if}
    </div>
  </section>
{/if}

{#if nativeErro}
  <!-- FORA do `{#if open}` de proposito: abrirTerminalNativo fecha o painel ANTES do POST sair, entao
       um erro so chega depois que a `<section>` acima ja sumiu do DOM. -->
  <div class="tp-native-erro" role="alert">
    <p>{nativeErro}</p>
    <button onclick={() => { clearTimeout(nativeErroTimer); nativeErro = null; }} aria-label="Fechar aviso">✕</button>
  </div>
{/if}

<style>
  .tp { display: flex; flex-direction: column; height: 320px; border-top: 1px solid var(--border-subtle); resize: vertical; overflow: hidden; }
  /* z-index 40, nao 5: precisa cobrir o .board-overlay (DesktopShell.svelte, z-index:30) quando o
     terminal e aberto de dentro do chat-overlay do board/canvas e depois maximizado. */
  .tp.max { position: absolute; inset: 0; height: auto; z-index: 40; }
  /* transparent de proposito: quem carrega o material e o conteiner (regra de vidro do CLAUDE.md). */
  .tp-bar { display: flex; align-items: center; gap: var(--space-2); padding: var(--space-1) var(--space-2); background: transparent; }
  .tp-abas { display: flex; gap: var(--space-1); flex: 1; min-width: 0; }
  .tp-aba {
    padding: 2px var(--space-2); border-radius: var(--radius-sm); border: 1px solid transparent;
    background: transparent; color: var(--text-muted); font-size: var(--text-xs); cursor: pointer;
    max-width: 40%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .tp-aba:hover { background: var(--bg-hover); }
  .tp-aba.sel { background: var(--accent-dim); color: var(--accent); }
  /* Posicionamento relativo: as duas telas se empilham em cima uma da outra (position:absolute) e a
     visivel e escolhida por `visibility`, nao `display:none` -- display:none zeraria a caixa e o
     ResizeObserver/fit() da aba escondida mediriam 0x0 na proxima vez que ficasse visivel. */
  .tp-screens { flex: 1; min-height: 0; position: relative; }
  .tp-screen { position: absolute; inset: 0; background: var(--surface-inset); }
  .tp-screen.hidden { visibility: hidden; }
  .tp-status { display: flex; align-items: center; justify-content: center; }
  .tp-msg { margin: 0; font-size: var(--text-sm); color: var(--text-muted); }
  .tp-erro { margin: 0; padding: 0 var(--space-4); font-size: var(--text-sm); color: var(--error); text-align: center; }
  /* Flutua fora do painel (que pode estar fechado quando isto aparece) -- superficie propria porque
     nao ha painel de vidro por baixo pra herdar (regra de vidro: --surface-raised, nao --bg-elevated). */
  .tp-native-erro {
    position: fixed; right: var(--space-4); bottom: var(--space-4); z-index: 50;
    display: flex; align-items: center; gap: var(--space-3); max-width: 360px;
    padding: var(--space-3) var(--space-3); border-radius: var(--radius-md);
    border: 1px solid var(--border-subtle); background: var(--surface-raised);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.24);
  }
  .tp-native-erro p { margin: 0; font-size: var(--text-sm); color: var(--text-primary); }
  .tp-native-erro button {
    flex-shrink: 0; border: none; background: transparent; color: var(--text-muted); cursor: pointer;
  }
</style>

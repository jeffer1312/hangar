<script lang="ts">
  // Saúde dos harnesses: uma linha por CLI (Claude Code, Codex, Pi, omp, Kimi) com o que o app
  // instalou nele — hooks, extensões, ponte de skills, login espalhado, contas — e um botão por
  // item fora do lugar que roda o conserto que já existe no servidor. Existe porque cada peça
  // dessas falha calada: sessão sem estado, skill que sumiu, CLI deslogado, e a pessoa só descobre
  // no meio do trabalho.
  import {
    listarHarnesses, consertarHarness, codexIntegracaoEstado, codexIntegracaoReconciliar,
    instalacaoEstado, instalarHarness,
    type Harness, type ItemHarness, type IntegracaoCodex, type MensagemCodex, type Instalacao,
  } from '../../lib/credenciais';
  import { patchConfig, patchConfigForServer } from '@hangar/core';
  import * as m from '../../paraglide/messages';
  import { getLocale } from '../../paraglide/runtime';
  import ProvedorIcone from '../icons/ProvedorIcone.svelte';
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import type { Server } from '../../lib/auth';
  import HarnessOpcoes from './HarnessOpcoes.svelte';

  interface Props { apiTarget: Server | null }
  let { apiTarget }: Props = $props();

  let lista = $state<Harness[]>([]);
  let carregando = $state(false);
  let erro = $state('');
  let consertando = $state<string | null>(null);
  let feito = $state('');
  let opcoesClaude = $state<string | null>(null);
  let integracao = $state<IntegracaoCodex | null>(null);
  let erroIntegracao = $state('');
  let reconciliando = $state(false);
  let integracaoOcupada = $derived(reconciliando || (integracao?.estado === 'executando' && !erroIntegracao));
  interface ConsultaIntegracao {
    alvo: Server | null;
    controle: AbortController;
    timer?: ReturnType<typeof setTimeout>;
    requisicao: number;
    timerInst?: ReturnType<typeof setTimeout>;
    reqInst: number;
  }
  let consulta: ConsultaIntegracao | null = null;

  // Instalar um CLI que falta. O estado vem do servidor por polling — a instalação vive lá, então
  // fechar a tela ou recarregar o app no meio dela não perde o progresso nem a saída do comando.
  let inst = $state<Instalacao | null>(null);
  let erroInst = $state('');
  let erroInstCli = $state<string | null>(null);
  let confirmar = $state<Harness | null>(null);
  let instalando = $derived(inst?.fase === 'rodando');

  function comandoDe(h: Harness): string | null { return inst?.comandos?.[h.id] ?? null; }
  // Só http(s): a URL vem do servidor e vira `href`, e um `javascript:` ali executaria no clique.
  // Mesma regra que `lib/markdown.ts` já aplica na outra superfície que renderiza link de fora.
  function manualDe(h: Harness): string | null {
    const u = inst?.manual?.[h.id] ?? null;
    return u && /^https?:\/\//i.test(u) ? u : null;
  }

  async function consultarInstalacao(ctx: ConsultaIntegracao, cli?: string) {
    if (ctx.timerInst) clearTimeout(ctx.timerInst);
    const requisicao = ++ctx.reqInst;
    // Tentativa nova zera o erro da anterior; poll de rotina, não — ver abaixo.
    if (cli) { erroInst = ''; erroInstCli = null; }
    try {
      const estado = cli
        ? await instalarHarness(ctx.alvo, cli, ctx.controle.signal)
        : await instalacaoEstado(ctx.alvo, ctx.controle.signal);
      if (ctx.controle.signal.aborted || requisicao !== ctx.reqInst) return;
      // Só uma instalação ANDANDO apaga o erro. Zerando a cada chamada — ou a cada resposta boa —,
      // o erro que impede a instalação de COMEÇAR (409 de outra em curso, 500, teto de tempo)
      // aparecia e o próprio poll de reparo o apagava 1,2s depois: a pessoa clicava, lia um
      // instante e ficava sem nada. Erro transitório de poll continua sumindo, porque ali a
      // instalação está rodando e a resposta seguinte diz isso.
      if (estado.fase === 'rodando') { erroInst = ''; erroInstCli = null; }
      const terminou = inst?.fase === 'rodando' && estado.fase === 'pronto';
      // Normaliza o `log` na entrada: um backend mais velho (ou uma resposta de outra forma) não
      // pode derrubar a tela inteira por causa de um campo ausente — mesmo precedente do
      // `statusline.read` exigir dict antes de usar o valor (CLAUDE.md).
      inst = { ...estado, log: Array.isArray(estado?.log) ? estado.log : [] };
      if (estado.fase === 'rodando') {
        ctx.timerInst = setTimeout(() => { void consultarInstalacao(ctx); }, 1200);
      } else if (terminou) {
        // Quem diz se instalou é o disco relido, não o `rc` do comando: recarrega o card.
        void carregar();
      }
    } catch (e) {
      if (ctx.controle.signal.aborted || requisicao !== ctx.reqInst) return;
      erroInst = e instanceof Error ? e.message : String(e);
      // De quem é o erro: sem isto ele era desenhado dentro do card da instalação ANTERIOR.
      erroInstCli = cli ?? inst?.harness ?? null;
      // O trabalho vive no SERVIDOR: uma resposta perdida (blip de rede, ou o teto de 8s da
      // chamada) não pode congelar a tela em "rodando" — e congelava de vez, porque `instalando`
      // ficava `true` para sempre, o que desabilita o botão de todos os cards e faz o ↻ pular a
      // releitura. Reagenda: o estado real está lá e a próxima resposta desempata.
      // `cli` cobre o outro lado: se o POST estourar, `inst` nunca vira "rodando" e o card
      // voltaria a oferecer "Instalar" com uma instalação já correndo no servidor.
      if (inst?.fase === 'rodando' || cli) {
        ctx.timerInst = setTimeout(() => { void consultarInstalacao(ctx); }, 1200);
      }
    }
  }

  function instalar(h: Harness) {
    confirmar = null;
    if (!consulta) return;
    // Uma instalação pode ter começado noutro aparelho enquanto a confirmação estava aberta.
    // Fechar a caixa e não fazer nada é o clique que some — diz o porquê.
    if (instalando) { erroInst = m.harness_inst_ocupado(); erroInstCli = h.id; return; }
    void consultarInstalacao(consulta, h.id);
  }

  async function consultarIntegracao(ctx: ConsultaIntegracao, reconciliar = false) {
    if (ctx.timer) clearTimeout(ctx.timer);
    const requisicao = ++ctx.requisicao;
    if (reconciliar) reconciliando = true;
    erroIntegracao = '';
    try {
      const estado = await (reconciliar ? codexIntegracaoReconciliar : codexIntegracaoEstado)(ctx.alvo, ctx.controle.signal);
      if (ctx.controle.signal.aborted || requisicao !== ctx.requisicao) return;
      integracao = estado;
      if (estado.estado === 'executando') {
        ctx.timer = setTimeout(() => { void consultarIntegracao(ctx); }, 1500);
      }
    } catch (e) {
      if (!ctx.controle.signal.aborted && requisicao === ctx.requisicao) {
        erroIntegracao = e instanceof Error ? e.message : String(e);
      }
    } finally {
      if (!ctx.controle.signal.aborted && requisicao === ctx.requisicao) reconciliando = false;
    }
  }

  function reconciliarIntegracao() {
    if (consulta && !integracaoOcupada) {
      void consultarIntegracao(consulta, true);
    }
  }

  let trocandoAutomatica = $state(false);
  // O interruptor nunca muda sozinho: `checked` é o dado do servidor; o onchange repõe o dado,
  // grava, e a releitura é quem muda a tela (regra das Máquinas, CLAUDE.md).
  async function trocarAutomatica(ev: Event) {
    const alvo = ev.currentTarget as HTMLInputElement;
    const querido = alvo.checked;
    alvo.checked = !querido;
    // `consulta` é trocada pelo $effect quando o servidor muda; a gravação e a releitura são do
    // contexto que existia no clique — trocar de servidor no meio não pode reconsultar o outro.
    const ctx = consulta;
    if (!ctx || trocandoAutomatica) return;
    trocandoAutomatica = true;
    erroIntegracao = '';
    try {
      await (ctx.alvo ? patchConfigForServer(ctx.alvo, { codex_sync: querido })
                      : patchConfig({ codex_sync: querido }));
      if (consulta === ctx) await consultarIntegracao(ctx);
    } catch (e) {
      if (consulta === ctx) erroIntegracao = e instanceof Error ? e.message : String(e);
    } finally {
      trocandoAutomatica = false;
    }
  }

  function atualizar() {
    void carregar();
    if (consulta && !reconciliando) void consultarIntegracao(consulta);
    // Sem guard de "instalando": `consultarInstalacao` já limpa o timer e incrementa a geração no
    // topo, então reentrar é seguro — e o guard trancava justamente a saída manual de uma tela
    // presa em "rodando".
    if (consulta) void consultarInstalacao(consulta);
  }

  // Mensagem da integração: código do backend → frase daqui (harness_codex_m_<codigo>); código que
  // este app não conhece mostra o `texto` em pt em vez de sumir.
  function textoDe(msg: MensagemCodex | null | undefined): string {
    if (!msg) return '';
    if (typeof msg === 'string') return msg;
    const fn = msg.codigo ? (m as Record<string, unknown>)[`harness_codex_m_${msg.codigo}`] : undefined;
    return typeof fn === 'function' ? (fn as (p: Record<string, string>) => string)(msg.params ?? {}) : msg.texto;
  }

  const ESTADOS_INTEGRACAO: Record<IntegracaoCodex['estado'], () => string> = {
    ocioso: m.harness_codex_ocioso, executando: m.harness_codex_executando,
    ok: m.harness_codex_ok, parcial: m.harness_codex_parcial,
    erro: m.harness_codex_erro, indisponivel: m.harness_codex_indisponivel,
  };

  function dataIntegracao(valor: string | null): string {
    if (!valor) return m.harness_codex_nunca();
    const data = new Date(valor);
    return Number.isNaN(data.getTime()) ? valor : data.toLocaleString(getLocale());
  }

  // Alvo capturado na chamada e resposta descartada se ele mudou: trocar de servidor com a
  // requisição em voo não pode pintar a lista da máquina errada.
  let ger = 0;

  async function carregar() {
    const alvo = apiTarget;
    const g = ++ger;
    carregando = true; erro = '';
    try {
      const r = await listarHarnesses(alvo);
      if (g === ger) lista = r;
    } catch (e) { if (g === ger) erro = e instanceof Error ? e.message : String(e); }
    finally { if (g === ger) carregando = false; }
  }

  async function consertar(id: string) {
    if (consertando) return;
    const alvo = apiTarget;
    const g = ++ger;
    consertando = id; erro = ''; feito = '';
    try {
      const r = await consertarHarness(alvo, id);
      if (g !== ger) return;
      lista = r.harnesses;
      feito = r.feito;
    } catch (e) { if (g === ger) erro = e instanceof Error ? e.message : String(e); }
    finally { if (g === ger) consertando = null; }
  }

  $effect(() => {
    const ctx: ConsultaIntegracao = {
      alvo: apiTarget, controle: new AbortController(), requisicao: 0, reqInst: 0,
    };
    consulta = ctx;
    lista = []; feito = ''; consertando = null; opcoesClaude = null;
    integracao = null; erroIntegracao = ''; reconciliando = false;
    inst = null; erroInst = ''; confirmar = null;
    void carregar();
    void consultarIntegracao(ctx);
    // Também na montagem: é desta resposta que sai a lista de quem dá pra instalar por botão nesta
    // máquina, e sem ela nenhum card ausente saberia o que oferecer.
    void consultarInstalacao(ctx);
    return () => {
      // Nem uma resposta atrasada nem o próximo poll podem atravessar a troca de servidor.
      ++ger;
      ++ctx.reqInst;
      ctx.controle.abort();
      if (ctx.timer) clearTimeout(ctx.timer);
      if (ctx.timerInst) clearTimeout(ctx.timerInst);
      consulta = null;
    };
  });

  // O código vem do servidor; a frase é daqui. Código desconhecido (backend mais novo que o app)
  // aparece cru em vez de sumir — sumir esconderia justamente o item que mudou.
  const TEXTOS: Record<string, (p: Record<string, string>) => string> = {
    skills_ok: (p) => (p.origem ? m.harness_skills_origem({ n: p.n ?? '', origem: p.origem }) : m.harness_skills_ok({ n: p.n ?? '' })),
    mcp_ok: (p) => m.harness_mcp_ok({ n: p.n ?? '', lista: p.lista ?? '' }),
    mcp_nenhum: () => m.harness_mcp_nenhum(),
    modelo_padrao: (p) => m.harness_modelo_padrao({ modelo: p.modelo ?? '' }),
    modelo_padrao_nenhum: () => m.harness_modelo_padrao_nenhum(),
    hooks_nenhum: () => m.harness_hooks_nenhum(),
    hooks_codex: (p) => m.harness_hooks_codex({ n: p.n ?? '', eventos: p.eventos ?? '' }),
    tmux_bloco_ok: () => m.harness_tmux_bloco_ok(),
    tmux_bloco_ausente: () => m.harness_tmux_bloco_ausente(),
    tmux_term_ok: (p) => m.harness_tmux_term_ok({ valor: p.valor ?? '' }),
    tmux_term_ruim: (p) => m.harness_tmux_term_ruim({ valor: p.valor ?? '' }),
    tmux_truecolor_ok: () => m.harness_tmux_truecolor_ok(),
    tmux_truecolor_ruim: () => m.harness_tmux_truecolor_ruim(),
    tmux_titulo_ok: () => m.harness_tmux_titulo_ok(),
    tmux_titulo_ruim: (p) => m.harness_tmux_titulo_ruim({ valor: p.valor ?? '' }),
    tmux_mouse_on: () => m.harness_tmux_mouse_on(),
    tmux_mouse_off: () => m.harness_tmux_mouse_off(),
    tmux_persist_on: () => m.harness_tmux_persist_on(),
    tmux_persist_off: () => m.harness_tmux_persist_off(),
    sem_ponte: () => m.harness_sem_ponte(),
    ponte_ausente: () => m.harness_ponte_ausente(),
    links_pendurados: (p) => m.harness_links_pendurados({ n: p.n ?? '', total: p.total ?? '' }),
    ponte_fora_da_config: (p) => m.harness_ponte_fora_da_config({ n: p.n ?? '', cli: p.cli ?? '' }),
    config_ilegivel: () => m.harness_config_ilegivel(),
    extensoes_ok: (p) => m.harness_extensoes_ok({ n: p.n ?? '' }),
    faltam: (p) => m.harness_faltam({ lista: p.lista ?? '' }),
    extensoes_outra_fonte: (p) => (p.faltam
      ? m.harness_extensoes_outra_fonte_e_faltam({ lista: p.lista ?? '', faltam: p.faltam })
      : m.harness_extensoes_outra_fonte({ lista: p.lista ?? '' })),
    faltam_n: (p) => m.harness_faltam_n({ n: p.n ?? '' }),
    hooks_ok: (p) => m.harness_hooks_ok({ n: p.n ?? '' }),
    nenhuma_conta: () => m.harness_nenhuma_conta(),
    so_conta_padrao: () => m.harness_so_conta_padrao(),
    contas_ok: (p) => m.harness_contas_ok({ n: p.n ?? '', lista: p.lista ?? '' }),
    plugins_ok: (p) => m.harness_plugins_ok({ n: p.n ?? '', lista: p.lista ?? '' }),
    plugins_com_problema: (p) => m.harness_plugins_com_problema({ n: p.n ?? '', lista: p.lista ?? '' }),
    credenciais_ok: (p) => m.harness_credenciais_ok({ tem: p.tem ?? '' }),
    credenciais_faltam: (p) => m.harness_credenciais_faltam({ tem: p.tem ?? '', faltam: p.faltam ?? '' }),
    wrapper_ok: (p) => m.harness_wrapper_ok({ onde: p.onde ?? '' }),
    wrapper_falta: (p) => m.harness_wrapper_falta({ lista: p.lista ?? '' }),
    wrapper_sem_shell: () => m.harness_wrapper_sem_shell(),
    statusline_ok: () => m.harness_statusline_ok(),
    fullscreen_ok: () => m.harness_fullscreen_ok(),
    fullscreen_desligado: () => m.harness_fullscreen_desligado(),
    fullscreen_claude_desligado: () => m.harness_fullscreen_claude_desligado(),
    fullscreen_por_escolha: () => m.harness_fullscreen_por_escolha(),
    sem_statusline: () => m.harness_sem_statusline(),
  };
  const ROTULOS: Record<string, () => string> = {
    hooks: m.harness_item_hooks, contas: m.harness_item_contas, credenciais: m.harness_item_credenciais,
    plugins: m.harness_item_plugins, fullscreen: m.harness_item_fullscreen,
    mcp: m.harness_item_mcp, modelo: m.harness_item_modelo,
    bloco: m.harness_item_tmux_bloco, default_terminal: m.harness_item_tmux_term, truecolor: m.harness_item_tmux_truecolor,
    titulo: m.harness_item_tmux_titulo, mouse: m.harness_item_tmux_mouse, persistencia: m.harness_item_tmux_persist,
    skills: m.harness_item_skills, extensoes: m.harness_item_extensoes, statusline: m.harness_item_statusline,
    wrapper: m.harness_item_wrapper,
  };
  function texto(i: ItemHarness): string { return (TEXTOS[i.codigo] ?? (() => i.codigo))(i.params); }
  function rotulo(i: ItemHarness): string { return (ROTULOS[i.id] ?? (() => i.id))(); }

  const ETAPAS_INST: Record<string, () => string> = {
    comando: m.harness_inst_etapa_comando,
    conferir: m.harness_inst_etapa_conferir,
    wrapper: m.harness_inst_etapa_wrapper,
    ajustes: m.harness_inst_etapa_ajustes,
  };
  function etapaInst(chave: string | null): string { return (ETAPAS_INST[chave ?? ''] ?? (() => chave ?? ''))(); }

  // A caixa de saída acompanha a última linha: um `npm install` de rede fria escreve por minutos, e
  // uma caixa parada na primeira linha é indistinguível de uma travada. Mas só acompanha quem JÁ
  // está no fim — quem rolou pra cima está lendo o erro, e puxá-lo de volta a cada poll é o oposto
  // do que essa caixa existe pra fazer.
  let logEl = $state<HTMLElement | null>(null);
  $effect(() => {
    const linhas = inst?.log.length ?? 0;
    if (!logEl || !linhas) return;
    if (logEl.scrollHeight - logEl.scrollTop - logEl.clientHeight < 40) logEl.scrollTop = logEl.scrollHeight;
  });
</script>

<div class="hs">
  <div class="hs-cab">
    <p class="st-secao hs-titulo">{m.harness_titulo()}</p>
    <button type="button" class="hs-refresh" onclick={atualizar} disabled={carregando || consertando !== null}
      aria-label={m.arq_recarregar()}>{carregando ? '…' : '↻'}</button>
  </div>
  <p class="hs-leg">{m.harness_legenda()}</p>

  {#each lista as h (h.id)}
    <div class="hs-card" class:fora={!h.instalado}>
      <div class="hs-topo">
        <span class="hs-ponto" class:ok={h.instalado && h.itens.every((i) => i.ok !== false)}
              class:ruim={h.instalado && h.itens.some((i) => i.ok === false)} aria-hidden="true"></span>
        <ProvedorIcone tipo={h.id === 'claude' ? 'claude' : 'chave'}
          baseUrl={h.id === 'kimi' ? 'https://api.kimi.com' : h.id === 'codex' ? 'https://api.openai.com' : ''}
          iniciais={h.id === 'omp' ? 'ω' : h.id === 'pi' ? 'π' : h.id === 'tmux' ? '⌗' : h.nome.slice(0, 2).toUpperCase()} size={22} />
        <span class="hs-nome">{h.nome}</span>
        <span class="hs-versao">{h.instalado ? (h.versao || m.harness_instalado()) : m.harness_nao_instalado()}</span>
        {#if h.id === 'claude'}
          <button type="button" class="hs-btn hs-opcoes" aria-label={m.sessao_aria_opcoes({ n: h.nome })}
            onclick={() => { opcoesClaude = h.nome; }}>{m.sessao_opcoes()}</button>
        {/if}
      </div>
      {#each h.itens as i (i.id)}
        <div class="hs-item">
          <span class="hs-marca" class:ok={i.ok === true && !i.info} class:ruim={i.ok === false} aria-hidden="true"
            >{i.info ? '·' : i.ok === true ? '✓' : i.ok === false ? '✕' : '?'}</span>
          <span class="hs-item-txt"><b>{rotulo(i)}</b> {texto(i)}</span>
          {#if i.conserto}
            <button type="button" class="hs-btn" onclick={() => consertar(i.conserto!)}
              disabled={consertando !== null}
              >{consertando === i.conserto ? '…'
                : i.conserto.startsWith('sync:') ? m.harness_sincronizar()
                : (i.ok === false ? m.harness_consertar() : m.harness_refazer())}</button>
          {/if}
        </div>
      {/each}
      {#if !h.instalado}
        <div class="hs-item">
          <span class="hs-marca" aria-hidden="true">·</span>
          {#if comandoDe(h)}
            <span class="hs-item-txt">{m.harness_inst_disponivel()}</span>
            <button type="button" class="hs-btn" onclick={() => (confirmar = h)}
              disabled={instalando}>{m.harness_inst_botao()}</button>
          {:else}
            <span class="hs-item-txt">
              {m.harness_inst_manual()}
              {#if manualDe(h)}
                <a class="hs-link" href={manualDe(h)} target="_blank" rel="noreferrer noopener">{manualDe(h)}</a>
              {/if}
            </span>
          {/if}
        </div>
      {/if}
      <!-- Erro do card DESTE harness. Sem a marca de dono, o erro de instalar o omp era desenhado
           dentro do card do Kimi, e o rodapé que existe pro erro solto ficava suprimido. -->
      {#if erroInst && erroInstCli === h.id}
        <p class="hs-aviso erro" role="alert">{erroInst}</p>
      {/if}
      <!-- Fase que este app não conhece não mostra nada. Com `!== 'ocioso'` ela caía no `{:else}`
           e pintava uma falha que não aconteceu — sucesso virando erro é tão mentira quanto o
           contrário. -->
      {#if inst && inst.harness === h.id && (inst.fase === 'rodando' || inst.fase === 'pronto')}
        <div class="hs-inst">
          <p class="hs-aviso" role="status">
            {#if instalando}
              {m.harness_inst_andamento({ passo: inst.passo, total: inst.total, etapa: etapaInst(inst.etapa) })}
            {:else if inst.ok}
              {m.harness_inst_pronto()}
            {:else}
              {m.harness_inst_falhou({ etapa: etapaInst(inst.etapa) })}
            {/if}
          </p>
          {#if inst.erro}<p class="hs-aviso erro" role="alert">{inst.erro}</p>{/if}
          <!-- Etapa pulada não pode viver só no log: a manchete acima promete "ligado ao app". -->
          {#each inst.avisos ?? [] as aviso}<p class="hs-aviso" role="status">{aviso}</p>{/each}
          <!-- `tabindex` porque a caixa rola: conteúdo rolável sem foco é inalcançável sem mouse.
               Sem `role="log"` de propósito — faria o leitor narrar cada linha do `npm install`. -->
          {#if inst.log.length}
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <!-- O `tabindex` é o certo aqui e a regra não distingue o caso: a caixa ROLA, e
                 conteúdo rolável sem foco não se alcança pelo teclado (WCAG 2.1.1). `group` com
                 rótulo, e não `log`, porque `log` implica região viva e faria o leitor de tela
                 narrar cada linha do `npm install`. -->
            <pre class="hs-inst-log" role="group" tabindex="0" aria-label={m.harness_inst_log()}
              bind:this={logEl}>{inst.log.join('\n')}</pre>
          {/if}
        </div>
      {/if}
      {#if h.id === 'codex'}
        <div class="hs-integracao">
          <div class="hs-item hs-integracao-cab">
            <span class="hs-item-txt"><b>{m.harness_codex_integracao()}</b></span>
            <button type="button" class="hs-btn" onclick={reconciliarIntegracao}
              disabled={integracaoOcupada}
              >{integracaoOcupada ? m.harness_codex_executando() : m.harness_codex_reconciliar()}</button>
          </div>
          {#if integracao}
            <label class="hs-item hs-automatica">
              <span class="hs-item-txt">
                <b>{m.harness_codex_automatica()}</b>
                <span class="hs-ajuda">{m.harness_codex_automatica_ajuda()}</span>
              </span>
              <input type="checkbox" class="switch" checked={integracao.automatica}
                disabled={trocandoAutomatica} onchange={trocarAutomatica} />
            </label>
            <p class="hs-aviso" role="status">
              {ESTADOS_INTEGRACAO[integracao.estado]?.() ?? integracao.estado}
              {#if textoDe(integracao.etapa)} · {textoDe(integracao.etapa)}{/if}
            </p>
            <p class="hs-aviso">{m.harness_codex_ultima({ data: dataIntegracao(integracao.ultima_execucao) })}</p>
            {#if integracao.proxima_atualizacao}
              <p class="hs-aviso">{m.harness_codex_proxima({ data: dataIntegracao(integracao.proxima_atualizacao) })}</p>
            {/if}
            <p class="hs-aviso">{m.harness_codex_plugins({ n: integracao.plugins.length })}</p>
            {#if integracao.skills}
              <p class="hs-aviso">{m.harness_codex_skills({ ponte: integracao.skills.ponte, nativas: integracao.skills.nativas })}</p>
            {/if}
            {#if integracao.plugins.length}
              <ul class="hs-plugins">
                {#each integracao.plugins as plugin}
                  <li><b>{plugin.id}</b> · {plugin.versao} · {plugin.origem}</li>
                {/each}
              </ul>
            {/if}
            {#if integracao.confianca_pendente}
              <p class="hs-aviso" role="status">{m.harness_codex_confianca()}</p>
            {/if}
            {#each integracao.avisos as aviso}<p class="hs-aviso">{textoDe(aviso)}</p>{/each}
            {#each integracao.erros as falha}<p class="hs-aviso erro" role="alert">{textoDe(falha)}</p>{/each}
          {/if}
          {#if erroIntegracao}<p class="hs-aviso erro" role="alert">{erroIntegracao}</p>{/if}
        </div>
      {/if}
    </div>
  {/each}

  {#if feito}<p class="hs-aviso" role="status">{feito}</p>{/if}
  {#if erro}<p class="hs-aviso erro" role="alert">{erro}</p>{/if}
  <!-- Só o erro que não achou dono na lista (falha do poll na montagem): com seis cards, o erro
       de instalar o Kimi desenhado embaixo do tmux não se liga a nada. -->
  {#if erroInst && !lista.some((h) => h.id === erroInstCli)}
    <p class="hs-aviso erro" role="alert">{erroInst}</p>
  {/if}
</div>

{#if confirmar}
  {@const alvo = confirmar}
  <ConfirmDialog
    title={m.harness_inst_conf_titulo({ nome: alvo.nome })}
    aria={m.harness_inst_conf_titulo({ nome: alvo.nome })}
    role="dialog"
    wide
    actions={[
      { label: m.harness_inst_conf_cancelar(), onClick: () => (confirmar = null) },
      { label: m.harness_inst_botao(), kind: 'primary', onClick: () => instalar(alvo) },
    ]}
    onClose={() => (confirmar = null)}
  >
    <p class="hs-conf-txt">{m.harness_inst_conf_corpo()}</p>
    <pre class="hs-conf-cmd">{comandoDe(alvo)}</pre>
    <p class="hs-conf-txt">{m.harness_inst_conf_depois()}</p>
  </ConfirmDialog>
{/if}

{#if opcoesClaude}
  <HarnessOpcoes {apiTarget} nome={opcoesClaude} onClose={() => { opcoesClaude = null; }} />
{/if}

<style>
  .hs { container-type: inline-size; padding: var(--space-2) var(--space-3) var(--space-5); }
  .hs-cab { display: flex; align-items: center; gap: var(--space-2); margin: 0 0 var(--space-1); }
  .hs-titulo { flex: 1; margin: 0; }
  .hs-refresh { width: 28px; height: 28px; min-height: 0; min-width: 0; display: grid; place-items: center;
                border-radius: var(--radius-full); background: transparent; border: 1px solid var(--border-subtle);
                color: var(--text-secondary); }
  .hs-leg { margin: 0 0 var(--space-3); font-size: var(--text-xs); color: var(--text-muted); }
  .hs-card { padding: var(--space-2) var(--space-3); margin-bottom: var(--space-2);
             border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
             background: var(--surface-inset); }
  .hs-card.fora { opacity: 0.6; }
  .hs-topo { display: flex; align-items: center; gap: var(--space-2); }
  .hs-ponto { width: 8px; height: 8px; border-radius: 50%; background: var(--text-muted); }
  .hs-ponto.ok { background: var(--success, #3fb950); }
  .hs-ponto.ruim { background: var(--error); }
  .hs-nome { flex: 1; font-weight: 600; color: var(--text-primary); }
  .hs-versao { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); }
  .hs-item { display: flex; align-items: center; gap: var(--space-2); margin-top: 6px; font-size: var(--text-sm); }
  .hs-marca { width: 16px; text-align: center; color: var(--text-muted); font-size: var(--text-xs); }
  .hs-marca.ok { color: var(--success, #3fb950); }
  .hs-marca.ruim { color: var(--error); }
  .hs-item-txt { flex: 1; min-width: 0; color: var(--text-secondary); overflow-wrap: anywhere; }
  .hs-item-txt b { color: var(--text-primary); font-weight: 600; }
  .hs-automatica { cursor: pointer; }
  .hs-ajuda { display: block; font-size: var(--text-xs); color: var(--text-muted); }
  .hs-btn { flex-shrink: 0; min-height: 0; height: 26px; padding: 0 var(--space-2);
            font-size: var(--text-xs); border-radius: var(--radius-sm);
            background: var(--surface-raised); border: 1px solid var(--border-subtle); color: var(--text-primary); }
  .hs-opcoes { min-height: 36px; }
  @container (max-width: 400px) {
    .hs-topo { flex-wrap: wrap; }
    .hs-opcoes { min-height: 44px; }
  }
  .hs-aviso { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-secondary); }
  .hs-aviso.erro { color: var(--error); }
  .hs-link { color: var(--accent); overflow-wrap: anywhere; }
  /* `--surface-raised`, e não `--bg-elevated` cru: o card já é `--surface-inset` e as duas
     superfícies precisam acompanhar o véu do papel de parede juntas (regra do CLAUDE.md). */
  .hs-inst-log { margin: var(--space-1) 0 0; padding: var(--space-2); max-height: 190px; overflow: auto;
                 background: var(--surface-raised); border: 1px solid var(--border-subtle);
                 border-radius: var(--radius-sm); font-family: var(--font-mono);
                 font-size: var(--text-xs); color: var(--text-secondary);
                 white-space: pre-wrap; overflow-wrap: anywhere; }
  .hs-inst { margin-top: var(--space-1); }
  /* Escopado: o corpo do ConfirmDialog entra por snippet, compilado no escopo DESTE arquivo. */
  .hs-conf-txt { margin: 0; font-size: var(--text-sm); color: var(--text-secondary); }
  /* Selecionável de propósito: ninguém aprova instalar software sem poder copiar e conferir o
     comando exato que vai rodar. */
  .hs-conf-cmd { margin: 0; padding: var(--space-2); user-select: text;
                 background: var(--surface-raised); border: 1px solid var(--border-subtle);
                 border-radius: var(--radius-sm); font-family: var(--font-mono);
                 font-size: var(--text-xs); color: var(--text-primary);
                 white-space: pre-wrap; overflow-wrap: anywhere; }
  .hs-integracao { border-top: 1px solid var(--border-subtle); margin-top: var(--space-2); padding-top: var(--space-1); }
  .hs-integracao-cab { flex-wrap: wrap; }
  .hs-integracao .hs-aviso, .hs-plugins { overflow-wrap: anywhere; }
  .hs-plugins { margin: var(--space-1) 0 0; padding-left: var(--space-4); font-size: var(--text-xs); color: var(--text-secondary); }
</style>

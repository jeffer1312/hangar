<script lang="ts">
  import * as m from '../../paraglide/messages';
  import { tick } from 'svelte';
  import { getActiveId, listServers, type Server } from '../../lib/auth';
  import { parseConfig } from '../../lib/configRoute';
  import { isTimeoutError } from '../../lib/api';
  import { alcanceDoServidor, fraseDeEstado, pareamentoDoServidor, type EnderecoAlcance, type EstadoEndereco, type TipoEndereco } from '../../lib/alcance';
  import { copyText } from '../../lib/clipboard';

  // O alvo da config espelha a resolução do App (App.svelte): ?srv= explícito, senão o
  // ATIVO. O alvo também pode vir por PROP (o SettingsModal passa o resolvedServer): com o
  // seletor de servidor no painel (19/08/2026) a troca acontece SEM remontar a tela, então a
  // prop é o caminho reativo — e a resolução pela rota fica de fallback pra quem monta direto
  // (os testes).
  interface Props {
    alvo?: Server | null;
  }
  let { alvo = undefined }: Props = $props();

  function servidorAlvo(): Server | null {
    const r = parseConfig(location.hash);
    const porSrv = r?.srv ? listServers().find((s) => s.id === r.srv) ?? null : null;
    return porSrv ?? listServers().find((s) => s.id === getActiveId()) ?? null;
  }

  function servidorAtual(): Server | null {
    return alvo !== undefined ? alvo : servidorAlvo();
  }

  let carregando = $state(true);
  let erro = $state('');
  let loopback = $state(false);
  let bind = $state('');
  let enderecos = $state<EnderecoAlcance[]>([]);
  // Sinal de que a lista JÁ foi medida (resolveu ou falhou). É o que o bloco de
  // pareamento espera para poder concluir: enquanto em voo, `enderecos` é [] e
  // "lista vazia" e "lista ainda não chegou" seriam o mesmo valor (rodada 2).
  let listaMedida = $state(false);

  $effect(() => {
    const s = servidorAtual();
    if (!s) return;
    let vivo = true;
    carregando = true;
    listaMedida = false;
    // Troca de alvo com a tela MONTADA (seletor do grupo, 19/08): tudo que foi medido ou
    // revelado pro servidor anterior sai ANTES da nova medição — endereços e QR de outra
    // máquina com o seletor dizendo a nova seriam mentira (achado da revisão).
    enderecos = []; loopback = false; bind = ''; erro = '';
    parEstado = 'escondido'; parUrl = ''; parQr = ''; parErro = ''; parTipo = '';
    parGeracao++;   // mata pareamento em voo do alvo anterior
    alcanceDoServidor(s)
      .then((r) => {
        if (!vivo) return;
        loopback = r.loopback;
        bind = r.bind;
        enderecos = r.enderecos;
        erro = '';
        listaMedida = true;
      })
      .catch((e) => {
        if (!vivo) return;
        // Estado NOMEADO de falha; o detalhe é dado do servidor/rede (não vira chave).
        erro = `${m.falha_conexao()}: ${e instanceof Error ? e.message : m.erro_desconhecido()}`;
        listaMedida = true;
      })
      .finally(() => {
        if (vivo) carregando = false;
      });
    return () => {
      vivo = false;
    };
  });

  // Linhas que TODO servidor tem (rede local e público) pintam como "Testando…" enquanto a
  // resposta não chega — é o estado nomeado `testando`; as condicionais (nesta máquina,
  // Tailscale) nascem quando o backend responde.
  const LINHAS_EM_VOO: EnderecoAlcance[] = [
    { tipo: 'rede_local', url: '', estado: 'testando', tempo_ms: null },
    { tipo: 'publico', url: '', estado: 'testando', tempo_ms: null },
  ];

  // Farol e texto têm cores próprias por estado (mock estados 1 e 3): "não configurado"
  // NÃO usa a cor de erro — não estar configurado não é defeito, é neutro.
  const farolPorEstado: Record<EstadoEndereco, string> = {
    ok: 'ok', falhou: 'nao', testando: 'testando', nao_configurado: 'testando',
  };
  const glifoPorEstado: Record<EstadoEndereco, string> = {
    ok: '●', falhou: '●', testando: '◌', nao_configurado: '○',
  };
  const textoPorEstado: Record<EstadoEndereco, string> = {
    ok: 'ok', falhou: 'nao', testando: 'neutro', nao_configurado: 'neutro',
  };

  function nomeDoTipo(t: EnderecoAlcance['tipo']): string {
    switch (t) {
      case 'nesta_maquina': return m.acesso_nesta_maquina();
      case 'rede_local': return m.acesso_rede_local();
      case 'tailscale': return m.acesso_tailscale();
      case 'publico': return m.acesso_publico();
    }
  }

  // ── Pareamento (Task 6) ────────────────────────────────────────────────────────
  // Estado nomeado: `escondido` antes do toque; `carregando` enquanto a rota responde;
  // `revelado` com o par (url + qr_svg); `erro` quando a rota recusa; `sem_candidato`
  // quando nenhum endereço respondeu (aí não há o que revelar — bloqueador 2).
  let parEstado = $state<'escondido' | 'carregando' | 'revelado' | 'erro' | 'sem_candidato'>('escondido');
  let parUrl = $state('');
  let parQr = $state('');
  let parErro = $state('');
  // Vazio quando não há candidato: sem candidato não se chama a rota (bloqueador 2).
  let parTipo = $state<TipoEndereco | ''>('');
  let parGeracao = 0;

  // Endereços que podem ser EMBUTIDOS no QR: só os que responderam (estado ok) e não
  // são "nesta máquina" (o mock nunca mostra o QR apontando para 127.0.0.1 — de fora
  // ele não alcança; e o endereço de pareamento substitui o loopback pelo IP da LAN).
  let parCandidatos = $derived(
    enderecos.filter((e) => e.estado === 'ok' && e.tipo !== 'nesta_maquina'),
  );
  // Padrão: o candidato de MENOR tempo medido (empate → ordem da lista), não o primeiro
  // da ordem do backend — a frase "respondeu mais rápido" descreve a escolha AUTOMÁTICA,
  // e ela tem de ser verdade (bloqueador 3).
  let parPadrao = $derived<TipoEndereco | ''>(
    [...parCandidatos].sort((a, b) => (a.tempo_ms ?? 0) - (b.tempo_ms ?? 0))[0]?.tipo ?? '',
  );
  $effect(() => {
    const escolhidoValido = parCandidatos.some((e) => e.tipo === parTipo);
    if (!escolhidoValido && parCandidatos.length > 0) {
      // Troca o padrão (mais rápido) ou, na falta de escolha manual, o primeiro.
      const novo = parTipo === '' ? parPadrao : parCandidatos[0]!.tipo;
      parTipo = novo;
      // Se o QR já estava revelado, recarrega com o novo endereço — senão o seletor
      // diria um endereço e o QR mostraria outro (mesma família do bloqueador 3).
      if (parEstado === 'revelado') revelarPar();
    }
    // Sem candidato: estado nomeado, sem botão (bloqueador 2). Mas só depois de
    // MEDIR: enquanto a lista está em voo, `enderecos` é [] e "lista vazia" e
    // "lista ainda não chegou" seriam o mesmo valor — concluir agora é afirmar que
    // nada respondeu antes de testar (bloqueador da rodada 2). E lista que FALHOU
    // também não mediu endereço nenhum: quem não respondeu foi o servidor, não há
    // endereço a liberar (bloqueador da rodada 3).
    if (listaMedida && !erro) {
      parEstado = parCandidatos.length === 0 ? 'sem_candidato' : parEstado === 'sem_candidato' ? 'escondido' : parEstado;
    }
  });

  // Falha de transporte (teto estourado, fetch caído) não tem texto traduzível — mostra
  // só a frase da casa. Erro DA API já chega traduzido pelo errorDetail e pode aparecer
  // inteiro (bloqueador 4: "signal timed out" cru em inglês numa tela em português).
  function frasePorFalha(e: unknown): string {
    const cru = isTimeoutError(e) || e instanceof TypeError || !(e instanceof Error);
    return cru ? m.falha_conexao() : `${m.falha_conexao()}: ${e.message}`;
  }

  // Referências para o foco (bloqueador 5): revelar leva o foco ao seletor; esconder
  // devolve ao botão que abriu. Depois de `await tick()`, porque o nó de destino ainda
  // não existe no DOM no momento da transição.
  let parRef = $state<HTMLElement | null>(null);
  let mostrarRef = $state<HTMLButtonElement | null>(null);

  async function revelarPar() {
    const s = servidorAtual();
    if (!s || parTipo === '') return; // sem candidato não se chama a rota (bloqueador 2)
    parEstado = 'carregando';
    const geracao = ++parGeracao;
    try {
      const r = await pareamentoDoServidor(s, parTipo);
      if (geracao !== parGeracao) return; // uma troca de endereço veio no meio
      parUrl = r.url;
      parQr = r.qr_svg;
      parErro = '';
      parEstado = 'revelado';
      await tick();
      parRef?.focus(); // foco no primeiro controle do bloco revelado (bloqueador 5)
    } catch (e) {
      if (geracao !== parGeracao) return;
      // Estado NOMEADO de falha; o detalhe é dado do servidor/rede (não vira chave).
      parErro = frasePorFalha(e);
      parEstado = 'erro';
    }
  }

  async function trocarParTipo(tipo: TipoEndereco) {
    if (tipo === parTipo && parEstado === 'revelado') return;
    parTipo = tipo;
    if (parEstado === 'revelado' || parEstado === 'erro') {
      await revelarPar();
    }
  }

  async function esconderPar() {
    parEstado = 'escondido';
    parUrl = '';
    parQr = '';
    parErro = '';
    await tick();
    mostrarRef?.focus(); // devolve o foco ao botão que abriu (bloqueador 5)
  }

  // Copiar só faz sentido num endereço que RESPONDEU (o mock nunca mostra o botão
  // nas linhas falhou/testando/não-configurado, nem nesta máquina): endereço que
  // falhou não é pra copiar — é pra consertar.
  function mostraCopiar(e: EnderecoAlcance): boolean {
    return e.estado === 'ok' && e.tipo !== 'nesta_maquina';
  }
</script>

{#snippet linha(e: EnderecoAlcance)}
  <li class="ac-linha">
    <span class="ac-farol {farolPorEstado[e.estado]}" aria-hidden="true">{glifoPorEstado[e.estado]}</span>
    <span class="ac-txt">
      <span class="ac-nome">{nomeDoTipo(e.tipo)}</span>
      <span class="ac-url">{e.estado === 'nao_configurado' ? m.acesso_nao_configurado() : e.url}</span>
      <span class="ac-estado {textoPorEstado[e.estado]}">{fraseDeEstado(e)}</span>
    </span>
    {#if mostraCopiar(e)}
      <button class="ac-copiar" onclick={() => copyText(e.url)}>{m.acesso_copiar()}</button>
    {/if}
  </li>
{/snippet}

{#snippet blocoPar()}
  {#if parEstado === 'sem_candidato'}
    <!-- Sem nenhum endereço que respondeu: estado NOMEADO, sem botão — não há o que
         revelar (bloqueador 2). A lista acima é onde se conserta. -->
    <div class="ac-oculto">
      <p>{m.acesso_par_sem_candidato()}</p>
    </div>
  {:else if parEstado === 'escondido'}
    <div class="ac-oculto">
      <p>{m.acesso_oculto_aviso()}</p>
      <button class="ac-btn primaria" bind:this={mostrarRef} onclick={() => revelarPar()} disabled={parTipo === ''}>{m.acesso_mostrar_codigo()}</button>
    </div>
  {:else}
    <div class="ac-par">
      {#if parEstado !== 'erro'}
        <!-- No erro o quadrado branco do QR não é desenhado (bloqueador 4: retângulo
             opaco de 176×176 com "Testando…" para sempre, sem saída). -->
        <div class="ac-qr" aria-hidden="true">
          {#if parQr}
            <!-- O QR é SVG pronto do backend (decisão de plano: o front só tem qr-scanner, que lê e não gera). -->
            {@html parQr}
          {:else}
            <span class="ac-qr-vazio">{m.acesso_testando()}</span>
          {/if}
        </div>
      {/if}
      <div class="ac-par-col">
        <div>
          <p class="ac-cod-rot">{m.acesso_codigo_rotulo()}</p>
          <div class="ac-cod">{parUrl}</div>
        </div>
        {#if parEstado === 'carregando'}
          <p class="ac-par-copy">{m.acesso_testando()}</p>
        {:else if parEstado === 'erro'}
          <p class="ac-par-copy aviso-erro" role="alert">{parErro}</p>
        {:else}
          <div class="ac-par-escolha">
            <label class="ac-par-label">
              <span class="ac-cod-rot">{m.acesso_selecionar_endereco()}</span>
              <select
                class="ac-select"
                bind:this={parRef}
                value={parTipo}
                onchange={(e) => trocarParTipo((e.currentTarget as HTMLSelectElement).value as TipoEndereco)}
              >
                {#each parCandidatos as c (c.tipo)}
                  <option value={c.tipo}>{nomeDoTipo(c.tipo)}</option>
                {/each}
              </select>
            </label>
            <!-- Fora do <label>: o nome acessível do seletor volta a ser só "Endereço
                 no QR", sem a frase de aviso inteira (bloqueador 5). -->
            <span class="ac-par-aviso">{m.acesso_par_trocar_aviso()}</span>
          </div>
          {#if parTipo !== '' && parTipo === parPadrao}
            <!-- Só quando a escolha é a AUTOMÁTICA (mais rápido): com escolha manual,
                 a frase "respondeu mais rápido" mentiria (bloqueador 3). -->
            <p class="ac-par-copy">{m.acesso_par_escolhido({ rede: nomeDoTipo(parTipo) })}</p>
          {/if}
        {/if}
        <!-- Esconder nos TRÊS estados revelados (carregando/erro/revelado): o erro não
             pode ser beco sem saída (bloqueador 4). Copiar endereço só no revelado. -->
        <div class="ac-acoes">
          {#if parEstado === 'revelado'}
            <button class="ac-btn" onclick={() => copyText(parUrl)}>{m.acesso_copiar_endereco()}</button>
          {/if}
          <button class="ac-btn" onclick={() => esconderPar()}>{m.acesso_esconder()}</button>
        </div>
      </div>
    </div>
  {/if}
{/snippet}

<div class="ac">
  {#if loopback}
    <div class="ac-alerta">
      <span class="ac-farol nao" aria-hidden="true">▲</span>
      <span class="ac-alerta-txt">
        <b>{m.acesso_alerta_loopback_1({ endereco: bind })}</b><br>
        {m.acesso_alerta_loopback_2({ variavel: 'CP_LAN_BIND_IP', valor: 'auto' })}
      </span>
    </div>
  {/if}

  <p class="ac-secao">{m.acesso_secao_enderecos()}</p>
  <p class="ac-legenda">{m.acesso_legenda_enderecos()}</p>

  <ul class="ac-cartao">
    {#if carregando}
      {#each LINHAS_EM_VOO as e (e.tipo)}
        {@render linha(e)}
      {/each}
    {:else if erro}
      <li class="ac-linha aviso-erro" role="alert">{erro}</li>
    {:else}
      {#each enderecos as e (e.tipo)}
        {@render linha(e)}
      {/each}
    {/if}
  </ul>

  {#if !carregando && !erro && bind}
    <p class="ac-legenda">{m.acesso_escuta_em({ ip: bind })}</p>
  {/if}

  <hr class="ac-sep">

  <p class="ac-secao">{m.acesso_parear_titulo()}</p>
  <p class="ac-legenda">{m.acesso_legenda_qr()}</p>

  {@render blocoPar()}
</div>

<style>
  /* Envelope da tela inteira (precedente: ContasSettings .ct-superficie): é ELE que
     é contêiner de consulta — sem isto o @container (max-width:620px) lá embaixo
     não tinha ancestral container acima do bloco de pareamento (o .ac-cartao era o
     único container do arquivo, e o pareamento é irmão dele). */
  .ac {
    container-type: inline-size;
  }
  .ac-secao {
    margin: 0 0 var(--space-1) var(--space-2);
    color: var(--text-muted);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .ac-legenda {
    margin: 0 var(--space-2) var(--space-3);
    color: var(--text-muted);
    font-size: var(--text-xs);
    line-height: 1.4;
  }
  .ac-cartao {
    margin: 0;
    padding: 0;
    list-style: none;
    background: var(--surface-card);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .ac-linha {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    padding: var(--space-3);
    min-height: 52px;
  }
  .ac-linha + .ac-linha {
    border-top: 1px solid var(--border-subtle);
  }
  .ac-linha.aviso-erro {
    background: transparent;
    color: var(--error);
    font-size: var(--text-sm);
  }
  .ac-farol {
    flex-shrink: 0;
    width: 1.4em;
    text-align: center;
    font-size: 15px;
    line-height: 1.5;
  }
  .ac-farol.ok { color: var(--success); }
  .ac-farol.nao { color: var(--error); }
  .ac-farol.testando { color: var(--text-muted); }
  .ac-txt {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .ac-nome { color: var(--text-primary); font-size: var(--text-sm); }
  .ac-url {
    color: var(--text-secondary);
    font-size: var(--text-xs);
    font-family: var(--font-mono);
    word-break: break-all;
  }
  .ac-estado { font-size: var(--text-xs); line-height: 1.35; }
  .ac-estado.ok { color: var(--success); }
  .ac-estado.nao { color: var(--warning); }
  .ac-estado.neutro { color: var(--text-muted); }
  .ac-copiar {
    flex-shrink: 0;
    align-self: center;
    height: 30px;
    min-height: 0;
    padding: 0 var(--space-3);
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-subtle);
    background: var(--surface-raised);
    color: var(--text-secondary);
    font-size: var(--text-xs);
    font-family: inherit;
  }

  /* Separador entre a lista de endereços e o pareamento (mock estado 1) */
  .ac-sep {
    height: 1px;
    background: var(--border-subtle);
    margin: var(--space-4) 0 var(--space-3);
    border: 0;
  }

  /* Pareamento — QR + código lado a lado (mock estado 2) */
  .ac-par {
    display: grid;
    grid-template-columns: 176px 1fr;
    gap: var(--space-4);
    align-items: start;
    padding: var(--space-4);
    background: var(--surface-card);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }
  .ac-qr {
    width: 176px;
    height: 176px;
    padding: 10px;
    border-radius: var(--radius-sm);
    background: #fff;
    box-sizing: border-box;
  }
  .ac-qr :global(svg) {
    width: 100%;
    height: 100%;
    display: block;
  }
  .ac-qr-vazio {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: var(--text-xs);
  }
  .ac-par-col {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    min-width: 0;
  }
  .ac-cod-rot {
    color: var(--text-muted);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin: 0 0 var(--space-1);
  }
  .ac-cod {
    font-family: var(--font-mono);
    font-size: var(--text-lg);
    letter-spacing: 0.08em;
    color: var(--text-primary);
    background: var(--surface-inset);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    padding: var(--space-2) var(--space-3);
    user-select: all;
    word-break: break-all;
  }
  .ac-par-copy {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
    line-height: 1.45;
  }
  .ac-par-copy.aviso-erro {
    color: var(--error);
  }
  .ac-par-escolha {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .ac-par-label {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }
  .ac-select {
    align-self: flex-start;
    max-width: 100%;
    height: 36px;
    min-height: 0;
    padding: 0 var(--space-3);
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-subtle);
    background: var(--surface-raised);
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-family: inherit;
  }
  .ac-par-aviso {
    font-size: var(--text-xs);
    color: var(--text-muted);
    line-height: 1.45;
  }
  .ac-acoes {
    display: flex;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .ac-btn {
    height: 36px;
    min-height: 0;
    padding: 0 var(--space-4);
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-subtle);
    background: var(--surface-raised);
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-family: inherit;
  }
  .ac-btn.primaria {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }
  .ac-btn:disabled {
    opacity: 0.55;
    cursor: default;
  }

  /* Estado escondido: o QR atrás de um toque (mock estado 1) */
  .ac-oculto {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-6) var(--space-4);
    background: var(--surface-card);
    border: 1px dashed var(--border-default);
    border-radius: var(--radius-md);
    text-align: center;
  }
  .ac-oculto p {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
    max-width: 40ch;
    line-height: 1.45;
  }
  .ac-alerta {
    display: flex;
    gap: var(--space-2);
    align-items: flex-start;
    margin: 0 0 var(--space-3);
    padding: var(--space-3);
    background: var(--surface-card);
    border: 1px solid var(--border-default);
    border-left: 3px solid var(--warning);
    border-radius: var(--radius-md);
  }
  .ac-alerta-txt {
    font-size: var(--text-xs);
    color: var(--text-secondary);
    line-height: 1.45;
  }
  .ac-alerta-txt b { color: var(--text-primary); font-weight: 600; }

  /* Alvo de toque no celular: o botao declara min-height:0, que anula a regra global
     `button { min-height: 44px }` do app.css:546 — sem isto ele fica 30px na folha
     estreita. Mesmo padrao da aba Contas (ContasSettings.svelte). */
  @container (max-width: 620px) {
    .ac-copiar { height: 44px; min-height: 44px; }
    /* Pareamento: no celular o QR empilha (mock não desenha, mas a régua de alvo de
       toque vale — botão de 36px fica abaixo de 44px na folha estreita). */
    .ac-par { grid-template-columns: 1fr; }
    .ac-qr { justify-self: center; }
    .ac-select, .ac-btn { height: 44px; min-height: 44px; }
  }
</style>
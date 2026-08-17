<script lang="ts">
  import * as m from '../../paraglide/messages';
  import { getActiveId, listServers, type Server } from '../../lib/auth';
  import { parseConfig } from '../../lib/configRoute';
  import { alcanceDoServidor, fraseDeEstado, pareamentoDoServidor, type EnderecoAlcance, type EstadoEndereco, type TipoEndereco } from '../../lib/alcance';
  import { copyText } from '../../lib/clipboard';

  // O alvo da config espelha a resolução do App (App.svelte): ?srv= explícito, senão o
  // ATIVO. AcessoSettings nasceu sem prop (a Task 1 registrou a aba com <AcessoSettings />)
  // e o SettingsModal é intocável — a tela resolve o próprio servidor pela rota.
  function servidorAlvo(): Server | null {
    const r = parseConfig(location.hash);
    const porSrv = r?.srv ? listServers().find((s) => s.id === r.srv) ?? null : null;
    return porSrv ?? listServers().find((s) => s.id === getActiveId()) ?? null;
  }

  let carregando = $state(true);
  let erro = $state('');
  let loopback = $state(false);
  let bind = $state('');
  let enderecos = $state<EnderecoAlcance[]>([]);

  $effect(() => {
    const s = servidorAlvo();
    if (!s) return;
    let vivo = true;
    carregando = true;
    alcanceDoServidor(s)
      .then((r) => {
        if (!vivo) return;
        loopback = r.loopback;
        bind = r.bind;
        enderecos = r.enderecos;
        erro = '';
      })
      .catch((e) => {
        if (!vivo) return;
        // Estado NOMEADO de falha; o detalhe é dado do servidor/rede (não vira chave).
        erro = `${m.falha_conexao()}: ${e instanceof Error ? e.message : m.erro_desconhecido()}`;
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
  // `revelado` com o par (url + qr_svg); `erro` quando a rota recusa. Trocar o
  // endereço escolhido recarrega o QR — a escolha vem da lista testada lá em cima.
  let parEstado = $state<'escondido' | 'carregando' | 'revelado' | 'erro'>('escondido');
  let parUrl = $state('');
  let parQr = $state('');
  let parErro = $state('');
  let parTipo = $state<TipoEndereco>('rede_local');
  let parGeracao = 0;

  // Endereços que podem ser EMBUTIDOS no QR: só os que responderam (estado ok) e não
  // são "nesta máquina" (o mock nunca mostra o QR apontando para 127.0.0.1 — de fora
  // ele não alcança; e o endereço de pareamento substitui o loopback pelo IP da LAN).
  let parCandidatos = $derived(
    enderecos.filter((e) => e.estado === 'ok' && e.tipo !== 'nesta_maquina'),
  );
  // O padrão é o PRIMEIRO candidato que respondeu, não "rede_local" fixo — numa
  // máquina onde a rede local falhou e só o Tailscale respondeu, o QR já nasce
  // apontando para o endereço que funciona.
  $effect(() => {
    const escolhidoValido = parCandidatos.some((e) => e.tipo === parTipo);
    if (!escolhidoValido) {
      parTipo = parCandidatos[0]?.tipo ?? 'rede_local';
    }
  });

  async function revelarPar() {
    const s = servidorAlvo();
    if (!s) return;
    const tipo = parTipo || 'rede_local';
    parEstado = 'carregando';
    const geracao = ++parGeracao;
    try {
      const r = await pareamentoDoServidor(s, tipo);
      if (geracao !== parGeracao) return; // uma troca de endereço veio no meio
      parUrl = r.url;
      parQr = r.qr_svg;
      parErro = '';
      parEstado = 'revelado';
    } catch (e) {
      if (geracao !== parGeracao) return;
      // Estado NOMEADO de falha; o detalhe é dado do servidor/rede (não vira chave).
      parErro = `${m.falha_conexao()}: ${e instanceof Error ? e.message : m.erro_desconhecido()}`;
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

  function esconderPar() {
    parEstado = 'escondido';
    parUrl = '';
    parQr = '';
    parErro = '';
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
  {#if parEstado === 'escondido'}
    <div class="ac-oculto">
      <p>{m.acesso_oculto_aviso()}</p>
      <button class="ac-btn primaria" onclick={() => revelarPar()}>{m.acesso_mostrar_codigo()}</button>
    </div>
  {:else}
    <div class="ac-par">
      <div class="ac-qr" aria-hidden="true">
        {#if parQr}
          <!-- O QR é SVG pronto do backend (decisão de plano: o front só tem qr-scanner, que lê e não gera). -->
          {@html parQr}
        {:else}
          <span class="ac-qr-vazio">{m.acesso_testando()}</span>
        {/if}
      </div>
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
          <label class="ac-par-escolha">
            <span class="ac-cod-rot">{m.acesso_selecionar_endereco()}</span>
            <select
              class="ac-select"
              value={parTipo}
              onchange={(e) => trocarParTipo((e.currentTarget as HTMLSelectElement).value as TipoEndereco)}
            >
              {#each parCandidatos as c (c.tipo)}
                <option value={c.tipo}>{nomeDoTipo(c.tipo)}</option>
              {/each}
            </select>
            <span class="ac-par-aviso">{m.acesso_par_trocar_aviso()}</span>
          </label>
          <p class="ac-par-copy">{m.acesso_par_escolhido({ rede: nomeDoTipo(parTipo) })}</p>
          <div class="ac-acoes">
            <button class="ac-btn" onclick={() => copyText(parUrl)}>{m.acesso_copiar_endereco()}</button>
            <button class="ac-btn" onclick={() => esconderPar()}>{m.acesso_esconder()}</button>
          </div>
        {/if}
      </div>
    </div>
  {/if}
{/snippet}

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

<hr class="ac-sep">

<p class="ac-secao">{m.acesso_parear_titulo()}</p>
<p class="ac-legenda">{m.acesso_legenda_qr()}</p>

{@render blocoPar()}


<style>
  .ac-secao {
    margin: 0 0 var(--space-1);
    color: var(--text-muted);
    font-size: var(--text-xs);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .ac-legenda {
    margin: 0 0 var(--space-3);
    color: var(--text-muted);
    font-size: var(--text-xs);
    line-height: 1.4;
  }
  .ac-cartao {
    container-type: inline-size;
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
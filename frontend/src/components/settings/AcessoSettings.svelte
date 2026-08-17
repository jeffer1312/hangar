<script lang="ts">
  import * as m from '../../paraglide/messages';
  import { getActiveId, listServers, type Server } from '../../lib/auth';
  import { parseConfig } from '../../lib/configRoute';
  import { alcanceDoServidor, fraseDeEstado, type EnderecoAlcance, type EstadoEndereco } from '../../lib/alcance';
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
</style>
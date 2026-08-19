<script lang="ts">
  // Faixa de cota do rodapé do desktop. Uma coluna por CONTA (credencial), janelas 5h/7d, cor só
  // acima de 80% (sempre junto do número), leitura velha esmaecida com a idade ao lado. Leva à
  // aba Contas por callback — o DesktopShell desvia pra rota (?config=contas), nunca importando
  // o componente da aba.
  //
  // Fonte: /api/cotas (lib/contaEstado.listarCotas), que pergunta ao PROVEDOR com a credencial de
  // cada conta. Antes daqui a faixa parseava a linha de statusline guardada na pasta da conta —
  // e como essa pasta é um symlink pra conta padrão nesta máquina, as três contas mostravam o
  // MESMO número; conta sem sessão aberta não mostrava nada. Cota é da credencial, não da sessão.
  //
  // O backend já guarda a leitura por 5 min, então o poll daqui é barato: dentro do TTL ele nem
  // toca a rede. A sessão que está rodando AGORA não depende deste ciclo pra parecer viva — a
  // statusline dela continua desenhando o número dentro do chat.
  import { onMount } from 'svelte';
  import * as m from '../paraglide/messages';
  import { listarCotas, formatarIntervalo, type CotaConta } from '../lib/contaEstado';
  import { faixaDeCota, faltaPara, diaDoReset, janelaLonga, motivoParado, motivoSessaoViva } from '../lib/cota';

  interface Props {
    // Muda quando a sessão/servidor alvo muda no shell — re-busca (o endpoint é do servidor
    // ATIVO; sem re-busca a faixa mostraria as contas do servidor antigo para sempre).
    serverKey: string;
    // Leva à aba Contas. Quem constrói é o DesktopShell: () => abrirConfig('contas', getActiveId()).
    onIrParaContas: () => void;
  }
  let { serverKey, onIrParaContas }: Props = $props();

  let contas = $state<CotaConta[]>([]);
  // Relógio local para a idade e a contagem até o reset andarem sem bater na rede.
  let agora = $state(Date.now() / 1000);

  const contasComIdade = $derived(
    contas.map((c) => (c.ts == null ? c : { ...c, idade_s: Math.max(0, agora - c.ts) })),
  );
  const linha = $derived(faixaDeCota(contasComIdade));

  // Geração descarta resposta em voo de servidor anterior (mesmo papel do `vivo` do shell).
  let geracao = 0;
  async function carregar() {
    const g = ++geracao;
    try {
      // `null` explícito, decisão escrita: a faixa quer mesmo o servidor ATIVO (o componente
      // navega com getActiveId() e o endpoint é da máquina da sessão).
      const lista = await listarCotas(null);
      if (g !== geracao) return;
      contas = lista;
    } catch {
      // Falha de rede não apaga leitura boa: o que já está na tela envelhece sozinho (o `agora`
      // sobe e a conta vira `velha` com a idade ao lado). Zerar aqui fazia a faixa inteira
      // desaparecer num 500 de um segundo.
    }
  }

  $effect(() => {
    void serverKey;
    void carregar();
  });

  onMount(() => {
    const t = setInterval(() => {
      agora = Date.now() / 1000;
      void carregar();
    }, 60_000);
    return () => clearInterval(t);
  });
</script>

{#if linha}
  <div class="quota-faixa">
    <div class="quota-trilho">
      {#each linha as c (c.id)}
        <span class="quota-conta" class:velha={c.velha} class:base={c.ativa}>
          <span class="quota-nome">{c.label}</span>
          {#if c.janelas.length === 0}
            <!-- Conta conhecida sem número: nomeada e vazia, nunca como zero. Quando o motivo é a
                 credencial (vencida / nunca entrou) a faixa diz o que fazer; quando é falha de
                 leitura, só o traço — inventar explicação numa tira de 26px é pior que o traço. -->
            <span class="quota-vazio"
              >{c.estado === 'expirada' || c.estado === 'sem_credencial'
                ? (motivoSessaoViva(c.motivo) ? m.cota_sessao_viva()
                  : motivoParado(c.motivo) ? m.cota_conta_parada() : m.cota_precisa_entrar())
                : '—'}</span>
          {:else}
            {#each c.janelas as j (j.rotulo)}
              <span class="quota-v {j.nivel}">
                <span class="quota-rot">{j.rotulo}</span><b>{Math.round(j.pct)}%</b>
              </span>
              <!-- O reset acompanha TODA janela, não só a que aperta: "quando volta" é metade da
                   informação de uma cota. O formato muda com a escala — janela longa (7d) mostra o
                   DIA ("↺sáb 18h"), porque "↺4d6h" ninguém converte de cabeça; janela curta mostra
                   quanto falta ("↺2h10"), que é como se pensa em horas. -->
              {@const reset = janelaLonga(j.resetTs, agora)
                ? diaDoReset(j.resetTs, agora)
                : faltaPara(j.resetTs, agora)}
              {#if reset}<span class="quota-reset">↺{reset}</span>{/if}
            {/each}
          {/if}
          {#if c.velha && c.idade_s != null}
            <span class="quota-idade">{m.cota_idade({ n: formatarIntervalo(c.idade_s) })}</span>
          {/if}
        </span>
      {/each}
    </div>
    <span class="quota-fim">
      <button type="button" class="quota-link" onclick={onIrParaContas}>{m.contas_titulo()}</button>
    </span>
  </div>
{/if}

<style>
  /* Irmã de .shell-linha: flex-shrink:0, altura de uma linha, 28px + borda. Superfície
     --glass-panel, o mesmo material da sidebar — acompanha o slider de Transparência sozinha. */
  .quota-faixa {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    height: 28px;
    padding: 0 var(--space-3);
    background: var(--glass-panel);
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
  }
  /* Trilho rolável, mesmo desenho da .tabs-strip do SessionTabs: em janela estreita as contas
     saem de vista por rolagem, em vez de encolherem até o nome desaparecer. */
  /* Aparência → Painéis. A faixa é irmã da sidebar e do painel de contexto, então segue a MESMA
     regra deles, em vez de ser sempre uma parede de ponta a ponta:
       "Soltos" (padrão) -> card flutuante, com margem, canto e sombra iguais aos outros dois;
       "Colados"         -> parede, encostada nas bordas, com o risco de separação em cima.
     Antes ela era parede nos dois modos: com os painéis flutuando, sobrava uma barra chapada
     colada no fundo da janela que não pertencia a nada. */
  :global(html:not([data-panels='edge'])) .quota-faixa {
    margin: 0 var(--space-3) var(--space-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-xl);
    box-shadow: var(--elev-3);
  }
  /* Colados: encosta nas bordas e NÃO desenha risco nenhum contra os painéis — com a mesma
     superfície dos dois, a linha era a única coisa dizendo que ali havia duas peças. Sem ela, a
     faixa lê como continuação do painel que está em cima dela, que é o que se quer (pedido do
     usuário, 18/08).
     Trade-off aceito, não descuido: a faixa atravessa a janela, e sob o TRECHO do meio está a
     coluna do chat, que é outra superfície — ali o risco separava algo de verdade. Borda de caixa
     não varia por trecho (ou some nos dois lados, ou fica nos dois), e o que se perde foi medido:
     `--border-subtle` é alpha 0.07 entre `#100e11` e `rgb(26 24 29 / .86)`, cores quase iguais. O
     precedente do painel de contexto (que zera a borda e devolve só a `border-left`) não serve
     aqui: ele é vertical e encosta no chat por UM lado inteiro, não por um pedaço. */
  :global(html[data-panels='edge']) .quota-faixa {
    border: none;
  }
  /* MESMO material dos painéis, não um próprio. Medido em 18/08: sidebar e painel de contexto
     resolviam pra `rgb(38 36 44 / .46)` (o `--glass-bg`, porque o liquid está ligado) enquanto esta
     faixa ficava em `rgb(26 24 29 / .86)` — mais escura e mais opaca, e era só isso que fazia ela
     parecer de outro app. O par de regras é o mesmo da Sidebar: `--glass-panel` como rede de
     segurança e `--glass-bg` quando o Chromium mantém o backdrop-filter. */
  :global(html[data-liquid][data-theme='dark']) .quota-faixa {
    background: var(--glass-bg);
  }

  .quota-trilho {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    /* Espaço ENTRE contas: o dobro do espaço de dentro da pílula (--space-2 abaixo). Eram os dois
       7px, e com o mesmo respiro dentro e fora as cinco contas liam como um bloco só — a pílula
       desenhava a separação e o espaçamento a desmanchava. */
    gap: var(--space-4);
    overflow-x: auto;
    scrollbar-width: none;
  }
  .quota-trilho::-webkit-scrollbar { display: none; }
  /* Uma pílula por conta. A barrinha de progresso saiu de propósito (18/08): em 2% ela não
     desenhava nada e o número já dizia tudo — oito barras cinzas eram ruído, não informação.
     Quem separa as contas agora é a pílula, não um risquinho de 1px.
     --surface-raised (não --bg-elevated cru) pra caixa entrar no véu do papel de parede. */
  .quota-conta {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    flex-shrink: 0;
    padding: 2px var(--space-2);
    border-radius: 999px;
    background: var(--surface-raised);
  }
  .quota-nome {
    color: var(--text-primary);
    font-weight: 500;
    max-width: 18ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* A conta-base do app (a que uma sessão nova nasce usando) ganha um ponto: é a resposta pra
     "qual eu vou gastar se abrir agora". Ponto e não negrito — todos os nomes já são fortes. */
  .quota-conta.base .quota-nome::before {
    content: '';
    display: inline-block;
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--accent);
    margin-right: 6px;
    vertical-align: middle;
  }
  /* O par janela+número: o rótulo ("5h") é o mais apagado da linha, o número é claro. Essa é a
     hierarquia que faltava — antes rótulo, barra e número tinham o mesmo peso. */
  .quota-v { color: var(--text-secondary); font-variant-numeric: tabular-nums; }
  .quota-v b { font-weight: 600; }
  .quota-rot { color: var(--text-muted); font-size: 10px; margin-right: 2px; }
  .quota-v.alerta, .quota-v.alerta b { color: var(--warning); }
  .quota-v.cheio, .quota-v.cheio b { color: var(--error); }
  /* Quanto falta pro reset só aparece na janela que já está no vermelho/amarelo: é ali que a
     pergunta "quando volta?" existe. Nas outras seria ruído numa tira de 28px. */
  .quota-reset { color: var(--text-muted); font-size: 10px; font-variant-numeric: tabular-nums; }
  .quota-vazio { color: var(--text-muted); }
  /* Dado velho parece velho SEM apagar o texto: a cor de alerta cai pro tom neutro e a pista
     textual é a idade ao lado, que fica em contraste cheio. opacity no texto não serve: nem
     0,90 chega aos 4,5:1 neste fundo. */
  .quota-conta.velha .quota-v,
  .quota-conta.velha .quota-v b { color: var(--text-muted); }
  .quota-idade { color: var(--text-muted); }
  .quota-fim { flex-shrink: 0; margin-left: var(--space-3); display: flex; align-items: center; gap: var(--space-2); }
  .quota-link {
    min-height: 20px;
    min-width: 0;
    color: var(--text-muted);
    border: none;
    border-bottom: 1px dotted var(--border-strong);
    background: transparent;
    padding: 0;
    font: inherit;
    cursor: pointer;
  }
  .quota-link:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; border-radius: 3px; }
  @media (hover: hover) { .quota-link:hover { color: var(--text-secondary); } }
</style>

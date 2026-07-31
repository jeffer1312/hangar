<script lang="ts">
  import BottomSheet from '../BottomSheet.svelte';
  import SettingsRow from './SettingsRow.svelte';
  import AppearanceSettings from './AppearanceSettings.svelte';
  import ServerSettings from './ServerSettings.svelte';
  import EnginesSettings from './EnginesSettings.svelte';
  import { criarConfigServidor } from '../../lib/serverConfig.svelte';
  import type { TelaConfig } from '../../lib/configRoute';
  import type { Server } from '../../lib/auth';

  interface Props {
    tela: TelaConfig;
    alvo: Server | null;
    nomeAlvo: string | null;
    semServidor: boolean;
    onIrPara: (t: TelaConfig) => void;
    onVoltar: () => void;
    onFechar: () => void;
  }
  let { tela, alvo, nomeAlvo, semServidor, onIrPara, onVoltar, onFechar }: Props = $props();

  const store = criarConfigServidor(() => alvo);

  // Depender do ID, nao do objeto: `listServers()` faz JSON.parse a cada chamada (auth.ts), entao
  // `alvo` e um objeto NOVO a cada recomputo — um efeito que dependesse dele recarregaria (e
  // zeraria o rascunho) a cada troca de tela. O `?? '-'` cobre o caso sem alvo (servidor ativo).
  $effect(() => {
    alvo?.id ?? '-';
    store.carregar();
  });

  const TITULO: Record<TelaConfig, string> = {
    root: 'Configurações',
    aparencia: 'Aparência',
    notificacoes: 'Notificações',
    anexos: 'Anexos e transcrição',
    avancado: 'Avançado do servidor',
    motores: 'Motores de modelo',
  };

  const LINHAS = [
    { id: 'aparencia', secao: 'App', rotulo: 'Aparência', icone: '🎨',
      descricao: 'tema, fundo, leitura e texto', servidor: false },
    { id: 'notificacoes', secao: 'Servidor', rotulo: 'Notificações', icone: '🔔',
      descricao: 'quando avisar que terminou, caiu ou travou', servidor: true },
    { id: 'anexos', secao: 'Servidor', rotulo: 'Anexos e transcrição', icone: '📎',
      descricao: 'chave da Groq e por quanto tempo guardar', servidor: true },
    { id: 'avancado', secao: 'Servidor', rotulo: 'Avançado do servidor', icone: '🛠️',
      descricao: 'automações, editor e o que só muda pelo .env', servidor: true },
    { id: 'motores', secao: 'Servidor', rotulo: 'Motores de modelo', icone: '🔌',
      descricao: 'rodar uma sessão em outro provedor', servidor: true },
  ] as const satisfies readonly { id: TelaConfig; secao: string; rotulo: string; icone: string; descricao: string; servidor: boolean }[];
  const SECOES = ['App', 'Servidor'] as const;

  let tituloEl = $state<HTMLElement | null>(null);

  // Trocar de rota destroi a linha que tinha o foco e o activeElement cai no <body>: leitor de tela
  // fica mudo e o Tab recomeca do zero. Mover o foco pro titulo (que muda a cada tela) anuncia a
  // troca e da um ponto de partida.
  $effect(() => { tela; tituloEl?.focus(); });

  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });

  // No desktop a raiz (a lista de linhas) so existe no celular: as duas colunas ja mostram a
  // navegacao inteira, entao 'root' cai na Aparencia. Resolvido NO RENDER, nunca por navegacao —
  // um redirect disparado no resize seria um push a mais no historico, que a rota ja acerta sozinha.
  const telaAtual = $derived(isDesktop && tela === 'root' ? ('aparencia' as TelaConfig) : tela);

  // VOLTAR e FECHAR sao coisas diferentes, e o BottomSheet so conhece "fechar": ele chama onClose no
  // Esc, no toque no backdrop E no swipe pra baixo. Passar `voltar` como onClose faria o swipe — o
  // gesto mais usado no celular, que e a view deste plano — subir um nivel em vez de sair, e de
  // dentro de uma sub-tela a folha ficaria impossivel de fechar. Entao o BottomSheet recebe o
  // onFechar de verdade, e voltar e SO o botao do cabecalho.
  function botaoEsquerdo() {
    if (tela === 'root') onFechar();
    else onVoltar();
  }
</script>

<BottomSheet open={true} onClose={onFechar} ariaLabel={TITULO[telaAtual]}
             wide={isDesktop} centered={isDesktop} split={isDesktop}>
  {#if isDesktop}
    <div class="st-split">
      <aside class="st-nav">
        {#each SECOES as secao (secao)}
          <p class="st-secao">{secao === 'Servidor' && nomeAlvo ? `Servidor · ${nomeAlvo}` : secao}</p>
          {#each LINHAS.filter((l) => l.secao === secao) as l (l.id)}
            <button class="st-nav-item" class:sel={telaAtual === l.id}
                    aria-current={telaAtual === l.id ? 'page' : undefined}
                    disabled={l.servidor && semServidor}
                    onclick={() => onIrPara(l.id)}>
              <span class="st-nav-icone" aria-hidden="true">{l.icone}</span>{l.rotulo}
            </button>
          {/each}
        {/each}
      </aside>
      <section class="st-conteudo">
        {@render corpo()}
      </section>
      <!-- O ✕ fica FORA da coluna que rola: dentro dela, `position: absolute` rolaria junto com o
           conteudo (e `sticky` num container com padding brigaria com o topo do formulario). -->
      <button class="st-fechar" onclick={onFechar} aria-label="Fechar">✕</button>
    </div>
  {:else}
    <header class="st-head">
      <button class="st-icone" onclick={botaoEsquerdo}
        aria-label={tela === 'root' ? 'Fechar' : 'Voltar'}>{tela === 'root' ? '✕' : '‹'}</button>
      <!-- tabindex=-1: alvo do foco na troca de tela, sem entrar na ordem do Tab. -->
      <h2 class="st-titulo" bind:this={tituloEl} tabindex="-1">{TITULO[tela]}</h2>
      <span class="st-icone st-vazio" aria-hidden="true"></span>
    </header>
    {@render corpo()}
  {/if}
</BottomSheet>

{#snippet corpo()}
  {#if telaAtual === 'root'}
    {#each SECOES as secao (secao)}
      <p class="st-secao">{secao}</p>
      <div class="st-cartao">
        {#each LINHAS.filter((l) => l.secao === secao) as l (l.id)}
          <SettingsRow icone={l.icone} rotulo={l.rotulo} descricao={l.descricao}
            desabilitada={l.servidor && semServidor}
            motivo="escolha um servidor na lista pra configurar"
            onPick={() => onIrPara(l.id)} />
        {/each}
      </div>
    {/each}
  {:else if telaAtual === 'aparencia'}
    <AppearanceSettings />
  {:else if telaAtual === 'motores'}
    <EnginesSettings targetServer={alvo} />
  {:else}
    <ServerSettings {store} secao={telaAtual} />
  {/if}
{/snippet}

<style>
  .st-head {
    display: grid; grid-template-columns: 32px 1fr 32px; align-items: center;
    gap: var(--space-2); margin-bottom: var(--space-4);
  }
  .st-titulo {
    margin: 0; text-align: center;
    font-size: var(--text-base); font-weight: 600; color: var(--text-primary);
  }
  .st-titulo:focus { outline: none; }   /* alvo programatico: o anel aqui so confundiria */
  .st-icone {
    width: 32px; height: 32px; border-radius: var(--radius-full);
    border: 1px solid var(--border-subtle); background: var(--surface-raised);
    color: var(--text-secondary); font-size: var(--text-base); line-height: 1; cursor: pointer;
  }
  .st-vazio { border: 0; background: transparent; }   /* espelha o botao pro titulo ficar no centro */
  .st-secao {
    margin: var(--space-4) 0 var(--space-1) var(--space-2);
    color: var(--text-muted); font-size: var(--text-xs);
    text-transform: uppercase; letter-spacing: 0.05em;
  }
  /* Cartao arredondado com as linhas dentro, no formato do iOS. `--surface-card` entra no veu do
     papel de parede junto com o resto (CLAUDE.md, "Transparencia"). */
  .st-cartao {
    background: var(--surface-card);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .st-cartao > :global(button + button) { border-top: 1px solid var(--border-subtle); }

  /* ⚠ `grid-template-rows: minmax(0, 1fr)` NAO e enfeite: sem ele a linha implicita e `auto`, a
     trilha cresce ate a altura do formulario, o overflow-y da coluna nunca dispara e o
     `overflow: hidden` do .sheet.split CORTA o fim do conteudo — inclusive o botao Salvar. */
  .st-split {
    display: grid;
    grid-template-columns: 232px 1fr;
    grid-template-rows: minmax(0, 1fr);
    height: 100%;
    position: relative;
  }
  .st-nav {
    overflow-y: auto; min-height: 0;
    padding: var(--space-3) var(--space-2);
    border-right: 1px solid var(--border-subtle);
  }
  .st-conteudo { overflow-y: auto; min-height: 0; padding: var(--space-4); }
  .st-fechar {
    position: absolute; top: var(--space-3); right: var(--space-3); z-index: 1;
    width: 32px; height: 32px; border-radius: var(--radius-full);
    border: 1px solid var(--border-subtle); background: var(--surface-raised);
    color: var(--text-secondary); font-size: var(--text-base); line-height: 1; cursor: pointer;
  }
  .st-nav-item {
    display: flex; align-items: center; justify-content: flex-start; gap: var(--space-2);
    width: 100%; padding: var(--space-2) var(--space-3); border: 0; border-radius: var(--radius-md);
    background: transparent; color: var(--text-primary); font-size: var(--text-sm); text-align: left;
    cursor: pointer;
  }
  @media (hover: hover) { .st-nav-item:not(:disabled):hover { background: var(--bg-hover); } }
  .st-nav-item.sel { background: var(--bg-elevated); }   /* realce de estado: --bg-* cru e o certo */
  .st-nav-item:disabled { color: var(--text-muted); cursor: default; }
  .st-nav-icone { flex-shrink: 0; width: 1.4em; text-align: center; }
</style>

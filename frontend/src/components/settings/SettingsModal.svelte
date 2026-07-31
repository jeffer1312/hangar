<script lang="ts">
  import BottomSheet from '../BottomSheet.svelte';
  import SettingsRow from './SettingsRow.svelte';
  import AppearanceSettings from './AppearanceSettings.svelte';
  import ServerSettings from './ServerSettings.svelte';
  import EnginesSettings from './EnginesSettings.svelte';
  import type { Server } from '../../lib/auth';

  interface Props {
    open: boolean;
    onClose: () => void;
    targetServer?: Server | null;
    /** Sem servidor alvo (celular, servidor removido): as linhas DE SERVIDOR ficam sem entrada. */
    semServidor?: boolean;
  }
  let { open, onClose, targetServer = null, semServidor = false }: Props = $props();

  type Rota = 'root' | 'aparencia' | 'servidor' | 'motores';

  // Rota por ID, nunca por indice: mexer na lista nao pode abrir outra tela debaixo do usuario.
  // PAI de cada tela — "voltar" sobe UM nivel. Sem este mapa, sair de Motores (onde se chega DE
  // Servidor) jogava direto na raiz, dois niveis pra tras.
  const PAI: Record<Exclude<Rota, 'root'>, Rota> = {
    aparencia: 'root',
    servidor: 'root',
    motores: 'servidor',
  };
  const TITULO: Record<Rota, string> = {
    root: 'Configurações',
    aparencia: 'Aparência',
    servidor: 'Configurações do servidor',
    motores: 'Motores de modelo',
  };

  const LINHAS = [
    { id: 'aparencia', secao: 'App', rotulo: 'Aparência', icone: '🎨',
      descricao: 'tema, fundo, leitura e texto', servidor: false },
    { id: 'servidor', secao: 'Servidor', rotulo: 'Configurações do servidor', icone: '⚙️',
      descricao: 'chaves, retenção e automações', servidor: true },
    { id: 'motores', secao: 'Servidor', rotulo: 'Motores de modelo', icone: '🔌',
      descricao: 'rodar uma sessão em outro provedor', servidor: true },
  ] as const;
  const SECOES = ['App', 'Servidor'] as const;

  let rota = $state<Rota>('root');
  let tituloEl = $state<HTMLElement | null>(null);

  // Reabrir cai na RAIZ. Guardar a ultima sub-tela parece esperto e nao e: quem abre "Configuracoes"
  // espera o indice, nao a tela onde parou semana passada.
  $effect(() => { if (open) rota = 'root'; });

  // Trocar de rota destroi a linha que tinha o foco e o activeElement cai no <body>: leitor de tela
  // fica mudo e o Tab recomeca do zero. Mover o foco pro titulo (que muda a cada tela) anuncia a
  // troca e da um ponto de partida.
  $effect(() => { rota; tituloEl?.focus(); });

  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });

  // VOLTAR e FECHAR sao coisas diferentes, e o BottomSheet so conhece "fechar": ele chama onClose no
  // Esc, no toque no backdrop E no swipe pra baixo. Passar `voltar` como onClose faria o swipe — o
  // gesto mais usado no celular, que e a view deste plano — subir um nivel em vez de sair, e de
  // dentro de uma sub-tela a folha ficaria impossivel de fechar. Entao o BottomSheet recebe o
  // onClose de verdade, e voltar e SO o botao do cabecalho.
  function botaoEsquerdo() {
    if (rota === 'root') onClose();
    else rota = PAI[rota];
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={TITULO[rota]} wide={isDesktop} centered={isDesktop}>
  <header class="st-head">
    <button class="st-icone" onclick={botaoEsquerdo}
      aria-label={rota === 'root' ? 'Fechar' : 'Voltar'}>{rota === 'root' ? '✕' : '‹'}</button>
    <!-- tabindex=-1: alvo do foco na troca de tela, sem entrar na ordem do Tab. -->
    <h2 class="st-titulo" bind:this={tituloEl} tabindex="-1">{TITULO[rota]}</h2>
    <span class="st-icone st-vazio" aria-hidden="true"></span>
  </header>

  {#if rota === 'root'}
    {#each SECOES as secao (secao)}
      <p class="st-secao">{secao}</p>
      <div class="st-cartao">
        {#each LINHAS.filter((l) => l.secao === secao) as l (l.id)}
          <SettingsRow icone={l.icone} rotulo={l.rotulo} descricao={l.descricao}
            desabilitada={l.servidor && semServidor}
            motivo="escolha um servidor na lista pra configurar"
            onPick={() => (rota = l.id)} />
        {/each}
      </div>
    {/each}
  {:else if rota === 'aparencia'}
    <AppearanceSettings />
  {:else if rota === 'servidor'}
    <ServerSettings {targetServer} onOpenMotores={() => (rota = 'motores')} />
  {:else}
    <EnginesSettings {targetServer} />
  {/if}
</BottomSheet>

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
</style>

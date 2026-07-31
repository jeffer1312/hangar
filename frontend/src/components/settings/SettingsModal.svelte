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

<BottomSheet open={true} onClose={onFechar} ariaLabel={TITULO[tela]} wide={isDesktop} centered={isDesktop}>
  <header class="st-head">
    <button class="st-icone" onclick={botaoEsquerdo}
      aria-label={tela === 'root' ? 'Fechar' : 'Voltar'}>{tela === 'root' ? '✕' : '‹'}</button>
    <!-- tabindex=-1: alvo do foco na troca de tela, sem entrar na ordem do Tab. -->
    <h2 class="st-titulo" bind:this={tituloEl} tabindex="-1">{TITULO[tela]}</h2>
    <span class="st-icone st-vazio" aria-hidden="true"></span>
  </header>

  {#if tela === 'root'}
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
  {:else if tela === 'aparencia'}
    <AppearanceSettings />
  {:else if tela === 'motores'}
    <EnginesSettings targetServer={alvo} />
  {:else}
    <ServerSettings {store} secao={tela} />
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

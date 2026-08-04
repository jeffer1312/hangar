<script lang="ts">
  import ModalDialog from './ModalDialog.svelte';
  import Chat from '../screens/Chat.svelte';

  // Sessão do PAR aberta num modal, inteira: o mesmo `Chat` do resto do app (transcript + composer),
  // não um resumo. Mesma ideia do overlay do quadro/canvas (DesktopShell.svelte:206), só que por cima
  // do chat atual em vez da view — dá pra ler e RESPONDER o par sem sair da própria sessão.
  interface Props {
    // null = fechado. O nome também é a identidade do modal: trocar de membro remonta o Chat.
    name: string | null;
    desktop?: boolean;
    onClose: () => void;
    onNavigateToChat?: (name: string) => void;   // "abrir de vez" -> fecha o modal e navega
  }
  let { name, desktop = false, onClose, onNavigateToChat }: Props = $props();
</script>

{#if name}
  <!-- {#key}: o Chat guarda SSE e histórico amarrados à sessão e precisa REMONTAR quando o membro
       muda — mesma razão do {#key} do overlay em DesktopShell.svelte:212. Como o modal é um só
       (`name` é string|null, nunca lista), no máximo 2 SSE ficam abertos: o chat de trás e este. -->
  {#key name}
    <ModalDialog open={true} ariaLabel={`Sessão ${name}`} className="pair-chat-dialog" onClose={onClose}>
      <div class="pcm">
        <button class="pcm-close" onclick={onClose} aria-label="Fechar" title="Fechar (Esc)">×</button>
        <Chat
          sessionName={name}
          {desktop}
          nested={true}
          onBack={onClose}
          onNavigateToChat={(n) => { onClose(); onNavigateToChat?.(n); }}
        />
      </div>
    </ModalDialog>
  {/key}
{/if}

<style>
  /* O painel do ModalDialog é estreito e rolável por padrão (modal-dialog: 560px + overflow:auto);
     um chat precisa do contrário — largo, alto e SEM rolagem própria (quem rola é a lista dentro).
     :global porque a classe cai no elemento do ModalDialog, fora do escopo deste componente. */
  :global(.pair-chat-dialog) {
    width: min(1040px, 100%);
    height: min(860px, 100%);
    max-height: calc(100dvh - var(--space-8));
    overflow: hidden;
  }
  /* No celular o modal ocupa a tela: um chat espremido em 560px não é usável. */
  @media (max-width: 819px) {
    :global(.pair-chat-dialog) { width: 100%; height: 100%; }
  }

  .pcm { position: relative; height: 100%; overflow: hidden; border-radius: inherit; }
  /* O Chat nasce com `height: 100vh` (Chat.svelte:1143) porque normalmente ocupa a tela inteira.
     Dentro do modal ele sobra: medido em 1440×900, o modal tem 860px e os 40px a mais cortavam a
     linha de baixo do composer (pill do modelo/enviar). Aqui ele vale a altura do modal — a regra
     leva as duas classes do dialog porque a de lá nasce com a classe de escopo do Svelte
     (`.chat-screen.svelte-x`) e empata com um seletor de duas. */
  :global(.modal-dialog.pair-chat-dialog .chat-screen) { height: 100%; }
  /* Mesmo botão do overlay do quadro (DesktopShell .split-close). Fica ACIMA do chat porque o
     header do Chat em desktop não tem "voltar" — sem ele o Esc seria a única saída. */
  .pcm-close {
    /* Janela menor que ~1072px: o modal ocupa a largura toda e este × encosta na borda direita,
       onde ficam os controles da janela no PWA. Soma --cp-wco-right (0 fora do PWA). */
    position: absolute; top: 8px; right: calc(10px + var(--cp-wco-right)); z-index: 20;
    width: 28px; height: 28px;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    background: var(--bg-elevated); color: var(--text-secondary);
    font-size: 16px; line-height: 1; cursor: pointer;
  }
  .pcm-close:hover { color: var(--text-primary); background: var(--bg-hover); }
  /* No celular o Chat tem o "‹" próprio (que aqui fecha o modal) e o × caía POR CIMA do anel de
     uso — medido em 414px. Some: já existe saída, e sobra o Esc/backdrop. */
  @media (max-width: 819px) { .pcm-close { display: none; } }
</style>

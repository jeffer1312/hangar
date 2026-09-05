<script lang="ts">
import { intlLocale } from '../lib/locale';
import { renderMarkdown } from '../lib/markdown';
import { parseCanal } from '@hangar/core';
import * as m from '../paraglide/messages';
  interface Props {
    text: string;
    ts?: number | null;
    animate?: boolean;   // false = bubble de HISTORICO remontada (paginacao/janela): sem fade
    from?: string | null;          // recado de OUTRA sessao (hangar-send): nome da sessao remetente
    scope?: 'peer' | 'group' | 'panel' | null; // 'group' = aviso pro grupo ([grupo: X]); 'panel' = recado automático do app ([painel: X])
    onForward?: (() => void) | null; // abre o picker "encaminhar pra sessao" (botao ↗)
    onOpenPeer?: (() => void) | null; // tap no chip "de: X" -> abre o chat da sessao remetente
  }
  let { text, ts, animate = true, from = null, scope = 'peer', onForward = null, onOpenPeer = null }: Props = $props();

  // Recado de OUTRA sessão é escrito por um agente: vem em markdown, e sem renderizar o usuário lê
  // "**Você não precisa fazer nada.**" com os asteriscos à mostra (regra do app: markdown nunca
  // aparece cru). O que VOCÊ digitou continua texto puro — ali um `*` é um asterisco mesmo.
  const canal = $derived(from ? parseCanal(text) : null);
  const corpo = $derived(canal ? canal.text : text);
  const html = $derived(from ? renderMarkdown(corpo) : '');

  function formatTime(ts: number | null | undefined): string {
    if (!ts) return '';
    return new Date(ts * 1000).toLocaleTimeString(intlLocale(), {
      hour: '2-digit',
      minute: '2-digit',
    });
  }
</script>

<!-- ponytail: sem long-press / oncontextmenu aqui de proposito — roubavam a selecao de texto do
     iOS e o menu nativo do desktop. Encaminhar = botao ↗ na linha do horario. -->
<div class="bubble-wrap" class:noanim={!animate}>
  <div
    class="bubble"
    class:peer={!!from}
    class:group={scope === 'group'}
    class:panel={scope === 'panel'}
  >
    {#if from}
      {@const label = scope === 'group' ? m.board_peer_grupo({ n: from }) : scope === 'panel' ? m.board_peer_painel({ n: from }) : m.board_peer_de({ n: from })}
      <div class="peer-head">
        {#if onOpenPeer && scope !== 'panel'}
          <button class="peer-chip peer-chip--link" onclick={onOpenPeer}
                  aria-label={m.user_abrir_chat_de({ n: from })} title={m.user_abrir_chat_de({ n: from })}>{label} ›</button>
        {:else}
          <span class="peer-chip">{label}</span>
        {/if}
        {#if canal}<span class="canal">{canal.canal}</span>{/if}
      </div>
    {/if}
    {#if from}
      <!-- Sem superfície de XSS: `renderMarkdown` escapa tudo antes de montar o HTML. -->
      <div class="bubble-text md">{@html html}</div>
    {:else}
      <p class="bubble-text">{text}</p>
    {/if}
  </div>
  {#if ts || onForward}
    <div class="msg-actions">
      {#if ts}
        <span class="ts">{formatTime(ts)}</span>
      {/if}
      {#if onForward}
        <button class="msg-fwd" onclick={onForward} aria-label={m.forward_para_outra()} title={m.forward_para_outra()}></button>
      {/if}
    </div>
  {/if}
</div>

<style>
  /* Mesma margem esquerda da resposta do Claude: quem separa uma da outra e o BALAO, nao o lado da
     tela — igual ao terminal, onde o prompt do usuario e a resposta comecam na mesma coluna. */
  .bubble-wrap {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    animation: bubble-in 220ms var(--ease-out) both;
    margin-bottom: var(--space-3);
  }

  /* Historico remontado (paginacao/janela): entra parado. */
  .bubble-wrap.noanim { animation: none; }

  .bubble {
    position: relative;
    background: var(--bubble-user);
    color: var(--text-primary);
    /* 80% do container, com teto de leitura: na coluna larga do desktop (ate 1400px) 80% viraria
       um balao de ~1100px com linhas ilegiveis. */
    max-width: min(80%, 46rem);
    padding: var(--space-3) var(--space-4);
    /* Rabinho no canto inferior ESQUERDO agora que o balao mora na esquerda. */
    border-radius: 18px 18px 18px 4px;
    word-break: break-word;
  }

  /* Recado de OUTRA sessao Claude (hangar-send): borda/fundo accent pra nao passar por msg do usuario. */
  .bubble.peer {
    background: var(--accent-dim);
    border: 1px solid var(--accent);
  }
  /* Aviso pro GRUPO ([grupo: X]): âmbar, pra distinguir do recado 1:1 (accent). */
  .bubble.group {
    background: rgba(255, 159, 10, 0.12);
    border-color: var(--warning, #ff9f0a);
  }
  .bubble.group .peer-chip { color: var(--warning, #ff9f0a); }
  /* Recado AUTOMÁTICO do app ([painel: X]): neutro, pra ler como configuração e não como pessoa. */
  .bubble.panel {
    background: color-mix(in srgb, var(--text-muted) 12%, transparent);
    border-color: var(--text-muted);
  }
  .bubble.panel .peer-chip { color: var(--text-muted); }

  .peer-head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: var(--space-1);
  }

  .peer-chip {
    display: block;
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--accent);
  }

  /* Canal do recado ("[vigia] ..."): etiqueta ao lado do remetente, e o corpo começa na frase. */
  .canal {
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    padding: 1px 7px;
    border-radius: var(--radius-full);
    background: var(--fill-subtle);
    color: var(--text-secondary);
  }

  /* Chip clicável (navega pro chat do remetente): mesmo visual, affordance no hover/active. */
  .peer-chip--link {
    background: none; border: none; padding: 0; text-align: left; cursor: pointer;
    -webkit-tap-highlight-color: transparent;
  }
  .peer-chip--link:hover { text-decoration: underline; }
  .peer-chip--link:active { opacity: 0.7; }

  .bubble-text {
    /* Acompanha o texto do assistente (Aparencia -> Texto da conversa): a conversa e uma coisa so,
       e so um dos lados mudar de tamanho fica pior que nao ter o ajuste. */
    font-size: calc(var(--text-base) * var(--cp-text-scale, 1));
    line-height: calc(1.55 * var(--cp-lh-scale, 1));
    white-space: pre-wrap;
  }

  /* Markdown do recado. Menos do que o `.prose` da resposta do agente de propósito: aqui não cabe
     tabela nem bloco de código com cabeçalho — recado é frase, ênfase e nome de sessão em `code`.
     `pre-wrap` sai: quem quebra a linha agora são os <p>. */
  .bubble-text.md { white-space: normal; }
  .bubble-text.md :global(p) { margin: 0; }
  .bubble-text.md :global(p + p) { margin-top: var(--space-1); }
  .bubble-text.md :global(p.para) { margin-top: var(--space-3); }
  .bubble-text.md :global(strong) { font-weight: 600; }
  .bubble-text.md :global(em) { font-style: italic; }
  .bubble-text.md :global(code) {
    font-family: var(--font-mono);
    font-size: 0.875em;
    background: rgb(255 255 255 / 0.08);
    padding: 2px 5px;
    border-radius: 4px;
    box-decoration-break: clone;
    -webkit-box-decoration-break: clone;
  }
  .bubble-text.md :global(pre) {
    background: rgb(0 0 0 / 0.22);
    border-radius: var(--radius-sm);
    padding: var(--space-2);
    margin: var(--space-2) 0;
    overflow-x: auto;
    font-size: 0.875em;
  }
  .bubble-text.md :global(ul) { list-style: disc; margin: var(--space-2) 0; padding-left: 1.4em; }
  .bubble-text.md :global(ol) { list-style: decimal; margin: var(--space-2) 0; padding-left: 1.5em; }
  .bubble-text.md :global(li) { margin: 2px 0; }
  .bubble-text.md :global(a) { color: var(--accent); text-decoration: underline; }
  .bubble.group .bubble-text.md :global(a) { color: var(--warning, #ff9f0a); }

  /* Encaminhar na linha do horário (abaixo do balão): no toque fica SEMPRE visível — é a única
     forma de encaminhar agora que o long-press saiu. No desktop, só no hover. */
  .msg-actions {
    display: flex; align-items: center; gap: var(--space-1);
    margin-top: var(--space-1);
  }
  .msg-fwd {
    width: 28px; height: 28px; padding: 0;
    display: flex; align-items: center; justify-content: center;
    border: none; border-radius: var(--radius-sm);
    background: transparent; color: var(--text-muted);
    opacity: 0.5; transition: opacity 120ms var(--ease-out), background 120ms var(--ease-out);
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
  }
  .msg-fwd::before { content: '↗'; font-size: 14px; line-height: 1; }
  @media (hover: hover) and (pointer: fine) {
    .msg-fwd { opacity: 0; }
    .bubble-wrap:hover .msg-fwd { opacity: 0.55; }
    .msg-fwd:hover { opacity: 1 !important; background: var(--bg-hover); color: var(--text-primary); }
  }

  .ts {
    font-size: var(--text-xs);
    color: var(--text-muted);
    padding-right: var(--space-1);
  }
</style>

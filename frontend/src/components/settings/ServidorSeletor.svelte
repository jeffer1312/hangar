<script lang="ts">
  // Seletor de QUAL servidor as telas do grupo "Servidor" configuram. Nasceu de pedido repetido
  // do usuário (19/08/2026): o rótulo "Servidor · X" DIZIA o alvo mas não trocava — trocar exigia
  // ir à tela Servidores e voltar. Select nativo de propósito: no celular vira o picker do
  // sistema, no desktop o dropdown de sempre, e foco/leitor de tela vêm de graça.
  import * as m from '../../paraglide/messages';
  import type { Server } from '../../lib/auth';

  interface Props {
    servidores: Server[];
    atualId: string;
    onTrocar: (id: string) => void;
  }
  let { servidores, atualId, onTrocar }: Props = $props();

  // Com uma máquina só não há troca a fazer — mas o seletor continua existindo, apagado e com o
  // motivo à vista. Some-lo escondia do usuário que a escolha existe (regra "controle que não
  // some"); o motivo é texto visível, nunca tooltip, porque toque não tem hover.
  const soUma = $derived(servidores.length <= 1);
  // O motivo é texto ao lado; sem o describedby o leitor de tela anuncia "desabilitado" e não diz
  // por quê — que é justamente o que esta tela passou a existir pra contar.
  const motivoId = $props.id();
</script>

<!-- Sem bind: o valor é controlado pela rota (?srv=). O change avisa, o App troca o alvo e a
     remontagem traz o valor novo — um select editável localmente mentiria durante o carregamento. -->
<span class="srv-wrap">
  <select class="srv-sel" value={atualId} aria-label={m.config_modal_trocar_servidor()}
          disabled={soUma} aria-describedby={soUma ? motivoId : undefined}
          onchange={(e) => onTrocar(e.currentTarget.value)}>
    {#each servidores as s (s.id)}
      <option value={s.id}>{s.label}</option>
    {/each}
  </select>
  {#if soUma}<span class="srv-motivo" id={motivoId}>{m.config_modal_uma_maquina()}</span>{/if}
</span>

<style>
  /* `flex-basis: 100%` no motivo (abaixo) põe ele numa linha própria: ao lado do select ele
     espremeria o nome da máquina na coluna estreita da navegação (232px). */
  .srv-wrap { display: inline-flex; align-items: center; flex-wrap: wrap; gap: 2px var(--space-2); min-width: 0; }
  /* Motivo do apagado: mesma receita do `motivo` da linha de configuração — texto pequeno e
     discreto ao lado do controle, sem virar aviso. */
  .srv-motivo {
    flex-basis: 100%;
    font-size: var(--text-xs); color: var(--text-muted);
    text-transform: none; letter-spacing: normal; font-weight: 400;
  }
  .srv-sel:disabled { cursor: default; color: var(--text-muted); opacity: 0.7; }
  .srv-sel {
    appearance: none; -webkit-appearance: none;
    height: 30px; min-height: 0; padding: 0 var(--space-5) 0 var(--space-3);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    background: var(--surface-raised); color: var(--text-primary);
    font-family: inherit; font-size: var(--text-xs); font-weight: 600;
    /* Morando dentro do rótulo de seção (maiúsculas, espaçadas), o select precisa voltar à
       forma normal — o nome da máquina é dado, não título de seção. */
    text-transform: none; letter-spacing: normal;
    cursor: pointer; max-width: 100%;
    /* Chevron próprio (appearance:none tira o nativo): SVG inline, mesma cor muted do chrome. */
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='10' height='6' viewBox='0 0 10 6'%3E%3Cpath d='M1 1l4 4 4-4' fill='none' stroke='%238d8489' stroke-width='1.5' stroke-linecap='round'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right var(--space-2) center;
  }
  .srv-sel:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  @media (hover: hover) { .srv-sel:hover { border-color: var(--border-default); } }
</style>

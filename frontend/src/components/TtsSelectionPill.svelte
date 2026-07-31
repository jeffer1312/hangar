<script lang="ts">
  import { onMount } from 'svelte';
  import { ttsSelection, iniciarCapturaDeSelecao } from '../lib/ttsSelection.svelte';
  import { ouvirTexto } from '../lib/ouvir';

  let isDesktop = $state(false);

  onMount(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    isDesktop = mq.matches;
    const aoTrocar = (e: MediaQueryListEvent) => { isDesktop = e.matches; };
    mq.addEventListener('change', aoTrocar);
    const parar = iniciarCapturaDeSelecao();
    return () => { mq.removeEventListener('change', aoTrocar); parar(); };
  });

  const rotulo = $derived(`🔊 Ouvir · ${ttsSelection.texto.length.toLocaleString('pt-BR')} car.`);

  function ouvir() {
    const texto = ttsSelection.texto;
    ttsSelection.limpar();
    // Chamada DIRETO do handler: ouvirTexto destrava o audio de forma sincrona (gesto do iOS).
    ouvirTexto(texto, (msg) => Promise.resolve(window.confirm(msg)));
  }
</script>

{#if ttsSelection.ativa}
  <button
    class="tts-sel"
    class:flutuante={isDesktop}
    style:left={isDesktop ? `${ttsSelection.x}px` : undefined}
    style:top={isDesktop ? `${ttsSelection.y + 6}px` : undefined}
    onpointerdown={(e) => e.preventDefault()}
    onclick={ouvir}
  >{rotulo}</button>
{/if}

<style>
  /* Celular: barra rente ao composer — nao disputa espaco com o menu nativo do Safari, que nasce
     colado na selecao. Desktop: pill flutuante no fim da selecao.
     onpointerdown preventDefault: sem isso o proprio toque no botao colapsa a selecao antes do
     click disparar. Sem backdrop-filter/transform em barra fixa: no WebKit pinta retangulo preto
     na rolagem por inercia (mesmo motivo do TtsBar). */
  .tts-sel {
    position: fixed;
    z-index: 39;
    padding: 8px 14px;
    border-radius: 999px;
    border: 1px solid var(--border-subtle);
    background: var(--surface-raised);
    color: var(--text-primary);
    font-size: 13px;
    cursor: pointer;
    left: 50%;
    margin-left: -90px;
    /* --cp-tts-bar-h (publicada no App.svelte): soma a altura da BARRA DO PLAYER quando ela esta
       ativa, senao a pill nasce no mesmo lugar da TtsBar e tapa play/posicao/velocidade — caso
       real: ouvir um trecho e selecionar o proximo enquanto o audio toca. NAO usar --cp-tts-h aqui
       (esse e o TOTAL barra+pill, que empurraria esta pill sozinha quando ela e a unica na tela). */
    bottom: calc(var(--cp-dock-h, 150px) + 10px + var(--cp-tts-bar-h, 0px));
  }
  .tts-sel.flutuante {
    bottom: auto;
    margin-left: 0;
    transform: translateX(-100%);
  }
</style>

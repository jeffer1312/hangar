<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import type { AnimationItem } from 'lottie-web';

  interface Props {
    data: unknown; // JSON da animacao (importado)
    size?: number; // lado do quadrado em px
    loop?: boolean;
    autoplay?: boolean;
    frame?: number | null; // quando parado (autoplay=false), congela NESTE frame
  }
  let { data, size = 24, loop = true, autoplay = true, frame = null }: Props = $props();

  let el: HTMLDivElement;
  let anim: AnimationItem | undefined;

  onMount(() => {
    let dead = false;
    // lottie-web (~250KB) fora do bundle inicial: import dinamico no 1o mount (chunk proprio,
    // cacheado dai em diante). O spinner e progressivo por natureza — nao trava nada esperando.
    import('lottie-web').then(({ default: lottie }) => {
      if (dead || !el) return;
      anim = lottie.loadAnimation({
        container: el,
        renderer: 'svg',
        loop,
        autoplay,
        animationData: data,
      });
      // O $effect do frame pode ter rodado antes do load (anim undefined): aplica a pose atual.
      if (!autoplay && frame != null) anim.goToAndStop(frame, true);
    });
    return () => { dead = true; };
  });

  // Parado num frame especifico (ex: pose distinta por estado). Reage a mudanca de frame sem remount.
  $effect(() => {
    if (anim && !autoplay && frame != null) anim.goToAndStop(frame, true);
  });

  onDestroy(() => anim?.destroy());
</script>

<div bind:this={el} style:width={`${size}px`} style:height={`${size}px`}></div>

<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import lottie, { type AnimationItem } from 'lottie-web';

  interface Props {
    data: unknown; // JSON da animacao (importado)
    size?: number; // lado do quadrado em px
    loop?: boolean;
    autoplay?: boolean;
  }
  let { data, size = 24, loop = true, autoplay = true }: Props = $props();

  let el: HTMLDivElement;
  let anim: AnimationItem | undefined;

  onMount(() => {
    anim = lottie.loadAnimation({
      container: el,
      renderer: 'svg',
      loop,
      autoplay,
      animationData: data,
    });
  });

  onDestroy(() => anim?.destroy());
</script>

<div bind:this={el} style:width={`${size}px`} style:height={`${size}px`}></div>

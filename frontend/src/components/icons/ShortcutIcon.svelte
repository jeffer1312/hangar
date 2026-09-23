<script lang="ts" module>
  // Conjunto curado dos atalhos customizados da fileira. Separado do ConfigIcone porque o `nome`
  // AQUI vem da config gravada pelo usuário (string solta), então a fronteira de segurança é
  // outra: o {@html} só recebe o conteúdo deste mapa estático — nome desconhecido cai no
  // fallback, nunca vira HTML. Traçados no estilo Feather, stroke 2, família da fileira.
  export const GLYPHS: Record<string, string> = {
    bolt:       '<path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z"/>',
    play:       '<path d="M5 3l14 9-14 9V3z"/>',
    rocket:    '<path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z"/><path d="M12 15l-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z"/><path d="M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0"/><path d="M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5"/>',
    gear: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>',
    git:        '<path d="M6 3v12"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>',
    chat:      '<path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z"/>',
    star:    '<path d="M12 2l3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77 5.82 21.02 7 14.14 2 9.27l6.91-1.01L12 2z"/>',
    folder:      '<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>',
    key:      '<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>',
    terminal:   '<path d="M4 17l6-6-6-6"/><path d="M12 19h8"/>',
    globe:      '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3a13.7 13.7 0 0 1 3.5 9 13.7 13.7 0 0 1-3.5 9 13.7 13.7 0 0 1-3.5-9A13.7 13.7 0 0 1 12 3z"/>',
    robot:       '<rect x="4" y="8" width="16" height="11" rx="2"/><path d="M12 8V5"/><circle cx="12" cy="3.5" r="1.5"/><path d="M9 13h.01"/><path d="M15 13h.01"/><path d="M9.5 16.5h5"/>',
  };

  /** Ícone da config ("emoji:🚀" | "glifo:bolt" | ausente) → forma renderizável. */
  export function parseIcon(icon: string | undefined): { emoji: string } | { glyph: string } {
    if (icon?.startsWith('emoji:')) {
      const e = icon.slice('emoji:'.length).trim();
      if (e) return { emoji: e };
    }
    if (icon?.startsWith('glifo:')) {
      const g = icon.slice('glifo:'.length);
      // hasOwn, não `in`: "glifo:constructor" passaria pelo protótipo.
      if (Object.hasOwn(GLYPHS, g)) return { glyph: g };
    }
    return { glyph: 'bolt' };
  }
</script>

<script lang="ts">
  interface Props {
    icon?: string;
    size?: number;
  }
  let { icon = undefined, size = 18 }: Props = $props();
  const shape = $derived(parseIcon(icon));
</script>

{#if 'emoji' in shape}
  <span class="emoji" style:font-size="{size - 2}px" aria-hidden="true">{shape.emoji}</span>
{:else}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
       stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{@html GLYPHS[shape.glyph]}</svg>
{/if}

<style>
  .emoji { line-height: 1; display: inline-flex; align-items: center; justify-content: center; }
</style>

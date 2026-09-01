<script lang="ts">
  // Ícone de traço das linhas de Configurações — substitui o emoji, que muda de desenho e de cor
  // conforme a plataforma (o 🎤 do iOS não é o do Android) e destoava do resto do app, que é todo
  // SVG stroke 1.8. Traçados no estilo Feather, mesma família dos ícones do drawer.
  interface Props {
    // keyof do mapa, não string solta: um typo ('mci') falha no typecheck em vez de renderizar
    // um SVG vazio em silêncio. O {@html} abaixo só recebe o CONTEÚDO deste mapa estático —
    // `nome` seleciona a chave, nunca vira HTML. Manter assim (fronteira de segurança).
    nome: keyof typeof TRACADOS;
    size?: number;
  }
  let { nome, size = 18 }: Props = $props();

  const TRACADOS: Record<string, string> = {
    globo:   '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3a13.7 13.7 0 0 1 3.5 9 13.7 13.7 0 0 1-3.5 9 13.7 13.7 0 0 1-3.5-9A13.7 13.7 0 0 1 12 3z"/>',
    pincel:  '<path d="M12 21a9 9 0 1 1 9-9c0 2-1.5 3-3 3h-2a2 2 0 0 0-1.5 3.3c.6.7.4 2.7-2.5 2.7z"/><circle cx="7.5" cy="11.5" r="1.1" fill="currentColor" stroke="none"/><circle cx="10.5" cy="7.5" r="1.1" fill="currentColor" stroke="none"/><circle cx="15" cy="7.5" r="1.1" fill="currentColor" stroke="none"/><circle cx="17.5" cy="11" r="1.1" fill="currentColor" stroke="none"/>',
    mic:     '<rect x="9" y="2" width="6" height="12" rx="3"/><path d="M5 10v2a7 7 0 0 0 14 0v-2"/><path d="M12 19v3"/>',
    recibo:  '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/><path d="M16 13H8"/><path d="M16 17H8"/>',
    info:    '<circle cx="12" cy="12" r="9"/><path d="M12 16v-5"/><path d="M12 8h.01"/>',
    sinal:   '<path d="M5 12.55a11 11 0 0 1 14.08 0"/><path d="M8.53 16.11a6 6 0 0 1 6.95 0"/><path d="M12 20h.01"/>',
    pessoa:  '<path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>',
    tela:    '<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8"/><path d="M12 17v4"/>',
    sino:    '<path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.7 21a2 2 0 0 1-3.4 0"/>',
    clipe:   '<path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"/>',
    chave:   '<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>',
    plug:    '<path d="M9 2v6"/><path d="M15 2v6"/><path d="M6 8h12v4a6 6 0 0 1-12 0z"/><path d="M12 18v4"/>',
    sliders: '<path d="M21 5h-7"/><path d="M10 5H3"/><path d="M21 12h-9"/><path d="M8 12H3"/><path d="M21 19h-5"/><path d="M12 19H3"/><path d="M14 3v4"/><path d="M8 10v4"/><path d="M16 17v4"/>',
  };
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{@html TRACADOS[nome]}</svg>

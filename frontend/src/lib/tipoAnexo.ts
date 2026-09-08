// O que o tile do anexo precisa saber do arquivo. O ÍCONE não mora aqui: quem desenha é o
// `FileIcon` (material-icon-theme, o mesmo da aba Arquivos), que já conhece extensão de verdade.
// Aqui só ficam as duas perguntas que mudam o tile: dá pra mostrar o conteúdo (imagem) e dá pra
// tirar um quadro (vídeo).
export type TipoAnexo = 'imagem' | 'video' | 'arquivo';

export function tipoDoArquivo(file: File): TipoAnexo {
  if (file.type.startsWith('image/')) return 'imagem';
  if (file.type.startsWith('video/')) return 'video';
  // Extensão como plano B: um `.mov` gravado no iPhone às vezes chega com `type` vazio.
  const ext = (file.name.split('.').pop() || '').toLowerCase();
  if (['mp4', 'mov', 'webm', 'mkv', 'avi', 'm4v'].includes(ext)) return 'video';
  return 'arquivo';
}

/**
 * Primeiro quadro de um vídeo, como data: URI — a miniatura do tile.
 *
 * Nada de `URL.createObjectURL` no `<video>` da tela: o tile é 56px e segurar o arquivo inteiro
 * decodificando ali é caro por anexo. Aqui o vídeo vive só o tempo de um quadro e some.
 * Devolve '' quando o navegador não decodifica aquele formato — quem chama cai no ícone.
 */
export function quadroDeVideo(file: File): Promise<string> {
  return new Promise((resolve) => {
    const url = URL.createObjectURL(file);
    const video = document.createElement('video');
    let respondeu = false;
    const acabar = (valor: string) => {
      if (respondeu) return;
      respondeu = true;
      URL.revokeObjectURL(url);
      video.src = '';
      resolve(valor);
    };
    // Teto: vídeo que nunca dispara `seeked` (codec sem suporte, arquivo truncado) deixaria a
    // promessa pendurada e o tile sem miniatura pra sempre.
    const prazo = setTimeout(() => acabar(''), 4000);
    video.muted = true;
    video.playsInline = true;
    video.preload = 'metadata';
    video.onloadeddata = () => {
      // 0.1s, não 0: o primeiro quadro de muito vídeo é preto.
      video.currentTime = Math.min(0.1, (video.duration || 1) / 2);
    };
    video.onseeked = () => {
      clearTimeout(prazo);
      try {
        const canvas = document.createElement('canvas');
        const lado = 112;                       // 56px em telas de densidade 2
        canvas.width = lado;
        canvas.height = lado;
        const ctx = canvas.getContext('2d');
        if (!ctx || !video.videoWidth) return acabar('');
        // Recorte central quadrado: o tile é quadrado e `object-fit: cover` faria o mesmo, mas
        // guardar o quadro inteiro custaria mais bytes por nada.
        const corte = Math.min(video.videoWidth, video.videoHeight);
        ctx.drawImage(video, (video.videoWidth - corte) / 2, (video.videoHeight - corte) / 2,
                      corte, corte, 0, 0, lado, lado);
        acabar(canvas.toDataURL('image/jpeg', 0.7));
      } catch {
        acabar('');                             // canvas "sujo" (origem cruzada) e afins
      }
    };
    video.onerror = () => { clearTimeout(prazo); acabar(''); };
    video.src = url;
  });
}

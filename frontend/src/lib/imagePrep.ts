// Preparo de imagem ANTES do upload.
//
// Duas coisas resolvidas aqui:
//
// 1. Foto do celular subia crua. Medido numa pasta real: 4032x3024, 2554 KB. A visão do modelo
//    trabalha em ~1568px no lado maior — acima disso a imagem é reduzida de qualquer jeito antes de
//    virar token. Ou seja: aqueles 4032px viram 1568px na outra ponta, e você pagou o upload
//    inteiro por VPN/4G pra nada. Encolher aqui entrega EXATAMENTE o mesmo pro modelo com arquivo
//    muito menor. Não é economia de token — é de transferência, disco e espera.
//
// 2. HEIC (padrão do iPhone). Não está entre os formatos que o app faz miniatura nem entre os que o
//    Read abre: subia e virava um anexo que ninguém consegue ver. Passar pelo canvas converte pra
//    JPEG — quando o navegador sabe decodificar (Safari sabe).
//
// Regra de ouro: qualquer falha devolve o arquivo ORIGINAL. Anexo é do usuário; otimizar não pode
// custar o anexo.

export const MAX_EDGE = 1568;

// Quanto do começo do arquivo basta pra saber se é animado (os marcadores ficam no cabeçalho).
const CABECALHO = 4096;

// Formatos que o app mostra e o modelo abre. Fora daqui (heic/heif/tiff...), convertemos.
const OK_TYPES = new Set(['image/jpeg', 'image/png', 'image/gif', 'image/webp']);

/** Lado maior alvo, mantendo proporção. Devolve null quando não precisa mexer. */
export function targetSize(
  w: number,
  h: number,
  maxEdge = MAX_EDGE,
): { w: number; h: number } | null {
  if (!(w > 0 && h > 0)) return null;
  const maior = Math.max(w, h);
  if (maior <= maxEdge) return null;
  const k = maxEdge / maior;
  // round pra não gerar 0 em imagens muito alongadas (ex: 4000x3 -> altura 1).
  return { w: Math.max(1, Math.round(w * k)), h: Math.max(1, Math.round(h * k)) };
}

/** True se o arquivo precisa passar pelo canvas (por tamanho OU por formato). */
export function precisaPreparo(type: string, w: number, h: number): boolean {
  return !OK_TYPES.has(type) || targetSize(w, h) !== null;
}

/**
 * Procura um marcador ASCII de 4 letras nos primeiros bytes — é assim que WebP e PNG anunciam
 * animação: `ANIM` no container RIFF do WebP, `acTL` (antes do `IDAT`) no APNG.
 *
 * Sem isto, um WebP/APNG animado grande demais passava pelo canvas e voltava como um quadro só,
 * sem aviso nenhum: o usuário mandava um GIF-equivalente e recebia uma foto parada.
 */
export function temMarcador(bytes: Uint8Array, marcador: string): boolean {
  const alvo = [...marcador].map((c) => c.charCodeAt(0));
  outer: for (let i = 0; i + alvo.length <= bytes.length; i++) {
    for (let k = 0; k < alvo.length; k++) if (bytes[i + k] !== alvo[k]) continue outer;
    return true;
  }
  return false;
}

/** Formatos animáveis que o modelo JÁ abre: se for animado, não vale a pena achatar. */
async function ehAnimadoPreservavel(file: File): Promise<boolean> {
  const marcador = file.type === 'image/webp' ? 'ANIM' : file.type === 'image/png' ? 'acTL' : '';
  if (!marcador) return false;
  try {
    const buf = await file.slice(0, CABECALHO).arrayBuffer();
    return temMarcador(new Uint8Array(buf), marcador);
  } catch {
    return false;
  }
}

function novoNome(nome: string, ext: string): string {
  const base = nome.replace(/\.[^.]+$/, '') || 'imagem';
  return `${base}.${ext}`;
}

/**
 * Devolve a imagem pronta pra upload (ou o próprio arquivo, se não houver o que ganhar).
 *
 * GIF passa direto: o canvas achataria a animação num quadro só — perder a animação é pior do que
 * subir alguns KB a mais.
 */
export async function prepareImage(file: File): Promise<File> {
  if (!file.type.startsWith('image/') || file.type === 'image/gif') return file;
  if (typeof createImageBitmap !== 'function') return file;
  // WebP/APNG animados ficam como estão: perder a animação é pior do que subir uns KB a mais.
  // (AVIF animado é a exceção consciente: o modelo não abre AVIF, então converter e perder a
  // animação ainda é melhor do que entregar um anexo que ninguém consegue ver.)
  if (await ehAnimadoPreservavel(file)) return file;

  let bmp: ImageBitmap;
  try {
    bmp = await createImageBitmap(file);
  } catch {
    return file;                      // navegador não decodifica (ex: HEIC no Chrome) -> sobe cru
  }
  try {
    if (!precisaPreparo(file.type, bmp.width, bmp.height)) return file;

    const alvo = targetSize(bmp.width, bmp.height) ?? { w: bmp.width, h: bmp.height };
    const canvas = document.createElement('canvas');
    canvas.width = alvo.w;
    canvas.height = alvo.h;
    const ctx = canvas.getContext('2d');
    if (!ctx) return file;
    ctx.drawImage(bmp, 0, 0, alvo.w, alvo.h);

    // PNG continua PNG: as imagens de tela do app são print de UI e texto, onde JPEG borra as
    // bordas. Todo o resto (foto, HEIC) sai JPEG.
    const png = file.type === 'image/png';
    const mime = png ? 'image/png' : 'image/jpeg';
    const blob = await new Promise<Blob | null>((ok) =>
      canvas.toBlob(ok, mime, png ? undefined : 0.85),
    );
    if (!blob || blob.size === 0) return file;
    // Reencodar pode ENGORDAR (png pequeno, jpeg já bem comprimido). Nesse caso, fica o original —
    // desde que o original seja um formato que o app/modelo aceitam.
    if (blob.size >= file.size && OK_TYPES.has(file.type)) return file;

    return new File([blob], novoNome(file.name, png ? 'png' : 'jpg'), { type: mime });
  } catch {
    return file;
  } finally {
    bmp.close?.();
  }
}

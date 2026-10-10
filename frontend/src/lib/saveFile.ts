export type SaveResult = 'saved' | 'cancelled' | 'tap-again';

// Arquivo já baixado à espera do segundo toque: o iOS só abre a folha de compartilhar dentro do
// gesto do usuário, e um download demorado o deixa vencer.
const ready = new Map<string, File>();

// Celular salva pela folha de compartilhar ("Salvar imagem", "Salvar em Arquivos"): no app
// instalado, um <a download> abre o arquivo por cima do app, sem caminho de volta. O desktop
// segue baixando como antes.
function shouldShare(file: File): boolean {
  return (
    typeof navigator.canShare === 'function' &&
    window.matchMedia('(pointer: coarse)').matches &&
    navigator.canShare({ files: [file] })
  );
}

export async function saveFile(url: string, name: string): Promise<SaveResult> {
  let file = ready.get(url);
  if (!file) {
    const resp = await fetch(url);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const blob = await resp.blob();
    file = new File([blob], name, { type: blob.type || 'application/octet-stream' });
  }
  if (shouldShare(file)) {
    try {
      await navigator.share({ files: [file] });
      ready.delete(url);
      return 'saved';
    } catch (e) {
      const kind = (e as DOMException)?.name;
      if (kind === 'AbortError') return 'cancelled';
      if (kind === 'NotAllowedError') {
        ready.clear();
        ready.set(url, file);
        return 'tap-again';
      }
      throw e;
    }
  }
  ready.delete(url);
  const href = URL.createObjectURL(file);
  const a = document.createElement('a');
  a.href = href;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(href), 60_000);
  return 'saved';
}

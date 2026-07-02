// Copia texto pro clipboard, com fallback execCommand pra LAN via HTTP puro (onde a Clipboard API
// nao existe fora de contexto seguro). Um lugar so -> bubbles e sidebar nao duplicam o fallback.
export async function copyText(s: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(s);
  } catch {
    const ta = document.createElement('textarea');
    ta.value = s;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    document.execCommand('copy');
    ta.remove();
  }
}

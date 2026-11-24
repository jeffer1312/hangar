// Sem importar `expo`: a folha Portas, que pergunta isto, entra nos testes de várias telas. Os módulos nativos já
// estão em `globalThis.expo.modules` quando a folha abre (o app carregou outros módulos expo antes).
type Native = { isSupported(): boolean };

/** iOS 17+ (proxy por armazenamento do WKWebView) ou System WebView com `PROXY_OVERRIDE`. Binário sem o
 * módulo (antes do native-7) responde `false` e a porta abre pela prévia do Connect. */
export function portTunnelSupported(): boolean {
  const native = (globalThis as { expo?: { modules?: Record<string, Native | undefined> } }).expo?.modules?.PortTunnel;
  try { return !!native?.isSupported(); } catch { return false; }
}

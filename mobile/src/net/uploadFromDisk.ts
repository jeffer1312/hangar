// `fetch` que sobe o arquivo do disco pelo envio nativo, ignorando o `body`. O fetch do Expo lê o
// arquivo inteiro para o JS e o reconverte por base64 ao enviar: um zip grande estoura o Hermes.
export function sendFromDisk(uri: string): typeof fetch {
  return async (input, init) => {
    const signal = init?.signal ?? undefined;
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    try {
      // Carregado só no envio: a tela que importa este módulo não arrasta o módulo nativo junto.
      const { File } = await import('expo-file-system');
      const result = await new File(uri).upload(url, {
        httpMethod: 'POST',
        headers: init?.headers as Record<string, string>,
        sessionType: 'foreground',
        signal,
      });
      return new Response(result.body, { status: result.status, headers: result.headers });
    } catch (e) {
      // O envio nativo rejeita com AbortError genérico; o motivo do sinal diz que foi o prazo.
      throw signal?.aborted ? signal.reason : e;
    }
  };
}

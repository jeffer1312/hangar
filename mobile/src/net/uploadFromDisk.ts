// `fetch` que sobe o arquivo do disco pelo envio nativo, ignorando o `body`. O fetch do Expo lê o
// arquivo inteiro para o JS e o reconverte por base64 ao enviar: um zip grande estoura o Hermes.
export function sendFromDisk(uri: string): typeof fetch {
  return async (input, init) => {
    const signal = init?.signal;
    if (signal?.aborted) throw signal.reason;
    const FileSystem = await import('expo-file-system/legacy');
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    const task = FileSystem.createUploadTask(url, uri, {
      httpMethod: 'POST',
      headers: init?.headers as Record<string, string>,
      uploadType: FileSystem.FileSystemUploadType.BINARY_CONTENT,
      sessionType: FileSystem.FileSystemSessionType.FOREGROUND,
    });
    const cancel = () => { void task.cancelAsync(); };
    signal?.addEventListener('abort', cancel, { once: true });
    try {
      const result = await task.uploadAsync();
      if (!result) throw signal?.reason;
      return new Response(result.body, { status: result.status, headers: result.headers });
    } catch (e) {
      throw signal?.aborted ? signal.reason : e;
    } finally {
      signal?.removeEventListener('abort', cancel);
    }
  };
}

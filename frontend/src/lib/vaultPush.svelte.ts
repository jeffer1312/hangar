// Resultado do ÚLTIMO push do vault pro hub de sync.
//
// Existe porque o push roda num listener SOLTO: quem mexe na lista de servidores (trocar token,
// renomear, remover) chama notifyChanged() e segue a vida — o push acontece depois, em
// App.svelte, sem devolver nada pra quem agiu. Falhar calado ali é o pior modo de errar numa
// ROTAÇÃO DE TOKEN: você troca a chave num aparelho, funciona nele, e os outros continuam com a
// antiga. O sintoma chega dias depois como 401, e a leitura natural é "errei o token" — não "o
// sync não subiu".
//
// Singleton reativo (mesmo padrão do sessionsStore.svelte.ts): o AccountMenu lê direto, sem
// prop-drilling por Sidebar/SessionList — que são justamente os dois arquivos que vivem divergindo.
//
// 'idle' cobre quem NUNCA configurou sync: nesse caso o listener do App nem chega a ser
// registrado, ninguém publica nada aqui, e a UI não mostra aviso nenhum. Sem falso alarme.
export type VaultPushEstado = 'idle' | 'ok' | 'locked' | 'error';

let estado = $state<VaultPushEstado>('idle');
let detalhe = $state('');

export const vaultPush = {
  get estado() { return estado; },
  get detalhe() { return detalhe; },

  ok() {
    estado = 'ok';
    detalhe = '';
  },
  // encKey nulo DENTRO do listener = tinha sync e deslogou (o listener só nasce no establishSync).
  // Não é o caso de "nunca usou sync", então avisar aqui é informação, não ruído.
  locked() {
    estado = 'locked';
    detalhe = 'sync deslogado — a mudança ficou só neste aparelho';
  },
  error(e: unknown) {
    estado = 'error';
    detalhe = `não subiu pro hub: ${e instanceof Error ? e.message : String(e)}`;
  },
  clear() {
    estado = 'idle';
    detalhe = '';
  },
};

// Chaves do {#each} da lista de mensagens.
//
// Por que isto existe: no Svelte 5, chave repetida num `{#each}` keyed NAO e um aviso — e um throw
// (`each_key_duplicate`), e ele derruba a arvore inteira. Na pratica: a conversa abre VAZIA e a tela
// para de responder, com o cromo (navbar, composer) ainda desenhado por cima. Foi o que aconteceu
// numa sessao real: o transcript trazia duas entradas de fila consumidas no MESMO milissegundo com o
// MESMO texto, e o id do backend (`queued:<timestamp>:<md5 do texto>`) saiu igual pras duas.
//
// A regra do app e que uma feature (aqui, a fila duravel) nunca pode derrubar o chat. O id vindo do
// backend pode ser corrigido — e deve —, mas a lista precisa ser imune a QUALQUER transcript, porque
// ela le arquivo que outro processo escreve e nao controla.
//
// Estabilidade importa tanto quanto unicidade: a mesma lista, na mesma ordem, tem que gerar sempre as
// mesmas chaves, senao o Svelte destroi e recria os nos a cada render (perde foco, scroll e animacao).
// Por isso o desempate e por ORDEM DE APARICAO, deterministico, e nao um contador aleatorio.
export function chavesUnicas(ids: readonly string[]): string[] {
  // `brutos` guarda TODOS os ids originais, nao so os ja emitidos: sem isso, o sufixo que geramos
  // podia roubar um id que existe de verdade mais adiante na lista — com ['a', 'a', 'a#2'], o
  // desempate do segundo 'a' virava 'a#2' e colidia com o terceiro item. O teste pegou.
  const brutos = new Set(ids);
  const usados = new Set<string>();
  return ids.map((id) => {
    // A primeira ocorrencia mantem o id CRU: no caso normal (sem colisao) a chave continua sendo
    // exatamente o id do evento, e nada que dependa disso muda.
    if (!usados.has(id)) { usados.add(id); return id; }
    // ponytail: a busca recomeca do 2 a cada repeticao, entao K copias do MESMO id custam O(K²).
    // O teto real e a janela de render da lista (WINDOW=120 em MessageList) e o caso que motivou
    // isto tinha DUAS copias. Se algum dia aparecer transcript com centenas de ids identicos, o
    // upgrade e guardar o ultimo `n` por id num Map em vez de recontar.
    let n = 2;
    let cand = `${id}#${n}`;
    while (usados.has(cand) || brutos.has(cand)) cand = `${id}#${++n}`;
    usados.add(cand);
    return cand;
  });
}

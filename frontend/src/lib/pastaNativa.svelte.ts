/**
 * Seletor NATIVO de pasta — a ponte com o shell Electron, agora usada em DOIS lugares (o modal de
 * "Nova sessão" e "Pastas mapeadas", nas Configurações). Era código solto dentro do modal; virou
 * módulo quando o segundo ponto apareceu, pra a trava de clique duplo e o tratamento de erro
 * existirem numa versão só.
 *
 * `window.hangar.pickFolder` é exposto pelo preload do shell (shell/preload.cjs → IPC
 * 'hangar:pick-folder' → `dialog.showOpenDialog`, em shell/main.cjs). É multiplataforma: o mesmo
 * caminho vale em Linux, Windows e macOS — não há ramo por sistema ali.
 *
 * Fora do shell (navegador, celular, PWA) a ponte não existe, e não há substituto: a File System
 * Access API devolve um HANDLE, não o caminho absoluto, e o backend precisa do caminho. Por isso a
 * regra dos dois pontos de uso é a mesma — o campo de texto é o caminho principal e NUNCA some; o
 * botão é um acréscimo que aparece quando a ponte está lá.
 *
 * A pasta escolhida é da máquina do SHELL. Apontando pra um backend remoto, o caminho pode não
 * existir do outro lado — o erro aparece no fluxo normal (cwd inválido / raiz que não varre).
 */
type PickFolder = () => Promise<string | null>;

/**
 * Lido a cada chamada, não uma vez no import: o preload injeta `window.hangar` antes da página, mas
 * um módulo importado no topo do bundle avaliaria isso cedo demais pra quem monta o componente
 * depois (é o que os testes fazem), e a const congelaria `undefined` pra sempre.
 */
export function seletorDePasta(): PickFolder | undefined {
  return (window as { hangar?: { pickFolder?: PickFolder } }).hangar?.pickFolder;
}

/**
 * Estado de UMA tela que abre o diálogo: disponibilidade, trava de concorrência e a mensagem de
 * erro. A trava existe porque dois diálogos nativos abertos ao mesmo tempo resolvem fora de ordem e
 * o último a voltar sobrescreveria a escolha do primeiro, calado.
 */
export function criarSeletorNativo() {
  let ocupado = $state(false);
  let erro = $state('');
  return {
    /** Há shell Electron por trás? Falso no navegador e no celular — ali a tela fica como está. */
    get disponivel() { return seletorDePasta() !== undefined; },
    get ocupado() { return ocupado; },
    get erro() { return erro; },
    /** Abre o diálogo e entrega o caminho escolhido. Cancelar (null) não chama `aoEscolher`. */
    async escolher(aoEscolher: (caminho: string) => void) {
      const pick = seletorDePasta();
      if (!pick || ocupado) return;
      ocupado = true;
      erro = '';
      try {
        const p = await pick();
        if (p) aoEscolher(p);
      } catch (e) {
        erro = e instanceof Error ? e.message : String(e);
      } finally {
        ocupado = false;
      }
    },
  };
}

// Uma consulta só do breakpoint de 820px para as folhas. Cada uma carregava a mesma cópia de
// matchMedia + listener + limpeza; o listener aqui vive enquanto a página vive, que é o escopo real
// da pergunta ("esta janela é desktop?").
const MQ = '(min-width: 820px)';

let atual = $state(typeof window !== 'undefined' && window.matchMedia(MQ).matches);

if (typeof window !== 'undefined') {
  window.matchMedia(MQ).addEventListener('change', (e) => (atual = e.matches));
}

export const desktop = {
  get atual() {
    return atual;
  },
};

// @vitest-environment happy-dom
// A janela de render tem que voltar pra cauda quando uma CARGA de histórico chega — mesmo com a
// lista já montada e com o scroll fora do fim. Sem isso, o único caminho que avançava `windowEnd`
// era o effect de auto-scroll, que congela de propósito quando o usuário está lendo o histórico:
// a cauda recém-chegada ficava fora da fatia e a resposta não aparecia (regressão confirmada no
// aparelho em 06/09/2026 — mandar msg, sair, voltar; só a segunda entrada mostrava).
import { describe, it, expect, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import MessageList from './MessageList.svelte';
import type { ChatEvent } from '@hangar/core';

const msgs = (n: number): ChatEvent[] =>
  Array.from({ length: n }, (_, i) => ({ kind: 'user_msg', id: `e${i}`, text: `msg-${i}` }) as ChatEvent);

// happy-dom não faz layout: scrollHeight/clientHeight/scrollTop são 0, então um evento de scroll
// nasce com folga 0 = "está no fim". Forjar as três medidas é o que permite reproduzir o estado
// que congela a janela — alguém rolou pra cima.
//
// `scrollTop` entra como propriedade GRAVÁVEL, e não como valor fixo: no DOM real ela é de
// leitura e escrita, e o componente escreve nela (`revealOlder` empurra o scroll pra preservar o
// ponto de leitura ao revelar página antiga). Forjada read-only, a atribuição estoura e a exceção
// sai como rejeição não tratada — teste passando com erro pendurado ao lado.
function rolarPraCima(lista: HTMLElement) {
  for (const [prop, valor] of [['scrollHeight', 5000], ['clientHeight', 500]] as const) {
    Object.defineProperty(lista, prop, { value: valor, configurable: true });
  }
  let topo = 100;
  Object.defineProperty(lista, 'scrollTop', {
    configurable: true,
    get: () => topo,
    set: (v: number) => { topo = v; },
  });
  lista.dispatchEvent(new Event('scroll'));
}

describe('janela da MessageList numa carga de histórico', () => {
  let alvo: HTMLElement;
  beforeEach(() => {
    alvo = document.createElement('div');
    document.body.appendChild(alvo);
  });

  it('re-ancora na cauda quando a âncora muda, mesmo com o scroll longe do fim', async () => {
    const props = $state({
      events: msgs(200),
      stateEvent: null,
      pending: [],
      sessionName: 's',
      dockH: 0,
      ancora: 0,
      onSelectOption: () => {},
      onCancel: () => {},
    });
    const comp = mount(MessageList, { target: alvo, props });
    await tick();
    expect(alvo.textContent).toContain('msg-199');

    rolarPraCima(alvo.querySelector('.message-list') as HTMLElement);
    await tick();

    // Chegou a cauda do servidor com um evento novo, e a lista continua rolada pra cima.
    props.events = [...msgs(200), { kind: 'user_msg', id: 'novo', text: 'msg-resposta' } as ChatEvent];
    await tick();
    expect(alvo.textContent).not.toContain('msg-resposta');   // janela congelada: é o defeito

    // A âncora sobe junto com a carga -> a janela volta pra cauda e a resposta aparece.
    props.ancora += 1;
    await tick();
    expect(alvo.textContent).toContain('msg-resposta');

    unmount(comp as never);
  });

  it('sem âncora nova, rolar pra cima continua congelando a janela (leitura não é arrastada)', async () => {
    const props = $state({
      events: msgs(200),
      stateEvent: null,
      pending: [],
      sessionName: 's',
      dockH: 0,
      ancora: 0,
      onSelectOption: () => {},
      onCancel: () => {},
    });
    const comp = mount(MessageList, { target: alvo, props });
    await tick();
    rolarPraCima(alvo.querySelector('.message-list') as HTMLElement);
    await tick();
    props.events = [...msgs(200), { kind: 'user_msg', id: 'vivo', text: 'msg-ao-vivo' } as ChatEvent];
    await tick();
    expect(alvo.textContent).not.toContain('msg-ao-vivo');
    unmount(comp as never);
  });
});

// Quais SEGREDOS o servidor tem configurados — só o booleano, nunca o valor. Existe para o app
// esconder, FORA da tela de configuração, o que não tem como funcionar: sem chave da ElevenLabs o
// chip "Ouvir" não é um botão que falha, é um botão que não devia estar ali.
import { getConfig } from '@hangar/core';

// Ler em voz alta funciona por DOIS motores (backend/app/tts.py): ElevenLabs, ou o comando local
// quando configurado. `tts_local_cmd` não é segredo — o backend calcula `definido` como
// `valor is not None`, sempre true pra um `str` com default "" — então a pergunta certa é sobre o
// VALOR (campos.<chave>.valor), nunca o `definido`. Único lugar que sabe a resposta: quem decidir
// "dá pra ler em voz alta aqui?" fora da tela de configuração chama esta função, nunca reimplementa
// o critério.
export function podeLerCriterio(temChaveEleven: boolean, comandoLocal: unknown): boolean {
  return temChaveEleven || Boolean(String(comandoLocal ?? '').trim());
}

const estado = $state<{ definidos: Record<string, boolean>; valores: Record<string, unknown> }>({
  definidos: {},
  valores: {},
});

// Contador de CHAMADAS, mesmo padrão do `ultimoServidorConsultado` em DesktopShell.svelte: duas
// chamadas de `carregar()` não têm ordem entre si (troca rápida de servidor A -> B dispara um GET
// pra cada), e a resposta de A pode chegar DEPOIS da de B. Sem isto, a resposta velha de A pintava
// por cima do resultado certo de B — o chip "Ouvir" respondia pelo servidor errado até a troca
// seguinte corrigir.
let geracao = 0;

export const segredos = {
  temChave(chave: string): boolean {
    return estado.definidos[chave] === true;
  },
  // A pergunta que todo portão "some sem como funcionar" fora da tela de configuração faz —
  // pill do composer, alto-falante por mensagem, atalho Ctrl/Cmd+Shift+Espaço.
  podeLer(): boolean {
    return podeLerCriterio(this.temChave('elevenlabs_api_key'), estado.valores['tts_local_cmd']);
  },
  // Trocar de servidor ativo troca o dono das chaves: o que era verdade no servidor de casa não
  // vale no do trabalho. Sem isto o chip do chat responderia pela máquina errada. Bumping da
  // geração aqui também descarta qualquer `carregar()` ainda em voo (ex: id virou null, sem
  // chamada nova) — sem isto uma resposta atrasada reencheria `definidos` por cima do esquecido.
  esquecer(): void {
    geracao++;
    estado.definidos = {};
    estado.valores = {};
  },
  // Nunca relança: isto é enfeite de interface, e uma falha aqui não pode derrubar quem chamou.
  async carregar(): Promise<void> {
    const minha = ++geracao;
    try {
      const cfg = await getConfig();
      if (minha !== geracao) return; // outra troca já é mais nova — resposta velha, descarta
      const novo: Record<string, boolean> = {};
      const novosValores: Record<string, unknown> = {};
      for (const [k, v] of Object.entries(cfg.campos ?? {})) {
        novo[k] = v?.definido === true;
        novosValores[k] = v?.valor;
      }
      estado.definidos = novo;
      estado.valores = novosValores;
    } catch {
      // silêncio deliberado: sem resposta, seguimos com o que já sabíamos.
    }
  },
};

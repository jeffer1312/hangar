// Quais SEGREDOS o servidor tem configurados — só o booleano, nunca o valor. Existe para o app
// esconder, FORA da tela de configuração, o que não tem como funcionar: sem chave da ElevenLabs o
// chip "Ouvir" não é um botão que falha, é um botão que não devia estar ali.
import { getConfig } from './api';

const estado = $state<{ definidos: Record<string, boolean> }>({ definidos: {} });

export const segredos = {
  temChave(chave: string): boolean {
    return estado.definidos[chave] === true;
  },
  // Trocar de servidor ativo troca o dono das chaves: o que era verdade no servidor de casa não
  // vale no do trabalho. Sem isto o chip do chat responderia pela máquina errada.
  esquecer(): void {
    estado.definidos = {};
  },
  // Nunca relança: isto é enfeite de interface, e uma falha aqui não pode derrubar quem chamou.
  async carregar(): Promise<void> {
    try {
      const cfg = await getConfig();
      const novo: Record<string, boolean> = {};
      for (const [k, v] of Object.entries(cfg.campos ?? {})) novo[k] = v?.definido === true;
      estado.definidos = novo;
    } catch {
      // silêncio deliberado: sem resposta, seguimos com o que já sabíamos.
    }
  },
};

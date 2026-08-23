import { create } from 'zustand';
import { getConfig, patchConfig } from '@hangar/core';
import { ehEstilo } from '@hangar/core';
import type { EstiloDitado } from '@hangar/core';

const PADRAO: EstiloDitado = 'prosa';

let escritas = 0;
let carregado = false;
let carregando: Promise<void> | null = null;

interface DitadoEstiloState {
  valor: EstiloDitado;
  pronto: boolean;
  carregar: () => Promise<void>;
  revalidar: () => Promise<void>;
  trocar: (novo: EstiloDitado) => Promise<void>;
  _zerarParaTeste: () => void;
}

export const useDitadoEstiloStore = create<DitadoEstiloState>((set, get) => ({
  valor: PADRAO,
  pronto: false,

  carregar: async () => {
    if (get().pronto) return;
    if (carregando) return carregando;
    const escritasNoInicio = escritas;
    try {
      carregando = getConfig()
        .then((cfg) => {
          const v = (cfg as unknown as { campos?: Record<string, { valor: unknown }> }).campos?.ditado_estilo?.valor;
          if (ehEstilo(v) && escritas === escritasNoInicio) {
            set({ valor: v });
          }
          carregado = true;
          set({ pronto: true });
        })
        .catch(() => {})
        .finally(() => {
          carregando = null;
        });
      return carregando;
    } catch {
      carregando = null;
      return Promise.resolve();
    }
  },

  revalidar: async () => {
    carregado = false;
    set({ pronto: false });
    return get().carregar();
  },

  trocar: async (novo: EstiloDitado) => {
    const antes = get().valor;
    const minha = ++escritas;
    set({ valor: novo });
    try {
      await patchConfig({ ditado_estilo: novo });
      carregado = true;
      set({ pronto: true });
    } catch (e) {
      if (escritas === minha) {
        set({ valor: antes });
      }
      throw e;
    }
  },

  _zerarParaTeste: () => {
    escritas = 0;
    carregado = false;
    carregando = null;
    set({ valor: PADRAO, pronto: false });
  },
}));

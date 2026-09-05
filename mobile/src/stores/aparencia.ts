import { create } from 'zustand';
import { UnistylesRuntime } from 'react-native-unistyles';
import { PENSAMENTO_TOOLS, hexParaRgb, type PensamentoTools, type GroupBy } from '@hangar/core';
import { prefs } from './prefs';
import { aplicarMaterial } from '../theme/aplicarMaterial';

export type Tema = 'system' | 'light' | 'dark';
export type Fundo = 'flat' | 'texture' | 'aurora' | 'image';
const TEMAS: Tema[] = ['system', 'light', 'dark'];
const FUNDOS: Fundo[] = ['flat', 'texture', 'aurora', 'image'];
const AGRUPAR: GroupBy[] = ['none', 'server', 'project'];
const K = 'aparencia.tema';
const K_PENSAMENTO = 'aparencia.pensamentoTools';
const K_AGRUPAR = 'lista.agrupar';
const K_FUNDO = 'aparencia.fundo';
const K_IMAGEM = 'aparencia.imagemUri';
const K_PANEL = 'aparencia.panelAlpha';
const K_SURFACE = 'aparencia.surfaceAlpha';
const K_ACENTO = 'aparencia.acento';

// Piso do painel: abaixo de 0.3 o texto do header/composer deixa de ter contraste sobre foto clara.
const PANEL_MIN = 0.3;
const PANEL_PADRAO = 0.86;
const faixa = (v: number, min: number) => Math.max(min, Math.min(1, v));

function ler(): Tema {
  const v = prefs.getString(K) as Tema | undefined;
  return v && TEMAS.includes(v) ? v : 'system';
}

// Valor desconhecido cai no padrão em vez de virar um quarto modo: escrito por versão futura,
// um `if` que não casa com nenhum ramo deixaria a conversa sem bloco de pensamento nenhum.
function lerPensamento(): PensamentoTools {
  const v = prefs.getString(K_PENSAMENTO) as PensamentoTools | undefined;
  return v && PENSAMENTO_TOOLS.includes(v) ? v : 'busca';
}

export function ehGroupBy(v: unknown): v is GroupBy {
  return AGRUPAR.includes(v as GroupBy);
}

function lerAgrupar(): GroupBy {
  const v = prefs.getString(K_AGRUPAR);
  return ehGroupBy(v) ? v : 'server';
}

// Unistyles: `adaptiveThemes` segue o SO; fixar tema exige desligar o adaptativo antes de setTheme.
function aplicar(t: Tema) {
  if (t === 'system') UnistylesRuntime.setAdaptiveThemes(true);
  else { UnistylesRuntime.setAdaptiveThemes(false); UnistylesRuntime.setTheme(t); }
}

function lerFundo(): Fundo {
  const v = prefs.getString(K_FUNDO) as Fundo | undefined;
  return v && FUNDOS.includes(v) ? v : 'flat';
}

function lerAlpha(k: string, padrao: number, min: number): number {
  const v = prefs.getNumber(k);
  return typeof v === 'number' && Number.isFinite(v) ? faixa(v, min) : padrao;
}

function lerAcento(): string | null {
  const v = prefs.getString(K_ACENTO);
  return v && hexParaRgb(v) ? v : null;
}

interface Aparencia {
  tema: Tema;
  setTema: (t: Tema) => void;
  pensamentoTools: PensamentoTools;
  setPensamentoTools: (v: PensamentoTools) => void;
  agrupar: GroupBy;
  setAgrupar: (v: GroupBy) => void;
  fundo: Fundo;
  setFundo: (v: Fundo) => void;
  imagemUri: string | null;
  /** Copia a foto escolhida pro diretório do app (a uri do picker é temporária) e liga o fundo `image`. */
  setImagemUri: (uri: string | null) => Promise<void>;
  panelAlpha: number;
  setPanelAlpha: (v: number) => void;
  surfaceAlpha: number;
  setSurfaceAlpha: (v: number) => void;
  acento: string | null;
  setAcento: (v: string | null) => void;
}

export const useAparencia = create<Aparencia>((set, get) => {
  // Um só caminho pros três valores que vivem no tema: grava, guarda no estado e reaplica.
  const material = (patch: Partial<Pick<Aparencia, 'panelAlpha' | 'surfaceAlpha' | 'acento'>>) => {
    set(patch);
    aplicarMaterial(get());
  };
  return {
    tema: ler(),
    // aplicar primeiro: se o Unistyles falhar, nada persiste — senão o cold start seguinte repete a falha.
    setTema: (t) => { aplicar(t); prefs.set(K, t); set({ tema: t }); },
    pensamentoTools: lerPensamento(),
    setPensamentoTools: (v) => { prefs.set(K_PENSAMENTO, v); set({ pensamentoTools: v }); },
    agrupar: lerAgrupar(),
    setAgrupar: (v) => { prefs.set(K_AGRUPAR, v); set({ agrupar: v }); },
    fundo: lerFundo(),
    setFundo: (v) => { prefs.set(K_FUNDO, v); set({ fundo: v }); },
    imagemUri: prefs.getString(K_IMAGEM) ?? null,
    setImagemUri: async (uri) => {
      if (!uri) {
        prefs.remove(K_IMAGEM);
        prefs.set(K_FUNDO, 'flat');
        set({ imagemUri: null, fundo: 'flat' });
        return;
      }
      const anterior = get().imagemUri;
      try {
        const destino = await copiarPapelDeParede(uri, anterior);
        prefs.set(K_IMAGEM, destino);
        prefs.set(K_FUNDO, 'image');
        set({ imagemUri: destino, fundo: 'image' });
      } catch {
        // Imports tardios: o store roda em teste de node, e Toast + mensagens arrastam a UI inteira.
        const [{ toast }, m] = await Promise.all([import('../ui/Toast'), import('../paraglide/messages')]);
        toast.erro(m.aparencia_fundo_erro_copia());
      }
    },
    panelAlpha: lerAlpha(K_PANEL, PANEL_PADRAO, PANEL_MIN),
    setPanelAlpha: (v) => { const n = faixa(v, PANEL_MIN); prefs.set(K_PANEL, n); material({ panelAlpha: n }); },
    surfaceAlpha: lerAlpha(K_SURFACE, 1, 0),
    setSurfaceAlpha: (v) => { const n = faixa(v, 0); prefs.set(K_SURFACE, n); material({ surfaceAlpha: n }); },
    acento: lerAcento(),
    setAcento: (v) => {
      const hex = v && hexParaRgb(v) ? v : null;
      if (hex) prefs.set(K_ACENTO, hex); else prefs.remove(K_ACENTO);
      material({ acento: hex });
    },
  };
});

// Nome novo a cada troca: o expo-image cacheia por uri, e regravar o mesmo caminho mostraria a foto antiga.
async function copiarPapelDeParede(uri: string, anterior: string | null): Promise<string> {
  const FileSystem = await import('expo-file-system/legacy');
  const destino = `${FileSystem.documentDirectory}wallpaper-${Date.now()}.jpg`;
  await FileSystem.copyAsync({ from: uri, to: destino });
  if (anterior) await FileSystem.deleteAsync(anterior, { idempotent: true }).catch(() => {});
  return destino;
}

export function aplicarTemaSalvo() { aplicar(useAparencia.getState().tema); }

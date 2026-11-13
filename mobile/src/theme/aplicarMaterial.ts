import { UnistylesRuntime } from 'react-native-unistyles';
import { themeDark, themeLight } from '@hangar/core';
import { tokensDe, type Paleta } from './paleta';
import type { Fundo, Leitura } from '../stores/aparencia';

/** Texto da conversa (Aparência › Texto da conversa): escalas sobre o tamanho e a entrelinha de fábrica. */
export interface Conversa {
  texto: number;
  linha: number;
  /** Tamanho do código, em px. */
  codigo: number;
  /** Largura da coluna, 0.5–1: no celular só estreitar faz diferença. */
  coluna: number;
}
// Código em 14: o tamanho que o app já usava (o Rust parte de 12,5, que é o do Compacto).
export const CONVERSA_PADRAO: Conversa = { texto: 1, linha: 1, codigo: 14, coluna: 1 };

export interface Material {
  panelAlpha: number;
  surfaceAlpha: number;
  acento: string | null;
  paleta?: Paleta;
  tinta?: number;
  forcaTinta?: number;
  /** Contraste da Leitura Texto: só pinta quando a leitura em vigor é Texto. */
  contraste?: number;
  leitura?: Leitura;
  fundo?: Fundo;
  imagemUri?: string | null;
  conversa?: Conversa;
}

/** Foto escolhida atrás das caixas (`busy_background` do Rust). */
export function hasImageBackground(s: { fundo?: Fundo; imagemUri?: string | null }): boolean {
  return s.fundo === 'image' && !!s.imagemUri;
}

/** Leitura em vigor (`effective_reading` do Rust): a Automática vira Texto só sobre imagem. */
export function leituraEmVigor(s: { leitura?: Leitura; fundo?: Fundo; imagemUri?: string | null }): Exclude<Leitura, 'auto'> {
  const l = s.leitura ?? 'auto';
  if (l !== 'auto') return l;
  return hasImageBackground(s) ? 'text' : 'none';
}

// Deriva SEMPRE dos tokens de fábrica, nunca do tema atual: acumular acento sobre acento faria a
// cor derivar a cada troca, e tirar o acento não teria como voltar ao original.
// `reduzir` = "Reduzir transparência" do sistema: cola os dois alphas em 1 e some com o vidro de
// tudo de uma vez. Vem por argumento, não do estado, porque não é preferência do app e não persiste.
export function aplicarMaterial(s: Material, { reduzir = false, escalaTexto = 1 }: { reduzir?: boolean; escalaTexto?: number } = {}) {
  const conversa = s.conversa ?? CONVERSA_PADRAO;
  const cores = {
    paleta: s.paleta ?? 'classic',
    acento: s.acento,
    tinta: s.tinta ?? 0,
    forcaTinta: s.forcaTinta ?? 0.4,
    contraste: leituraEmVigor(s) === 'text' ? s.contraste ?? 0.3 : null,
  };
  for (const [nome, base] of [['light', themeLight], ['dark', themeDark]] as const) {
    UnistylesRuntime.updateTheme(nome, (t) => ({
      ...t,
      tokens: tokensDe(base, nome, cores),
      panelAlpha: reduzir ? 1 : s.panelAlpha,
      surfaceAlpha: reduzir ? 1 : s.surfaceAlpha,
      conversa: { ...conversa, texto: conversa.texto * escalaTexto },
    }));
  }
}

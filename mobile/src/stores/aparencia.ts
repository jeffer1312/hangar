import { create } from 'zustand';
import { AccessibilityInfo } from 'react-native';
import { UnistylesRuntime } from 'react-native-unistyles';
import {
  PENSAMENTO_TOOLS,
  hexParaRgb,
  isBackgroundEffect,
  type BackgroundEffect,
  type PensamentoTools,
  type GroupBy,
} from '@hangar/core';
import { prefs } from './prefs';
import { aplicarMaterial, CONVERSA_PADRAO, type Conversa } from '../theme/aplicarMaterial';
export { leituraEmVigor } from '../theme/aplicarMaterial';
import type { Paleta } from '../theme/paleta';

export type Tema = 'system' | 'light' | 'dark';
export type Fundo = 'flat' | 'texture' | 'aurora' | 'image';
export type Idioma = 'system' | 'pt' | 'en';
/** Leitura do Rust: o que segura o texto da conversa sobre o fundo. */
export type Leitura = 'auto' | 'none' | 'text' | 'sheet';
/** Como a chamada de ferramenta aparece na conversa (ToolLook do Rust). */
export type Ferramentas = 'classic' | 'chips' | 'tree';
export type DestaquePergunta = 'accent' | 'amber';
/** Gaveta da tela inicial: sessões agrupadas ou conversas (vivas e fechadas) por recência. */
export type Organizar = 'sessions' | 'conversations';
export type HomeScreen = 'sessions' | 'newConversation';
const TEMAS: Tema[] = ['system', 'light', 'dark'];
const FUNDOS: Fundo[] = ['flat', 'texture', 'aurora', 'image'];
const IDIOMAS: Idioma[] = ['system', 'pt', 'en'];
const PALETAS: Paleta[] = ['classic', 'neutral'];
const LEITURAS: Leitura[] = ['auto', 'none', 'text', 'sheet'];
const FERRAMENTAS: Ferramentas[] = ['classic', 'chips', 'tree'];
const DESTAQUES: DestaquePergunta[] = ['accent', 'amber'];
const AGRUPAR: GroupBy[] = ['none', 'server', 'project'];
const ORGANIZAR: Organizar[] = ['sessions', 'conversations'];
const HOME_SCREENS: HomeScreen[] = ['sessions', 'newConversation'];
const K_HOME_SCREEN = 'aparencia.homeScreen';
const K = 'aparencia.tema';
const K_IDIOMA = 'aparencia.idioma';
const K_PENSAMENTO = 'aparencia.pensamentoTools';
const K_AGRUPAR = 'lista.agrupar';
const K_ORGANIZAR = 'lista.organizar';
const K_FUNDO = 'aparencia.fundo';
const K_IMAGEM = 'aparencia.imagemUri';
const K_PANEL = 'aparencia.panelAlpha';
const K_SURFACE = 'aparencia.surfaceAlpha';
const K_ACENTO = 'aparencia.acento';
const K_EFEITO = 'aparencia.efeito';
const K_PALETA = 'aparencia.paleta';
const K_TINTA = 'aparencia.tinta';
const K_FORCA_TINTA = 'aparencia.forcaTinta';
const K_LEITURA = 'aparencia.leitura';
const K_SOLIDEZ_FOLHA = 'aparencia.solidezFolha';
const K_CONTRASTE = 'aparencia.contraste';
const K_CONVERSA = 'aparencia.conversa';
const K_FERRAMENTAS = 'aparencia.ferramentas';
const K_TAREFAS = 'aparencia.tarefas';
const K_GRAFICO = 'aparencia.graficoTabela';
const K_DESTAQUE_PERGUNTA = 'aparencia.destaquePergunta';

// Padrões do Rust (appearance.rs DEFAULT), salvo onde o celular já tinha comportamento próprio:
// Árvore era o único desenho das ferramentas aqui e o botão Gráfico sempre aparecia.
const FORCA_TINTA_PADRAO = 0.4;
const SOLIDEZ_FOLHA_PADRAO = 0.6;
const CONTRASTE_PADRAO = 0.3;
/** "Estilo: Compacto" do Rust, na parte que existe no celular (sem fonte, coluna nem barra lateral). */
export const CONVERSA_COMPACTA: Conversa = { texto: 1, linha: 1.08, codigo: 12.5, coluna: 0.94 };

// Piso do painel: abaixo de 0.3 o texto do header/composer deixa de ter contraste sobre foto clara.
const PANEL_MIN = 0.3;
const PANEL_PADRAO = 0.86;
// NaN passa por Math.max/min sem virar nada: sem o Number.isFinite um slider quebrado escreveria
// NaN no tema e todo rgba() do app viraria cor inválida.
const faixa = (v: number, min: number, padrao: number) =>
  Number.isFinite(v) ? Math.max(min, Math.min(1, v)) : padrao;

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

function lerIdioma(): Idioma {
  const v = prefs.getString(K_IDIOMA) as Idioma | undefined;
  return v && IDIOMAS.includes(v) ? v : 'system';
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
  return typeof v === 'number' ? faixa(v, min, padrao) : padrao;
}

// Valor desconhecido vira `none`, como o `#[serde(other)]` do Rust.
function lerEfeito(): BackgroundEffect {
  const v = prefs.getString(K_EFEITO);
  return isBackgroundEffect(v) ? v : 'none';
}

function lerAcento(): string | null {
  const v = prefs.getString(K_ACENTO);
  return v && hexParaRgb(v) ? v : null;
}

function lerOpcao<T extends string>(k: string, lista: readonly T[], padrao: T): T {
  const v = prefs.getString(k) as T | undefined;
  return v && lista.includes(v) ? v : padrao;
}

function lerFaixa(k: string, min: number, max: number, padrao: number): number {
  const v = prefs.getNumber(k);
  return typeof v === 'number' && Number.isFinite(v) ? Math.max(min, Math.min(max, v)) : padrao;
}

function lerBool(k: string, padrao: boolean): boolean {
  const v = prefs.getBoolean(k);
  return typeof v === 'boolean' ? v : padrao;
}

// Valor fora da escala (arquivo de outra versão, slider quebrado) volta para dentro dela, como o `clamped` do Rust.
function conversaValida(v: Partial<Conversa> | null | undefined): Conversa {
  const f = (x: unknown, min: number, max: number, padrao: number) =>
    typeof x === 'number' && Number.isFinite(x) ? Math.max(min, Math.min(max, x)) : padrao;
  return {
    texto: f(v?.texto, 0.5, 1.5, CONVERSA_PADRAO.texto),
    linha: f(v?.linha, 0.5, 1.5, CONVERSA_PADRAO.linha),
    codigo: f(v?.codigo, 8, 24, CONVERSA_PADRAO.codigo),
    coluna: f(v?.coluna, 0.5, 1, CONVERSA_PADRAO.coluna),
  };
}

function lerConversa(): Conversa {
  try {
    const v = prefs.getString(K_CONVERSA);
    return conversaValida(v ? JSON.parse(v) : null);
  } catch {
    return CONVERSA_PADRAO;
  }
}

interface Aparencia {
  tema: Tema;
  setTema: (t: Tema) => void;
  /** `system` segue o idioma do aparelho; pt/en é escolha manual e vence o sistema. */
  idioma: Idioma;
  setIdioma: (v: Idioma) => void;
  pensamentoTools: PensamentoTools;
  setPensamentoTools: (v: PensamentoTools) => void;
  agrupar: GroupBy;
  setAgrupar: (v: GroupBy) => void;
  organizar: Organizar;
  setOrganizar: (v: Organizar) => void;
  homeScreen: HomeScreen;
  setHomeScreen: (v: HomeScreen) => void;
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
  /** Efeito sobre a imagem de fundo (effects.rs do desktop). */
  efeito: BackgroundEffect;
  setEfeito: (v: BackgroundEffect) => void;
  /** Última variante pronta no cache; não persiste, o arquivo é que fica. */
  efeitoImagem: { fonte: string; efeito: BackgroundEffect; claro: boolean; uri: string } | null;
  efeitoProcessando: boolean;
  paleta: Paleta;
  setPaleta: (v: Paleta) => void;
  /** Índice em TINTAS (theme/paleta.ts); 0 = sem tinta. */
  tinta: number;
  setTinta: (v: number) => void;
  forcaTinta: number;
  setForcaTinta: (v: number) => void;
  leitura: Leitura;
  setLeitura: (v: Leitura) => void;
  solidezFolha: number;
  setSolidezFolha: (v: number) => void;
  contraste: number;
  setContraste: (v: number) => void;
  conversa: Conversa;
  setConversa: (v: Partial<Conversa>) => void;
  ferramentas: Ferramentas;
  setFerramentas: (v: Ferramentas) => void;
  tarefas: boolean;
  setTarefas: (v: boolean) => void;
  graficoTabela: boolean;
  setGraficoTabela: (v: boolean) => void;
  destaquePergunta: DestaquePergunta;
  setDestaquePergunta: (v: DestaquePergunta) => void;
  /** "Estilo: Compacto": texto, entrelinha, código, coluna e ferramentas em Árvore. */
  aplicarCompacto: () => void;
  /** "Voltar ao padrão" do Rust: não mexe em tema, paleta, fundo nem no jeito da conversa. */
  redefinir: () => void;
}

// "Reduzir transparência" do sistema não é preferência do app: não persiste, e vale por cima do
// que estiver gravado. Mora aqui, e não em cada componente, porque quem apaga o vidro é o TEMA.
let reduzirTransparencia = false;
AccessibilityInfo.isReduceTransparencyEnabled()
  .then((v) => { reduzirTransparencia = v; if (v) reaplicarMaterial(); })
  .catch(() => {});
AccessibilityInfo.addEventListener('reduceTransparencyChanged', (v) => {
  reduzirTransparencia = v;
  reaplicarMaterial();
});

function reaplicarMaterial() {
  aplicarMaterial(useAparencia.getState(), { reduzir: reduzirTransparencia });
}

export const useAparencia = create<Aparencia>((set, get) => {
  // Um apply por tick: arrastar um slider dispara um setter por quadro, e cada apply é uma escrita
  // no MMKV mais dois updateTheme (re-render da árvore inteira). O estado muda na hora — a tela
  // responde —, mas a preferência só desce pro disco DEPOIS de o tema aceitar o valor: apply que
  // levanta deixa a fila intacta pro tick seguinte, em vez de persistir o que não pintou.
  const aGravar: Array<() => void> = [];
  let agendado: ReturnType<typeof setTimeout> | null = null;
  const material = (patch: Partial<Aparencia>, gravar: () => void) => {
    set(patch);
    aGravar.push(gravar);
    if (agendado) return;
    agendado = setTimeout(() => {
      agendado = null;
      aplicarMaterial(get(), { reduzir: reduzirTransparencia });
      for (const f of aGravar.splice(0)) f();
    }, 0);
  };
  return {
    tema: ler(),
    // aplicar primeiro: se o Unistyles falhar, nada persiste — senão o cold start seguinte repete a falha.
    setTema: (t) => { aplicar(t); prefs.set(K, t); set({ tema: t }); },
    idioma: lerIdioma(),
    // Só grava: as mensagens do paraglide são funções compiladas na carga, então quem troca de
    // idioma precisa recarregar o app — a tela avisa.
    setIdioma: (v) => { prefs.set(K_IDIOMA, v); set({ idioma: v }); },
    pensamentoTools: lerPensamento(),
    setPensamentoTools: (v) => { prefs.set(K_PENSAMENTO, v); set({ pensamentoTools: v }); },
    agrupar: lerAgrupar(),
    setAgrupar: (v) => { prefs.set(K_AGRUPAR, v); set({ agrupar: v }); },
    organizar: lerOpcao(K_ORGANIZAR, ORGANIZAR, 'sessions'),
    setOrganizar: (v) => { prefs.set(K_ORGANIZAR, v); set({ organizar: v }); },
    homeScreen: lerOpcao(K_HOME_SCREEN, HOME_SCREENS, 'sessions'),
    setHomeScreen: (v) => { prefs.set(K_HOME_SCREEN, v); set({ homeScreen: v }); },
    fundo: lerFundo(),
    // Passa pelo tema: com a Leitura Automática, imagem atrás liga o contraste do Texto.
    setFundo: (v) => material({ fundo: v }, () => prefs.set(K_FUNDO, v)),
    imagemUri: papelNaPastaAtual(prefs.getString(K_IMAGEM) ?? null),
    setImagemUri: async (uri) => {
      if (!uri) {
        material({ imagemUri: null, fundo: 'flat' }, () => { prefs.remove(K_IMAGEM); prefs.set(K_FUNDO, 'flat'); });
        return;
      }
      const anterior = get().imagemUri;
      try {
        const destino = await copiarPapelDeParede(uri, anterior);
        material({ imagemUri: destino, fundo: 'image' }, () => { prefs.set(K_IMAGEM, destino); prefs.set(K_FUNDO, 'image'); });
      } catch {
        // Imports tardios: o store roda em teste de node, e Toast + mensagens arrastam a UI inteira.
        const [{ toast }, m] = await Promise.all([import('../ui/Toast'), import('../paraglide/messages')]);
        toast.erro(m.aparencia_fundo_erro_copia());
      }
    },
    panelAlpha: lerAlpha(K_PANEL, PANEL_PADRAO, PANEL_MIN),
    setPanelAlpha: (v) => {
      const n = faixa(v, PANEL_MIN, PANEL_PADRAO);
      material({ panelAlpha: n }, () => prefs.set(K_PANEL, n));
    },
    surfaceAlpha: lerAlpha(K_SURFACE, 1, 0),
    setSurfaceAlpha: (v) => {
      const n = faixa(v, 0, 1);
      material({ surfaceAlpha: n }, () => prefs.set(K_SURFACE, n));
    },
    acento: lerAcento(),
    setAcento: (v) => {
      const hex = v && hexParaRgb(v) ? v : null;
      material({ acento: hex }, () => { if (hex) prefs.set(K_ACENTO, hex); else prefs.remove(K_ACENTO); });
    },
    efeito: lerEfeito(),
    setEfeito: (v) => { prefs.set(K_EFEITO, v); set({ efeito: v }); },
    efeitoImagem: null,
    efeitoProcessando: false,
    paleta: lerOpcao(K_PALETA, PALETAS, 'classic'),
    setPaleta: (v) => material({ paleta: v }, () => prefs.set(K_PALETA, v)),
    tinta: lerFaixa(K_TINTA, 0, 3, 0),
    setTinta: (v) => {
      const n = Math.max(0, Math.min(3, Math.round(v)));
      material({ tinta: n }, () => prefs.set(K_TINTA, n));
    },
    forcaTinta: lerFaixa(K_FORCA_TINTA, 0.05, 1, FORCA_TINTA_PADRAO),
    setForcaTinta: (v) => {
      const n = faixa(v, 0.05, FORCA_TINTA_PADRAO);
      material({ forcaTinta: n }, () => prefs.set(K_FORCA_TINTA, n));
    },
    leitura: lerOpcao(K_LEITURA, LEITURAS, 'auto'),
    setLeitura: (v) => material({ leitura: v }, () => prefs.set(K_LEITURA, v)),
    solidezFolha: lerFaixa(K_SOLIDEZ_FOLHA, 0, 1, SOLIDEZ_FOLHA_PADRAO),
    setSolidezFolha: (v) => {
      const n = faixa(v, 0, SOLIDEZ_FOLHA_PADRAO);
      prefs.set(K_SOLIDEZ_FOLHA, n);
      set({ solidezFolha: n });
    },
    contraste: lerFaixa(K_CONTRASTE, 0, 1, CONTRASTE_PADRAO),
    setContraste: (v) => {
      const n = faixa(v, 0, CONTRASTE_PADRAO);
      material({ contraste: n }, () => prefs.set(K_CONTRASTE, n));
    },
    conversa: lerConversa(),
    setConversa: (v) => {
      const n = conversaValida({ ...get().conversa, ...v });
      material({ conversa: n }, () => prefs.set(K_CONVERSA, JSON.stringify(n)));
    },
    ferramentas: lerOpcao(K_FERRAMENTAS, FERRAMENTAS, 'tree'),
    setFerramentas: (v) => { prefs.set(K_FERRAMENTAS, v); set({ ferramentas: v }); },
    tarefas: lerBool(K_TAREFAS, false),
    setTarefas: (v) => { prefs.set(K_TAREFAS, v); set({ tarefas: v }); },
    graficoTabela: lerBool(K_GRAFICO, true),
    setGraficoTabela: (v) => { prefs.set(K_GRAFICO, v); set({ graficoTabela: v }); },
    destaquePergunta: lerOpcao(K_DESTAQUE_PERGUNTA, DESTAQUES, 'accent'),
    setDestaquePergunta: (v) => { prefs.set(K_DESTAQUE_PERGUNTA, v); set({ destaquePergunta: v }); },
    aplicarCompacto: () => {
      get().setFerramentas('tree');
      get().setConversa(CONVERSA_COMPACTA);
    },
    redefinir: () => {
      const s = get();
      s.setAcento(null);
      s.setTinta(0);
      s.setForcaTinta(FORCA_TINTA_PADRAO);
      s.setPanelAlpha(PANEL_PADRAO);
      s.setSurfaceAlpha(1);
      s.setLeitura('auto');
      s.setSolidezFolha(SOLIDEZ_FOLHA_PADRAO);
      s.setContraste(CONTRASTE_PADRAO);
      s.setConversa(CONVERSA_PADRAO);
      s.setDestaquePergunta('accent');
    },
  };
});

// O iOS troca a pasta do app ao reinstalar: o caminho salvo aponta pra uma pasta que não existe mais.
export function papelNaPastaAtual(salvo: string | null, pasta?: string | null): string | null {
  const nome = salvo?.match(/\/(wallpaper-\d+\.jpg)$/)?.[1];
  if (!nome) return salvo;
  let atual = pasta;
  // ponytail: require síncrono só no app; no teste de node o módulo nativo não carrega e o caminho fica como está.
  if (atual === undefined) {
    try { atual = (require('expo-file-system/legacy') as { documentDirectory: string | null }).documentDirectory; }
    catch (err) {
      if (typeof __DEV__ !== 'undefined') console.warn('wallpaper-folder-unavailable', err);
      atual = null;
    }
  }
  return atual ? `${atual}${nome}` : salvo;
}

// Nome novo a cada troca: o expo-image cacheia por uri, e regravar o mesmo caminho mostraria a foto antiga.
async function copiarPapelDeParede(uri: string, anterior: string | null): Promise<string> {
  const FileSystem = await import('expo-file-system/legacy');
  const destino = `${FileSystem.documentDirectory}wallpaper-${Date.now()}.jpg`;
  await FileSystem.copyAsync({ from: uri, to: destino });
  if (anterior) await FileSystem.deleteAsync(anterior, { idempotent: true }).catch(() => {});
  return destino;
}

export function aplicarTemaSalvo() { aplicar(useAparencia.getState().tema); }

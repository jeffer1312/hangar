// As duas listas de máquinas — a do navegador (este aparelho acompanha) e a do servidor (peers,
// os servidores se falam) — são donos diferentes e continuam separadas. A tela mostra UMA linha
// por máquina, casando as duas pelo identificador: pela URL viravam duas, porque o celular
// conhece a máquina pelo IP da rede local e o servidor pelo Tailscale.
import type { Server } from './auth';
import type { PeerView } from './peers';
import type { LadoState } from './registrarPeerDoisLados';

// Por que o identificador do servidor não chegou. Sem isto "não responde" e "responde, mas está
// sem nome" viravam a MESMA frase, e as duas se consertam de jeito diferente: uma é a máquina
// fora do ar, a outra é um campo para preencher.
export type MotivoSemId = 'vazio' | 'sem_resposta' | 'token';

export interface LinhaMaquina {
  chave: string;
  nome: string;
  identificador: string | null;
  // Motivo da entrada principal. Ausente = ainda não se perguntou; 'vazio' = respondeu.
  motivoId?: MotivoSemId;
  // Entrada principal deste aparelho (a que responde, de preferência) e TODAS as entradas que
  // são a mesma máquina: guardar LAN + Tailscale da mesma máquina é uma linha só.
  navegador: Server | null;
  navegadores: Server[];
  motivos: Record<string, MotivoSemId>;
  peer: PeerView | null;
  estaMaquina: boolean;
}

export function unirMaquinas(
  servidores: Server[],
  ids: Record<string, string | null>,
  peers: PeerView[],
  escolhidoId: string | null,
  motivos: Record<string, MotivoSemId> = {},
): LinhaMaquina[] {
  const porId = new Map(peers.map((p) => [p.id, p]));
  const porHost = new Map(peers.map((p) => [hostDe(p.base_url), p]));
  // Chave da máquina: o identificador que ela respondeu (ou o último lembrado). Sem nenhum, o
  // endereço igual ao de um peer é a única pista; sem ela a entrada fica sozinha.
  const grupos = new Map<string, Server[]>();
  for (const s of servidores) {
    const chave = ids[s.id] ?? porHost.get(hostDe(s.baseUrl))?.id ?? `srv:${s.id}`;
    grupos.set(chave, [...(grupos.get(chave) ?? []), s]);
  }
  const usados = new Set<string>();
  const linhas: LinhaMaquina[] = [...grupos].map(([chave, grupo]) => {
    const identificador = chave.startsWith('srv:') ? null : chave;
    const peer = identificador ? porId.get(identificador) ?? null : null;
    if (peer) usados.add(peer.id);
    const principal = grupo.find((s) => s.id === escolhidoId)
      ?? grupo.find((s) => !s.disabled && motivos[s.id] === 'vazio') ?? grupo.find((s) => !s.disabled) ?? grupo[0];
    return {
      chave: `srv:${principal.id}`,
      nome: principal.label,
      identificador,
      motivoId: motivos[principal.id],
      navegador: principal,
      navegadores: grupo,
      motivos: Object.fromEntries(grupo.filter((s) => motivos[s.id]).map((s) => [s.id, motivos[s.id]])),
      peer,
      estaMaquina: grupo.some((s) => s.id === escolhidoId),
    };
  });
  for (const p of peers) {
    if (usados.has(p.id)) continue;
    linhas.push({ chave: `peer:${p.id}`, nome: p.id, identificador: p.id, navegador: null, navegadores: [], motivos: {}, peer: p, estaMaquina: false });
  }
  return linhas.sort((a, b) => Number(b.estaMaquina) - Number(a.estaMaquina) || a.nome.localeCompare(b.nome));
}

// Último identificador que cada entrada deste aparelho respondeu. Máquina desligada não responde
// o nome dela, e sem esta lembrança a mesma máquina fora do ar voltava a ser duas linhas.
const IDS_LEMBRADOS = 'cp_server_ids';
export function rememberedIds(): Record<string, string> {
  try {
    const v = JSON.parse(localStorage.getItem(IDS_LEMBRADOS) ?? '{}');
    return v && typeof v === 'object' ? v : {};
  } catch { return {}; }
}
export function rememberIds(ids: Record<string, string>): void {
  try { localStorage.setItem(IDS_LEMBRADOS, JSON.stringify(ids)); } catch { /* vale só nesta abertura */ }
}

// A linha da lista diz UMA coisa: se as sessões desta máquina aparecem neste aparelho.
export type SessionsState = 'aparecem' | 'testando' | 'nao_responde' | 'token_recusado' | 'sem_token' | 'desligada' | 'desligada_aqui';

export function sessionsState(linha: LinhaMaquina, st: EstadoPeer | undefined): SessionsState {
  if (linha.peer?.enabled === false) return 'desligada';
  if (linha.navegadores.length && linha.navegadores.every((s) => s.disabled)) return 'desligada_aqui';
  if (linha.navegador) {
    if (linha.motivoId === undefined) return 'testando';
    if (linha.motivoId === 'token') return 'token_recusado';
    return linha.motivoId === 'sem_resposta' ? 'nao_responde' : 'aparecem';
  }
  const ida = st?.lados.find((l) => l.lado === 'ida');
  if (!st || st.testando || !ida) return 'testando';
  return ida.estado === 'ok' ? 'sem_token' : 'nao_responde';
}

export const recolhida = (s: SessionsState) => s === 'nao_responde' || s === 'desligada';

// `em` = quando foi medido. Sem ele o resultado salvo passaria por medição de agora.
export interface EstadoPeer { lados: LadoState[]; ok: boolean; testando?: boolean; em?: number }

// Último resultado de cada máquina, salvo neste aparelho. A tela abre com ele e mede de novo por
// trás: sem isto ela ficava toda em "Testando…" até a máquina mais lenta responder.
const RESULTADOS = 'cp_machines_state';
interface Salvo { motivos: Record<string, MotivoSemId>; peers: Record<string, Record<string, EstadoPeer>> }
function lerSalvo(): Salvo {
  try {
    const v = JSON.parse(localStorage.getItem(RESULTADOS) ?? '{}');
    return { motivos: v?.motivos ?? {}, peers: v?.peers ?? {} };
  } catch { return { motivos: {}, peers: {} }; }
}
function gravarSalvo(s: Salvo): void {
  try { localStorage.setItem(RESULTADOS, JSON.stringify(s)); } catch { /* vale só nesta abertura */ }
}
export const savedMotivos = (): Record<string, MotivoSemId> => lerSalvo().motivos;
export const savedPeerStates = (alvo: string): Record<string, EstadoPeer> => lerSalvo().peers[alvo] ?? {};
export function saveMotivos(motivos: Record<string, MotivoSemId>): void {
  gravarSalvo({ ...lerSalvo(), motivos });
}
export function savePeerStates(alvo: string, estados: Record<string, EstadoPeer>): void {
  const s = lerSalvo();
  const medidos = Object.fromEntries(Object.entries(estados).filter(([, e]) => !e.testando && e.lados.length));
  gravarSalvo({ ...s, peers: { ...s.peers, [alvo]: medidos } });
}

export type TipoEstado =
  | 'desligada' | 'sem_identificador' | 'nao_responde' | 'token_aparelho_recusado'
  | 'testando' | 'token_recusado' | 'ida_outra_maquina' | 'volta_outra_maquina' | 'parcial'
  | 'volta_sem_medir' | 'volta_sem_registro' | 'ok' | 'neutro';

export interface EstadoDaLinha {
  farol: 'ok' | 'nao' | 'test' | 'neutro';
  tipo: TipoEstado;
  ida?: LadoState;
  volta?: LadoState;
}

const FALHA: LadoState['estado'][] = ['falhou', 'recusou', 'estranho'];

const SEM_ID: Record<MotivoSemId, TipoEstado> = {
  vazio: 'sem_identificador',
  sem_resposta: 'nao_responde',
  token: 'token_aparelho_recusado',
};

// Uma decisão só para a linha curta da lista e para o detalhe: se cada um derivasse o estado,
// a lista podia dizer "Tudo certo" com o detalhe mostrando a volta falhando.
export function estadoDaLinha(linha: LinhaMaquina, st: EstadoPeer | undefined): EstadoDaLinha {
  const ida = st?.lados.find((l) => l.lado === 'ida');
  const volta = st?.lados.find((l) => l.lado === 'volta');
  const falhaReal = !!st && !st.ok && [ida, volta].some((l) => !!l && FALHA.includes(l.estado));
  const desligada = linha.peer?.enabled === false;
  let farol: EstadoDaLinha['farol'];
  if (desligada) farol = 'neutro';
  else if (st?.testando) farol = 'test';
  else if (!linha.peer && !st) farol = 'neutro';   // só navegador: nada para testar
  else if (!st) farol = 'test';
  else if (st.ok) farol = 'ok';
  else farol = falhaReal ? 'nao' : 'test';       // nao_configurado não é falha
  let tipo: TipoEstado;
  if (desligada) tipo = 'desligada';
  else if (linha.navegador && !linha.identificador) tipo = SEM_ID[linha.motivoId ?? 'vazio'];
  else if (st?.testando) tipo = 'testando';
  else if (volta?.estado === 'recusou' && volta.motivo === 'credencial') tipo = 'token_recusado';
  // `estranho` é o endereço guardado respondendo como OUTRA máquina — o único modo de falha que
  // se conserta trocando o endereço, e não esperando a máquina voltar. Vem antes do `parcial`,
  // que é o balde genérico: senão ele o engole e a tela só diz "só de ida".
  else if (volta?.estado === 'estranho') tipo = 'volta_outra_maquina';
  else if (ida?.estado === 'estranho') tipo = 'ida_outra_maquina';
  else if (falhaReal) tipo = 'parcial';
  else if (volta?.estado === 'nao_configurado' && volta.motivo === 'token') tipo = 'volta_sem_medir';
  else if (volta?.estado === 'nao_configurado' && volta.motivo === 'registro') tipo = 'volta_sem_registro';
  else if (st?.ok) tipo = 'ok';
  else tipo = 'neutro';
  // Máquina que este aparelho segue e não responde (ou recusa o token dele) é falha, não espera:
  // sem `peer` não há medição nenhuma, e o farol neutro dizia "nada a relatar".
  if (tipo === 'nao_responde' || tipo === 'token_aparelho_recusado') farol = 'nao';
  return { farol, tipo, ida, volta };
}

// O resultado ÚNICO do interruptor de recados: qual cartão o detalhe mostra, cada um com o botão
// que resolve. Deriva de estadoDaLinha para a medição ter um dono só.
export type RecadosCard =
  | 'desligado' | 'sem_meu_id' | 'sem_id_dele' | 'sem_resposta' | 'token_recusado' | 'testando'
  | 'ok' | 'endereco_volta' | 'volta_outra' | 'ida_outra' | 'ida_falhou' | 'falta_token'
  | 'sem_registro' | 'pausada';

export function recadosCard(linha: LinhaMaquina, st: EstadoPeer | undefined, meuIdentificador: string): RecadosCard {
  if (!linha.peer) {
    if (!meuIdentificador) return 'sem_meu_id';
    if (!linha.identificador) {
      if (linha.motivoId === 'token') return 'token_recusado';
      return linha.motivoId === 'vazio' ? 'sem_id_dele' : 'sem_resposta';
    }
    return 'desligado';
  }
  const e = estadoDaLinha(linha, st);
  switch (e.tipo) {
    case 'desligada': return 'pausada';
    case 'ok': return 'ok';
    case 'token_recusado': return 'token_recusado';
    case 'volta_outra_maquina': return 'volta_outra';
    case 'ida_outra_maquina': return 'ida_outra';
    case 'parcial': return e.ida?.estado === 'ok' ? 'endereco_volta' : 'ida_falhou';
    case 'volta_sem_medir': return 'falta_token';
    case 'volta_sem_registro': return 'sem_registro';
    default: return 'testando';
  }
}

// Host E caminho: atrás de um proxy por caminho (pocket.exemplo/casa, pocket.exemplo/notebook)
// máquinas diferentes dividem o mesmo host.
function hostDe(url: string): string {
  try {
    const u = new URL(url);
    return (u.host + u.pathname.replace(/\/+$/, '')).toLowerCase();
  } catch { return url.trim().replace(/\/+$/, '').toLowerCase(); }
}

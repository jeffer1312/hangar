// Registro dos DOIS lados de um peer (Task 8): um gesto na aba Servidores registra A em B E B em A,
// usando a credencial que o celular já guarda de cada um. Lógica pura, testada sozinha (precedente:
// lib/sessions.test.ts) — a tela só pinta o resultado.
//
// Contrato do "o que mostra quando falha": cada lado tem um estado NOMEADO, nunca um erro genérico.
// Um lado falhar não pode deixar o outro registrado em silêncio — o par completo só vira sucesso
// quando os dois gravaram E os dois testes passaram.
import type { Server } from './auth';
import { getBaseUrl, getToken, listAllServers } from './auth';
import { checkPeer, getIdentificador, gravarPeer } from './peers';
import { alcanceDoServidor } from './alcance';

export interface LadoState {
  lado: 'ida' | 'volta';
  estado: 'ok' | 'estranho' | 'falhou' | 'recusou' | 'nao_configurado';
  motivo?: string;
  // Quem atendeu no endereço testado. Só interessa no `estranho`: é o nome da máquina ERRADA,
  // e sem ele a tela diz que algo está torto sem dizer o que atendeu.
  identificador?: string;
  // Endereço medido. Na volta é o que o PEER guardou desta máquina — não dá para deduzir da
  // linha, e é justamente o que o bloco de correção conserta.
  url?: string;
  tempo_ms?: number | null;
}

export interface RegistroPeerResult {
  ok: boolean;
  id: string;
  base_url: string;
  // Estados dos dois lados; sempre presente (vazio = nenhum lado chegou a rodar).
  lados: LadoState[];
  // Endereço alternativo pedido: preenchido quando um lado falhou (o bloco de correção).
  endereco_alternativo?: string;
  // O endereço do DONO gravado no peer — o que o bloco de correção edita. Devolvido porque pode
  // não ser a URL que este navegador usa (ver `enderecoDoDono`).
  meu_endereco: string;
}

/** Faz a gravação E o teste dos dois lados. `dono` é o servidor que a aba está editando;
 *  `alvo` é o servidor registrado (o peer). */
export async function registrarPeerDoisLados(
  dono: Server | null,
  alvo: { id: string; base_url: string; token: string },
  // Endereço do DONO a gravar no peer. Vem preenchido do bloco de correção ("qual endereço o X
  // deve usar para chegar aqui?"); vazio, é resolvido por `enderecoDoDono`.
  meuEndereco = '',
): Promise<RegistroPeerResult> {
  // 1) Grava o peer no DONO (o lado que a aba edita): o vínculo local. `dono` null = modo
  //    global — o cliente `gravarPeer(null, …)` usa o servidor ATIVO (mesmo contrato do
  //    resto da aba). Sem servidor nenhum cadastrado a gravação falha e o erro é nomeado.
  try {
    await gravarPeer(dono, {
      id: alvo.id,
      base_url: alvo.base_url,
      token: alvo.token,
    });
  } catch (e) {
    return {
      ok: false,
      id: alvo.id,
      base_url: alvo.base_url,
      meu_endereco: meuEndereco || (dono?.baseUrl ?? getBaseUrl()),
      lados: [{ lado: 'ida', estado: 'falhou', motivo: String((e as Error)?.message ?? e) }],
    };
  }

  // 2) Resolve a identidade do DONO — o que o peer precisa guardar para chamar esta máquina
  //    de volta: o nome REAL do backend (CP_SERVER_ID), não o rótulo local do celular.
  // O id tem que ser o da lista: é por ele que a resposta tira a marca de offline do servidor.
  const norm = (u: string) => u.replace(/\/+$/, '');
  const naLista = listAllServers().find((s) => norm(s.baseUrl) === norm(alvo.base_url));
  const remoto: Server = { id: naLista?.id ?? alvo.id, label: alvo.id, baseUrl: alvo.base_url, token: alvo.token };
  const meuToken = dono?.token ?? getToken() ?? '';
  const meuBase = meuEndereco || (await enderecoDoDono(dono));
  let meuId = '';
  try { meuId = (await getIdentificador(dono)).identificador ?? ''; } catch { meuId = ''; }

  // 3) Sem o identificador do DONO (servidor sem CP_SERVER_ID) nada é gravado no peer — gravar
  //    com id vazio tomaria 400 do validar_id — e a volta vira estado nomeado (não é defeito).
  if (meuId) {
    try {
      await gravarPeer(remoto, { id: meuId, base_url: meuBase, token: meuToken });
    } catch {
      // gravação do lado do peer falhou: o estado real sai da checagem abaixo
    }
  }

  // 4) Testa os dois lados. A ida pergunta ao DONO sobre o ALVO; a volta pergunta ao PEER sobre
  //    o DONO (o espelho do que acabamos de gravar — quem responde é a máquina de cá).
  //    `dono` null = modo global: a checagem da ida usa o servidor ATIVO (cliente com alvo null).
  const lados: LadoState[] = [];
  const ida = await checkPeer(dono, alvo.base_url, alvo.id).catch((e) => ({
    estado: 'falhou' as const,
    motivo: String((e as Error)?.message ?? e),
  }));
  lados.push({ lado: 'ida', ...ida });
  if (meuId) {
    // O lado remoto fala com o servidor EXPLÍCITO do peer (a credencial guardada do par) — o
    // mesmo contrato de apiFetchForServer: um 401 daqui não pode derrubar a credencial ativa.
    const volta = await checkPeer(remoto, meuBase, meuId).catch((e) => ({
      estado: 'falhou' as const,
      motivo: String((e as Error)?.message ?? e),
    }));
    lados.push({ lado: 'volta', ...volta });
  } else {
    lados.push({ lado: 'volta', estado: 'nao_configurado', motivo: 'identificador' });
  }

  const ok = lados.every((l) => l.estado === 'ok');
  return { ok, id: alvo.id, base_url: alvo.base_url, meu_endereco: meuBase, lados };
}

// `0.0.0.0` entra junto do loopback: gravado num peer ele também resolve para a máquina que leu o
// endereço, e não para quem o escreveu.
const LOOPBACK = /^(localhost|127(\.\d+){3}|0\.0\.0\.0|\[?::1\]?)$/i;

function ehLoopback(url: string): boolean {
  try { return LOOPBACK.test(new URL(url).hostname); } catch { return false; }
}

// O peer guarda um endereço de API; `/api/alcance` devolve os endereços da TELA (porta do front,
// que é `front_port or port`). Onde as duas diferem, o candidato aponta para o front e o peer bate
// no Vite. Candidato sem porta é proxy (Tailscale/público) e serve os dois — esse passa.
function serveDeApi(candidato: string, base: string): boolean {
  try {
    const c = new URL(candidato).port;
    return !c || c === new URL(base).port;
  } catch { return false; }
}

/** Endereço do DONO para o PEER guardar — o que a volta vai bater.
 *
 *  O padrão continua sendo a URL que este navegador usa: ela é a única medida de verdade que o
 *  aparelho tem. A exceção é o loopback, e ela não é uma heurística: `http://127.0.0.1:8765`
 *  gravado no peer aponta para o PRÓPRIO peer, então a volta bate nele mesmo, volta com o
 *  identificador dele e o par nunca fecha — com o agravante de parecer registrado. Nesse caso
 *  quem responde é o servidor, por `/api/alcance`: ele mediu por quais endereços chegam nele. */
async function enderecoDoDono(dono: Server | null): Promise<string> {
  const base = dono?.baseUrl ?? getBaseUrl();
  if (!ehLoopback(base)) return base;
  const token = dono?.token ?? getToken() ?? '';
  try {
    const alcance = await alcanceDoServidor(dono ?? { id: '', label: '', baseUrl: base, token });
    const melhor = alcance.enderecos
      .filter((e) => e.estado === 'ok' && e.tipo !== 'nesta_maquina' && !ehLoopback(e.url) && serveDeApi(e.url, base))
      .sort((a, b) => (a.tempo_ms ?? 0) - (b.tempo_ms ?? 0))[0];
    return melhor?.url ?? base;
  } catch {
    // Servidor sem a rota ou fora do ar: grava o que se sabe. A volta vira estado nomeado na
    // tela — melhor que recusar o registro inteiro por causa de uma medição.
    return base;
  }
}

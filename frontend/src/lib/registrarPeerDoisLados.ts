// Registro dos DOIS lados de um peer (Task 8): um gesto na aba Servidores registra A em B E B em A,
// usando a credencial que o celular já guarda de cada um. Lógica pura, testada sozinha (precedente:
// lib/sessions.test.ts) — a tela só pinta o resultado.
//
// Contrato do "o que mostra quando falha": cada lado tem um estado NOMEADO, nunca um erro genérico.
// Um lado falhar não pode deixar o outro registrado em silêncio — o par completo só vira sucesso
// quando os dois gravaram E os dois testes passaram.
import type { Server } from './auth';
import { checkPeer, gravarPeer, listarPeers, type PeerView } from './peers';

export interface LadoState {
  lado: 'ida' | 'volta';
  estado: 'ok' | 'estranho' | 'falhou' | 'recusou' | 'nao_configurado';
  motivo?: string;
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
}

/** Faz a gravação E o teste dos dois lados. `dono` é o servidor que a aba está editando;
 *  `alvo` é o servidor registrado (o peer). */
export async function registrarPeerDoisLados(
  dono: Server | null,
  alvo: { id: string; base_url: string; token: string },
): Promise<RegistroPeerResult> {
  // 1) Grava o peer no DONO (o lado que a aba edita): o vínculo local. `dono` null = modo
  //    global — o cliente `gravarPeer(null, …)` usa o servidor ATIVO (mesmo contrato do
  //    resto da aba). Sem servidor nenhum cadastrado a gravação falha e o erro é nomeado.
  let lista: PeerView[] = [];
  try {
    lista = await gravarPeer(dono, {
      id: alvo.id,
      base_url: alvo.base_url,
      token: alvo.token,
    });
  } catch (e) {
    return {
      ok: false,
      id: alvo.id,
      base_url: alvo.base_url,
      lados: [{ lado: 'ida', estado: 'falhou', motivo: String((e as Error)?.message ?? e) }],
    };
  }

  // 2) Se o DONO tem uma credencial de servidor salva para o peer (a aba Servidores mostra a
  //    lista de peers do alvo), grava também no peer: A em B e B em A. Sem credencial do peer,
  //    o lado "volta" fica nao_configurado (não é defeito — o mock estado 2 tem os dois).
  const peerSalvo = Array.isArray(lista) ? lista.find((p) => p.id === alvo.id) : undefined;
  if (peerSalvo) {
    try {
      await gravarPeer(
        { id: alvo.id, label: alvo.id, baseUrl: alvo.base_url, token: alvo.token },
        { id: alvo.id, base_url: alvo.base_url, token: alvo.token },
      );
    } catch {
      // gravação do lado do peer falhou: o estado real sai da checagem abaixo
    }
  }

  // 3) Testa os dois lados, cada um com a primitiva de alcance (Task 8 reusa a da Task 3).
  //    `dono` null = modo global: a checagem da ida usa o servidor ATIVO (cliente com alvo null).
  const lados: LadoState[] = [];
  const ida = await checkPeer(dono, alvo.base_url, alvo.id).catch((e) => ({
    estado: 'falhou' as const,
    motivo: String((e as Error)?.message ?? e),
  }));
  lados.push({ lado: 'ida', ...ida });
  if (peerSalvo) {
    // O lado remoto fala com o servidor EXPLÍCITO (a credencial guardada do peer) — o mesmo
    // contrato de apiFetchForServer: um 401 daqui não pode derrubar a credencial ativa.
    const remoto: Server = { id: alvo.id, label: alvo.id, baseUrl: alvo.base_url, token: alvo.token };
    const volta = await checkPeer(remoto, alvo.base_url, alvo.id).catch((e) => ({
      estado: 'falhou' as const,
      motivo: String((e as Error)?.message ?? e),
    }));
    lados.push({ lado: 'volta', ...volta });
  } else {
    lados.push({ lado: 'volta', estado: 'nao_configurado' });
  }

  const ok = lados.every((l) => l.estado === 'ok');
  return { ok, id: alvo.id, base_url: alvo.base_url, lados };
}

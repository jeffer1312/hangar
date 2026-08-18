// Registro dos DOIS lados de um peer (Task 8): um gesto na aba Servidores registra A em B E B em A,
// usando a credencial que o celular já guarda de cada um. Lógica pura, testada sozinha (precedente:
// lib/sessions.test.ts) — a tela só pinta o resultado.
//
// Contrato do "o que mostra quando falha": cada lado tem um estado NOMEADO, nunca um erro genérico.
// Um lado falhar não pode deixar o outro registrado em silêncio — o par completo só vira sucesso
// quando os dois gravaram E os dois testes passaram.
import type { Server } from './auth';
import { getBaseUrl, getToken } from './auth';
import { checkPeer, getIdentificador, gravarPeer } from './peers';

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
      lados: [{ lado: 'ida', estado: 'falhou', motivo: String((e as Error)?.message ?? e) }],
    };
  }

  // 2) Resolve a identidade do DONO — o que o peer precisa guardar para chamar esta máquina
  //    de volta: o nome REAL do backend (CP_SERVER_ID), não o rótulo local do celular.
  const remoto: Server = { id: alvo.id, label: alvo.id, baseUrl: alvo.base_url, token: alvo.token };
  const meuBase = dono?.baseUrl ?? getBaseUrl();
  const meuToken = dono?.token ?? getToken() ?? '';
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
  return { ok, id: alvo.id, base_url: alvo.base_url, lados };
}

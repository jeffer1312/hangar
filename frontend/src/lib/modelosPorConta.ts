// Catálogo de modelos de uma conta/provider ANTES de existir sessão (GET /api/model-options), com
// a memória do último modelo escolhido. Vive fora do CreateSessionSheet porque a tela de
// orquestração oferece a mesma escolha (provider → conta → modelo) e uma segunda cópia divergiria.
import { contextModel, hasContext, modelOptions, modelOptionsForServer, type ModelOption, type Server } from '@hangar/core';

// Os cinco escolhem modelo, cada um de uma fonte diferente (picker do Claude, `pi --list-models`,
// config.toml do Kimi, `model/list` do Codex; omp reusa a fonte do Pi). Lista explícita:
// `provider !== 'codex'` como guarda foi o que deixou o Kimi de fora na primeira versão da tela.
export const PROVIDERS_COM_MODELO = ['claude', 'pi', 'kimi', 'codex', 'omp'] as const;
export const temEscolhaDeModelo = (provider: string) =>
  (PROVIDERS_COM_MODELO as readonly string[]).includes(provider);

// Catálogo do Pi traz ids que se REPETEM entre providers (k3 existe na kimi-coding E na
// kimi-jefferson). O valor da opção é provider/id: sem isso a chave do each colide e o
// `pi --model k3` cru é ambíguo. Nos outros formatos não há provider e o id já é único.
export const valorModelo = (m: ModelOption) => (m.provider ? `${m.provider}/${m.id}` : m.id);

export interface CatalogoDaConta {
  models: ModelOption[];
  // Cache frio de uma conta Claude sem sessão viva: só `opus/sonnet/haiku`.
  reduced: boolean;
  // Último modelo lembrado, SÓ se ainda estiver na lista — modelo tirado do provedor não pode
  // virar flag às cegas (a sessão subiria e falharia no primeiro turno).
  lembrado: string;
  esforcoLembrado: string;
  // A janela de 1M lembrada vem separada do modelo: o valor da lista nunca carrega o sufixo `[1m]`.
  contextoLembrado: boolean;
}

export async function carregarModelos(
  q: { provider: string; engine?: string | null; configDir?: string | null;
    codexAccount?: string | null; engineAccount?: string | null; server?: Server | null; signal?: AbortSignal },
  chaveMemoria?: string,
): Promise<CatalogoDaConta> {
  if (!temEscolhaDeModelo(q.provider)) return { models: [], reduced: false, lembrado: '', esforcoLembrado: '', contextoLembrado: false };
  // A conta ChatGPT só entra quando existe: os chamadores sem ela seguem com a mesma chamada.
  const extra = q.engineAccount ? [q.engineAccount] as const : [] as const;
  const r = q.server
    ? await modelOptionsForServer(q.server, q.provider, q.engine, q.configDir, q.codexAccount, q.signal, ...extra)
    : await modelOptions(q.provider, q.engine, q.configDir, q.codexAccount, ...extra);
  let lembrado = '';
  let esforcoLembrado = '';
  let contextoLembrado = false;
  if (chaveMemoria) {
    try {
      const l = localStorage.getItem(chaveMemoria);
      const base = l ? contextModel(l, false) : '';
      const mod = base ? r.models.find((x) => valorModelo(x) === base) : undefined;
      if (mod) {
        lembrado = base;
        // `[1m]` só volta onde o nativo aceita (choices.rs matched_model): conta ChatGPT do proxy e modelo com Fast.
        contextoLembrado = hasContext(l!) && q.provider === 'claude' && !!q.engineAccount && !!mod.supports_fast;
      }
      esforcoLembrado = localStorage.getItem(chaveMemoria + ':effort') ?? '';
    } catch { /* storage bloqueado: sem memória, sem erro */ }
  }
  return { models: r.models, reduced: r.reduced, lembrado, esforcoLembrado, contextoLembrado };
}

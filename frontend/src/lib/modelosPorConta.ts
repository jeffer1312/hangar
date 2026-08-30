// Catálogo de modelos de uma conta/provider ANTES de existir sessão (GET /api/model-options), com
// a memória do último modelo escolhido. Vive fora do CreateSessionSheet porque a tela de
// orquestração oferece a mesma escolha (provider → conta → modelo) e uma segunda cópia divergiria.
import { modelOptions, type ModelOption } from './api';

// Os quatro escolhem modelo, cada um de uma fonte diferente (picker do Claude, `pi --list-models`,
// config.toml do Kimi, `model/list` do Codex). Lista explícita: `provider !== 'codex'` como guarda
// foi o que deixou o Kimi de fora na primeira versão da tela.
export const PROVIDERS_COM_MODELO = ['claude', 'pi', 'kimi', 'codex'] as const;
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
}

export async function carregarModelos(
  q: { provider: string; engine?: string | null; configDir?: string | null },
  chaveMemoria?: string,
): Promise<CatalogoDaConta> {
  if (!temEscolhaDeModelo(q.provider)) return { models: [], reduced: false, lembrado: '', esforcoLembrado: '' };
  const r = await modelOptions(q.provider, q.engine, q.configDir);
  let lembrado = '';
  let esforcoLembrado = '';
  if (chaveMemoria) {
    try {
      const l = localStorage.getItem(chaveMemoria);
      if (l && r.models.some((mod) => valorModelo(mod) === l)) lembrado = l;
      esforcoLembrado = localStorage.getItem(chaveMemoria + ':effort') ?? '';
    } catch { /* storage bloqueado: sem memória, sem erro */ }
  }
  return { models: r.models, reduced: r.reduced, lembrado, esforcoLembrado };
}

<script lang="ts">
  import { getConfig, getConfigForServer, patchConfig, patchConfigForServer, type CampoConfig } from '../../lib/api';
  import type { Server } from '../../lib/auth';

  // Configuração do servidor pelo app. Até aqui tudo vinha só de env/.env: pra mudar a chave da
  // transcrição ou a retenção de anexos era preciso editar arquivo no servidor e reiniciar o
  // serviço — do celular, impossível.
  //
  // O segredo entra mas não sai: o backend devolve mascarado (gsk_••••1234). Dá pra conferir QUAL
  // chave está lá e trocá-la; não dá pra copiar de volta.
  interface Props {
    // Servidor a configurar. null = o ativo global (desktop, que TEM um ativo). No mobile a lista é
    // agregada e não há "ativo": sem isto, editar a config pelo drawer batia no servidor globalmente
    // ativo, que pode ser OUTRA máquina — dá pra trocar a chave do servidor errado sem perceber.
    targetServer?: Server | null;
    // Os motores são uma LISTA de registros com segredo cada um e têm tela própria: quem monta esta
    // aqui é que sabe como chegar lá (trocar de rota no modal único, abrir outra folha…).
    onOpenMotores: () => void;
  }
  let { targetServer = null, onOpenMotores }: Props = $props();

  interface Campo {
    chave: string;
    rotulo: string;
    ajuda: string;
    tipo: 'texto' | 'segredo' | 'numero' | 'liga';
    sufixo?: string;
  }

  const CAMPOS: Campo[] = [
    { chave: 'groq_api_key', rotulo: 'Chave da Groq', tipo: 'segredo',
      ajuda: 'Transcreve áudio gravado e a fala dos vídeos anexados. Vazia = transcrição desligada.' },
    { chave: 'upload_retention_days', rotulo: 'Guardar anexos por', tipo: 'numero', sufixo: 'dias',
      ajuda: 'Anexo mais velho que isso é apagado no próximo upload. 0 = nunca apagar.' },
    { chave: 'automations', rotulo: 'Automações', tipo: 'liga',
      ajuda: 'Chave mestra do que roda sem você olhar: encadeamento de sessão e auto-resume.' },
    { chave: 'notify_finished', rotulo: 'Avisar quando terminar', tipo: 'liga',
      ajuda: 'Notificação quando um turno longo acaba.' },
    { chave: 'finish_min_seconds', rotulo: 'Turno curto não avisa', tipo: 'numero', sufixo: 'seg',
      ajuda: 'Turno mais rápido que isso não gera notificação.' },
    { chave: 'notify_dead', rotulo: 'Avisar quando cair', tipo: 'liga',
      ajuda: 'Notificação quando uma sessão morre.' },
    { chave: 'stall_seconds', rotulo: 'Marcar travada após', tipo: 'numero', sufixo: 'seg',
      ajuda: 'Sessão "trabalhando" e calada por mais que isso ganha o aviso de travada.' },
    { chave: 'editor', rotulo: 'Editor', tipo: 'texto',
      ajuda: 'Binário que abre a pasta da sessão no desktop (ex: code, subl).' },
  ];

  const ROTULO_LEITURA: Record<string, string> = {
    port: 'Porta', lan_bind_ip: 'IP de bind', server_id: 'ID deste servidor',
    public_url: 'URL pública', scan_roots: 'Raízes do scanner',
  };

  let campos = $state<Record<string, CampoConfig>>({});
  let leitura = $state<Record<string, string | number>>({});
  let rascunho = $state<Record<string, string | number | boolean>>({});
  let carregando = $state(false);
  let salvando = $state(false);
  let erro = $state('');
  let salvo = $state(false);

  // Recarrega na montagem: outro dispositivo (ou o .env) pode ter mudado no meio. Quem mostra esta
  // tela só a monta quando ela está à vista, então montar É abrir.
  $effect(() => {
    carregar();
  });

  async function carregar() {
    carregando = true;
    erro = '';
    try {
      const c = targetServer ? await getConfigForServer(targetServer) : await getConfig();
      campos = c.campos;
      leitura = c.somente_leitura;
      rascunho = {};
    } catch (e) {
      erro = e instanceof Error ? e.message : 'Falha ao carregar';
    } finally {
      carregando = false;
    }
  }

  function valorAtual(chave: string): string | number | boolean {
    if (chave in rascunho) return rascunho[chave];
    const v = campos[chave]?.valor;
    return v ?? '';
  }

  const temMudanca = $derived(Object.keys(rascunho).length > 0);

  async function salvar() {
    if (!temMudanca) return;
    salvando = true;
    erro = '';
    salvo = false;
    try {
      const r = targetServer ? await patchConfigForServer(targetServer, rascunho) : await patchConfig(rascunho);
      campos = r.campos;
      rascunho = {};
      salvo = true;
      setTimeout(() => (salvo = false), 2500);
    } catch (e) {
      // Erro de validação do servidor aparece como veio ("upload_retention_days: esperado número"):
      // é mais útil que um "falhou" genérico.
      erro = e instanceof Error ? e.message : 'Falha ao salvar';
    } finally {
      salvando = false;
    }
  }
</script>

<div class="cfg">
  <header class="cfg-head">
    <h2>Configurações</h2>
    <p class="sub">Valem para este servidor, na hora — sem reiniciar.</p>
  </header>

  {#if carregando}
    <p class="aviso">Carregando…</p>
  {:else if erro && !Object.keys(campos).length}
    <p class="aviso erro">{erro}</p>
    <button class="btn" onclick={carregar}>Tentar de novo</button>
  {:else}
    <div class="lista">
      {#each CAMPOS as c (c.chave)}
        {@const estado = campos[c.chave]}
        <div class="linha" class:liga={c.tipo === 'liga'}>
          <div class="txt">
            <label class="rot" for={`cfg-${c.chave}`}>
              {c.rotulo}
              {#if estado?.origem === 'app'}<span class="tag">editado</span>{/if}
            </label>
            <span class="ajuda">{c.ajuda}</span>
          </div>

          {#if c.tipo === 'liga'}
            <input
              id={`cfg-${c.chave}`}
              class="switch"
              type="checkbox"
              checked={valorAtual(c.chave) === true}
              onchange={(e) => (rascunho[c.chave] = e.currentTarget.checked)}
            />
          {:else if c.tipo === 'numero'}
            <span class="campo-num">
              <input
                id={`cfg-${c.chave}`}
                type="number"
                inputmode="numeric"
                min="0"
                value={valorAtual(c.chave)}
                oninput={(e) => (rascunho[c.chave] = e.currentTarget.value)}
              />
              {#if c.sufixo}<span class="sufixo">{c.sufixo}</span>{/if}
            </span>
          {:else if c.tipo === 'segredo'}
            <!-- O segredo ENTRA mas não sai. O campo fica VAZIO: pré-preencher com a máscara faz
                 qualquer toque no input mandar o texto mascarado de volta e sobrescrever a chave
                 real. A máscara aparece ao lado, como informação, não como valor editável. -->
            {#if estado?.definido}
              <span class="mascara" title="A chave não volta inteira do servidor">
                {estado.valor} <span class="mascara-nota">configurada</span>
              </span>
            {/if}
            <input
              id={`cfg-${c.chave}`}
              class="campo-txt"
              type="text"
              autocomplete="off"
              autocapitalize="off"
              spellcheck={false}
              placeholder={estado?.definido ? 'colar nova chave para trocar' : 'colar a chave'}
              value={(rascunho[c.chave] as string) ?? ''}
              oninput={(e) => (rascunho[c.chave] = e.currentTarget.value)}
            />
          {:else}
            <input
              id={`cfg-${c.chave}`}
              class="campo-txt"
              type="text"
              autocomplete="off"
              autocapitalize="off"
              spellcheck={false}
              value={valorAtual(c.chave)}
              oninput={(e) => (rascunho[c.chave] = e.currentTarget.value)}
            />
          {/if}
        </div>
      {/each}
    </div>

    <!-- Motores são uma LISTA de registros com segredo cada um: não cabem no layout de
         linha-por-setting desta tela. Vão numa tela própria, alcançada daqui. -->
    <button class="atalho" onclick={onOpenMotores}>
      <span class="txt">
        <span class="rot">Motores de modelo</span>
        <span class="ajuda">
          Rodar uma sessão em Kimi, num gateway próprio ou noutro modelo — sem mexer na sua conta.
        </span>
      </span>
      <span class="seta" aria-hidden="true">›</span>
    </button>

    <div class="somente-leitura">
      <h3>Só pelo servidor</h3>
      <p class="ajuda">
        Mudar qualquer uma exige editar o <code>.env</code> e reiniciar o serviço — por isso não
        são editáveis daqui.
      </p>
      {#each Object.entries(leitura) as [k, v] (k)}
        <div class="ro-linha">
          <span class="ro-rot">{ROTULO_LEITURA[k] ?? k}</span>
          <span class="ro-val">{v === '' ? '—' : v}</span>
        </div>
      {/each}
    </div>
  {/if}

  {#if erro && Object.keys(campos).length}<p class="aviso erro">{erro}</p>{/if}
</div>

{#if !carregando && Object.keys(campos).length}
  <div class="rodape">
    {#if salvo}<span class="ok">salvo</span>{/if}
    <button class="btn primario" onclick={salvar} disabled={!temMudanca || salvando}>
      {salvando ? 'Salvando…' : 'Salvar'}
    </button>
  </div>
{/if}

<style>
  .cfg { padding: var(--space-2) var(--space-4) var(--space-4); }
  .cfg-head h2 { margin: 0; font-size: var(--text-lg); font-weight: 600; color: var(--text-primary); }
  .cfg-head .sub { margin: 2px 0 var(--space-4); font-size: var(--text-xs); color: var(--text-muted); }

  .lista { display: flex; flex-direction: column; }
  .atalho {
    display: flex; align-items: center; justify-content: space-between; gap: var(--space-4);
    width: 100%; padding: var(--space-3) 0;
    background: none; text-align: left;
    border-bottom: 1px solid var(--border-subtle);
  }
  .atalho .seta { font-size: var(--text-lg); color: var(--text-muted); flex-shrink: 0; }
  .linha {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border-subtle);
  }
  /* Liga/desliga fica na MESMA linha do rótulo: o controle é pequeno e o texto manda. */
  .linha.liga { flex-direction: row; align-items: center; justify-content: space-between; gap: var(--space-4); }

  .txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .rot {
    display: flex; align-items: center; gap: var(--space-2);
    font-size: var(--text-base); font-weight: 600; color: var(--text-primary);
  }
  /* "editado" = veio de override, não do .env — sem isso não dá pra saber de onde o valor vem. */
  .tag {
    font-size: 10px; font-weight: 700; letter-spacing: 0.04em; text-transform: uppercase;
    color: var(--accent); background: var(--accent-dim);
    padding: 1px 6px; border-radius: var(--radius-full);
  }
  .ajuda { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; }

  input[type='text'], input[type='number'] {
    height: 40px;
    background: var(--bg-base);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 16px;                 /* 16px evita o zoom automático do iOS ao focar */
    padding: 0 var(--space-3);
    outline: none;
    min-width: 0;
  }
  input:focus { border-color: var(--accent); }
  .campo-num { display: flex; align-items: center; gap: var(--space-2); }
  .campo-num input { width: 100px; }
  .sufixo { font-size: var(--text-xs); color: var(--text-muted); }

  .mascara {
    display: inline-flex; align-items: baseline; gap: var(--space-2);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-secondary);
  }
  .mascara-nota { font-family: var(--font-ui); color: var(--success); font-size: 11px; }

  /* `.switch` é global (app.css) — vocabulário único de liga/desliga do app. */

  .somente-leitura { margin-top: var(--space-5); }
  .somente-leitura h3 {
    margin: 0 0 4px; font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary);
  }
  .ro-linha {
    display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-4);
    padding: var(--space-2) 0; border-bottom: 1px solid var(--border-subtle);
  }
  .ro-rot { font-size: var(--text-sm); color: var(--text-secondary); }
  .ro-val {
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 60%;
  }

  .aviso { font-size: var(--text-sm); color: var(--text-muted); margin: var(--space-3) 0; }
  .aviso.erro { color: var(--error); }

  /* CHROME FUNCIONAL, sólido de propósito: esta faixa fica GRUDADA no fim da folha enquanto o
     formulário rola por baixo. `--bg-surface` cru aqui não é esquecimento da regra de Transparência
     (CLAUDE.md) — com token de véu o texto do formulário atravessaria os botões. Mesmo caso do
     `.acoes` dos Motores. NÃO converter. */
  .rodape {
    position: sticky; bottom: 0;
    display: flex; align-items: center; justify-content: flex-end; gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    padding-bottom: calc(var(--space-3) + env(safe-area-inset-bottom));
    background: var(--bg-surface);
    border-top: 1px solid var(--border-subtle);
  }
  .ok { font-size: var(--text-xs); color: var(--success); }
  .btn {
    height: 40px; padding: 0 var(--space-4);
    border-radius: var(--radius-md);
    background: var(--bg-elevated); color: var(--text-primary);
    font-size: var(--text-sm); font-weight: 600;
  }
  .btn.primario { background: var(--accent); color: #fff; }
  .btn:disabled { opacity: 0.45; }
</style>

<script lang="ts">
  import HangarMark from '../icons/HangarMark.svelte';
  import * as m from '../../paraglide/messages';
  import { getAtualizacao } from '../../lib/api';
  import { atualizarUI } from '../../lib/atualizarUI.svelte';
  const REPO = 'https://github.com/jeffer1312/hangar';

  /**
   * `git describe` legível pra quem usa.
   *
   * O formato cru é de desenvolvedor: `v0.1.2-130-g787a0cf8-dirty` quer dizer "130 commits depois
   * da tag v0.1.2, no commit 787a0cf8, com arquivos modificados". Quem abre o Sobre quer saber
   * qual versão está rodando, não a distância até a última tag — e o `-dirty` sem explicação lê
   * como defeito. Vira `787a0cf8` (o que identifica de verdade) mais um aviso à parte quando há
   * mudanças locais.
   */
  function legivel(v: string): { versao: string; local: boolean } {
    const local = v.endsWith('-dirty');
    const limpo = local ? v.slice(0, -'-dirty'.length) : v;
    const m2 = /-g([0-9a-f]{7,})$/.exec(limpo);      // `…-g<hash>` → só o hash
    return { versao: m2 ? m2[1] : limpo, local };
  }

  // Procurar na hora, sem esperar o relógio de 30min do servidor. Este é o lugar natural do
  // pedido: é aqui que a versão instalada está escrita.
  type Estado = 'parado' | 'procurando' | 'tem' | 'em-dia' | 'erro';
  let estado = $state<Estado>('parado');
  let versaoServidor = $state('');

  const daTela = $derived(legivel(__HANGAR_VERSION__));
  const doServidor = $derived(versaoServidor ? legivel(versaoServidor) : null);

  async function procurar() {
    estado = 'procurando';
    try {
      const d = await getAtualizacao(true);   // clique = vai à rede antes de comparar
      versaoServidor = d.versoes.backend;
      // Duas razões pra oferecer a atualização, como na barra: commit novo lá fora, ou o servidor
      // rodando código diferente do que já está no disco (quem puxou na mão e não reiniciou).
      estado = d.atualizacao_disponivel || d.versoes.repo !== d.versoes.backend ? 'tem' : 'em-dia';
    } catch {
      estado = 'erro';
    }
  }
</script>

<div class="about">
  <span class="mark" aria-hidden="true"><HangarMark size={56} /></span>
  <strong>Hangar</strong>
  <span class="version">{daTela.versao} · {__HANGAR_BUILD_DATE__}</span>
  {#if daTela.local}<span class="nota">{m.atualizar_com_mudancas_locais()}</span>{/if}
  <p>{m.config_sobre_desc()}</p>
  <a href={REPO} target="_blank" rel="noopener noreferrer">github.com/jeffer1312/hangar</a>

  <div class="atualizar">
    {#if estado === 'tem'}
      <button class="bt primario" onclick={() => atualizarUI.abrir()}>{m.atualizar_botao()}</button>
      <span class="nota">{m.atualizar_disponivel_titulo()}</span>
    {:else}
      <button class="bt" onclick={procurar} disabled={estado === 'procurando'}>
        {estado === 'procurando' ? m.atualizar_carregando() : m.atualizar_procurar_curto()}
      </button>
      {#if estado === 'em-dia'}
        <span class="nota">{m.atualizar_em_dia_sub()}</span>
      {:else if estado === 'erro'}
        <span class="nota erro">{m.atualizar_procurar_falhou()}</span>
      {/if}
    {/if}
    {#if doServidor && versaoServidor !== __HANGAR_VERSION__}
      <!-- Só quando divergem: a tela roda o bundle do build, e o servidor pode estar noutro
           commit. Quando batem, mostrar duas linhas iguais é ruído. -->
      <span class="version">{m.atualizar_versao_servidor()}: {doServidor.versao}</span>
    {/if}
  </div>
</div>

<style>
  .about { display: flex; flex-direction: column; align-items: center; text-align: center; gap: var(--space-2); padding: var(--space-6) var(--space-4); }
  .mark { color: var(--accent); }
  strong { font-size: var(--text-lg); color: var(--text-primary); }
  .version { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted); }
  p { margin: 0; max-width: 40ch; color: var(--text-secondary); font-size: var(--text-sm); }
  a { color: var(--accent); font-size: var(--text-sm); }
  a:hover { text-decoration: underline; }

  .atualizar { display: flex; flex-direction: column; align-items: center; gap: var(--space-2);
               margin-top: var(--space-4); padding-top: var(--space-4);
               border-top: 1px solid var(--border-subtle); width: 100%; }
  .bt { border-radius: var(--radius-md); padding: var(--space-2) var(--space-4);
        font-family: inherit; font-size: var(--text-sm); font-weight: 500;
        border: 1px solid var(--border-subtle); background: var(--surface-raised);
        color: var(--text-secondary); }
  .bt.primario { background: var(--accent); color: var(--text-inverse); border-color: transparent;
                 font-weight: 600; }
  .bt:disabled { opacity: 0.5; }
  .nota { font-size: var(--text-xs); color: var(--text-muted); }
  .nota.erro { color: var(--erro, #d97070); }
</style>

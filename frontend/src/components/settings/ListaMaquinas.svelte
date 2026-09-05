<script lang="ts">
  // Só desenha: recebe linhas + estado, devolve ações por callback. Quem grava é MaquinasSettings
  // (Task 4) — é o que permite testar a lista sem rede.
  import * as m from '../../paraglide/messages';
  import type { LinhaMaquina } from '../../lib/maquinas';
  import type { LadoState } from '../../lib/registrarPeerDoisLados';

  interface EstadoPeer { lados: LadoState[]; ok: boolean; testando?: boolean }

  interface Props {
    linhas: LinhaMaquina[];
    estados: Record<string, EstadoPeer>;
    meuIdentificador: string;
    carregando: boolean;
    corrige: { id: string; url: string } | null;
    onAcompanhar: (linha: LinhaMaquina, ligar: boolean) => void;
    onFalar: (linha: LinhaMaquina, ligar: boolean) => void;
    onEditar: (linha: LinhaMaquina) => void;
    onCorrige: (url: string | null) => void;
    onTestarDeNovo: (linha: LinhaMaquina) => void;
    onRemover: (linha: LinhaMaquina) => void;
    onAdicionar: () => void;
  }

  let { linhas, estados, meuIdentificador, carregando, corrige, onAcompanhar, onFalar, onEditar, onCorrige, onTestarDeNovo, onRemover, onAdicionar }: Props = $props();

  function estadoDe(linha: LinhaMaquina): EstadoPeer | undefined {
    return linha.identificador ? estados[linha.identificador] : undefined;
  }
  function ladoDe(st: EstadoPeer | undefined, lado: 'ida' | 'volta'): LadoState | undefined {
    return st?.lados.find((l) => l.lado === lado);
  }
  function farol(temPeer: boolean, st: EstadoPeer | undefined, falhaReal: boolean): 'ok' | 'nao' | 'test' | 'neutro' {
    // testando pode chegar antes do peer existir (registro em curso) — checa primeiro
    if (st?.testando) return 'test';
    if (!temPeer && !st) return 'neutro'; // só navegador, nada pra testar — não é "testando"
    if (!st) return 'test';
    if (st.ok) return 'ok';
    return falhaReal ? 'nao' : 'test'; // nao_configurado (sem token/registro) não é falha, é cinza
  }
  function selo(l: LadoState | undefined): string {
    if (!l) return '·';
    if (l.estado === 'ok') return '✓';
    if (l.estado === 'nao_configurado') return '·';
    return '✗';
  }
</script>

<ul class="mq-lista">
  {#each linhas as linha (linha.chave)}
    {@const st = estadoDe(linha)}
    {@const ida = ladoDe(st, 'ida')}
    {@const volta = ladoDe(st, 'volta')}
    {@const falhaReal = !!st && !st.ok && (ida?.estado === 'falhou' || ida?.estado === 'recusou' || ida?.estado === 'estranho' || volta?.estado === 'falhou' || volta?.estado === 'recusou' || volta?.estado === 'estranho')}
    {@const desligada = linha.peer?.enabled === false}
    {@const farolEstado = desligada ? 'neutro' : farol(!!linha.peer, st, falhaReal)}
    <li class="mq-linha" data-chave={linha.chave}>
      <span class="mq-farol" class:ok={farolEstado === 'ok'} class:nao={farolEstado === 'nao'} class:neutro={farolEstado === 'neutro'}>
        {farolEstado === 'test' ? '◌' : farolEstado === 'neutro' ? '·' : '●'}
      </span>
      <span class="mq-txt">
        <span class="mq-nome">{linha.nome}</span>
        <span class="mq-url">{linha.navegador?.baseUrl ?? linha.peer?.base_url}</span>
        {#if desligada}
          <span class="mq-hint">{m.maquinas_peer_desligado()}</span>
        {:else if linha.navegador && !linha.identificador}
          <span class="mq-hint">{m.maquinas_sem_identificador()}</span>
        {:else if st?.testando}
          <span class="mq-hint">{m.peers_estado_testando()}</span>
        {:else if volta?.estado === 'recusou' && volta.motivo === 'credencial'}
          <span class="mq-hint">{m.maquinas_volta_token_recusado()}</span>
        {:else if falhaReal}
          <span class="mq-hint">
            {m.peers_estado_parcial()}
            <span class="pr-lados">
              <span class="pr-lado" class:ok={ida?.estado === 'ok'} class:nao={ida && ida.estado !== 'ok' && ida.estado !== 'nao_configurado'} title={ida && ida.estado !== 'ok' ? ida.motivo : undefined}>{selo(ida)} {m.peers_lado_ida()}</span>
              <span class="pr-lado" class:ok={volta?.estado === 'ok'} class:nao={volta && volta.estado !== 'ok' && volta.estado !== 'nao_configurado'} title={volta && volta.estado !== 'ok' ? volta.motivo : undefined}>{selo(volta)} {m.peers_lado_volta()}</span>
            </span>
          </span>
        {:else if volta?.estado === 'nao_configurado' && volta.motivo === 'token'}
          <span class="mq-hint">{m.maquinas_volta_sem_medir()}</span>
        {:else if volta?.estado === 'nao_configurado' && volta.motivo === 'registro'}
          <span class="mq-hint">{m.maquinas_volta_sem_registro()}</span>
        {/if}
        <!-- Independente do estado acima: "só o servidor conhece" é sempre a instrução acionável
             quando não há navegador, mesmo com um estado de teste já rodado. -->
        {#if !linha.navegador && linha.peer}
          <span class="mq-hint">{m.maquinas_so_no_servidor()}</span>
        {/if}
      </span>
      <span class="mq-caixas">
        <label class="mq-caixa">
          <input type="checkbox" class="switch mq-acompanhar" checked={!!linha.navegador}
                 onchange={(e) => { const alvo = e.currentTarget; const ligar = alvo.checked; alvo.checked = !ligar; onAcompanhar(linha, ligar); }} />
          {m.maquinas_acompanhar()}
        </label>
        <label class="mq-caixa">
          {#if linha.estaMaquina}
            <span class="mq-tag">{m.maquinas_esta()}</span>
          {:else}
            <input type="checkbox" class="switch mq-falar" checked={!!linha.peer}
                   disabled={!meuIdentificador || !linha.identificador}
                   onchange={(e) => { const alvo = e.currentTarget; const ligar = alvo.checked; alvo.checked = !ligar; onFalar(linha, ligar); }} />
            {m.maquinas_falar()}
          {/if}
        </label>
      </span>
      {#if linha.navegador}
        <button class="mq-editar" aria-label={m.servidor_editar_aria({ nome: linha.nome })} onclick={() => onEditar(linha)}>✎</button>
      {/if}
      <!-- Esta máquina sai só pelo Sair: removê-la daqui é deslogar o aparelho. -->
      {#if !linha.estaMaquina}
        <button class="mq-editar mq-remover" aria-label={m.maquinas_remover_aria({ nome: linha.nome })} title={m.lista_remover()} onclick={() => onRemover(linha)}>✕</button>
      {/if}
      {#if corrige?.id === linha.identificador}
        <div class="corrige">
          <p>{m.peers_corrige_1({ nome: linha.nome, endereco: linha.peer?.base_url ?? '' })}</p>
          <p><b>{m.peers_corrige_pergunta({ nome: linha.nome })}</b></p>
          <input class="corrige-input" value={corrige.url}
                 aria-label={m.peers_corrige_pergunta({ nome: linha.nome })}
                 oninput={(e) => onCorrige(e.currentTarget.value)} />
          <div class="acoes">
            <button class="btn primaria" onclick={() => onTestarDeNovo(linha)}>{m.peers_testar_novamente()}</button>
            <button class="btn" onclick={() => onCorrige(null)}>{m.peers_so_ida()}</button>
          </div>
        </div>
      {/if}
    </li>
  {:else}
    {#if carregando}
      <li class="mq-vazio">{m.comum_carregando()}</li>
    {:else}
      <li class="mq-vazio">{m.maquinas_vazio()}</li>
    {/if}
  {/each}
</ul>
<button class="ss-btn mq-add" onclick={onAdicionar}>+ {m.sessao_adicionar_servidor()}</button>

<style>
  .mq-lista { list-style: none; margin: 0; padding: 0; background: var(--surface-card); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); overflow: hidden; container-type: inline-size; }
  .mq-linha { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-3); padding: var(--space-3); }
  .mq-linha + .mq-linha { border-top: 1px solid var(--border-subtle); }
  .mq-farol { flex-shrink: 0; width: 1.2em; text-align: center; font-size: 14px; color: var(--text-muted); }
  .mq-farol.ok { color: var(--success); }
  .mq-farol.nao { color: var(--error); }
  .mq-farol.neutro { color: var(--text-muted); }
  .mq-txt { flex: 1 1 200px; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .mq-nome { font-size: var(--text-sm); color: var(--text-primary); }
  .mq-url { font-family: var(--font-mono); font-size: 11px; color: var(--text-muted); word-break: break-all; }
  .mq-hint { font-size: var(--text-xs); line-height: 1.35; color: var(--text-muted); }
  .mq-caixas { display: flex; flex-wrap: wrap; gap: var(--space-2) var(--space-4); }
  .mq-caixa { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-xs); color: var(--text-muted); }
  /* Celular estreito: caixas com max-content estouravam a largura e empurravam o ✎ pra baixo do
     painel — container query porque quem aperta é a largura do PAINEL, não a da janela. */
  @container (max-width: 420px) {
    .mq-caixas { flex-basis: 100%; }
  }
  .mq-tag { flex-shrink: 0; font-size: 10px; font-weight: 600; color: var(--accent); }
  .mq-editar { width: 32px; height: 32px; min-height: 0; flex-shrink: 0; color: var(--text-muted); font-size: var(--text-sm); border-radius: var(--radius-sm); }
  .mq-editar:hover { color: var(--accent); background: var(--bg-hover); }
  .mq-remover:hover { color: var(--error); }
  .mq-vazio { padding: var(--space-3); font-size: var(--text-xs); color: var(--text-muted); }

  .pr-lados { display: flex; gap: var(--space-2); flex-shrink: 0; margin-top: 2px; }
  .pr-lado { display: flex; align-items: center; gap: 4px; font-size: 11px; color: var(--text-muted);
             padding: 2px var(--space-2); border-radius: var(--radius-full);
             background: var(--surface-raised); border: 1px solid var(--border-subtle); }
  .pr-lado.ok { color: var(--success); }
  .pr-lado.nao { color: var(--error); }

  .corrige { flex-basis: 100%; margin-top: var(--space-2); padding: var(--space-3); background: var(--surface-card);
             border: 1px solid var(--border-default); border-left: 3px solid var(--warning);
             border-radius: var(--radius-md); }
  .corrige p { margin: 0 0 var(--space-2); font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.45; }
  .corrige b { color: var(--text-primary); font-weight: 600; }
  .corrige-input { width: 100%; height: 34px; padding: 0 var(--space-3);
                   background: var(--surface-inset); border: 1px solid var(--border-default);
                   border-radius: var(--radius-sm); color: var(--text-primary);
                   font-family: var(--font-mono); font-size: var(--text-sm); box-sizing: border-box; }
  .acoes { display: flex; gap: var(--space-2); margin-top: var(--space-3); }
  .btn { height: 36px; min-height: 0; padding: 0 var(--space-4); border-radius: var(--radius-sm);
         border: 1px solid var(--border-subtle); background: var(--surface-raised);
         color: var(--text-primary); font-size: var(--text-sm); font-family: inherit; }
  .btn.primaria { background: var(--accent); border-color: var(--accent); color: #fff; }

  .mq-add { display: flex; align-items: center; justify-content: flex-start; gap: var(--space-2);
            width: 100%; min-height: 44px; margin-top: var(--space-2); padding: var(--space-2) var(--space-4);
            text-align: left; color: var(--text-primary); font-size: var(--text-sm); border-radius: 0; }
  .mq-add:hover { background: var(--bg-hover); }
</style>

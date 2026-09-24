<script lang="ts">
  // Lista curta: uma linha por servidor com uma frase de estado. Tocar abre o detalhe, onde moram
  // interruptores, token, correção e remoção. Só desenha — quem grava é MaquinasSettings.
  import * as m from '../../paraglide/messages';
  import DetalheServidor from './DetalheServidor.svelte';
  import { estadoDaLinha, type EstadoPeer, type EstadoDaLinha, type LinhaMaquina } from '../../lib/maquinas';

  interface Props {
    linhas: LinhaMaquina[];
    estados: Record<string, EstadoPeer>;
    meuIdentificador: string;
    carregando: boolean;
    corrige: { id: string; url: string } | null;
    // Gravação do identificador remoto: qual Server.id está em voo ('' = nenhum) e o erro POR
    // servidor. O detalhe recebe só a fatia dele — ele desenha uma máquina, não a lista.
    idSalvando: string;
    idErro: Record<string, string>;
    onAcompanhar: (linha: LinhaMaquina, ligar: boolean) => void;
    onFalar: (linha: LinhaMaquina, ligar: boolean) => void;
    onToggleScan: (linha: LinhaMaquina, ligar: boolean) => void;
    onEditar: (linha: LinhaMaquina) => void;
    onCorrige: (url: string | null) => void;
    onTestarDeNovo: (linha: LinhaMaquina) => void;
    onRemover: (linha: LinhaMaquina) => void;
    onSalvarIdentificador: (linha: LinhaMaquina, valor: string) => void;
  }

  let { linhas, estados, meuIdentificador, carregando, corrige, idSalvando, idErro, onAcompanhar, onFalar, onToggleScan, onEditar, onCorrige, onTestarDeNovo, onRemover, onSalvarIdentificador }: Props = $props();

  // Pela chave, não pelo objeto: a linha é recriada a cada carga, e o detalhe tem de acompanhar o
  // dado novo. Linha que sumiu (removida) fecha o detalhe sozinha. Editar fecha antes de abrir a
  // folha de edição, que é uma camada abaixo dos diálogos e ficaria escondida atrás dele.
  let aberta = $state<string | null>(null);
  const linhaAberta = $derived(aberta ? linhas.find((l) => l.chave === aberta) ?? null : null);

  const estadoDe = (l: LinhaMaquina) => (l.identificador ? estados[l.identificador] : undefined);

  function curto(l: LinhaMaquina, e: EstadoDaLinha): string {
    switch (e.tipo) {
      case 'desligada': return m.servidores_curto_desligado();
      case 'sem_identificador': return m.servidores_curto_sem_id();
      case 'nao_responde': return m.servidores_curto_nao_responde();
      case 'token_aparelho_recusado': return m.servidores_curto_token_recusado();
      case 'testando': return m.acesso_testando();
      case 'token_recusado': return m.servidores_curto_token_recusado();
      case 'ida_outra_maquina':
      case 'volta_outra_maquina': return m.servidores_curto_endereco_outra();
      case 'parcial': return e.ida?.estado === 'ok' ? m.servidores_curto_so_ida() : m.servidores_curto_ida_falhou();
      case 'volta_sem_medir': return m.servidores_curto_falta_token();
      case 'volta_sem_registro': return m.servidores_curto_so_ida();
      case 'ok': return l.navegador ? m.servidores_curto_ok() : m.servidores_curto_falta_token();
      case 'neutro':
        if (!l.navegador && l.peer) return m.servidores_curto_falta_token();
        return l.peer ? m.acesso_testando() : m.servidores_curto_sem_recados();
      default: {
        const semFrase: never = e.tipo;
        return semFrase;
      }
    }
  }
  const aviso = (e: EstadoDaLinha) => ['token_recusado', 'token_aparelho_recusado', 'parcial', 'sem_identificador',
    'nao_responde', 'ida_outra_maquina', 'volta_outra_maquina', 'volta_sem_registro', 'volta_sem_medir'].includes(e.tipo);
</script>

<ul class="mq-lista">
  {#each linhas as linha (linha.chave)}
    {@const e = estadoDaLinha(linha, estadoDe(linha))}
    <li>
      <button type="button" class="sv-linha" data-chave={linha.chave}
              aria-label={m.servidores_abrir_aria({ nome: linha.nome })}
              onclick={() => (aberta = linha.chave)}>
        <span class="mq-farol" class:ok={e.farol === 'ok'} class:nao={e.farol === 'nao'} class:neutro={e.farol === 'neutro'} aria-hidden="true">
          {e.farol === 'test' ? '◌' : e.farol === 'neutro' ? '·' : '●'}
        </span>
        <span class="sv-txt">
          <span class="sv-nome">{linha.nome}</span>
          <span class="sv-estado" class:nao={e.farol === 'nao'} class:aviso={e.farol !== 'nao' && aviso(e)}>{curto(linha, e)}</span>
        </span>
        <span class="sv-chev" aria-hidden="true">›</span>
      </button>
    </li>
  {:else}
    <li class="mq-vazio">{carregando ? m.comum_carregando() : m.maquinas_vazio()}</li>
  {/each}
</ul>

{#if linhaAberta}
  <DetalheServidor linha={linhaAberta} estado={estadoDe(linhaAberta)} {meuIdentificador} {corrige}
    idSalvando={!!linhaAberta.navegador && idSalvando === linhaAberta.navegador.id}
    idErro={idErro[linhaAberta.navegador?.id ?? ''] ?? ''}
    {onAcompanhar} {onFalar} {onToggleScan} {onCorrige} {onTestarDeNovo} {onRemover} {onSalvarIdentificador}
    onEditar={(l) => { aberta = null; onEditar(l); }}
    onFechar={() => (aberta = null)} />
{/if}

<style>
  .mq-lista { list-style: none; margin: 0; padding: 0; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); overflow: hidden; }
  .mq-lista > li + li { border-top: 1px solid var(--border-subtle); }
  .sv-linha { display: flex; align-items: center; gap: var(--space-3); width: 100%; min-height: 56px; padding: var(--space-2) var(--space-3);
              text-align: left; border-radius: 0; color: var(--text-primary); }
  .sv-linha:hover { background: var(--bg-hover); }
  .mq-farol { flex-shrink: 0; width: 1.2em; text-align: center; font-size: 14px; color: var(--text-muted); }
  .mq-farol.ok { color: var(--success); }
  .mq-farol.nao { color: var(--error); }
  .sv-txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .sv-nome { font-size: var(--text-sm); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .sv-estado { font-size: var(--text-xs); color: var(--text-secondary); }
  .sv-estado.nao { color: var(--error); }
  .sv-estado.aviso { color: var(--warning); }
  .sv-chev { flex-shrink: 0; color: var(--text-muted); font-size: 18px; }
  .mq-vazio { padding: var(--space-3); font-size: var(--text-xs); color: var(--text-muted); }
</style>

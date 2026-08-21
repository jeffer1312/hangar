// @vitest-environment happy-dom
// Aba Contas: a lista lê a fonte única (/api/conta-estado via lib/contaEstado), criar/apagar
// reusam as rotas de sempre (lib/api), conta deslogada NUNCA some da lista.
// (B6) Os 7 casos abaixo foram RESTAURADOS do pai f3189f6b (os dois describe da Task 4),
// e os 6 casos do login (Task 7) ficam AO LADO num terceiro describe — estender, não
// substituir. Regressão de uma Task aprovada e mergeada: o código que eles cobriam
// (novaConta/apagar/lista) continua vivo e ligado aos botões.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ContasSettings from './ContasSettings.svelte';
import { criarProps } from './props-reativas.svelte';
import * as m from '../../paraglide/messages';
import * as contaEstadoLib from '../../lib/contaEstado';
import * as apiLib from '../../lib/api';
import * as loginLib from '../../lib/loginConta';
import { mensagemDeErro } from '@hangar/core';
import * as credLib from '../../lib/credenciais';
import type { Credencial } from '../../lib/credenciais';
import type { Server } from '../../lib/auth';

vi.mock('../../lib/credenciais', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/credenciais')>();
  return { ...real, listarCredenciais: vi.fn(), definirApelido: vi.fn(async () => ({ id: 'x', apelido: null })) };
});
vi.mock('../../lib/api', () => ({
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  criarConta: vi.fn(async () => ({ path: '/x', label: 'x', active: false })),
  apagarConta: vi.fn(async () => {}),
  putEngine: vi.fn(async () => ({ motores: {} })),
  putEngineForServer: vi.fn(async () => ({ motores: {} })),
  deleteEngine: vi.fn(async () => ({ ok: true })),
  deleteEngineForServer: vi.fn(async () => ({ ok: true })),
  isAbortError: () => false,
  isTimeoutError: () => false,
}));
vi.mock('../../lib/loginConta', () => ({
  iniciarLogin: vi.fn(async () => ({ ok: true })),
  passoLogin: vi.fn(async () => ({ etapa: 'aguardando', url: 'https://claude.com/cai/oauth/authorize' })),
  confirmarLogin: vi.fn(async () => ({ ok: true, email: 'u@exemplo.com', plano: 'max' })),
  cancelarLogin: vi.fn(async () => ({ ok: true })),
}));

const credMock = vi.mocked(credLib);
const apiMock = vi.mocked(apiLib);
const loginMock = vi.mocked(loginLib);

// Uma linha da lista unificada. `nome` é o que a tela mostra; `nome_natural` é o que vai pras
// rotas — a distinção é o que faz Entrar/Apagar continuarem certos depois de renomear.
function claude(over: Partial<Credencial> = {}): Credencial {
  return {
    id: `claude:${over.path ?? '/home/u/.claude-jefferson'}`,
    tipo: 'claude', nome: 'jefferson', nome_natural: 'jefferson', ativa: true,
    path: '/home/u/.claude-jefferson',
    login: { estado: 'ok', loggedIn: true, email: 'pessoa@exemplo.com', plano: 'max' },
    usos: [],
    cota: { estado: 'lida', janelas: [{ rotulo: '5h', pct: 64 }, { rotulo: '7d', pct: 83 }], ts: 1, idade_s: 5 },
    ...over,
  };
}
function chave(over: Partial<Credencial> = {}): Credencial {
  return {
    id: 'chave:kimi', tipo: 'chave', nome: 'Kimi', nome_natural: 'Kimi', ativa: false,
    base_url: 'https://api.kimi.com/coding/v1', chave_mascarada: 'sk-kimi••••4f2a',
    usos: ['claude_code'],
    cota: { estado: 'lida', janelas: [{ rotulo: '5h', pct: 5 }], ts: 1, idade_s: 5 },
    ...over,
  };
}
const LOGADA: Credencial = claude();
const DESLOGADA: Credencial = claude({
  id: 'claude:/home/u/.claude-testes', path: '/home/u/.claude-testes',
  nome: 'testes', nome_natural: 'testes', ativa: false,
  login: { estado: 'ok', loggedIn: false }, cota: { estado: 'sem_credencial', janelas: [] },
});

// Alvo EXPLÍCITO (outra máquina): é o contrato do parecer — a aba tem de afirmar o alvo, não
// só que a função foi chamada. Os casos abaixo passam a exigir `ALVO` como primeiro argumento;
// o caminho global (null) tem caso próprio no fim.
const ALVO: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 't-b' };

function montar(contas: Credencial[], alvo: Server | null = ALVO) {
  credMock.listarCredenciais.mockResolvedValue(contas);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ContasSettings, { target: el, props: { apiTarget: alvo } });
  return { el, comp: comp as never };
}

beforeEach(() => { vi.clearAllMocks(); });

describe('ContasSettings — a lista', () => {
  it('mostra conta com em uso e e-mail (a frase do plano ficou de fora por decisão do árbitro — Task de ajuste serial quando o Lote A mergear)', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    const linha = t.el.querySelector<HTMLElement>('.ct-linha')!;
    expect(linha.querySelector('.ct-nome')!.textContent).toBe('jefferson');
    expect(linha.querySelector('.ct-emuso')!.textContent).toBe(m.contas_em_uso());
    expect(linha.querySelector('.ct-sub')!.textContent).toContain('pessoa@exemplo.com');
    // A coluna do limite mostra as JANELAS do provedor, não a idade da leitura: é a mesma
    // fonte da faixa do rodapé (/api/cotas), uma leitura por credencial.
    expect(linha.querySelector('.ct-cota')!.textContent).toContain('64%');
    expect(linha.querySelector('.ct-cota')!.textContent).toContain('83%');
    unmount(t.comp);
  });

  it('conta deslogada CONTINUA na lista, com Entrar (inerte) e estado de leitura explícito', async () => {
    const t = montar([LOGADA, DESLOGADA]);
    await tick(); await tick();
    const linhas = t.el.querySelectorAll<HTMLElement>('.ct-linha');
    expect(linhas.length).toBe(2);
    const fora = linhas[1];
    expect(fora.textContent).toContain('testes');
    expect(fora.textContent).toContain(m.contas_nao_conectada());
    // Sem credencial legível a coluna diz o que fazer, em vez de mostrar 0%.
    expect(fora.textContent).toContain(m.cota_precisa_entrar());
    expect(fora.querySelector('.ct-acao.primaria')!.textContent).toBe(m.contas_entrar());
    unmount(t.comp);
  });

  it('conta logada por chave de API (sem e-mail) NAO pode dizer que nao esta conectada', async () => {
    // Shape REAL medido em 17/08 na máquina do usuário, conta before-merge-20260418-185405:
    // `claude auth status --json` -> {loggedIn:true, authMethod:"api_key"} SEM email.
    const API_KEY: Credencial = claude({
      id: 'claude:/home/u/.claude-before-merge', path: '/home/u/.claude-before-merge',
      nome: 'before-merge', nome_natural: 'before-merge', ativa: false,
      login: { estado: 'ok', loggedIn: true, email: null, plano: null },
      cota: { estado: 'indisponivel', janelas: [] },
    });
    const t = montar([API_KEY]);
    await tick(); await tick();
    const linha = t.el.querySelector<HTMLElement>('.ct-linha')!;
    expect(linha.textContent).not.toContain(m.contas_nao_conectada());
    expect(linha.querySelector('.ct-acao.primaria')).toBeNull();
    unmount(t.comp);
  });

  it('estado indisponivel nao pode ser confundido com deslogada', async () => {
    const INDISP: Credencial = claude({
      id: 'claude:/home/u/.claude-x', path: '/home/u/.claude-x',
      nome: 'x', nome_natural: 'x', ativa: false,
      login: { estado: 'indisponivel', motivo: 'cli-indisponivel' },
      cota: { estado: 'indisponivel', janelas: [] },
    });
    const t = montar([INDISP]);
    await tick(); await tick();
    const linha = t.el.querySelector<HTMLElement>('.ct-linha')!;
    expect(linha.textContent).not.toContain(m.contas_nao_conectada());
    expect(linha.querySelector('.ct-acao.primaria')).toBeNull();
    unmount(t.comp);
  });

  it('limite velho aparece com a idade e sem parecer fresco', async () => {
    const t = montar([claude({ ativa: false,
      cota: { estado: 'lida', janelas: [{ rotulo: '5h', pct: 3 }], ts: 1, idade_s: 7200 } })]);
    await tick(); await tick();
    const cota = t.el.querySelector<HTMLElement>('.ct-cota')!;
    expect(cota.textContent).toContain(m.cota_ultima_leitura({ n: '2 h' }));
    expect(cota.classList.contains('velha')).toBe(true);
    unmount(t.comp);
  });

  it('sessao-viva NÃO manda abrir sessão — a sessão já está aberta (queixa do usuário, 19/08)', async () => {
    // Conta com sessão rodando e access token vencido: o CLI dela renova sozinho. "Abra uma
    // sessão nela" era a frase errada — o usuário leu isso estando DENTRO da sessão.
    const t = montar([claude({ ativa: true,
      cota: { estado: 'expirada', janelas: [], motivo: 'sessao-viva' } })]);
    await tick(); await tick();
    const linha = t.el.querySelector<HTMLElement>('.ct-linha')!;
    expect(linha.textContent).toContain(m.cota_sessao_viva());
    expect(linha.textContent).not.toContain(m.cota_conta_parada());
    unmount(t.comp);
  });

  it('renovacao-falhou continua mandando abrir uma sessão (é o gesto que renova)', async () => {
    const t = montar([claude({ ativa: false,
      cota: { estado: 'expirada', janelas: [], motivo: 'renovacao-falhou' } })]);
    await tick(); await tick();
    expect(t.el.querySelector<HTMLElement>('.ct-linha')!.textContent)
      .toContain(m.cota_conta_parada());
    unmount(t.comp);
  });
});

describe('ContasSettings — criar e apagar reusam as rotas de sempre', () => {
  it('+ Nova conta abre o catálogo de provedores (a escolha saiu da linha do rodapé)', async () => {
    // O rodapé tem UM botão. A escolha do provedor e o formulário vivem no modal
    // (NovaCredencialSheet): inline, a pergunta e as opções viravam cinco controles competindo
    // pela mesma linha — o que o usuário apontou em 18/08.
    const t = montar([LOGADA]);
    await tick(); await tick();
    const rodape = [...t.el.querySelectorAll<HTMLButtonElement>('.ct-rodape .ct-btn')];
    expect(rodape.map((b) => b.textContent)).toEqual([m.contas_nova()]);
    rodape[0].click();
    await tick(); await tick();
    // O catálogo é do modal, que monta em document.body (BottomSheet), não dentro da aba.
    expect(document.body.textContent).toContain(m.novacred_custom_nome());
    expect(document.body.textContent).toContain(m.novacred_claude_nome());
    unmount(t.comp);
  });

  it('apagar: kebab → menu → confirmação → apagarConta com o NOME (label) da conta', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-kebab')!.click();
    await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-menu-item')!.click();
    await tick();
    expect(t.el.querySelector('.ct-confirma')!.textContent).toContain(m.criar_apagar_fim());
    t.el.querySelector<HTMLButtonElement>('.ct-confirma-btn.perigo')!.click();
    await tick(); await tick();
    expect(apiMock.apagarConta).toHaveBeenCalledWith(ALVO, 'jefferson');
    expect(credMock.listarCredenciais).toHaveBeenCalledTimes(2);
    unmount(t.comp);
  });

  it('falha ao apagar mostra a mensagem traduzida do backend e nao engana com sucesso (Task 4)', async () => {
    // O usuário viu "não foi" sem mensagem nenhuma ao apagar pasta de backup: o aviso sumia
    // com o erro engolido. O detalhe do DELETE vira envelope {code, params, msg}; a mensagem
    // tem que chegar à tela TRADUZIDA. Este teste cai se o erro voltar a ser engolido
    // (sem .ct-aviso.erro, ou com o texto cru do servidor no lugar da chave).
    apiMock.apagarConta.mockRejectedValueOnce(
      Object.assign(new Error(mensagemDeErro('erro_conta_inexistente', { nome: 'backup' })!),
        { status: 404 }));
    const t = montar([LOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-kebab')!.click();
    await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-menu-item')!.click();
    await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-confirma-btn.perigo')!.click();
    await tick(); await tick();
    const aviso = t.el.querySelector<HTMLElement>('.ct-aviso.erro');
    expect(aviso).not.toBeNull();
    expect(aviso!.textContent).toContain(m.erro_conta_inexistente({ nome: 'backup' }));
    // Sem recarga da lista (não há sucesso pra recarregar) e sem fechar a confirmação.
    expect(credMock.listarCredenciais).toHaveBeenCalledTimes(1);
    expect(t.el.querySelector('.ct-confirma')).not.toBeNull();
    unmount(t.comp);
  });
});

describe('ContasSettings — o botão Entrar (Task 7)', () => {
  it('mostra os quatro passos do mock estado 2 quando começa o login', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(loginMock.iniciarLogin).toHaveBeenCalledWith(ALVO, 'testes');
    expect(t.el.querySelector('.ct-login')).not.toBeNull();
    const passos = [...t.el.querySelectorAll<HTMLElement>('.ct-passo')];
    expect(passos.length).toBe(4);
    expect(passos[0].textContent).toContain(m.contas_passo1());
    expect(passos[1].textContent).toContain(m.contas_passo2());
    expect(passos[2].textContent).toContain(m.contas_passo3());
    expect(passos[3].textContent).toContain(m.contas_passo4());
    // O link de autorização (quando o poll devolve a URL) é um <a> de verdade.
    const link = t.el.querySelector<HTMLAnchorElement>('.ct-link')!;
    expect(link).not.toBeNull();
    expect(link.getAttribute('href')).toContain('https://claude.com/cai/oauth/authorize');
    unmount(t.comp);
  });

  it('confirmar o código chama confirmarLogin e recarrega a lista', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const input = t.el.querySelector<HTMLInputElement>('.ct-campo-cod')!;
    input.value = 'CODE-123';
    input.dispatchEvent(new Event('input'));
    await tick();
    // O botão Confirmar código fica no SEGUNDO .ct-rodape (o primeiro é o da lista).
    const rodapeLogin = [...t.el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('.ct-btn.primario')!.click();
    await tick(); await tick();
    expect(loginMock.confirmarLogin).toHaveBeenCalledWith(ALVO, 'testes', 'CODE-123');
    // O pós-login recarrega a lista (o poll do passo não a recarrega — só o /passo).
    expect(credMock.listarCredenciais.mock.calls.length).toBe(2);
    unmount(t.comp);
  });

  it('desmontar durante o login cancela a tentativa e para o poll (B8)', async () => {
    // B8 — desmontar por QUALQUER porta (trocar de aba, fechar o modal, janela cruzar
    // 820px) não pode deixar o poll órfão nem a tentativa presa no servidor: o próximo
    // Entrar começaria do zero, sem 409.
    vi.useFakeTimers();
    try {
      const t = montar([DESLOGADA]);
      await tick(); await tick();
      t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
      await tick(); await tick(); await tick(); await tick();
      expect(loginMock.iniciarLogin).toHaveBeenCalledWith(ALVO, 'testes');
      unmount(t.comp);
      expect(loginMock.cancelarLogin).toHaveBeenCalledWith(ALVO, 'testes');
      const passosAntes = loginMock.passoLogin.mock.calls.length;
      // Depois do unmount o poll não pode mais bater em /login/passo.
      await vi.advanceTimersByTimeAsync(6000);
      expect(loginMock.passoLogin.mock.calls.length).toBe(passosAntes);
    } finally {
      vi.useRealTimers();
    }
  });

  it('desmontar ENQUANTO o iniciarLogin esta em voo cancela e nao arma poll (B4)', async () => {
    // B8 fechou a porta "desmontar com o painel jah aberto"; esta e a porta que o
    // onDestroy nao via: desmontar ENTRE o clique e a resposta do servidor. O loginDe so
    // e escrito depois do await — sem a flag `destruido`, o onDestroy nao cancela nada e
    // o setInterval fica orfao batendo em /login/passo (e a tentativa presa no servidor).
    vi.useFakeTimers();
    try {
      let resolver!: () => void;
      loginMock.iniciarLogin.mockReturnValue(
        new Promise<void>((r) => { resolver = r; }) as never);
      const t = montar([DESLOGADA]);
      await tick(); await tick();
      t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
      await tick(); await tick(); await tick(); await tick();
      expect(loginMock.iniciarLogin).toHaveBeenCalledWith(ALVO, 'testes');
      unmount(t.comp);      // desmonta ANTES de o iniciarLogin resolver
      resolver();           // a resposta chega num componente morto
      await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
      await tick();
      expect(loginMock.cancelarLogin).toHaveBeenCalledWith(ALVO, 'testes');
      const passosAntes = loginMock.passoLogin.mock.calls.length;
      await vi.advanceTimersByTimeAsync(6000);
      expect(loginMock.passoLogin.mock.calls.length).toBe(passosAntes);
    } finally {
      vi.useRealTimers();
    }
  });

  it('alvo null (sem ?srv=) segue pelo caminho global: chamadas com null (parecer)', async () => {
    // Comportamento final do parecer: sem ?srv= (ou srv == ativo) o alvo é null e tudo segue
    // como hoje — o caminho global com self-heal de 401.
    const t = montar([DESLOGADA], null);
    await tick(); await tick();
    expect(credMock.listarCredenciais).toHaveBeenCalledWith(null);
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(loginMock.iniciarLogin).toHaveBeenCalledWith(null, 'testes');
    unmount(t.comp);
  });

  it('trocar de alvo com login aberto cancela no alvo ANTIGO e limpa o painel (guard do parecer)', async () => {
    // A tentativa em voo vive no processo do backend do alvo antigo: trocar o ?srv= no meio
    // tem de cancelar LÁ (no capturado), nunca no alvo novo.
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' };
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    credMock.listarCredenciais.mockResolvedValue([DESLOGADA]);
    await tick(); await tick(); await tick();
    el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(loginMock.iniciarLogin).toHaveBeenCalledWith(A, 'testes');
    props.apiTarget = B;   // ?srv= trocou com a tentativa em voo
    await tick(); await tick(); await tick();
    expect(loginMock.cancelarLogin).toHaveBeenCalledWith(A, 'testes');
    expect(el.querySelector('.ct-login')).toBeNull();
    unmount(comp as never);
  });

  it('trocar de alvo com carga em voo descarta a resposta do alvo antigo (guard do parecer)', async () => {
    // A lista do alvo A (pendurada) chega ATRASADA, depois da troca pra B: não pode escrever
    // na tela nem sobrescrever a lista de B — o apagar clicado na lista errada sairia para a
    // máquina errada (o defeito que a Task 5 antiga levou duas rodadas pra fechar).
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' };
    let resolverA!: (v: Credencial[]) => void;
    credMock.listarCredenciais.mockImplementation((alvo: Server | null) =>
      alvo?.id === 'srv-b'
        ? Promise.resolve([claude({ id: 'claude:/home/u/.claude-b', nome: 'bbbb',
                                     nome_natural: 'bbbb', path: '/home/u/.claude-b' })])
        : new Promise<Credencial[]>((r) => { resolverA = r; }));
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    await tick(); await tick();
    props.apiTarget = B;
    await tick(); await tick(); await tick();
    resolverA([LOGADA]);   // resposta atrasada do alvo que saiu da tela
    await tick(); await tick();
    // A lista na tela é a de B; a resposta atrasada de A (label 'jefferson') foi descartada.
    expect([...el.querySelectorAll('.ct-nome')].map((n) => n.textContent)).toEqual(['bbbb']);
    unmount(comp as never);
  });

  it('objeto NOVO com a MESMA identidade não é troca de alvo: login em voo sobrevive (R1)', async () => {
    // O App reconstrói o Server a cada listServers() (JSON.parse do localStorage) e o sync sobe
    // versaoServidores sem o usuário tocar na aba — comparar o OBJETO mataria o login em voo com
    // o servidor sendo o mesmo. O guard compara a identidade composta (id+label+baseUrl+token).
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    credMock.listarCredenciais.mockResolvedValue([DESLOGADA]);
    await tick(); await tick(); await tick();
    el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(loginMock.iniciarLogin).toHaveBeenCalledWith(A, 'testes');
    const chamadas = credMock.listarCredenciais.mock.calls.length;
    props.apiTarget = { ...A };   // MESMO servidor, objeto reconstruído
    await tick(); await tick(); await tick();
    expect(loginMock.cancelarLogin).not.toHaveBeenCalled();
    expect(el.querySelector('.ct-login')).not.toBeNull();
    expect(credMock.listarCredenciais.mock.calls.length).toBe(chamadas);
    unmount(comp as never);
  });

  it('trocar de alvo com confirmar em voo: erro atrasado do alvo ANTIGO não pinta a tela da nova (rodada 2)', async () => {
    // O efeito limpou a tela na troca; a resposta atrasada de A não pode repintar o erro. O
    // `{#if loginErro}` é FORA do `{#if loginDe}` de propósito — por isso a ausência é o alvo.
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' };
    let rejeitar!: (e: Error) => void;
    loginMock.confirmarLogin.mockReturnValue(
      new Promise((_r, j) => { rejeitar = j; }) as never);
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    credMock.listarCredenciais.mockResolvedValue([DESLOGADA]);
    await tick(); await tick(); await tick();
    el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const input = el.querySelector<HTMLInputElement>('.ct-campo-cod')!;
    input.value = 'CODE-123';
    input.dispatchEvent(new Event('input'));
    await tick();
    const rodapeLogin = [...el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('.ct-btn.primario')!.click();
    await tick(); await tick();
    expect(loginMock.confirmarLogin).toHaveBeenCalledWith(A, 'testes', 'CODE-123');
    props.apiTarget = B;   // ?srv= trocou com o confirmar em voo
    await tick(); await tick(); await tick();
    rejeitar(Object.assign(new Error('ERRO-DA-MAQUINA-A'), { status: 0 }));   // resposta atrasada de A
    await tick(); await tick();
    expect(el.querySelector('.ct-aviso.erro')).toBeNull();
    unmount(comp as never);
  });

  it('trocar de alvo com confirmar em voo: sucesso atrasado do alvo ANTIGO não anuncia login na tela da nova (rodada 2)', async () => {
    // A pior forma do bloqueador: a aba de B anuncia, com e-mail e plano, um login que aconteceu
    // em A — o usuário age nisso (abre sessão em B esperando aquela conta).
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' };
    let resolver!: (v: { ok: boolean; email: string; plano: string }) => void;
    loginMock.confirmarLogin.mockReturnValue(
      new Promise((r) => { resolver = r; }) as never);
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    credMock.listarCredenciais.mockResolvedValue([DESLOGADA]);
    await tick(); await tick(); await tick();
    el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const input = el.querySelector<HTMLInputElement>('.ct-campo-cod')!;
    input.value = 'CODE-123';
    input.dispatchEvent(new Event('input'));
    await tick();
    const rodapeLogin = [...el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('.ct-btn.primario')!.click();
    await tick(); await tick();
    props.apiTarget = B;   // ?srv= trocou com o confirmar em voo
    await tick(); await tick(); await tick();
    resolver({ ok: true, email: 'conta-da-MAQUINA-A@x.com', plano: 'max' });   // resposta atrasada de A
    await tick(); await tick();
    // Nenhum aviso nenhum: o efeito limpou na troca e a resposta atrasada não repintou.
    expect([...el.querySelectorAll<HTMLElement>('.ct-aviso')]).toHaveLength(0);
    unmount(comp as never);
  });

  it('confirmar com erro ainda recarrega a lista — o login pode ter completado no servidor (parecer passo 4)', async () => {
    // Teto estourado corta o cliente mas o backend SEGUE: a conta acaba logada. A lista tem de
    // recarregar mesmo no ramo de erro, senão a tela mente sozinha dizendo deslogada.
    loginMock.confirmarLogin.mockRejectedValueOnce(
      Object.assign(new Error(m.erro_login_timeout()), { status: 0 }));
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const input = t.el.querySelector<HTMLInputElement>('.ct-campo-cod')!;
    input.value = 'CODE-123';
    input.dispatchEvent(new Event('input'));
    await tick();
    const rodapeLogin = [...t.el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('.ct-btn.primario')!.click();
    await tick(); await tick(); await tick();
    // montagem + recarga do ramo de erro: a lista não fica presa no estado antigo.
    expect(credMock.listarCredenciais.mock.calls.length).toBe(2);
    expect(t.el.querySelector<HTMLElement>('.ct-aviso.erro')!.textContent).toContain(m.erro_login_timeout());
    unmount(t.comp);
  });

  it('confirmar com campo vazio não chama a API', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const confirmar = t.el.querySelector<HTMLButtonElement>('.ct-rodape .ct-btn.primario')!;
    expect(confirmar.disabled).toBe(true);
    expect(loginMock.confirmarLogin).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('cancelar chama cancelarLogin, limpa o painel e reabilita o Entrar', async () => {
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    // O rodapé do login é o SEGUNDO .ct-rodape (o primeiro é o rodapé da lista, "+ Nova conta").
    const rodapeLogin = [...t.el.querySelectorAll<HTMLElement>('.ct-rodape')][1];
    rodapeLogin.querySelector<HTMLButtonElement>('button:not(.primario)')!.click();
    await tick(); await tick();
    expect(loginMock.cancelarLogin).toHaveBeenCalledWith(ALVO, 'testes');
    expect(t.el.querySelector('.ct-login')).toBeNull();
    expect(t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.disabled).toBe(false);
    unmount(t.comp);
  });

  it('erro do servidor (409 envelope) vira mensagem traduzida no aviso', async () => {
    // O backend manda {code, params, msg} (mensagens.py); o cliente deve devolver a MENSAGEM
    // traduzida pelo id, nunca o texto cru do servidor.
    loginMock.iniciarLogin.mockRejectedValueOnce(
      Object.assign(new Error(mensagemDeErro('erro_login_ja_em_curso')!), { status: 409 }));
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    expect(t.el.querySelector('.ct-login')).toBeNull();
    expect(t.el.querySelector<HTMLElement>('.ct-aviso.erro')!.textContent)
      .toContain(m.erro_login_ja_em_curso());
    unmount(t.comp);
  });

  it('erro de rede nao aparece como Failed to fetch cru', async () => {
    loginMock.iniciarLogin.mockRejectedValueOnce(new Error('Failed to fetch'));
    const t = montar([DESLOGADA]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-acao.primaria')!.click();
    await tick(); await tick(); await tick(); await tick();
    const aviso = t.el.querySelector<HTMLElement>('.ct-aviso.erro')!;
    expect(aviso.textContent).toContain(m.falha_conexao());
    expect(aviso.textContent).not.toContain('Failed to fetch');
    unmount(t.comp);
  });
});

describe('ContasSettings — botão de atualizar do cabeçalho', () => {
  // Referência Cloudscape: refresh no cabeçalho, lista VISÍVEL durante a busca, erro mantém os
  // dados velhos e vira aviso. O `forcar=true` é o que diferencia o botão do carregar inicial:
  // pede a leitura de cota de AGORA, não o cache de 5 min do servidor.
  it('o clique pede a leitura forçada (forcar=true) e mostra o "atualizado há"', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    // A carga inicial NÃO força: cache de 5 min vale pra abrir a tela.
    expect(credMock.listarCredenciais).toHaveBeenCalledWith(ALVO);
    // O carimbo nasce da carga inicial (idade < 1 min → o piso do formatarIntervalo é "1 min").
    expect(t.el.querySelector('.ct-atualizado')!.textContent)
      .toBe(m.contas_atualizado_ha({ n: '1 min' }));

    t.el.querySelector<HTMLButtonElement>('.ct-refresh')!.click();
    await tick(); await tick();
    expect(credMock.listarCredenciais).toHaveBeenLastCalledWith(ALVO, true);
    unmount(t.comp);
  });

  it('a lista NÃO some durante o refresh (nada de "Carregando" no lugar das contas)', async () => {
    let solta!: (v: Credencial[]) => void;
    const t = montar([LOGADA]);
    await tick(); await tick();
    credMock.listarCredenciais.mockReturnValueOnce(
      new Promise<Credencial[]>((res) => { solta = res; }));

    t.el.querySelector<HTMLButtonElement>('.ct-refresh')!.click();
    await tick();
    // Com a busca EM VOO, a linha da conta continua na tela e não há texto de carregando.
    expect(t.el.querySelector('.ct-linha')).not.toBeNull();
    expect(t.el.textContent).not.toContain(m.comum_carregando());
    expect(t.el.querySelector<HTMLButtonElement>('.ct-refresh')!.disabled).toBe(true);

    solta([LOGADA]);
    await tick(); await tick();
    expect(t.el.querySelector<HTMLButtonElement>('.ct-refresh')!.disabled).toBe(false);
    unmount(t.comp);
  });

  it('erro no refresh mantém os dados velhos e vira aviso embaixo da lista', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    credMock.listarCredenciais.mockRejectedValueOnce(new Error('sem rota'));

    t.el.querySelector<HTMLButtonElement>('.ct-refresh')!.click();
    await tick(); await tick();
    expect(t.el.querySelector('.ct-linha')).not.toBeNull();
    expect(t.el.querySelector<HTMLElement>('.ct-aviso.erro')!.textContent).toContain('sem rota');
    unmount(t.comp);
  });
});

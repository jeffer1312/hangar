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
import * as apiLib from '@hangar/core';
import * as loginLib from '../../lib/loginConta';
import { mensagemDeErro } from '@hangar/core';
import * as credLib from '../../lib/credenciais';
import type { Credencial } from '../../lib/credenciais';
import type { Server } from '../../lib/auth';
import { clienteQuery } from '../../lib/queries';

vi.mock('../../lib/credenciais', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/credenciais')>();
  // `sincronizarNosAgentes` entra no mock porque o MotorForm a chama DEPOIS do PUT: real, ela
  // faria um POST de verdade no meio do teste do card.
  return {
    ...real,
    listarCredenciais: vi.fn(),
    definirApelido: vi.fn(async () => ({ id: 'x', apelido: null })),
    sincronizarNosAgentes: vi.fn(async () => ({ resultado: { pi: { ok: true, motivo: '' } } })),
  };
});
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  criarConta: vi.fn(async () => ({ path: '/x', label: 'x', active: false })),
  apagarConta: vi.fn(async () => {}),
  putEngine: vi.fn(async () => ({ motores: {} })),
  putEngineForServer: vi.fn(async () => ({ motores: {} })),
  deleteEngine: vi.fn(async () => ({ ok: true })),
  deleteEngineForServer: vi.fn(async () => ({ ok: true })),
  getEngines: vi.fn(async () => ({ motores: {}, arquivo_corrompido: false, arquivo_caminho: '' })),
  getEnginesForServer: vi.fn(async () => ({ motores: {}, arquivo_corrompido: false, arquivo_caminho: '' })),
  engineModelos: vi.fn(async () => ({ modelos: [] })),
  engineModelosForServer: vi.fn(async () => ({ modelos: [] })),
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

// O cache das queries é um singleton de módulo e sobrevive à desmontagem — de propósito, é o que
// faz reabrir a tela ser instantâneo. Num teste isso vaza: sem limpar, o caso seguinte monta e
// recebe a lista do ANTERIOR em vez de chamar o mock dele.
beforeEach(() => { vi.clearAllMocks(); clienteQuery.clear(); });

describe('ContasSettings — a lista', () => {
  it('mostra conta com em uso e e-mail (a frase do plano ficou de fora por decisão do árbitro — Task de ajuste serial quando o Lote A mergear)', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    const linha = t.el.querySelector<HTMLElement>('.ct-card')!;
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
    const linhas = t.el.querySelectorAll<HTMLElement>('.ct-card');
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
    const linha = t.el.querySelector<HTMLElement>('.ct-card')!;
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
    const linha = t.el.querySelector<HTMLElement>('.ct-card')!;
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
    const linha = t.el.querySelector<HTMLElement>('.ct-card')!;
    expect(linha.textContent).toContain(m.cota_sessao_viva());
    expect(linha.textContent).not.toContain(m.cota_conta_parada());
    unmount(t.comp);
  });

  it('renovacao-falhou continua mandando abrir uma sessão (é o gesto que renova)', async () => {
    const t = montar([claude({ ativa: false,
      cota: { estado: 'expirada', janelas: [], motivo: 'renovacao-falhou' } })]);
    await tick(); await tick();
    expect(t.el.querySelector<HTMLElement>('.ct-card')!.textContent)
      .toContain(m.cota_conta_parada());
    unmount(t.comp);
  });
});

describe('ContasSettings — criar e apagar reusam as rotas de sempre', () => {
  it('+ Adicionar (cabeçalho) abre a folha perguntando O QUÊ criar', async () => {
    // O botão saiu do rodapé da lista e virou o primeiro controle do cabeçalho — criar era o
    // 13º item da tela. A folha (NovaCredencialSheet, montada em document.body) passa a abrir
    // no passo "o quê": conta do Claude, modelo pro Claude Code ou chave de outro agente; o
    // catálogo de provedores só aparece depois dessa escolha.
    const t = montar([LOGADA]);
    await tick(); await tick();
    const add = t.el.querySelector<HTMLButtonElement>(`.ct-cab button[aria-label="${m.contas_add_aria()}"]`)!;
    expect(add).not.toBeNull();
    expect(add.textContent).toContain(m.contas_add());
    add.click();
    await tick(); await tick();
    expect(document.body.textContent).toContain(m.contas_add_conta());
    expect(document.body.textContent).toContain(m.contas_add_modelo());
    expect(document.body.textContent).toContain(m.contas_add_chave());
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
    // O rodapé do login é o único que sobrou (o da lista virou o botão do cabeçalho).
    const rodapeLogin = t.el.querySelector<HTMLElement>('.ct-rodape.login')!;
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
    const rodapeLogin = el.querySelector<HTMLElement>('.ct-rodape.login')!;
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
    const rodapeLogin = el.querySelector<HTMLElement>('.ct-rodape.login')!;
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
    const rodapeLogin = t.el.querySelector<HTMLElement>('.ct-rodape.login')!;
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
    // O rodapé do login é o único .ct-rodape da tela (o da lista virou botão do cabeçalho).
    const rodapeLogin = t.el.querySelector<HTMLElement>('.ct-rodape.login')!;
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
  // `.ct-refresh` hoje casa DOIS botões (o primeiro é o de densidade) — o de atualizar se
  // distingue pelo aria-label.
  const refresh = (el: HTMLElement) =>
    el.querySelector<HTMLButtonElement>(`button[aria-label="${m.contas_atualizar()}"]`)!;
  it('o clique pede a leitura forçada (forcar=true) e mostra o "atualizado há"', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    // A carga inicial NÃO força: cache de 5 min vale pra abrir a tela.
    expect(credMock.listarCredenciais).toHaveBeenCalledWith(ALVO);
    // O carimbo nasce da carga inicial (idade < 1 min → o piso do formatarIntervalo é "1 min").
    expect(t.el.querySelector('.ct-atualizado')!.textContent)
      .toBe(m.contas_atualizado_ha({ n: '1 min' }));

    refresh(t.el).click();
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

    refresh(t.el).click();
    await tick();
    // Com a busca EM VOO, a linha da conta continua na tela e não há texto de carregando.
    expect(t.el.querySelector('.ct-card')).not.toBeNull();
    expect(t.el.textContent).not.toContain(m.comum_carregando());
    expect(refresh(t.el).disabled).toBe(true);

    solta([LOGADA]);
    await tick(); await tick();
    expect(refresh(t.el).disabled).toBe(false);
    unmount(t.comp);
  });

  it('erro no refresh mantém os dados velhos e vira aviso embaixo da lista', async () => {
    const t = montar([LOGADA]);
    await tick(); await tick();
    credMock.listarCredenciais.mockRejectedValueOnce(new Error('sem rota'));

    refresh(t.el).click();
    await tick(); await tick();
    expect(t.el.querySelector('.ct-card')).not.toBeNull();
    expect(t.el.querySelector<HTMLElement>('.ct-aviso.erro')!.textContent).toContain('sem rota');
    unmount(t.comp);
  });
});

describe('ContasSettings — modelo e opções da chave (Contas e modelos)', () => {
  const MOTORES = {
    motores: { kimi: { label: 'Kimi', base_url: 'https://api.kimi.com/coding', model: 'kimi-k3', context_window: 256000, api_key: 'sk-k••••4f2a', api_key_definida: true } },
    arquivo_corrompido: false, arquivo_caminho: '',
  };
  const SEM_MOTORES = { motores: {}, arquivo_corrompido: false, arquivo_caminho: '' };
  const botao = (raiz: Element, rotulo: string) =>
    [...raiz.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === rotulo)!;
  const abrirMotor = (raiz: Element) => botao(raiz, m.config_motores_editar()).click();
  // `vi.clearAllMocks()` do arquivo limpa CONTAGENS, não `mockResolvedValue`/`mockRejectedValue`:
  // sem repor aqui, o caso do arquivo corrompido (e o do erro) vazaria para o seguinte.
  beforeEach(() => {
    apiMock.getEnginesForServer.mockResolvedValue(SEM_MOTORES as never);
    apiMock.getEngines.mockResolvedValue(SEM_MOTORES as never);
    // O PUT devolve o mapa INTEIRO, e é dele que o card e o `{#if motor}` passam a viver: sem
    // repor, o `{ motores: {} }` do mock do arquivo desmontaria o bloco a cada Salvar.
    apiMock.putEngineForServer.mockResolvedValue({ motores: MOTORES.motores } as never);
    credMock.sincronizarNosAgentes.mockResolvedValue({ resultado: { pi: { ok: true, motivo: '' } } } as never);
  });

  it('chave presente no engines.json mostra o modelo no card e o botão de abrir', async () => {
    apiMock.getEnginesForServer.mockResolvedValue(MOTORES as never);
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    expect(apiMock.getEnginesForServer).toHaveBeenCalledWith(ALVO);
    // A janela mudou de lugar (virou chip), não sumiu: a linha do modelo é só o id do modelo.
    expect(t.el.querySelector('.ct-modelo')!.textContent).toBe('kimi-k3');
    expect(t.el.textContent).toContain(m.contas_chip_janela({ n: '256k' }));
    const btn = [...t.el.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === m.config_motores_editar());
    expect(btn).toBeDefined();
    unmount(t.comp);
  });

  it('abrir mostra o formulário DENTRO do card e esconde o endereço repetido; salvar atualiza o card sem GET novo e o bloco segue aberto', async () => {
    apiMock.getEnginesForServer.mockResolvedValue(MOTORES as never);
    apiMock.putEngineForServer.mockResolvedValueOnce({ motores: { kimi: { ...MOTORES.motores.kimi, model: 'kimi-k2.7' } } } as never);
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    expect(t.el.querySelector('.ct-card')!.textContent).toContain('https://api.kimi.com/coding/v1');
    [...t.el.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === m.config_motores_editar())!.click();
    await tick();
    const card = t.el.querySelector('.ct-card')!;
    expect(card.textContent).toContain(m.config_motores_avancado());
    // "Um dado, um lugar": com o bloco aberto o card não repete endereço · chave nem o modelo.
    expect(card.querySelector('.ct-sub')).toBeNull();
    [...card.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === m.ctx_salvar())!.click();
    await tick(); await tick(); await tick();
    expect(apiMock.putEngineForServer).toHaveBeenCalledWith(ALVO, 'kimi', expect.objectContaining({ model: 'kimi-k3' }));
    expect(apiMock.getEnginesForServer).toHaveBeenCalledTimes(1);
    // Continua aberto (o resultado da sincronização mora aí); fechar devolve o card com o modelo novo.
    expect(card.textContent).toContain(m.config_motores_avancado());
    [...card.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.trim() === m.sessao_fechar())!.click();
    await tick();
    expect(t.el.querySelector('.ct-modelo')!.textContent).toBe('kimi-k2.7');
    unmount(t.comp);
  });

  it('salvar DUAS vezes na mesma instância não deixa o resultado da sincronização anterior na tela', async () => {
    // O bloco não fecha depois de salvar, então a mesma instância salva de novo. Sem zerar a área
    // de resultado a cada Salvar, a sincronização da gravação ANTERIOR fica na tela ao lado do
    // erro desta — foi o defeito que a Task 1 pagou uma rodada.
    apiMock.getEnginesForServer.mockResolvedValue(MOTORES as never);
    // 1º Salvar sincroniza com sucesso, 2º falha: é o segundo que tem de deixar a tela sem a
    // linha verde do primeiro.
    credMock.sincronizarNosAgentes
      .mockResolvedValueOnce({ resultado: { pi: { ok: true, motivo: '' } } } as never)
      .mockRejectedValueOnce(new Error('sync caiu'));
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    abrirMotor(t.el); await tick();
    const card = t.el.querySelector('.ct-card')!;
    botao(card, m.ctx_salvar()).click();
    await tick(); await tick(); await tick(); await tick();
    expect(card.textContent).toContain(m.novacred_sync_ok());
    // Segundo Salvar: a linha verde do primeiro não pode sobreviver ao lado do erro deste.
    botao(card, m.ctx_salvar()).click();
    await tick(); await tick(); await tick(); await tick();
    expect(card.textContent).toContain('sync caiu');
    expect(card.textContent).not.toContain(m.novacred_sync_ok());
    unmount(t.comp);
  });

  it('abrir, fechar e abrir de novo entrega um formulário com o dado ATUAL, não o da primeira vez', async () => {
    apiMock.getEnginesForServer.mockResolvedValue(MOTORES as never);
    apiMock.putEngineForServer.mockResolvedValueOnce({ motores: { kimi: { ...MOTORES.motores.kimi, model: 'kimi-k2.7' } } } as never);
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    abrirMotor(t.el); await tick();
    const card = t.el.querySelector('.ct-card')!;
    botao(card, m.ctx_salvar()).click();
    await tick(); await tick(); await tick(); await tick();
    botao(card, m.sessao_fechar()).click();
    await tick();
    expect(t.el.querySelector('.mf')).toBeNull();
    // Reabrir monta um MotorForm NOVO: o retrato tem de sair do motor de agora (kimi-k2.7), não
    // do que estava no mapa quando o bloco abriu da primeira vez.
    abrirMotor(t.el); await tick();
    expect(t.el.querySelector<HTMLInputElement>('input[name="model"]')!.value).toBe('kimi-k2.7');
    unmount(t.comp);
  });

  it('trocar de alvo com o Salvar do motor EM VOO não escreve a resposta da máquina antiga no cache da nova', async () => {
    // Mesmo guard das outras mutações da tela: o MotorForm é desmontado na troca, mas o PUT em
    // voo segue vivo e devolveria o mapa do alvo ANTIGO sob a chave do NOVO — o card da máquina
    // nova mostraria um modelo que ela nunca teve.
    const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' };
    apiMock.getEnginesForServer.mockResolvedValue(MOTORES as never);
    credMock.listarCredenciais.mockResolvedValue([chave()]);
    let soltaPut!: (v: unknown) => void;
    apiMock.putEngineForServer.mockReturnValueOnce(new Promise((r) => { soltaPut = r as never; }) as never);
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    await tick(); await tick(); await tick();
    abrirMotor(el); await tick();
    botao(el.querySelector('.ct-card')!, m.ctx_salvar()).click();
    await tick();
    props.apiTarget = B;   // ?srv= trocou com o PUT em voo
    await tick(); await tick(); await tick();
    soltaPut({ motores: { kimi: { ...MOTORES.motores.kimi, model: 'da-maquina-antiga' } } });
    await tick(); await tick(); await tick();
    expect(clienteQuery.getQueryData(['motores', 'srv-b'])).not.toEqual(
      expect.objectContaining({ motores: expect.objectContaining({ kimi: expect.objectContaining({ model: 'da-maquina-antiga' }) }) }));
    unmount(comp as never);
  });

  // `carregar()` é o ponto único: é ele que o `onCriada` do NovaCredencialSheet e o apagar
  // chamam. Apagar o exercita de ponta a ponta — o caminho de criar passa pela mesma função.
  it('apagar uma chave revalida os motores (o botão nasce junto com a credencial)', async () => {
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    expect(apiMock.getEnginesForServer).toHaveBeenCalledTimes(1);
    t.el.querySelector<HTMLButtonElement>('.ct-kebab')!.click();
    await tick();
    [...t.el.querySelectorAll<HTMLButtonElement>('.ct-menu-item')].find((b) => b.textContent?.trim() === m.comum_apagar())!.click();
    await tick();
    [...t.el.querySelectorAll<HTMLButtonElement>('.ct-confirma-btn')].find((b) => b.textContent?.trim() === m.comum_apagar())!.click();
    await tick(); await tick(); await tick();
    expect(apiMock.deleteEngineForServer).toHaveBeenCalledWith(ALVO, 'kimi');
    expect(apiMock.getEnginesForServer).toHaveBeenCalledTimes(2);
    unmount(t.comp);
  });

  it('erro ao ler os motores aparece na tela em vez de sumir com o botão calado', async () => {
    apiMock.getEnginesForServer.mockRejectedValue(new Error('HTTP 500'));
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    expect(t.el.textContent).toContain('HTTP 500');
    expect(t.el.textContent).not.toContain(m.config_motores_editar());
    unmount(t.comp);
  });

  // Sem a lista de nomes na mão, o formulário de criação não tem como recusar um nome curto já
  // ocupado — e o PUT é substituição, então criar apagaria o motor do outro, calado. Enquanto a
  // consulta não voltou, ou se ela falhou, criar fica inerte: mesmo tratamento do arquivo corrompido.
  it('consulta de modelos ainda pendente: + Adicionar fica desabilitado', async () => {
    apiMock.getEnginesForServer.mockReturnValue(new Promise(() => {}) as never);
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    const nova = t.el.querySelector<HTMLButtonElement>(`button[aria-label="${m.contas_add_aria()}"]`)!;
    expect(nova.disabled).toBe(true);
    unmount(t.comp);
  });

  it('consulta de modelos que falhou: + Adicionar fica desabilitado e o erro aparece', async () => {
    apiMock.getEnginesForServer.mockRejectedValue(new Error('HTTP 500'));
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    const nova = t.el.querySelector<HTMLButtonElement>(`button[aria-label="${m.contas_add_aria()}"]`)!;
    expect(nova.disabled).toBe(true);
    expect(t.el.textContent).toContain('HTTP 500');
    unmount(t.comp);
  });

  // Controle: com a consulta boa o botão volta a funcionar. Sem ele, um `disabled` chumbado em
  // true passaria nos dois casos acima.
  it('consulta de modelos que voltou: + Adicionar fica habilitado', async () => {
    apiMock.getEnginesForServer.mockResolvedValue(SEM_MOTORES as never);
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    const nova = t.el.querySelector<HTMLButtonElement>(`button[aria-label="${m.contas_add_aria()}"]`)!;
    expect(nova.disabled).toBe(false);
    unmount(t.comp);
  });

  it('conta do Claude e credencial só de cota (kimi_cli) NÃO ganham botão nem modelo', async () => {
    apiMock.getEnginesForServer.mockResolvedValue(MOTORES as never);
    const t = montar([LOGADA, chave({ id: 'kimi:/home/u/.kimi-code', nome: 'Kimi CLI', nome_natural: 'Kimi CLI', usos: ['kimi_cli'], base_url: null })]);
    await tick(); await tick(); await tick();
    expect(t.el.textContent).not.toContain(m.config_motores_editar());
    expect(t.el.textContent).not.toContain('kimi-k3');
    unmount(t.comp);
  });

  it('engines.json corrompido: aviso no topo, nenhum botão de motor e + Adicionar desabilitado', async () => {
    apiMock.getEnginesForServer.mockResolvedValue({ motores: {}, arquivo_corrompido: true, arquivo_caminho: '/home/u/.claude/engines.json' } as never);
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    expect(t.el.textContent).toContain(m.config_motores_nao_consegui_1());
    expect(t.el.textContent).toContain('/home/u/.claude/engines.json');
    expect(t.el.textContent).not.toContain(m.config_motores_editar());
    const nova = t.el.querySelector<HTMLButtonElement>(`button[aria-label="${m.contas_add_aria()}"]`)!;
    expect(nova.disabled).toBe(true);
    unmount(t.comp);
  });

  // As três portas do mesmo defeito: a prop é getter VIVO, então o que o MotorForm lê depois do
  // await do PUT é o alvo de agora, não o de quem gravou. Nenhum valor guardado no PAI fecha isso.
  const A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'ta' };
  const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'tb' };

  it('PUT em voo em A, troca pra B e REABRE o bloco em B: a resposta de A entra sob a chave de A, nunca sob a de B', async () => {
    apiMock.getEnginesForServer.mockResolvedValue(MOTORES as never);
    let soltaPut!: (v: unknown) => void;
    apiMock.putEngineForServer.mockReturnValueOnce(new Promise((r) => { soltaPut = r as never; }) as never);
    credMock.listarCredenciais.mockResolvedValue([chave()]);
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    await tick(); await tick(); await tick();
    abrirMotor(el); await tick();
    botao(el.querySelector('.ct-card')!, m.ctx_salvar()).click();
    await tick();
    props.apiTarget = B;
    await tick(); await tick(); await tick();
    abrirMotor(el); await tick();           // a segunda porta: um bloco aberto em B enquanto o PUT de A voa
    soltaPut({ motores: { kimi: { ...MOTORES.motores.kimi, model: 'da-maquina-antiga' } } });
    await tick(); await tick(); await tick();
    expect(clienteQuery.getQueryData(['motores', 'srv-b'])).not.toEqual(
      expect.objectContaining({ motores: expect.objectContaining({ kimi: expect.objectContaining({ model: 'da-maquina-antiga' }) }) }));
    // E a resposta não se perde: mora sob a chave da máquina que respondeu.
    expect(clienteQuery.getQueryData(['motores', 'srv-a'])).toEqual(
      expect.objectContaining({ motores: expect.objectContaining({ kimi: expect.objectContaining({ model: 'da-maquina-antiga' }) }) }));
    unmount(comp as never);
  });

  it('PUT em voo em A, troca pra B: a sincronização nos agentes vai pra A (quem gravou), não pra B', async () => {
    apiMock.getEnginesForServer.mockResolvedValue(MOTORES as never);
    let soltaPut!: (v: unknown) => void;
    apiMock.putEngineForServer.mockReturnValueOnce(new Promise((r) => { soltaPut = r as never; }) as never);
    credMock.listarCredenciais.mockResolvedValue([chave()]);
    const props = criarProps({ apiTarget: A as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(ContasSettings, { target: el, props });
    await tick(); await tick(); await tick();
    abrirMotor(el); await tick();
    botao(el.querySelector('.ct-card')!, m.ctx_salvar()).click();
    await tick();
    props.apiTarget = B;
    await tick(); await tick(); await tick();
    soltaPut({ motores: MOTORES.motores });
    await tick(); await tick(); await tick();
    expect(credMock.sincronizarNosAgentes).toHaveBeenCalledTimes(1);
    expect(credMock.sincronizarNosAgentes.mock.calls[0][0]).toEqual(A);
    unmount(comp as never);
  });

  it('refresh do cabeçalho com o bloco aberto, depois Salvar: o card mostra o modelo novo (nada é descartado calado)', async () => {
    apiMock.getEnginesForServer.mockResolvedValue(MOTORES as never);
    apiMock.putEngineForServer.mockResolvedValueOnce({ motores: { kimi: { ...MOTORES.motores.kimi, model: 'kimi-k2.7' } } } as never);
    const t = montar([chave()]);
    await tick(); await tick(); await tick();
    abrirMotor(t.el); await tick();
    // atualizar(): ++geracao SEM trocar de alvo — é o segundo evento que mexe no mesmo contador.
    [...t.el.querySelectorAll<HTMLButtonElement>('.ct-refresh')].at(-1)!.click();
    await tick(); await tick(); await tick();
    const card = t.el.querySelector('.ct-card')!;
    botao(card, m.ctx_salvar()).click();
    await tick(); await tick(); await tick(); await tick();
    expect(card.textContent).toContain(m.novacred_sync_ok());
    botao(card, m.sessao_fechar()).click();
    await tick();
    expect(t.el.querySelector('.ct-modelo')!.textContent).toBe('kimi-k2.7');
    unmount(t.comp);
  });

  it('confirmação de apagar uma chave avisa que sessões abertas continuam', async () => {
    const t = montar([chave()]);
    await tick(); await tick();
    t.el.querySelector<HTMLButtonElement>('.ct-kebab')!.click();
    await tick();
    [...t.el.querySelectorAll<HTMLButtonElement>('.ct-menu-item')].find((b) => b.textContent?.trim() === m.comum_apagar())!.click();
    await tick();
    expect(t.el.querySelector('.ct-confirma')!.textContent).toContain(m.config_motores_sessoes_abertas());
    unmount(t.comp);
  });
});

describe('ContasSettings — as três seções da lista', () => {
  // A tela nasceu da fusão de duas telas antigas e ficou com 12 credenciais numa lista só, sem
  // seção nenhuma. Aqui cada credencial tem um lugar: conta do Claude, modelo pro Claude Code
  // (chave que está no engines.json) e chave de outro agente. A CÓPIA que o agentes_sync grava
  // no config.toml do Kimi tem o mesmo nome do motor — ela não é uma credencial a mais, é uma
  // linha dentro do card do modelo.
  const MOTORES = {
    motores: {
      kimi: {
        label: 'Kimi', base_url: 'https://api.kimi.com/coding', model: 'kimi-k3',
        context_window: 256000, subagent_model: 'kimi-k3-mini', adaptive_thinking: false,
        api_key: 'sk-k••••4f2a', api_key_definida: true,
      },
      'command-code': {
        // Sem context_window e sem subagent_model: é o caso dos chips "padrão"/"igual".
        label: 'Command Code', base_url: 'https://gw.exemplo.com/provider', model: 'gpt-5.6-luna',
        api_key: 'sk-cc••••1234', api_key_definida: true,
      },
    },
    arquivo_corrompido: false, arquivo_caminho: '',
  };
  const CC = chave({
    id: 'chave:command-code', nome: 'Command Code', nome_natural: 'command-code',
    base_url: 'https://gw.exemplo.com/provider', chave_mascarada: 'sk-cc••••1234',
  });
  // A cópia do sync: id `kimi:<nome do motor>` (backend/app/cotas.py). O nome exibido é
  // deliberadamente OUTRO — o que a linha do card diz é o nome do MOTOR, que é como o provider
  // se chama dentro do Kimi, não o apelido que a cópia carrega.
  const COPIA = chave({
    id: 'kimi:command-code', nome: 'Apelido da cópia', nome_natural: 'command-code',
    usos: ['kimi_cli'], base_url: null, cota: { estado: 'indisponivel', janelas: [] },
  });
  const KIMI_CLI = chave({
    id: 'kimi:outro', nome: 'Kimi CLI', nome_natural: 'outro', usos: ['kimi_cli'],
    base_url: null, cota: { estado: 'indisponivel', janelas: [] },
  });
  const CODEX = chave({
    id: 'codex:chatgpt', nome: 'ChatGPT', nome_natural: 'chatgpt', usos: [],
    base_url: null, cota: { estado: 'indisponivel', janelas: [] },
  });
  const LISTA = [LOGADA, chave(), CC, COPIA, KIMI_CLI, CODEX];

  const secoes = (el: HTMLElement) => [...el.querySelectorAll<HTMLElement>('.ct-grupo')];
  const nomes = (g: HTMLElement) => [...g.querySelectorAll('.ct-nome')].map((n) => n.textContent);
  const cardDe = (el: HTMLElement, nome: string) =>
    [...el.querySelectorAll<HTMLElement>('.ct-card')]
      .find((c) => c.querySelector('.ct-nome')?.textContent === nome)!;

  beforeEach(() => {
    apiMock.getEnginesForServer.mockResolvedValue(MOTORES as never);
    apiMock.getEngines.mockResolvedValue(MOTORES as never);
  });

  it('os três títulos aparecem nesta ordem: Claude, modelos, outros agentes', async () => {
    const t = montar(LISTA);
    await tick(); await tick(); await tick();
    expect(secoes(t.el).map((s) => s.querySelector('.st-secao')!.textContent))
      .toEqual([m.contas_secao_claude(), m.contas_secao_modelos(), m.contas_secao_outros()]);
    unmount(t.comp);
  });

  it('cada credencial cai na sua seção (chave com motor = modelo; kimi/codex = outros)', async () => {
    const t = montar(LISTA);
    await tick(); await tick(); await tick();
    const [claudes, modelos, outros] = secoes(t.el);
    expect(nomes(claudes)).toEqual(['jefferson']);
    expect(nomes(modelos)).toEqual(['Kimi', 'Command Code']);
    expect(nomes(outros)).toEqual(['Kimi CLI', 'ChatGPT']);
    unmount(t.comp);
  });

  it('a cópia do sync no Kimi não tem card próprio — é uma linha dentro do card do modelo', async () => {
    const t = montar(LISTA);
    await tick(); await tick(); await tick();
    // 5 cards para 6 credenciais: a cópia foi absorvida.
    expect(t.el.querySelectorAll('.ct-card').length).toBe(5);
    expect(t.el.textContent).not.toContain('Apelido da cópia');
    expect(cardDe(t.el, 'Command Code').textContent)
      .toContain(m.contas_sync_kimi({ nome: 'command-code' }));
    // E só no card dela: o outro modelo não ganhou a linha.
    expect(cardDe(t.el, 'Kimi').textContent).not.toContain(m.contas_sync_kimi({ nome: 'command-code' }));
    unmount(t.comp);
  });

  it('o botão de criar está no cabeçalho e o rodapé da lista não existe mais', async () => {
    const t = montar(LISTA);
    await tick(); await tick(); await tick();
    const cab = t.el.querySelector<HTMLElement>('.ct-cab')!;
    const add = cab.querySelector<HTMLButtonElement>(`button[aria-label="${m.contas_add_aria()}"]`)!;
    expect(add).not.toBeNull();
    expect(add.textContent!.trim()).toBe(`+ ${m.contas_add()}`);
    // À esquerda dos dois botões redondos do cabeçalho (densidade e atualizar).
    expect([...cab.querySelectorAll('button')].indexOf(add)).toBe(0);
    // Sem login em voo não sobra rodapé nenhum na tela.
    expect(t.el.querySelectorAll('.ct-rodape').length).toBe(0);
    expect([...t.el.querySelectorAll('button')].some((b) => b.textContent?.trim() === m.contas_nova()))
      .toBe(false);
    unmount(t.comp);
  });

  it('o card do modelo mostra provedor, modelo e os três chips', async () => {
    const t = montar(LISTA);
    await tick(); await tick(); await tick();
    const cc = cardDe(t.el, 'Command Code');
    // Provedor é o HOST da base_url, não a URL inteira — o caminho (/provider) é ruído.
    expect(cc.querySelector('.ct-provedor')!.textContent).toBe('gw.exemplo.com');
    expect(cc.textContent).toContain('gpt-5.6-luna');
    const chips = [...cc.querySelectorAll('.ct-chip')].map((c) => c.textContent);
    expect(chips).toEqual([
      m.contas_chip_janela_padrao(),          // sem context_window no motor
      m.contas_chip_subagente_igual(),        // sem subagent_model
      m.contas_chip_raciocinio_on(),          // default do backend é ligado
    ]);
    // O outro motor tem os três valores preenchidos.
    const kimi = cardDe(t.el, 'Kimi');
    expect([...kimi.querySelectorAll('.ct-chip')].map((c) => c.textContent)).toEqual([
      m.contas_chip_janela({ n: '256k' }),
      m.contas_chip_subagente({ id: 'kimi-k3-mini' }),
      m.contas_chip_raciocinio_off(),
    ]);
    unmount(t.comp);
  });

  it('seção vazia não some: mostra o texto de vazio dela', async () => {
    apiMock.getEnginesForServer.mockResolvedValue({ motores: {}, arquivo_corrompido: false, arquivo_caminho: '' } as never);
    const t = montar([LOGADA]);
    await tick(); await tick(); await tick();
    const [claudes, modelos, outros] = secoes(t.el);
    expect(claudes.querySelector('.ct-vazio')).toBeNull();
    expect(modelos.querySelector('.ct-vazio')!.textContent).toBe(m.contas_secao_modelos_vazio());
    expect(outros.querySelector('.ct-vazio')!.textContent).toBe(m.contas_secao_outros_vazio());
    unmount(t.comp);
  });

  it('a etiqueta "chave de API" fica só nos outros agentes — na seção de modelos ela repetia o título', async () => {
    const t = montar(LISTA);
    await tick(); await tick(); await tick();
    // Em "Chaves de outros agentes" a etiqueta informa: ali uma chave convive com o login do Codex.
    expect(cardDe(t.el, 'Kimi CLI').querySelector('.ct-tag')!.textContent).toBe(m.contas_tipo_chave());
    // No card do modelo, não: toda linha daquela seção é uma chave de API.
    expect(cardDe(t.el, 'Command Code').querySelector('.ct-tag')).toBeNull();
    expect(cardDe(t.el, 'Kimi').querySelector('.ct-tag')).toBeNull();
    unmount(t.comp);
  });

  it('Editar e Remover aparecem NOMEADOS no card do modelo, não só dentro do kebab', async () => {
    const t = montar(LISTA);
    await tick(); await tick(); await tick();
    const cc = cardDe(t.el, 'Command Code');
    const acoes = [...cc.querySelectorAll<HTMLButtonElement>('.ct-acao')].map((b) => b.textContent?.trim());
    expect(acoes).toContain(m.config_motores_editar());
    expect(acoes).toContain(m.lista_remover());
    // Remover cai na MESMA confirmação inline do kebab, com o aviso das sessões abertas.
    [...cc.querySelectorAll<HTMLButtonElement>('.ct-acao')]
      .find((b) => b.textContent?.trim() === m.lista_remover())!.click();
    await tick();
    const confirma = cardDe(t.el, 'Command Code').querySelector('.ct-confirma')!;
    expect(confirma.textContent).toContain(m.config_motores_sessoes_abertas());
    unmount(t.comp);
  });
});

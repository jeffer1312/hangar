// Reconhece um comando do hangar rodado no Bash e o que ele fez, pra o chat mostrar um cartão em
// vez de "Bash <linha enorme>" + saída crua.
//
// Duas regras que valem pra qualquer verbo novo aqui:
//  1. O que NÃO for reconhecido volta `null` e cai no card de Bash de sempre. Um cartão que erra a
//     leitura é pior que nenhum cartão.
//  2. A saída crua nunca some — quem mostra é o componente, num bloco fechado. O parse é uma
//     LEITURA da saída, não a substitui.

export type VerboHangar = 'criar' | 'listar' | 'recado' | 'parear' | 'desparear' | 'grupo';

export type SessaoListada = {
  nome: string;
  estado: string;
  cwd: string;
  /** Linha da direita quando o `cwd` não é o que interessa (idade da sessão, no ListAgents). */
  extra?: string;
};

export type AcaoHangar = {
  verbo: VerboHangar;
  /** Por onde o recado foi: o comando do hangar (padrão) ou a ferramenta nativa do Claude Code. */
  via?: 'hangar' | 'claude';
  /** Sessão criada / destinatária do recado / par. */
  alvo?: string;
  /** Texto do recado ou da tarefa do pareamento. */
  texto?: string;
  cwd?: string;
  provider?: string;
  motor?: string;
  conta?: string;
  worktree?: boolean;
  /** `--list`: uma linha por sessão. */
  sessoes?: SessaoListada[];
  /** `--group`: quem recebeu. */
  peers?: string[];
  /** Recado que ficou na fila em vez de ser digitado na hora. */
  enfileirado?: boolean;
  /** `SendMessage`: a ferramenta confirmou a entrega (`success: true`). */
  entregue?: boolean;
  /** `ListAgents`: o nome DESTA sessão, que a ferramenta diz e não lista junto das outras. */
  eu?: string;
  /** Falhou: a mensagem crua do backend (o componente traduz o que conhece). */
  erro?: string;
};

// Começo do comando, ou depois de `;` `&&` `||` `|`. Os prefixos que a gente de fato escreve na
// frente dele (variável de ambiente, `timeout N`, `command`, `nohup`) entram — sem isso um
// `timeout 25 hangar-send --list` não virava cartão. Um `echo hangar-send` continua de fora, que é
// o ponto de não casar em qualquer posição.
const CMD = /(?:^|[;&|]\s*)(?:\w+=\S+\s+|timeout\s+[\d.smh]+\s+|command\s+|nohup\s+)*(?:hangar-send|cp-send)\b(.*)$/;

/** Primeiro argumento que não é flag — o nome da sessão nos verbos que têm alvo.
 *
 * A flag aqui NUNCA consome o token seguinte, e isso não é descuido: este caminho é só o do
 * RECADO (`hangar-send [flags] <sessao> "msg"`) — `--new`, `--pair`, `--unpair` e `--group` saem
 * antes, em ramos próprios —, e ali toda flag é booleana (`--tmux`).
 * Deixando a flag comer um valor, `hangar-send --tmux teste-picker "..."` lia `teste-picker` como
 * valor do `--tmux` e o alvo virava a MENSAGEM INTEIRA: o cartão desenhava
 * "Abrir Chame AskUserQuestion uma vez com multiSelect true, 1 pergunta e 4 opcoes…" como se fosse
 * nome de sessão (relatado com print, 28/08/2026). */
function primeiroNome(args: string): string | null {
  const m = args.match(/^\s*(?:--[\w-]+\s*)*?(?:"([^"]+)"|'([^']+)'|([^\s"'-][^\s]*))/);
  return m ? (m[1] ?? m[2] ?? m[3] ?? null) : null;
}

/**
 * Última string entre aspas DEPOIS do alvo. O corte importa: em `--pair "sessao com espaco"` sem
 * tarefa, o único trecho entre aspas é o próprio nome da sessão, e pegá-lo como texto faria o
 * cartão mostrar o nome do par como se fosse o contrato dele.
 */
function entreAspas(args: string, depoisDe?: string | null): string | null {
  let resto = args;
  if (depoisDe) {
    const pos = args.indexOf(depoisDe);
    if (pos < 0) return null;
    resto = args.slice(pos + depoisDe.length);
  }
  const m = resto.match(/"([^"]*)"|'([^']*)'/g);
  if (!m?.length) return null;
  return m[m.length - 1].slice(1, -1);
}

/**
 * @param comando linha de comando do Bash
 * @param saida stdout+stderr do resultado (vazio enquanto roda)
 * @param falhou o tool_result veio marcado como erro
 */
export function lerComandoHangar(comando: string, saida: string, falhou: boolean): AcaoHangar | null {
  const casou = comando.match(CMD);
  if (!casou) return null;
  const args = casou[1];
  const out = saida.trim();
  // O `--help`/`--list` de outra máquina e o resto do universo de flags não têm cartão próprio;
  // só entram os verbos abaixo.
  const erro = falhou || /^erro\b|^erro HTTP|não pode|inválid|desconhecid/im.test(out) ? out || 'falhou' : undefined;

  if (/(^|\s)--new(\s|$)/.test(args)) {
    const nome = args.match(/--new\s+(?:"([^"]+)"|'([^']+)'|(\S+))/);
    const alvo = nome ? (nome[1] ?? nome[2] ?? nome[3]) : undefined;
    const criada = out.match(/sessão criada:\s*(\S+)\s*\(([^)]*)\)(.*)$/m);
    const extras = criada?.[3] ?? '';
    return {
      verbo: 'criar',
      alvo: criada?.[1] ?? alvo,
      cwd: criada?.[2],
      worktree: /(^|\/)(\.claude\/)?\.?worktrees?\//.test(criada?.[2] ?? ''),
      provider: extras.match(/\[(claude|codex|pi|kimi)\]/)?.[1],
      motor: extras.match(/\[motor:\s*([^\]]+)\]/)?.[1],
      conta: extras.match(/\[conta:\s*([^\]]+)\]/)?.[1],
      erro,
    };
  }

  if (/(^|\s)--pair(\s|$)/.test(args)) {
    const alvoPar = args.match(/--pair\s+(?:"([^"]+)"|'([^']+)'|(\S+))/)?.slice(1).find(Boolean);
    return {
      verbo: 'parear',
      alvo: alvoPar,
      texto: entreAspas(args, alvoPar) ?? undefined,
      erro,
    };
  }

  if (/(^|\s)--unpair(\s|$)/.test(args)) return { verbo: 'desparear', erro };

  if (/(^|\s)--group(\s|$)/.test(args)) {
    const peers = out.match(/aviso enviado ao grupo:\s*(.+)$/m)?.[1];
    return {
      verbo: 'grupo',
      texto: entreAspas(args) ?? undefined,
      peers: peers ? peers.split(',').map((p) => p.trim()).filter(Boolean) : undefined,
      erro,
    };
  }

  if (/(^|\s)--list(\s|$)/.test(args)) {
    // `%-24s %-15s %s`: nome, estado, cwd. Linha de aviso (⚠) e cabeçalho ficam de fora.
    const sessoes = out
      .split('\n')
      .map((l) => l.match(/^(\S+)\s{2,}(\S+)\s{2,}(.+)$/))
      .filter(Boolean)
      .map((m) => ({ nome: m![1], estado: m![2], cwd: m![3].trim() }));
    return { verbo: 'listar', sessoes: sessoes.length ? sessoes : undefined, erro };
  }

  // Recado 1:1 — o que sobra: `hangar-send <sessao> "msg"`.
  const alvo = primeiroNome(args);
  if (!alvo) return null;
  return {
    verbo: 'recado',
    alvo,
    texto: entreAspas(args, alvo) ?? undefined,
    enfileirado: /^na fila ->/m.test(out),
    erro,
  };
}

// ── A OUTRA via: `SendMessage` / `ListAgents`, ferramentas NATIVAS do Claude Code ────────────
// Mesma coisa que o `hangar-send` faz por comando, só que por socket e só entre sessões Claude
// desta máquina. Vira o MESMO cartão (recado e lista já existem lá); o que muda é o `via`, que o
// cartão usa pra marcar o ícone — e o fato de o dado já vir estruturado, sem parse de linha de
// comando.

/** Uma sessão do `ListAgents`: `  nome [ref]  ·  interactive  ·  idle  ·  tmux X:@1.%2  ·  started 19h ago` */
const AGENTE = /^\s+(\S+)\s+\[[^\]]+\]\s+·\s+\S+\s+·\s+(\S+)\s+·\s+tmux\s+(\S+)\s+·\s+started\s+(.+?)\s*$/;
/** A 1ª linha, que diz o nome desta sessão — ela não entra na lista das outras. */
const EU = /^This session is (\S+)/m;

/**
 * @param toolName nome da ferramenta do evento (`SendMessage`, `ListAgents`, …)
 * @param entrada `tool_input` cru do evento
 * @param saida texto do `tool_result` (vazio enquanto roda)
 * @param falhou o `tool_result` veio marcado como erro
 */
export function lerFerramentaClaude(
  toolName: string | null | undefined,
  entrada: unknown,
  saida: string,
  falhou: boolean,
): AcaoHangar | null {
  const out = saida.trim();
  const campos = (entrada ?? {}) as Record<string, unknown>;

  if (toolName === 'ListAgents') {
    const sessoes = out
      .split('\n')
      .map((l) => l.match(AGENTE))
      .filter(Boolean)
      // `busy` é o `working` do resto do app: o rótulo do cartão é um só, e traduzir aqui evita um
      // segundo vocabulário de estado circulando na UI.
      // `cwd` vazio de propósito: o que a ferramenta dá aqui é o alvo tmux (`hangar-2:@1921.%2050`),
      // que não é diretório nenhum — pôr o alvo naquele campo faria a coluna da direita mostrar um
      // endereço de pane com cara de caminho no dia em que `extra` faltasse.
      .map((m) => ({
        nome: m![1],
        estado: m![2] === 'busy' ? 'working' : m![2],
        cwd: '',
        extra: m![4],
      }));
    return {
      verbo: 'listar',
      via: 'claude',
      sessoes: sessoes.length ? sessoes : undefined,
      eu: out.match(EU)?.[1],
      erro: falhou ? out || 'falhou' : undefined,
    };
  }

  if (toolName !== 'SendMessage') return null;
  const alvo = typeof campos['to'] === 'string' ? campos['to'] : undefined;
  const texto = typeof campos['message'] === 'string' ? campos['message'] : undefined;
  // A ferramenta responde JSON (`{"success":…}`). Resultado que não é JSON — ou que é JSON de outra
  // forma — não vira "entregue" por otimismo: sem `success: true` explícito o cartão não afirma
  // nada, e o texto cru continua no bloco fechado.
  // Três estados, não dois: `true`, `false` e "não deu pra saber". `success` presente mas fora do
  // booleano (`"true"`, `1`, schema mudado) tem de cair no terceiro — lido como `false` ele pintava
  // cartão de ERRO em cima de um recado que chegou.
  let sucesso: boolean | null = null;
  try {
    const j = out ? JSON.parse(out) : null;
    if (j && typeof j === 'object' && typeof j.success === 'boolean') sucesso = j.success;
  } catch {
    sucesso = null;
  }
  return {
    verbo: 'recado',
    via: 'claude',
    alvo,
    texto,
    erro: falhou || sucesso === false ? out || 'falhou' : undefined,
    entregue: sucesso === true,
  };
}

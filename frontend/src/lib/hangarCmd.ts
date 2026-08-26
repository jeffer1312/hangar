// Reconhece um comando do hangar rodado no Bash e o que ele fez, pra o chat mostrar um cartão em
// vez de "Bash <linha enorme>" + saída crua.
//
// Duas regras que valem pra qualquer verbo novo aqui:
//  1. O que NÃO for reconhecido volta `null` e cai no card de Bash de sempre. Um cartão que erra a
//     leitura é pior que nenhum cartão.
//  2. A saída crua nunca some — quem mostra é o componente, num bloco fechado. O parse é uma
//     LEITURA da saída, não a substitui.

export type VerboHangar = 'criar' | 'listar' | 'recado' | 'parear' | 'desparear' | 'grupo';

export type SessaoListada = { nome: string; estado: string; cwd: string };

export type AcaoHangar = {
  verbo: VerboHangar;
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
  /** Falhou: a mensagem crua do backend (o componente traduz o que conhece). */
  erro?: string;
};

// Começo do comando, ou depois de `;` `&&` `||` `|`. Os prefixos que a gente de fato escreve na
// frente dele (variável de ambiente, `timeout N`, `command`, `nohup`) entram — sem isso um
// `timeout 25 hangar-send --list` não virava cartão. Um `echo hangar-send` continua de fora, que é
// o ponto de não casar em qualquer posição.
const CMD = /(?:^|[;&|]\s*)(?:\w+=\S+\s+|timeout\s+[\d.smh]+\s+|command\s+|nohup\s+)*(?:hangar-send|cp-send)\b(.*)$/;

/** Primeiro argumento que não é flag — o nome da sessão nos verbos que têm alvo. */
function primeiroNome(args: string): string | null {
  const m = args.match(/^\s*(?:--[\w-]+(?:\s+(?:"[^"]*"|'[^']*'|[^\s-][^\s]*))?\s*)*?(?:"([^"]+)"|'([^']+)'|([^\s"'-][^\s]*))/);
  return m ? (m[1] ?? m[2] ?? m[3] ?? null) : null;
}

function entreAspas(args: string): string | null {
  const m = args.match(/"([^"]*)"|'([^']*)'/g);
  if (!m?.length) return null;
  const ultimo = m[m.length - 1];
  return ultimo.slice(1, -1);
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
    return {
      verbo: 'parear',
      alvo: args.match(/--pair\s+(?:"([^"]+)"|'([^']+)'|(\S+))/)?.slice(1).find(Boolean),
      texto: entreAspas(args) ?? undefined,
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
    texto: entreAspas(args) ?? undefined,
    enfileirado: /^na fila ->/m.test(out),
    erro,
  };
}

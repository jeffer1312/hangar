// Tradução do JSON do `gh` para o que a faixa desenha. Sem `$`: tudo aqui é puro e testável.

import type { Checks, Falha, GhView, Job, Pr, Situacao } from '../types'

export type { Falha, GhView, Job, Pr, Situacao, Workflow } from '../types'

export type RunGh = {
  databaseId: number
  workflowName: string
  status: string
  conclusion: string
  headSha: string
  url: string
  startedAt?: string
  updatedAt?: string
}
type StepGh = { name: string; status: string; conclusion: string }
export type JobGh = { name: string; status: string; conclusion: string; steps?: StepGh[] }

/** status/conclusion do Actions (minúsculo no REST, maiúsculo no GraphQL do PR). */
export function situacao(status: string, conclusion: string | null | undefined): Situacao {
  const s = status.toLowerCase()
  if (s === 'in_progress') return 'rodando'
  if (s !== 'completed') return 'esperando'
  const c = (conclusion ?? '').toLowerCase()
  if (c === 'success' || c === 'neutral') return 'ok'
  if (c === 'skipped') return 'pulado'
  if (c === 'cancelled') return 'cancelado'
  return 'falhou'
}

export const emAndamento = (s: Situacao) => s === 'rodando' || s === 'esperando'

/** O passo que interessa: o que está rodando, ou o que falhou. */
function passoDe(j: JobGh, s: Situacao): string | null {
  const steps = j.steps ?? []
  if (s === 'rodando') return steps.find(p => p.status === 'in_progress')?.name ?? null
  if (s === 'falhou') return steps.find(p => situacao(p.status, p.conclusion) === 'falhou')?.name ?? null
  return null
}

export function jobs(lista: readonly JobGh[]): Job[] {
  return lista.map(j => {
    const s = situacao(j.status, j.conclusion)
    const steps = j.steps ?? []
    const feitos = steps.filter(p => p.status.toLowerCase() === 'completed').length
    const passos = steps.map(p => ({ nome: p.name, situacao: situacao(p.status, p.conclusion) }))
    return { nome: j.name, situacao: s, passo: passoDe(j, s), feitos, total: steps.length, passos }
  })
}

/** Data ISO do gh em ms; vazia ou a zero do Go (`0001-01-01…`, run que não começou) vira null. */
export function ms(iso: string | undefined): number | null {
  const t = iso ? Date.parse(iso) : NaN
  return Number.isFinite(t) && t > 0 ? t : null
}

/** Volta de ms à data do gh, para remontar o run de um commit que não respondeu. */
// `undefined`: leitura gravada pela versão que ainda não guardava as datas.
export const iso = (t: number | null | undefined) => (t == null ? undefined : new Date(t).toISOString())

/** Runs a mostrar, de listas por commit do mais novo para o mais velho (cada uma como o `gh` lista,
 *  do run mais novo): o último de cada workflow e, além dele, todo run que ainda não terminou. */
export function runsVisiveis(porCommit: readonly (readonly RunGh[])[]): RunGh[] {
  const vistos = new Set<string>()
  const ids = new Set<number>()
  const out: RunGh[] = []
  for (const r of porCommit.flat()) {
    const novo = !vistos.has(r.workflowName)
    if ((!novo && r.status === 'completed') || ids.has(r.databaseId)) continue
    vistos.add(r.workflowName)
    ids.add(r.databaseId)
    out.push(r)
  }
  return out
}

export type Empurrado = { sha: string; branch: string }

/** Commits empurrados pela sessão, o mais novo primeiro, sem repetir e com teto. */
export function lembrarCommit(lista: readonly Empurrado[], novo: Empurrado, max: number): Empurrado[] {
  return [novo, ...lista.filter(c => c.sha !== novo.sha)].slice(0, max)
}

/** Só `git push` registra commit; `gh pr|run|workflow` só pede consulta. */
export const ehPush = (cmd: string) => /\bgit\s+push\b/.test(cmd)

// Flags do `gh pr merge` que levam valor: o valor não é o PR.
const COM_VALOR = new Set(['-t', '--subject', '-b', '--body', '-F', '--body-file', '--match-head-commit', '-R', '--repo', '-A', '--author-email'])
const FIM = /^(;|&&|\|\||\||&)$/

export type AlvoMerge = { alvo: string; repo: string | null }

/** O PR de cada `gh pr merge` do comando (número, URL ou branch; '' é o da branch atual) e o `-R` dele. */
export function alvosMerge(cmd: string): AlvoMerge[] {
  return [...cmd.matchAll(/\bgh\s+pr\s+merge\b/g)].map(m => {
    // Palavra a palavra, com aspas inteiras: `--subject "fix 12; ok" 106` é o 106.
    const palavras = [...cmd.slice((m.index ?? 0) + m[0].length).matchAll(/"([^"]*)"|'([^']*)'|(\S+)/g)]
    let alvo: string | null = null
    let repo: string | null = null
    for (let i = 0; i < palavras.length; i++) {
      const p = palavras[i]!
      const solto = p[3]
      if (solto !== undefined && FIM.test(solto)) break
      const valor = p[1] ?? p[2] ?? solto ?? ''
      if (solto?.startsWith('-')) {
        const [flag, junto] = solto.split('=', 2)
        const v = junto ?? (COM_VALOR.has(flag ?? '') ? palavras[++i]?.slice(1).find(x => x !== undefined) ?? '' : null)
        if (flag === '-R' || flag === '--repo') repo = v
        continue
      }
      const fimColado = solto?.endsWith(';') ?? false
      alvo ??= fimColado ? valor.slice(0, -1) : valor
      if (fimColado) break
    }
    return { alvo: alvo ?? '', repo }
  })
}

type CheckGh = { __typename?: string; status?: string; conclusion?: string | null; state?: string }

export function checks(rollup: readonly CheckGh[] | null | undefined): Checks {
  const r: Checks = { ok: 0, falhou: 0, rodando: 0 }
  for (const c of rollup ?? []) {
    // StatusContext (status externo) só tem `state`; CheckRun tem status + conclusion.
    const s = c.__typename === 'StatusContext'
      ? ({ SUCCESS: 'ok', PENDING: 'esperando', EXPECTED: 'esperando' } as Record<string, Situacao>)[c.state ?? ''] ?? 'falhou'
      : situacao(c.status ?? '', c.conclusion)
    if (s === 'ok' || s === 'pulado') r.ok++
    else if (emAndamento(s)) r.rodando++
    else r.falhou++
  }
  return r
}

export type PrGh = {
  number: number
  title: string
  url: string
  state: Pr['estado']
  isDraft: boolean
  reviewDecision: string
  statusCheckRollup?: CheckGh[]
}

export function pr(p: PrGh): Pr {
  const rev = p.reviewDecision
  return {
    numero: p.number,
    titulo: p.title,
    url: p.url,
    estado: p.state,
    rascunho: p.isDraft,
    revisao: rev === 'APPROVED' || rev === 'CHANGES_REQUESTED' || rev === 'REVIEW_REQUIRED' ? rev : null,
    checks: checks(p.statusCheckRollup),
  }
}

/** Ainda há o que acompanhar: run ou check do PR que não terminou. */
export function precisaConsultar(v: GhView | null): boolean {
  if (!v) return false
  return v.workflows.some(w => emAndamento(w.situacao) || w.jobs.some(j => emAndamento(j.situacao)))
    || (v.pr?.estado === 'OPEN' && v.pr.checks.rodando > 0)
}

/** O que o stderr do gh diz sobre a falha: sem login, limite da API ou outra coisa. */
export function classificarFalha(msg: string): Falha {
  if (/gh auth login|HTTP 401|Bad credentials|authentication required|not logged in/i.test(msg)) return 'login'
  if (/rate limit/i.test(msg)) return 'limite'
  return 'outra'
}

const GRAVIDADE: Record<Falha, number> = { outra: 0, login: 1, limite: 2 }

/** De vários erros de uma consulta, o que mais pesa: limite > login > outra. */
export function piorErro(a: string | null, b: string): string {
  return a !== null && GRAVIDADE[classificarFalha(a)] >= GRAVIDADE[classificarFalha(b)] ? a : b
}

/** A linha de aviso da faixa: a falha nunca fica calada. */
export function textoAviso(f: Falha, msg: string, ate: string | null): string {
  if (f === 'login') return 'gh sem login · rode gh auth login'
  if (f === 'limite') return ate ? `limite da API até ${ate}` : 'limite da API do GitHub'
  const linha = msg.replace(/^Error:\s*/, '').split('\n')[0] ?? ''
  return `gh falhou: ${linha.length > 100 ? `${linha.slice(0, 99)}…` : linha}`
}

/** Comando Bash que costuma criar run ou PR novo: vale consultar logo depois. */
export const disparaRun = (cmd: string) => /\bgit\s+push\b|\bgh\s+(pr|run|workflow)\b/.test(cmd)

/** Endereço de repositório no GitHub (https ou ssh). */
export const ehGithub = (remoto: string) => /(^|[@/])github\.com[:/]/.test(remoto.trim())

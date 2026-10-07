import { describe, expect, test } from 'claude-code/testing'
import { alvosMerge, checks, classificarFalha, piorErro, textoAviso, disparaRun, ehGithub, ehPush, iso, jobs, lembrarCommit, ms, precisaConsultar, runsVisiveis, situacao } from './gh'
import type { GhView, RunGh, Situacao } from './gh'
import { barra, chavesJobs, contar, duracao, nomeJob, nomesJobs, passoCurto, progresso, rotulosAnteriores, separarPorCommit } from './faixa'

const run = (id: number, wf: string, sha: string, status = 'completed', conclusion = 'success'): RunGh =>
  ({ databaseId: id, workflowName: wf, status, conclusion, headSha: sha, url: `u/${id}` })

describe('gh', () => {
  test('situação do run e do job', () => {
    expect(situacao('in_progress', '')).toBe('rodando')
    expect(situacao('queued', '')).toBe('esperando')
    expect(situacao('completed', 'success')).toBe('ok')
    expect(situacao('COMPLETED', 'FAILURE')).toBe('falhou')
    expect(situacao('completed', 'timed_out')).toBe('falhou')
    expect(situacao('completed', 'skipped')).toBe('pulado')
  })

  test('último run de cada workflow, mais os de commit velho que ainda rodam', () => {
    const r = runsVisiveis([
      [run(5, 'CI', 'b'), run(4, 'Server', 'b', 'queued', '')],
      [run(3, 'Server', 'a', 'in_progress', ''), run(2, 'CI', 'a'), run(1, 'Native', 'a')],
    ])
    // CI velho terminado sai; Server velho rodando fica; Native só existe no velho e fica.
    expect(r.map(x => x.databaseId)).toEqual([5, 4, 3, 1])
  })

  test('commits lembrados: mais novo primeiro, sem repetir, com teto', () => {
    const c = (sha: string) => ({ sha, branch: 'x' })
    expect(lembrarCommit([c('b'), c('a')], c('c'), 2)).toEqual([c('c'), c('b')])
    expect(lembrarCommit([c('b'), c('a')], c('a'), 5)).toEqual([c('a'), c('b')])
  })

  test('gh sem login vira aviso na faixa', () => {
    const msg = 'gh run list --commit: To get started with GitHub CLI, please run:  gh auth login'
    expect(classificarFalha(msg)).toBe('login')
    expect(classificarFalha('failed to get runs: HTTP 401: Bad credentials (https://api.github.com/...)')).toBe('login')
    expect(textoAviso('login', msg, null)).toBe('gh sem login · rode gh auth login')
  })

  test('limite da API vira aviso com o horário de volta', () => {
    const msg = 'gh run list --commit: HTTP 403: API rate limit exceeded for user ID 123.'
    expect(classificarFalha(msg)).toBe('limite')
    expect(classificarFalha('GraphQL: API rate limit exceeded for user ID 123.')).toBe('limite')
    expect(textoAviso('limite', msg, '21:40')).toBe('limite da API até 21:40')
    expect(textoAviso('limite', msg, null)).toBe('limite da API do GitHub')
  })

  test('de vários erros, vale o mais grave', () => {
    const outra = 'dial tcp: timeout'
    const limite = 'HTTP 403: API rate limit exceeded'
    const login = 'HTTP 401: Bad credentials'
    expect(piorErro(null, outra)).toBe(outra)
    expect(piorErro(outra, limite)).toBe(limite)
    expect(piorErro(limite, login)).toBe(limite)
    expect(piorErro(outra, login)).toBe(login)
  })

  test('outra falha mostra a primeira linha do erro', () => {
    expect(classificarFalha('dial tcp: lookup api.github.com: no such host')).toBe('outra')
    expect(textoAviso('outra', 'Error: dial tcp: no such host\nmais', null)).toBe('gh falhou: dial tcp: no such host')
  })

  test('alvos do gh pr merge', () => {
    const so = (cmd: string) => alvosMerge(cmd).map(a => a.alvo)
    expect(so('gh pr merge 106 --merge && gh pr merge 104 --merge --admin')).toEqual(['106', '104'])
    expect(so('gh pr merge --squash --subject x')).toEqual([''])
    expect(so('gh pr merge https://github.com/a/b/pull/7')).toEqual(['https://github.com/a/b/pull/7'])
    expect(so('gh pr merge feat/x')).toEqual(['feat/x'])
    // Branch depois ou antes de flag, e valor de flag entre aspas, não viram outro PR.
    expect(so('gh pr merge feat/x --squash')).toEqual(['feat/x'])
    expect(so('gh pr merge --squash feat/x')).toEqual(['feat/x'])
    expect(so('gh pr merge --subject "fix 12; ok" 106')).toEqual(['106'])
    expect(so('gh pr merge --subject=x 106; echo fim')).toEqual(['106'])
    expect(so('gh pr merge 106; gh pr merge')).toEqual(['106', ''])
    expect(alvosMerge('gh pr merge -R a/b 106')).toEqual([{ alvo: '106', repo: 'a/b' }])
    expect(so('gh pr view 3')).toEqual([])
  })

  test('só git push registra commit', () => {
    expect(ehPush('git push -u origin x')).toBe(true)
    expect(ehPush('gh pr view 3')).toBe(false)
  })

  test('barra: um segmento por job, o que roda enche pelos passos; tempo do run', () => {
    const j = (situacao: Situacao, feitos = 0, total = 0) => ({ nome: 'x', situacao, passo: null, feitos, total, passos: [] })
    expect(barra([j('ok'), j('rodando', 1, 4), j('esperando')], 14)).toEqual([
      { s: 'ok', texto: '━━━━' }, { s: 'esperando', texto: ' ' },
      { s: 'rodando', texto: '━───' },
      { s: 'esperando', texto: ' ────' },
    ])
    // Sem espaço para os vãos, os segmentos se encostam.
    expect(barra([j('falhou'), j('ok')], 3).map(t => t.texto).join('')).toBe('━━━')
    expect(duracao(42_000)).toBe('42s')
    expect(duracao(252_000)).toBe('4m 12s')
    expect(duracao(3_780_000)).toBe('1h 03m')
    expect(ms('0001-01-01T00:00:00Z')).toBe(null)
    const wf = { id: 1, nome: 'CI', sha: 'a', situacao: 'rodando' as const, url: '', jobs: [j('ok'), j('rodando')], inicio: 1_000, fim: null }
    expect(progresso([wf], 61_000)).toBe('1/2 jobs · 1m 00s')
    // Terminado sem data de fim: sem tempo, não um relógio andando.
    expect(progresso([{ ...wf, situacao: 'ok' as const, jobs: [j('ok')] }], 61_000)).toBe('1/1 jobs')
    // Leitura gravada antes de existirem as datas.
    expect(iso(undefined)).toBe(undefined)
  })

  test('rótulo de run anterior nunca se repete', () => {
    const w = (id: number) => ({ id, nome: 'CI', sha: 'aaaaaaaa', situacao: 'rodando' as const, url: '', jobs: [], inicio: null, fim: null })
    // push e pull_request do mesmo commit: o id do run desempata
    expect(rotulosAnteriores([w(1), w(2)])).toEqual(['● CI aaaaaaa #1', '● CI aaaaaaa #2'])
  })

  test('job rodando mostra a etapa atual; job que falhou guarda o passo', () => {
    const [rodando, falhou] = jobs([
      { name: 'test', status: 'in_progress', conclusion: '', steps: [
        { name: 'checkout', status: 'completed', conclusion: 'success' },
        { name: 'pytest', status: 'in_progress', conclusion: '' },
      ] },
      { name: 'build', status: 'completed', conclusion: 'failure', steps: [
        { name: 'setup', status: 'completed', conclusion: 'success' },
        { name: 'Compile', status: 'completed', conclusion: 'failure' },
      ] },
    ])
    if (!rodando || !falhou) throw new Error('jobs perdidos')
    expect(rodando.passo).toBe('pytest')
    expect(falhou.passo).toBe('Compile')
    expect(contar([rodando, falhou])).toEqual({ ok: 0, falhou: 1, rodando: 1 })
  })

  test('job de matriz e passo encurtados', () => {
    expect(nomeJob('build (windows-latest, windows-x86_64, .exe, true)')).toBe('build windows')
    expect(nomeJob('statusline (ubuntu-latest)')).toBe('statusline ubuntu')
    expect(nomeJob('backend')).toBe('backend')
    expect(nomeJob('Lint (changed files) / report')).toBe('Lint (changed files) / report')
    // Curto repetido no workflow: volta o nome completo; homônimos (corte do GitHub) ganham chaves diferentes.
    const job = (nome: string) => ({ nome, situacao: 'ok' as const, passo: null, feitos: 0, total: 0, passos: [] })
    expect(nomesJobs([job('build (win, x64)'), job('build (win, arm)'), job('lint')]))
      .toEqual(['build (win, x64)', 'build (win, arm)', 'lint'])
    const [a, b] = chavesJobs('CI', [job('build (x...'), job('build (x...')])
    expect(a).not.toBe(b)
    expect(nomeJob('build (ubuntu-latest, linux-x86_64, x86_64-unknown-linux-gnu, true, target/x86_64-unknown-linux-g...')).toBe('build ubuntu')
    expect(passoCurto('Run uv run pytest -q')).toBe('uv run pytest -q')
    expect(passoCurto('Run cargo build --locked --release --target x86_64')).toBe('cargo build --locked --releas…')
  })

  test('run mais novo de cada workflow fica no detalhe; o superado que ainda roda vai para a linha de anteriores', () => {
    const w = (id: number, nome: string, sha: string) =>
      ({ id, nome, sha, situacao: 'rodando' as const, url: '', jobs: [], inicio: null, fim: null })
    const { atual, anteriores } = separarPorCommit([w(5, 'CI', 'bbbbbbbb'), w(4, 'CI', 'aaaaaaaa'), w(3, 'Native', 'aaaaaaaa')])
    expect(atual.map(x => x.id)).toEqual([5, 3])
    expect(anteriores.map(x => x.id)).toEqual([4])
    expect(rotulosAnteriores(anteriores)).toEqual(['● CI aaaaaaa'])
  })

  test('resumo dos checks do PR', () => {
    expect(checks([
      { __typename: 'CheckRun', status: 'COMPLETED', conclusion: 'SUCCESS' },
      { __typename: 'CheckRun', status: 'IN_PROGRESS', conclusion: null },
      { __typename: 'CheckRun', status: 'COMPLETED', conclusion: 'FAILURE' },
      { __typename: 'StatusContext', state: 'PENDING' },
    ])).toEqual({ ok: 1, falhou: 1, rodando: 2 })
  })

  test('consulta segue só enquanto algo roda', () => {
    const v = (s: 'ok' | 'rodando'): GhView => ({ branch: 'x', pr: null, workflows: [
      { id: 1, nome: 'CI', sha: 'x', situacao: s, url: '', jobs: [{ nome: 'a', situacao: s, passo: null, feitos: 0, total: 0, passos: [] }], inicio: null, fim: null },
    ] })
    expect(precisaConsultar(v('rodando'))).toBe(true)
    expect(precisaConsultar(v('ok'))).toBe(false)
    expect(precisaConsultar(null)).toBe(false)
  })

  test('comandos que disparam run e remoto do GitHub', () => {
    expect(disparaRun('git push -u origin x')).toBe(true)
    expect(disparaRun('gh pr create')).toBe(true)
    expect(disparaRun('git status')).toBe(false)
    expect(ehGithub('https://github.com/jeffer1312/hangar')).toBe(true)
    expect(ehGithub('git@github.com:a/b.git')).toBe(true)
    expect(ehGithub('https://gitlab.example.com/a/b.git')).toBe(false)
  })
})

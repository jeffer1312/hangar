import { expect, mock, test } from 'claude-code/testing'
import { chaveDe } from './faixa'

const LONGO = 'build (ubuntu-latest, linux-x86_64, x86_64-unknown-linux-gnu, true, target/x86_64-unknown-linux-gnu/release)'

const PR = { number: 103, title: 'feat: instalador', url: 'pr', state: 'OPEN', isDraft: false, reviewDecision: 'REVIEW_REQUIRED',
  statusCheckRollup: [
    { __typename: 'CheckRun', status: 'COMPLETED', conclusion: 'FAILURE' },
    { __typename: 'CheckRun', status: 'IN_PROGRESS', conclusion: null },
  ] }
const run = (id: number, wf: string, sha: string, status: string, conclusion = '') =>
  ({ databaseId: id, workflowName: wf, status, conclusion, headSha: sha, url: `u${id}` })
const RUNS: Record<string, unknown[]> = {
  cccccccc: [run(3, 'CI', 'cccccccc', 'completed', 'failure'), run(1, 'Native', 'cccccccc', 'in_progress')],
  bbbbbbbb: [run(2, 'CI', 'bbbbbbbb', 'in_progress')],
}
const JOBS: Record<string, unknown[]> = {
  3: [{ name: 'backend', status: 'completed', conclusion: 'success' },
    { name: 'passos', status: 'completed', conclusion: 'failure', steps: [{ name: 'Run de=x', status: 'completed', conclusion: 'failure' }] }],
  2: [],
  1: [{ name: LONGO, status: 'completed', conclusion: 'success', steps: [{ name: 'Set up job', status: 'completed', conclusion: 'success' }] },
    { name: 'build (windows-latest, windows-x86_64, .exe, true)', status: 'in_progress', conclusion: '',
    steps: [{ name: 'Run cargo build --locked', status: 'in_progress', conclusion: '' }] }],
}

function gh(argv: readonly string[]): string | null {
  const a = argv.join(' ')
  if (a === 'git remote get-url origin') return 'https://github.com/a/b'
  if (a === 'git branch --show-current') return 'feat/x'
  if (a === 'git rev-parse HEAD') return 'cccccccc'
  if (a.startsWith('gh pr view')) return JSON.stringify(PR)
  if (a.startsWith('gh run list')) return JSON.stringify(RUNS[argv[4] ?? ''] ?? [])
  if (a.startsWith('gh run view')) return JSON.stringify({ jobs: JOBS[argv[3] ?? ''] ?? [] })
  return null
}

const ABOVE = { hasSurvey: false, isWorking: false, maxRows: 40, bodyColumns: 100 } as never

const textos = (n: unknown): string[] => {
  if (typeof n === 'string') return [n]
  if (!n || typeof n !== 'object') return []
  const o = n as { props?: { label?: string }; children?: unknown[] }
  return [...(o.props?.label ? [o.props.label] : []), ...(o.children ?? []).flatMap(textos)]
}

test('faixa aberta detalha o run mais novo de cada workflow; ▾ recolhe numa linha só', async ($, on) => {
  mock.clock(on, { now: 1_000 })
  mock.store(on, { 'commits:sid': { em: 1, commits: [{ sha: 'cccccccc', branch: 'feat/x' }, { sha: 'bbbbbbbb', branch: 'feat/x' }] } })
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.id', () => ({ value: 'sid' }) as never)
  // A faixa do engine, sob a do mod: vazia.
  on('ui.render', () => ({ type: 'Box', props: {}, children: [] }) as never)
  on('process.run', (_, e) => {
    const out = gh(e.argv)
    return { value: { exitCode: out === null ? 1 : 0, stdout: out ?? '', stderr: '' } } as never
  })
  await $.session.start({ cwd: '/tmp', surface: 'terminal', isInteractive: true })
  const ui = await $.ui.mount({ plugin: 'github-actions', surface: 'desktop', component: 'AbovePrompt', props: ABOVE })
  for (let i = 0; i < 50 && !textos(await ui.drawn()).join('|').includes('passos'); i++) await ui.redraw()

  const aberta = textos(await ui.drawn()).join('|')
  expect(aberta).toContain('PR #103')
  expect(aberta).toContain('✕ passos')
  expect(aberta).toContain('● build windows')
  expect(aberta).toContain('○ CI bbbbbbb')
  expect(aberta).not.toContain('Run cargo build --locked|')

  // Clique no workflow lista os jobs; clique no job, os passos dele.
  await ui.press({ key: chaveDe('wf', 'Native') })
  const jobsAbertos = textos(await ui.drawn()).join('|')
  expect(jobsAbertos).toContain('▾ Native')
  expect(jobsAbertos).toContain('▸ build windows|0/1 passos')
  await ui.press({ key: chaveDe('job', 'Native\nbuild (windows-latest, windows-x86_64, .exe, true)\n0') })
  expect(textos(await ui.drawn()).join('|')).toContain('●|Run cargo build --locked')
  // Nome de matriz com mais de 64 caracteres: a chave curta não derruba a faixa.
  await ui.press({ key: chaveDe('job', `Native\n${LONGO}\n0`) })
  expect(textos(await ui.drawn()).join('|')).toContain('✓|Set up job')
  await ui.press({ key: chaveDe('wf', 'Native') })
  expect(textos(await ui.drawn()).join('|')).not.toContain('0/1 passos')

  await ui.press({ key: 'alternar' })
  const recolhida = textos(await ui.drawn())
  expect(recolhida).toContain('▸')
  expect(recolhida.join('|')).not.toContain('passos')
  expect(recolhida).toContain('✕1')
})

test('gh pr merge feito pela sessão passa a mostrar o build do commit de merge na base', async ($, on) => {
  const clock = mock.clock(on, { now: 1_000 })
  mock.store(on, {})
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('session.id', () => ({ value: 'sid' }) as never)
  on('ui.render', () => ({ type: 'Box', props: {}, children: [] }) as never)
  on('tool.call', () => ({ result: 'merged', isError: false }) as never)
  on('process.run', (_, e) => {
    const a = e.argv.join(' ')
    const out = a === 'git remote get-url origin' ? 'https://github.com/a/b'
      : a === 'git branch --show-current' ? 'main'
        : a === 'git rev-parse HEAD' ? 'aaaaaaaa'
          : a.startsWith('gh pr view 106') ? JSON.stringify({ state: 'MERGED', mergeCommit: { oid: 'dddddddd' }, baseRefName: 'main' })
            : a.startsWith('gh run list --commit dddddddd') ? JSON.stringify([run(9, 'Native', 'dddddddd', 'in_progress')])
              : a.startsWith('gh run view 9') ? JSON.stringify({ jobs: [] })
                : null
    return { value: { exitCode: out === null ? 1 : 0, stdout: out ?? '', stderr: out === null ? 'no pull requests found' : '' } } as never
  })
  await $.session.start({ cwd: '/tmp', surface: 'terminal', isInteractive: true })
  await $.tool.call({ tool: 'Bash', command: 'gh pr merge 106 --merge' } as never)
  await clock.advance(5_000)
  const ui = await $.ui.mount({ plugin: 'github-actions', surface: 'desktop', component: 'AbovePrompt', props: ABOVE })
  for (let i = 0; i < 50 && !textos(await ui.drawn()).join('|').includes('Native'); i++) await ui.redraw()
  const t = textos(await ui.drawn()).join('|')
  expect(t).toContain('▸ Native')
  expect(t).toContain('ddddddd')
})

import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import ts from 'typescript'

const source = readFileSync(new URL('../src/presentation.ts', import.meta.url), 'utf8')
const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2024 } })
const p = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`)
const r = (status, model = 'gpt-6.1-sol', effort = 'low') => ({ id: String(Math.random()), time: 0, model, effort, status })

test('accuracy only counts graded answers of the selected model and effort', () => {
  const records = [r('correct'), r('correct'), r('wrong'), r('failed'), r('pending'), r('wrong', 'gpt-5.6-sol'), r('wrong', 'gpt-6.1-sol', 'high')]
  assert.deepEqual(p.accuracy(records, 'gpt-6.1-sol', 'low'), { correct: 2, graded: 3, rate: 2 / 3 })
  assert.deepEqual(p.accuracy([r('failed'), r('pending')], 'gpt-6.1-sol', 'low'), { correct: 0, graded: 0, rate: null })
  assert.equal(p.accuracy(records, 'gpt-5.6-sol', 'low').rate, 0)
})

test('rate tone separates healthy, suspicious and degraded accounts', () => {
  assert.equal(p.rateTone(null), 'neutral')
  assert.equal(p.rateTone(1), 'success')
  assert.equal(p.rateTone(0.8), 'success')
  assert.equal(p.rateTone(0.6), 'warning')
  assert.equal(p.rateTone(0.2), 'danger')
  assert.equal(p.formatPercent(2 / 3), '67%')
})

test('effort none means the host decides and is hidden from the model label', () => {
  assert.equal(p.modelLabel('gpt-6.1-sol', 'none'), 'gpt-6.1-sol')
  assert.equal(p.modelLabel('gpt-6.1-sol', 'xhigh'), 'gpt-6.1-sol(xhigh)')
  assert.equal(p.effortText('none'), '默认')
  assert.equal(p.inScope(r('correct', 'm', 'none'), 'm', 'low'), false)
})

test('plans use the Codex client names and unknown plans sort last', () => {
  assert.equal(p.planInfo('pro').label, 'Pro 200')
  assert.equal(p.planInfo(' PROLITE ').label, 'Pro 100')
  assert.equal(p.planInfo('team').label, 'Business')
  assert.ok(p.planInfo('pro').rank < p.planInfo('plus').rank)
  assert.deepEqual([p.planInfo(null).label, p.planInfo('mystery').label], ['未知订阅', 'mystery'])
  assert.ok(p.planInfo('mystery').rank > p.planInfo('free').rank)
})

test('durations, numbers and credential states are readable', () => {
  assert.equal(p.formatDuration(undefined), '—')
  assert.equal(p.formatDuration(17_480), '17.5s')
  assert.equal(p.formatDuration(65_000), '1m05s')
  assert.equal(p.formatNumber(14125), '14,125')
  assert.equal(p.credentialStateText('ready'), '')
  assert.equal(p.credentialStateText('banned'), '已封禁')
  assert.equal(p.credentialStateText('new_state'), 'new_state')
  assert.equal(p.statusText.failed, '出错')
})

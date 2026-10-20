import { applyResolvedTheme, resolveTheme, DEFAULT_THEME_COLOR, DEFAULT_CUSTOM_THEME_COLOR } from '@codex-proxy/ui/theme'
import type { Account, RecordDetail, RecordStatus } from './api'

const params = new URLSearchParams(location.search)
const scenario = params.get('scenario')
const prompt = '不使用任何外部工具回答以下问题：\n\n在一个黑色的袋子里放有三种口味的糖果……（模拟题目）\n\n        苹果味  桃子味  西瓜味\n圆形       7      9      8\n五角星形   7      6      4\n'
const models = ['gpt-6-astra', 'gpt-6.1-sol', 'gpt-5.6-sol']
const plans = ['pro', 'prolite', 'plus', 'plus', 'team', 'free', null]
const answers: Record<Exclude<RecordStatus, 'pending' | 'failed'>, string> = {
  correct: '**答案：21**\n\n最坏情况下先取出全部西瓜味 12 个，再取出某一口味同形状的全部糖果……因此最少需要 **21** 个。',
  wrong: '最少需要 **20** 个糖果才能保证满足条件。',
}
let sequence = 0

function record(model: string, effort: string, status: RecordStatus, minutesAgo: number): RecordDetail {
  const id = `preview-${++sequence}`
  const base = {id, time: Date.now() - minutesAgo * 60_000, model, effort, status, request_id: `preview-request-${sequence}`}
  if (status === 'failed') return {...base, error: '单次请求达到 100 秒上限', duration_ms: 100_000}
  if (status === 'pending') return base
  return {...base, answer: answers[status], duration_ms: 12_000 + (sequence * 3731) % 20_000, input_tokens: 312, output_tokens: 600 + (sequence * 97) % 700, reasoning_tokens: 300 + (sequence * 53) % 500}
}
function history(index: number) {
  if (index % 5 === 4) return []
  const wrongEvery = index % 3 === 2 ? 2 : 9
  return Array.from({length: 6 + (index * 7) % 14}, (_, i) => record(i % 6 === 5 ? 'gpt-5.6-sol' : 'gpt-6.1-sol', 'low', i % 11 === 10 ? 'failed' : i % wrongEvery === 1 ? 'wrong' : 'correct', 600 - i * 20))
}
function account(index: number): Account & {details: RecordDetail[]} {
  const details = history(index)
  return {id: `demo-${index}`, name: 'OPENAI账号', email: index === 3 ? '   ' : `user${index + 1}@example.com`, plan: plans[index % plans.length]!, enabled: index !== 5, credential_state: index === 6 ? 'expired' : 'ready', running: false, details,
    records: details.map(({id, time, model, effort, status, duration_ms, input_tokens, output_tokens, reasoning_tokens}) => ({id, time, model, effort, status, duration_ms, input_tokens, output_tokens, reasoning_tokens}))}
}
const summary = ({answer: _a, answer_truncated: _t, error: _e, request_id: _r, ...rest}: RecordDetail) => rest

export function installPreview() {
  applyResolvedTheme(document.documentElement, resolveTheme(params.get('theme') === 'dark' ? 'dark' : 'light', DEFAULT_THEME_COLOR, DEFAULT_CUSTOM_THEME_COLOR))
  document.body.style.background = 'var(--cp-color-bg-layout)'
  document.body.style.padding = '24px'
  const accounts = scenario === 'empty' ? [] : Array.from({length: scenario === 'many' ? 60 : 8}, (_, i) => account(i))
  window.codexProxyPlugin = {version: 2, async request(input) {
    await new Promise(resolve => setTimeout(resolve, scenario === 'loading' ? 12000 : 80))
    const data = input.body ? JSON.parse(input.body) : {}
    const respond = (value: unknown, status = 200) => ({status, contentType: 'application/json', body: new TextEncoder().encode(JSON.stringify(value)).buffer})
    if (scenario === 'error') return respond({error: {message: '模拟：宿主账号目录读取失败，请重试'}}, 503)
    if (input.path === 'api/bootstrap') return respond({prompt, answer: '21', efforts: ['none', 'low', 'medium', 'high', 'xhigh', 'max'], timeout_seconds: 100, history_limit: 20, history_shown: 20, parallel_limit: 8,
      accounts: accounts.map(({details: _d, ...a}) => a), keys: [{id: 'demo-k', name: '降智测试专用', enabled: true}, {id: 'demo-k2', name: '备用 Key', enabled: true}]})
    if (input.path === 'api/models') return respond({models})
    const target = accounts.find(a => a.id === data.account_id)
    if (input.path === 'api/candy/clear') {
      for (const id of data.account_ids) { const a = accounts.find(a => a.id === id); if (a) { a.details = []; a.records = [] } }
      return respond({cleared: data.account_ids.length, skipped: []})
    }
    if (!target) return respond({error: {message: '模拟账号不存在'}}, 400)
    if (input.path === 'api/candy/record') {
      const found = target.details.find(r => r.id === data.record_id)
      return found ? respond({record: found}) : respond({error: {message: '记录不存在或已被清空'}}, 400)
    }
    if (input.path === 'api/candy/run') {
      if (!target.enabled) return respond({error: {message: '账号已停用或不再是 OpenAI 账号，请刷新列表'}}, 400)
      await new Promise(resolve => setTimeout(resolve, scenario === 'slow' ? 2500 : 500 + Math.random() * 700))
      const roll = Math.random(), status: RecordStatus = target.id === 'demo-2' ? (roll < .5 ? 'wrong' : 'correct') : roll < .08 ? 'failed' : roll < .2 ? 'wrong' : 'correct'
      const result = record(data.model, data.effort, status, 0)
      target.details = [...target.details, result].slice(-20)
      target.records = target.details.map(summary)
      return respond({record: summary(result), records: target.records})
    }
    return respond({error: {message: '未实现的模拟接口'}}, 404)
  }}
}

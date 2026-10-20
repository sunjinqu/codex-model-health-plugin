import type { RecordStatus, RecordSummary } from './api'

export type MarkKind = 'ok' | 'bad' | 'err' | 'run'
export type Tone = 'success' | 'warning' | 'danger' | 'neutral'
export const statusText: Record<RecordStatus, string> = {correct: '答对', wrong: '答错', failed: '出错', pending: '测试中'}
export const statusKind: Record<RecordStatus, MarkKind> = {correct: 'ok', wrong: 'bad', failed: 'err', pending: 'run'}

export function effortText(effort: string) { return effort === 'none' ? '默认' : effort }
export function modelLabel(model: string, effort: string) { return effort === 'none' ? model : `${model}(${effort})` }
export function inScope(record: RecordSummary, model: string, effort: string) { return record.model === model && record.effort === effort }

/** 只统计当前模型与推理强度下判过分的回答；出错与中断不计入 */
export function accuracy(records: RecordSummary[], model: string, effort: string) {
  const graded = records.filter(r => inScope(r, model, effort) && (r.status === 'correct' || r.status === 'wrong'))
  const correct = graded.filter(r => r.status === 'correct').length
  return {correct, graded: graded.length, rate: graded.length ? correct / graded.length : null}
}
export function rateTone(rate: number | null): Tone { return rate === null ? 'neutral' : rate >= .8 ? 'success' : rate >= .5 ? 'warning' : 'danger' }
export function formatPercent(rate: number) { return `${Math.round(rate * 100)}%` }
export function formatDuration(ms?: number) {
  if (ms == null) return '—'
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`
  const seconds = Math.round(ms / 1000)
  return `${Math.floor(seconds / 60)}m${String(seconds % 60).padStart(2, '0')}s`
}
export function formatNumber(value?: number) { return value == null ? '—' : value.toLocaleString('en-US') }
export function formatTime(value: number) { return new Date(value).toLocaleString('zh-CN', {hour12: false}) }

// 订阅名称与排序沿用 cpa-plugin-codex-candy-eval（MIT），与官方 Codex 客户端的叫法一致
const plans: {label: string; ids: string[]; tone: 'warning' | 'primary' | 'info' | 'neutral'}[] = [
  {label: 'Pro 500', ids: ['promax'], tone: 'warning'},
  {label: 'Pro 200', ids: ['pro'], tone: 'warning'},
  {label: 'Pro 100', ids: ['prolite', 'pro-lite', 'pro_lite'], tone: 'warning'},
  {label: 'Plus', ids: ['plus'], tone: 'primary'},
  {label: 'Go', ids: ['go'], tone: 'primary'},
  {label: 'Business Premium', ids: ['self_serve_business_prolite'], tone: 'info'},
  {label: 'Business', ids: ['team', 'self_serve_business_usage_based'], tone: 'info'},
  {label: 'Enterprise', ids: ['business', 'enterprise', 'ent26', 'enterprise_cbp_usage_based', 'hc'], tone: 'info'},
  {label: 'Enterprise (Automation)', ids: ['enterprise_cbp_automation'], tone: 'info'},
  {label: 'Edu Pro', ids: ['edu_pro'], tone: 'neutral'},
  {label: 'Edu Plus', ids: ['edu_plus'], tone: 'neutral'},
  {label: 'Edu', ids: ['edu', 'education'], tone: 'neutral'},
  {label: 'Free', ids: ['free'], tone: 'neutral'},
]
export function planInfo(plan?: string | null) {
  const id = plan?.trim().toLowerCase() ?? ''
  const rank = plans.findIndex(p => p.ids.includes(id))
  if (rank >= 0) return {id, label: plans[rank]!.label, tone: plans[rank]!.tone, rank}
  return {id, label: plan?.trim() || '未知订阅', tone: 'neutral' as const, rank: plans.length}
}

const credentialStates: Record<string, string> = {unknown: '未验证', expired: '凭据过期', banned: '已封禁', invalid: '凭据无效'}
export function credentialStateText(state: string) { return state === 'ready' ? '' : credentialStates[state] ?? state }

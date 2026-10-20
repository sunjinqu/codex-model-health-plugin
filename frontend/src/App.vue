<script setup lang="ts">
import { computed, onMounted, onScopeDispose, reactive, ref, watch } from 'vue'
import { BaseButton, BaseCard, BaseCheckbox, BaseConfirmModal, BaseIconButton, BaseInput, BaseNumberInput, BaseSelect, BaseSwitch, BaseTable, BaseTag } from '@codex-proxy/ui'
import type { BaseTableColumn, BaseTableSort } from '@codex-proxy/ui'
import { ArrowDownToLine, ArrowUpFromLine, Brain, Candy, ChevronRight, Clock, Copy, LoaderCircle, Play, RefreshCw, Search, Square, Trash2 } from '@lucide/vue'
import { api } from './api'
import type { Account, Bootstrap, ClearResult, RecordStatus, RunResult } from './api'
import { accountDisplayName } from './accountDisplay'
import { accuracy, credentialStateText, formatDuration, formatNumber, formatPercent, formatTime, inScope, modelLabel, planInfo, rateTone, statusText } from './presentation'
import StatusMark from './components/StatusMark.vue'
import RecordHistory from './components/RecordHistory.vue'

const preview = import.meta.env.DEV && new URLSearchParams(location.search).get('preview') === '1'
const boot = ref<Bootstrap | null>(null), accounts = ref<Account[]>([])
const loading = ref(true), error = ref(''), notices = ref<string[]>([]), copied = ref(''), clearNote = ref('')
const key = ref(''), model = ref(''), effort = ref('low'), runs = ref(1), parallel = ref(3)
const models = ref<string[]>([]), modelsLoading = ref(false)
const onlyEnabled = ref(true), planFilter = ref(''), search = ref(''), sort = ref<BaseTableSort>()
const selected = ref(new Set<string>()), expanded = ref(new Set<string>()), focus = ref<Record<string, {id: string; seq: number}>>({})
const progress = reactive(new Map<string, {done: number; total: number; started: boolean}>())
const batch = reactive({running: false, stopping: false, total: 0, done: 0, correct: 0, wrong: 0, failed: 0, skipped: 0})
const confirmClear = ref(false), clearing = ref(false)
let disposed = false, modelSequence = 0, focusSequence = 0

type Row = Account & {display: string; plan_label: string; plan_tone: 'warning' | 'primary' | 'info' | 'neutral'; plan_rank: number; stats: ReturnType<typeof accuracy>; last?: Account['records'][number]}
const columns: BaseTableColumn<Row>[] = [
  {key: 'select', kind: 'selection', label: '选择'},
  {key: 'account', label: '账号', kind: 'custom', size: '3xl', sortable: true},
  {key: 'latest', label: '最近一次', kind: 'custom', size: '3xl', sortable: true},
  {key: 'rate', label: '正确率', kind: 'custom', size: 'md', sortable: true},
  {key: 'marks', label: '最近记录', kind: 'custom', size: '2xl'},
  {key: 'actions', label: '操作', kind: 'actions'},
]

const busy = computed(() => batch.running || loading.value)
const keyOptions = computed(() => boot.value?.keys.map(k => ({value: k.id, label: k.name})) ?? [])
const modelOptions = computed(() => models.value.map(m => ({value: m, label: m})))
const effortOptions = computed(() => boot.value?.efforts.map(e => ({value: e, label: e === 'none' ? '默认（由主服务决定）' : e})) ?? [])
const planOptions = computed(() => {
  const plans = new Map<string, number>()
  for (const a of accounts.value) { const p = planInfo(a.plan); plans.set(p.label, p.rank) }
  return [{value: '', label: '全部订阅类型'}, ...[...plans].sort((a, b) => a[1] - b[1]).map(([label]) => ({value: label, label}))]
})
const rows = computed<Row[]>(() => {
  const query = search.value.trim().toLowerCase()
  const list = accounts.value
    .filter(a => (!onlyEnabled.value || a.enabled) && (!planFilter.value || planInfo(a.plan).label === planFilter.value) && (!query || `${a.email ?? ''} ${a.name} ${a.id}`.toLowerCase().includes(query)))
    .map(a => { const plan = planInfo(a.plan); return {...a, display: accountDisplayName(a) || a.id, plan_label: plan.label, plan_tone: plan.tone, plan_rank: plan.rank, stats: accuracy(a.records, model.value, effort.value), last: a.records.at(-1)} })
  const direction = sort.value?.direction === 'desc' ? -1 : 1
  const by: Record<string, (a: Row, b: Row) => number> = {
    account: (a, b) => a.display.localeCompare(b.display),
    latest: (a, b) => (a.last?.time ?? 0) - (b.last?.time ?? 0),
    rate: (a, b) => (a.stats.rate ?? -1) - (b.stats.rate ?? -1) || a.stats.graded - b.stats.graded,
  }
  const compare = sort.value ? by[sort.value.key] : undefined
  return list.sort((a, b) => compare ? compare(a, b) * direction : Number(b.enabled) - Number(a.enabled) || a.plan_rank - b.plan_rank || a.display.localeCompare(b.display))
})
const visibleTestable = computed(() => rows.value.filter(r => r.enabled))
const targets = computed(() => selected.value.size ? accounts.value.filter(a => selected.value.has(a.id) && a.enabled) : visibleTestable.value)
const allChecked = computed(() => visibleTestable.value.length > 0 && visibleTestable.value.every(r => selected.value.has(r.id)))
const someChecked = computed(() => !allChecked.value && visibleTestable.value.some(r => selected.value.has(r.id)))
const scopeLabel = computed(() => model.value ? modelLabel(model.value, effort.value) : '—')
const overall = computed(() => accuracy(accounts.value.flatMap(a => a.records), model.value, effort.value))
const canRun = computed(() => !!(key.value && model.value && !modelsLoading.value && !busy.value))
const clearScope = computed(() => selected.value.size ? `所选 ${selected.value.size} 个账号` : `全部 ${accounts.value.length} 个账号`)

function message(cause: unknown) { return cause instanceof Error ? cause.message : String(cause) }

async function load() {
  loading.value = true; error.value = ''
  try {
    const data = await api<Bootstrap>('bootstrap')
    boot.value = data; accounts.value = data.accounts
    const ids = new Set(data.accounts.map(a => a.id))
    selected.value = new Set([...selected.value].filter(id => ids.has(id)))
    // 沙箱页面无法使用浏览器存储；首次打开时沿用最近一次测试的模型与推理强度
    const latest = data.accounts.flatMap(a => a.records).sort((a, b) => b.time - a.time)[0]
    if (!model.value && latest) { model.value = latest.model; effort.value = latest.effort }
    if (!data.efforts.includes(effort.value)) effort.value = data.efforts.includes('low') ? 'low' : data.efforts[0] ?? 'none'
    parallel.value = Math.min(parallel.value, data.parallel_limit)
    const chosen = data.keys.some(k => k.id === key.value) ? key.value : data.keys[0]?.id ?? ''
    if (chosen === key.value) await loadModels(chosen)
    else key.value = chosen
  } catch (cause) { error.value = message(cause) }
  finally { loading.value = false }
}
async function loadModels(value: string) {
  const sequence = ++modelSequence
  models.value = []
  if (!value) { model.value = ''; return }
  modelsLoading.value = true
  try {
    const result = await api<{models: string[]}>('models', {key_id: value})
    if (sequence !== modelSequence) return
    models.value = result.models
    if (!result.models.includes(model.value)) model.value = result.models[0] ?? ''
  } catch (cause) { if (sequence === modelSequence) error.value = message(cause) }
  finally { if (sequence === modelSequence) modelsLoading.value = false }
}
watch(key, value => { void loadModels(value) })

function toggleSelect(id: string, value: boolean) { const next = new Set(selected.value); if (value) next.add(id); else next.delete(id); selected.value = next }
function toggleAll(value: boolean) { const next = new Set(selected.value); for (const r of visibleTestable.value) { if (value) next.add(r.id); else next.delete(r.id) } selected.value = next }
function toggleExpand(id: string) { const next = new Set(expanded.value); if (next.has(id)) next.delete(id); else next.add(id); expanded.value = next }
function openRecord(accountId: string, recordId: string) {
  focus.value = {...focus.value, [accountId]: {id: recordId, seq: ++focusSequence}}
  if (!expanded.value.has(accountId)) toggleExpand(accountId)
}
function setRecords(id: string, records: Account['records']) { const account = accounts.value.find(a => a.id === id); if (account) account.records = records }
function tally(status: RecordStatus) { if (status === 'correct') batch.correct++; else if (status === 'wrong') batch.wrong++; else batch.failed++ }

/** 每个账号串行作答，最多 parallel 个账号同时进行；页面关闭或点停止后不再发出新请求 */
async function runAccounts(list: Account[]) {
  if (batch.running || !canRun.value || !list.length) return
  const params = {key_id: key.value, model: model.value, effort: effort.value}
  const perAccount = Math.min(Math.max(Math.round(runs.value) || 1, 1), 10), workers = Math.min(Math.max(Math.round(parallel.value) || 1, 1), boot.value?.parallel_limit ?? 8)
  Object.assign(batch, {running: true, stopping: false, total: list.length * perAccount, done: 0, correct: 0, wrong: 0, failed: 0, skipped: 0})
  notices.value = []; clearNote.value = ''
  for (const a of list) progress.set(a.id, {done: 0, total: perAccount, started: false})
  const queue = [...list]
  const worker = async () => {
    while (queue.length && !batch.stopping && !disposed) {
      const account = queue.shift()!, state = progress.get(account.id)!
      state.started = true
      for (let i = 0; i < perAccount; i++) {
        if (batch.stopping || disposed) { batch.skipped += perAccount - i; batch.done += perAccount - i; break }
        try {
          const result = await api<RunResult>('candy/run', {account_id: account.id, ...params})
          setRecords(account.id, result.records); tally(result.record.status)
        } catch (cause) {
          // 未发出或未落盘的请求不算答题；该账号剩余次数跳过，其他账号继续
          notices.value = [...notices.value, `${accountDisplayName(account) || account.id}：${message(cause)}`]
          batch.skipped += perAccount - i; batch.done += perAccount - i
          break
        }
        state.done++; batch.done++
      }
      progress.delete(account.id)
    }
  }
  await Promise.all(Array.from({length: Math.min(workers, list.length)}, worker))
  for (const a of queue) { progress.delete(a.id); batch.skipped += perAccount; batch.done += perAccount }
  batch.running = false; batch.stopping = false
}
function runOne(id: string) { const account = accounts.value.find(a => a.id === id); if (account) void runAccounts([account]) }

async function clearRecords() {
  clearing.value = true
  const ids = selected.value.size ? [...selected.value] : accounts.value.map(a => a.id)
  try {
    const result = await api<ClearResult>('candy/clear', {account_ids: ids})
    const skipped = new Set(result.skipped)
    for (const id of ids) if (!skipped.has(id)) setRecords(id, [])
    clearNote.value = `已清空 ${ids.length - skipped.size} 个账号的记录${skipped.size ? `，${skipped.size} 个正在测试的账号已跳过` : ''}`
    confirmClear.value = false
  } catch (cause) { error.value = message(cause); confirmClear.value = false }
  finally { clearing.value = false }
}
function copyPrompt() {
  const area = document.getElementById('candy-prompt') as HTMLTextAreaElement
  area.focus(); area.select()
  // 宿主 iframe 不开放异步剪贴板；复制失败时保留选区供手动复制
  try { copied.value = document.execCommand('copy') ? '已复制' : '题目已选中，请手动复制' } catch { copied.value = '题目已选中，请手动复制' }
}

onMounted(() => { void load() })
onScopeDispose(() => { disposed = true })
</script>

<template>
  <main class="flex min-w-0 flex-col gap-4 font-sans text-cp-text">
    <div v-if="preview" class="preview-note" role="note">独立模拟预览 · 不代表真实账号的测试结果</div>

    <section class="intro">
      <div class="intro-text">
        <h1><Candy class="size-5" />账号降智测试</h1>
        <p>让模型解一道糖果题，答出 <b class="font-mono">{{ boot?.answer ?? '21' }}</b> 即为正确。同一模型与推理强度下多次作答，正确率明显偏低的账号可能被降智。</p>
        <details class="problem">
          <summary><ChevronRight class="size-4 chev" />查看题目</summary>
          <div class="problem-body">
            <textarea id="candy-prompt" :value="boot?.prompt ?? ''" readonly spellcheck="false" aria-label="糖果题题目" />
            <div class="problem-actions"><BaseButton size="sm" variant="ghost" @click="copyPrompt"><template #icon><Copy class="size-3.5" /></template>复制题目</BaseButton><span v-if="copied" role="status" class="muted text-cp-xs">{{ copied }}</span></div>
          </div>
        </details>
      </div>
      <BaseIconButton label="刷新账号和记录" :disabled="busy" @click="load"><RefreshCw class="size-4" /></BaseIconButton>
    </section>

    <div v-if="error" class="error-panel" role="alert"><strong>操作未完成</strong><p>{{ error }}</p><BaseButton size="sm" :disabled="busy" @click="load">重新读取</BaseButton></div>

    <BaseCard>
      <template #body>
        <form class="toolbar" @submit.prevent="batch.running ? (batch.stopping = true) : runAccounts(targets)">
          <div class="field"><label id="key-label">计量 Key</label><BaseSelect v-model="key" aria-labelledby="key-label" :options="keyOptions" :disabled="busy" placeholder="选择用于计量的 Key" filterable /></div>
          <div class="field field-wide"><label id="model-label">模型</label><BaseSelect v-model="model" aria-labelledby="model-label" :options="modelOptions" :loading="modelsLoading" :disabled="!key || batch.running" placeholder="先选择计量 Key" filterable /></div>
          <div class="field"><label id="effort-label">推理强度</label><BaseSelect v-model="effort" aria-labelledby="effort-label" :options="effortOptions" :disabled="batch.running" /></div>
          <div class="field field-narrow"><span class="field-label">每账号次数</span><BaseNumberInput v-model="runs" size="md" label="每账号次数" :min="1" :max="10" :step="1" :disabled="batch.running" /></div>
          <div class="field field-narrow"><span class="field-label">并发账号</span><BaseNumberInput v-model="parallel" size="md" label="并发账号" :min="1" :max="boot?.parallel_limit ?? 8" :step="1" :disabled="batch.running" /></div>
          <BaseButton v-if="batch.running" class="run-button" type="submit" :disabled="batch.stopping"><template #icon><Square class="size-3.5" /></template>{{ batch.stopping ? '等待当前请求…' : '停止' }}</BaseButton>
          <BaseButton v-else class="run-button" variant="primary" type="submit" :disabled="!canRun || !targets.length"><template #icon><Play class="size-4" /></template>{{ selected.size ? '测试所选' : '测试全部' }} ({{ targets.length }})</BaseButton>
        </form>
        <p class="toolbar-hint">真实调用计入所选 Key 的用量 · 单次请求限时 {{ boot?.timeout_seconds ?? 100 }} 秒（宿主管理接口上限），高推理强度可能超时 · 出错和中断不计入正确率</p>
        <div v-if="batch.total" class="batch" role="status">
          <div class="batch-facts">
            <span><LoaderCircle v-if="batch.running" class="size-3.5 spin" />{{ batch.running ? (batch.stopping ? '正在停止' : '测试中') : '本轮结束' }} <b class="font-mono">{{ batch.done }}/{{ batch.total }}</b></span>
            <span class="ok">答对 <b class="font-mono">{{ batch.correct }}</b></span>
            <span class="bad">答错 <b class="font-mono">{{ batch.wrong }}</b></span>
            <span class="err">出错 <b class="font-mono">{{ batch.failed }}</b></span>
            <span v-if="batch.skipped" class="muted">跳过 <b class="font-mono">{{ batch.skipped }}</b></span>
            <span v-if="batch.correct + batch.wrong" class="muted">本轮正确率 <b class="font-mono">{{ formatPercent(batch.correct / (batch.correct + batch.wrong)) }}</b></span>
          </div>
          <progress :value="batch.done" :max="batch.total" aria-label="本轮测试进度" />
        </div>
      </template>
    </BaseCard>

    <div v-if="notices.length" class="notice-panel" role="status"><strong>部分账号未完成</strong><ul><li v-for="(note, i) in notices" :key="i">{{ note }}</li></ul></div>

    <div class="legend">
      <span><StatusMark status="correct" />答对</span>
      <span><StatusMark status="wrong" />答错</span>
      <span><StatusMark status="failed" />请求出错 / 中断</span>
      <span><StatusMark status="correct" dim />其他模型或推理强度</span>
      <span class="legend-scope">正确率按 <b class="font-mono">{{ scopeLabel }}</b> 统计<template v-if="overall.graded"> · 全部账号 <b class="font-mono">{{ formatPercent(overall.rate!) }}</b>（{{ overall.correct }}/{{ overall.graded }}）</template></span>
    </div>

    <BaseCard>
      <template #body>
        <div class="list-head">
          <div class="list-filters">
            <h2>账号 <span class="muted font-normal text-cp-sm">{{ rows.length }} / {{ accounts.length }}</span></h2>
            <BaseSwitch v-model="onlyEnabled" label="仅启用" show-label />
            <BaseSelect v-model="planFilter" class="plan-filter" aria-label="订阅类型" :options="planOptions" size="sm" />
            <div class="search"><Search class="size-4" /><BaseInput v-model="search" size="sm" placeholder="搜索邮箱或名称" aria-label="搜索账号" /></div>
          </div>
          <BaseIconButton :label="`清空${clearScope}的测试记录`" :disabled="busy || !accounts.some(a => a.records.length)" @click="confirmClear = true"><Trash2 class="size-4" /></BaseIconButton>
        </div>
        <p v-if="clearNote" class="muted text-cp-xs" role="status">{{ clearNote }}</p>
        <p v-if="boot && boot.history_shown < boot.history_limit" class="muted text-cp-xs">账号较多，列表只显示每个账号最近 {{ boot.history_shown }} 次记录（实际保留 {{ boot.history_limit }} 次）。</p>
        <BaseTable :columns="columns" :rows="rows" :loading="loading" :sort="sort" :selected-row-keys="[...selected]" :expanded-row-keys="[...expanded]" show-header-when-empty empty-text="没有符合条件的账号。请先在主项目启用 OpenAI 账号，或调整筛选。" @sort-change="value => sort = value">
          <template #header-select><BaseCheckbox :model-value="allChecked" :indeterminate="someChecked" label="选择全部可测试账号" :disabled="batch.running || !visibleTestable.length" @update:model-value="toggleAll" /></template>
          <template #select="{row}"><BaseCheckbox :model-value="selected.has(row.id)" :label="`选择 ${row.display}`" :disabled="!row.enabled || batch.running" @update:model-value="value => toggleSelect(row.id, value)" /></template>
          <template #account="{row}">
            <button type="button" class="account-cell" :aria-expanded="expanded.has(row.id)" :aria-label="`${expanded.has(row.id) ? '收起' : '展开'} ${row.display} 的测试记录`" @click="toggleExpand(row.id)">
              <ChevronRight class="size-4 chev" :class="{open: expanded.has(row.id)}" />
              <span class="account-text">
                <span class="account-name" :title="row.display">{{ row.display }}</span>
                <span class="account-tags">
                  <BaseTag size="sm" :type="row.plan_tone">{{ row.plan_label }}</BaseTag>
                  <BaseTag v-if="!row.enabled" size="sm">已停用</BaseTag>
                  <BaseTag v-if="credentialStateText(row.credential_state)" size="sm" type="danger">{{ credentialStateText(row.credential_state) }}</BaseTag>
                </span>
              </span>
            </button>
          </template>
          <template #latest="{row}">
            <div v-if="row.last" class="latest">
              <div class="latest-line"><span class="status-pill" :class="row.last.status"><StatusMark :status="row.last.status" />{{ statusText[row.last.status] }}</span><span class="font-mono text-cp-sm truncate" :class="{muted: !inScope(row.last, model, effort)}">{{ modelLabel(row.last.model, row.last.effort) }}</span></div>
              <div class="latest-line metrics">
                <span class="metric" title="耗时"><Clock class="size-3.5" />{{ formatDuration(row.last.duration_ms) }}</span>
                <span class="metric" title="输入 Token"><ArrowDownToLine class="size-3.5" />{{ formatNumber(row.last.input_tokens) }}</span>
                <span class="metric" title="输出 Token"><ArrowUpFromLine class="size-3.5" />{{ formatNumber(row.last.output_tokens) }}</span>
                <span v-if="row.last.reasoning_tokens" class="metric" title="推理 Token"><Brain class="size-3.5" />{{ formatNumber(row.last.reasoning_tokens) }}</span>
              </div>
            </div>
            <span v-else class="muted">未测试</span>
          </template>
          <template #rate="{row}">
            <div v-if="row.stats.rate !== null" class="rate" :class="rateTone(row.stats.rate)"><b>{{ formatPercent(row.stats.rate) }}</b><small>{{ row.stats.correct }}/{{ row.stats.graded }}</small></div>
            <span v-else class="muted">—</span>
          </template>
          <template #marks="{row}">
            <div class="marks">
              <button v-for="r in row.records" :key="r.id" type="button" class="mark-hit" :title="`${formatTime(r.time)} · ${modelLabel(r.model, r.effort)} · ${statusText[r.status]}`" :aria-label="`${formatTime(r.time)} ${modelLabel(r.model, r.effort)} ${statusText[r.status]}`" @click="openRecord(row.id, r.id)"><StatusMark :status="r.status" :dim="!inScope(r, model, effort)" /></button>
            </div>
          </template>
          <template #actions="{row}">
            <BaseButton v-if="progress.get(row.id)?.started" size="sm" variant="ghost" disabled :aria-label="`测试中 ${progress.get(row.id)!.done}/${progress.get(row.id)!.total}`"><template #icon><LoaderCircle class="size-3.5 spin" /></template><span class="font-mono">{{ progress.get(row.id)!.done }}/{{ progress.get(row.id)!.total }}</span></BaseButton>
            <BaseButton v-else-if="progress.has(row.id)" size="sm" variant="ghost" disabled>{{ batch.stopping ? '已取消' : '排队中' }}</BaseButton>
            <BaseButton v-else size="sm" variant="soft" :disabled="!row.enabled || row.running || !canRun" :aria-label="`测试 ${row.display}`" @click="runOne(row.id)"><template #icon><Play class="size-3.5" /></template>测试</BaseButton>
          </template>
          <template #expanded="{row}"><RecordHistory :account-id="row.id" :records="row.records" :model="model" :effort="effort" :focus="focus[row.id]" /></template>
        </BaseTable>
      </template>
    </BaseCard>

    <BaseConfirmModal v-model="confirmClear" title="清空测试记录" :description="`将删除${clearScope}的全部糖果题记录，正确率随之重置。正在测试的账号会被跳过。`" destructive confirm-text="清空" :loading="clearing" @confirm="clearRecords" />
  </main>
</template>

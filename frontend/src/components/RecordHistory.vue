<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { BaseMarkdown } from '@codex-proxy/ui'
import { ArrowDownToLine, ArrowUpFromLine, Brain, ChevronRight, Clock } from '@lucide/vue'
import { api } from '../api'
import type { RecordDetail, RecordSummary } from '../api'
import { formatDuration, formatNumber, formatTime, inScope, modelLabel, statusText } from '../presentation'
import StatusMark from './StatusMark.vue'

const props = defineProps<{ accountId: string; records: RecordSummary[]; model: string; effort: string; focus?: {id: string; seq: number} }>()
// 已完成的记录不会再变，详情在页面内缓存；进行中的记录每次重新读取
const details = reactive(new Map<string, RecordDetail>())
const open = reactive(new Set<string>())
const loading = reactive(new Set<string>())
const errors = reactive(new Map<string, string>())
const newestFirst = computed(() => [...props.records].reverse())

async function toggle(record: RecordSummary, force = false) {
  if (open.has(record.id) && !force) { open.delete(record.id); return }
  open.add(record.id)
  if ((details.has(record.id) && record.status !== 'pending') || loading.has(record.id)) return
  loading.add(record.id); errors.delete(record.id)
  try { details.set(record.id, (await api<{record: RecordDetail}>('candy/record', {account_id: props.accountId, record_id: record.id})).record) }
  catch (cause) { errors.set(record.id, cause instanceof Error ? cause.message : String(cause)) }
  finally { loading.delete(record.id) }
}
watch(() => props.focus?.seq, () => { const record = props.records.find(r => r.id === props.focus?.id); if (record) void toggle(record, true) }, {immediate: true})
</script>

<template>
  <div class="history">
    <p v-if="!records.length" class="muted text-cp-sm m-0">这个账号还没有测试记录。</p>
    <article v-for="record in newestFirst" :key="record.id" class="history-item" :class="{focus: record.id === focus?.id}">
      <div class="history-head">
        <span class="status-pill" :class="record.status"><StatusMark :status="record.status" />{{ statusText[record.status] }}</span>
        <span class="font-mono text-cp-sm" :class="{muted: !inScope(record, model, effort)}">{{ modelLabel(record.model, record.effort) }}</span>
        <span class="metric" title="测试时间">{{ formatTime(record.time) }}</span>
        <span class="metric" title="耗时"><Clock class="size-3.5" />{{ formatDuration(record.duration_ms) }}</span>
        <span class="metric" title="输入 Token"><ArrowDownToLine class="size-3.5" />{{ formatNumber(record.input_tokens) }}</span>
        <span class="metric" title="输出 Token"><ArrowUpFromLine class="size-3.5" />{{ formatNumber(record.output_tokens) }}</span>
        <span v-if="record.reasoning_tokens" class="metric" title="推理 Token"><Brain class="size-3.5" />{{ formatNumber(record.reasoning_tokens) }}</span>
        <button type="button" class="answer-toggle" :aria-expanded="open.has(record.id)" @click="toggle(record)"><ChevronRight class="size-3.5 chev" />{{ open.has(record.id) ? '收起' : record.status === 'failed' ? '查看错误' : '查看回答' }}</button>
      </div>
      <div v-if="open.has(record.id)" class="answer-body">
        <p v-if="loading.has(record.id)" class="muted m-0" role="status">正在读取…</p>
        <p v-else-if="errors.has(record.id)" class="answer-error m-0" role="alert">{{ errors.get(record.id) }}</p>
        <template v-else-if="details.get(record.id)">
          <p v-if="details.get(record.id)!.error" class="answer-error m-0">{{ details.get(record.id)!.error }}</p>
          <BaseMarkdown v-else-if="details.get(record.id)!.answer" class="answer-markdown" :source="details.get(record.id)!.answer" />
          <p v-else class="muted m-0">{{ record.status === 'pending' ? '请求进行中，完成后可查看回答。' : '没有回答文本。' }}</p>
          <p class="answer-meta">
            <span v-if="details.get(record.id)!.answer_truncated">回答较长，只保存了前 3000 字节；判分使用完整回答。</span>
            <span v-if="details.get(record.id)!.request_id">请求 ID <code>{{ details.get(record.id)!.request_id }}</code></span>
          </p>
        </template>
      </div>
    </article>
  </div>
</template>

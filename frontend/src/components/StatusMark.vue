<script setup lang="ts">
import { CircleAlert, CircleCheck, CircleX, LoaderCircle } from '@lucide/vue'
import type { RecordStatus } from '../api'
import { statusKind } from '../presentation'

defineProps<{ status: RecordStatus; dim?: boolean }>()
</script>

<template>
  <CircleCheck v-if="status === 'correct'" class="mark" :class="[statusKind[status], {dim}]" aria-hidden="true" />
  <CircleX v-else-if="status === 'wrong'" class="mark" :class="[statusKind[status], {dim}]" aria-hidden="true" />
  <LoaderCircle v-else-if="status === 'pending'" class="mark spin" :class="statusKind[status]" aria-hidden="true" />
  <CircleAlert v-else class="mark" :class="[statusKind[status], {dim}]" aria-hidden="true" />
</template>

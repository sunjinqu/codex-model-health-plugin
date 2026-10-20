export interface BridgeInput { path: string; method: string; contentType?: string; body?: string }
export interface PluginHost { version: number; request(input: BridgeInput): Promise<{status: number; contentType: string; body: ArrayBuffer}> }
declare global { interface Window { codexProxyPlugin?: PluginHost } }

export async function api<T>(path: string, data?: object): Promise<T> {
  const host = window.codexProxyPlugin
  if (!host || host.version !== 2) throw new Error('请从 Codex Proxy 的插件菜单打开此页')
  const reply = await host.request({ path: `api/${path}`, method: data === undefined ? 'GET' : 'POST', ...(data === undefined ? {} : { contentType: 'application/json', body: JSON.stringify(data) }) })
  if (!reply.contentType.toLowerCase().startsWith('application/json')) throw new Error('插件响应类型错误，请检查安装包资源')
  const body = JSON.parse(new TextDecoder().decode(reply.body))
  if (reply.status < 200 || reply.status >= 300) throw new Error(body.message || body.error?.message || `请求失败（${reply.status}）`)
  return body as T
}

export type RecordStatus = 'pending' | 'correct' | 'wrong' | 'failed'
export interface RecordSummary { id: string; time: number; model: string; effort: string; status: RecordStatus; duration_ms?: number; input_tokens?: number; output_tokens?: number; reasoning_tokens?: number }
export interface RecordDetail extends RecordSummary { answer?: string; answer_truncated?: boolean; error?: string; request_id?: string }
export interface Account { id: string; name: string; email?: string | null; plan: string | null; enabled: boolean; credential_state: string; running: boolean; records: RecordSummary[] }
export interface ClientKey { id: string; name: string; enabled: boolean }
export interface Bootstrap { prompt: string; answer: string; efforts: string[]; timeout_seconds: number; history_limit: number; history_shown: number; parallel_limit: number; accounts: Account[]; keys: ClientKey[] }
export interface RunResult { record: RecordSummary; records: RecordSummary[] }
export interface ClearResult { cleared: number; skipped: string[] }

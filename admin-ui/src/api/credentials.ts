import axios from 'axios'
import { storage } from '@/lib/storage'
import type {
  CredentialsStatusResponse,
  BalanceResponse,
  SuccessResponse,
  SetDisabledRequest,
  SetPriorityRequest,
  AddCredentialRequest,
  AddCredentialResponse,
  ExportCredentialsResponse,
  RuntimeStatusResponse,
  RuntimeSettings,
  SetCredentialPolicyRequest,
  BatchCredentialPolicyRequest,
  DynamicProxyActionResponse,
  DynamicProxyBatchActionResponse,
} from '@/types/api'

// 创建 axios 实例
const api = axios.create({
  baseURL: '/api/admin',
  headers: {
    'Content-Type': 'application/json',
  },
})

// 请求拦截器添加 API Key
api.interceptors.request.use((config) => {
  const apiKey = storage.getApiKey()
  if (apiKey) {
    config.headers['x-api-key'] = apiKey
  }
  return config
})

// 获取所有凭据状态
export async function getCredentials(): Promise<CredentialsStatusResponse> {
  const { data } = await api.get<CredentialsStatusResponse>('/credentials')
  return data
}

// 设置凭据禁用状态
export async function setCredentialDisabled(
  id: number,
  disabled: boolean
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/disabled`,
    { disabled } as SetDisabledRequest
  )
  return data
}

// 设置凭据优先级
export async function setCredentialPriority(
  id: number,
  priority: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(
    `/credentials/${id}/priority`,
    { priority } as SetPriorityRequest
  )
  return data
}

// 重置失败计数
export async function resetCredentialFailure(
  id: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/credentials/${id}/reset`)
  return data
}

// 强制刷新 Token
export async function forceRefreshToken(
  id: number
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/credentials/${id}/refresh`)
  return data
}

// 获取凭据余额
export async function getCredentialBalance(id: number): Promise<BalanceResponse> {
  const { data } = await api.get<BalanceResponse>(`/credentials/${id}/balance`)
  return data
}

// 添加新凭据
export async function addCredential(
  req: AddCredentialRequest
): Promise<AddCredentialResponse> {
  const { data } = await api.post<AddCredentialResponse>('/credentials', req)
  return data
}

// 批量导出明文凭据
export async function exportCredentials(ids: number[]): Promise<ExportCredentialsResponse> {
  const { data } = await api.post<ExportCredentialsResponse>('/credentials/export', { ids })
  return data
}

// 获取运行时状态
export async function getRuntimeStatus(): Promise<RuntimeStatusResponse> {
  const { data } = await api.get<RuntimeStatusResponse>('/runtime')
  return data
}

export async function getRuntimeSettings(): Promise<RuntimeSettings> {
  const { data } = await api.get<RuntimeSettings>('/settings/runtime')
  return data
}

export async function setRuntimeSettings(settings: RuntimeSettings): Promise<RuntimeSettings> {
  const { data } = await api.put<RuntimeSettings>('/settings/runtime', settings)
  return data
}

export async function setCredentialPolicy(
  id: number,
  policy: SetCredentialPolicyRequest
): Promise<SuccessResponse> {
  const { data } = await api.patch<SuccessResponse>(`/credentials/${id}/policy`, policy)
  return data
}

export async function setCredentialPolicyBatch(
  request: BatchCredentialPolicyRequest
): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>('/credentials/policy/batch', request)
  return data
}

export async function clearCredentialCooldown(id: number): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>(`/credentials/${id}/cooldown/clear`)
  return data
}

export async function clearCredentialCooldownBatch(ids: number[]): Promise<SuccessResponse> {
  const { data } = await api.post<SuccessResponse>('/credentials/cooldown/clear-batch', { ids })
  return data
}

export async function bindDynamicProxy(id: number): Promise<DynamicProxyActionResponse> {
  const { data } = await api.post<DynamicProxyActionResponse>(`/credentials/${id}/dynamic-proxy/bind`)
  return data
}

export async function rotateDynamicProxy(id: number): Promise<DynamicProxyActionResponse> {
  const { data } = await api.post<DynamicProxyActionResponse>(`/credentials/${id}/dynamic-proxy/rotate`)
  return data
}

export async function verifyDynamicProxy(id: number): Promise<DynamicProxyActionResponse> {
  const { data } = await api.post<DynamicProxyActionResponse>(`/credentials/${id}/dynamic-proxy/verify`)
  return data
}

export async function clearDynamicProxy(id: number): Promise<SuccessResponse> {
  const { data } = await api.delete<SuccessResponse>(`/credentials/${id}/dynamic-proxy`)
  return data
}

export async function dynamicProxyBatchAction(
  action: 'bind' | 'rotate' | 'verify' | 'clear',
  ids: number[],
): Promise<DynamicProxyBatchActionResponse> {
  const { data } = await api.post<DynamicProxyBatchActionResponse>(`/dynamic-proxy/batch/${action}`, { ids })
  return data
}

// 删除凭据
export async function deleteCredential(id: number): Promise<SuccessResponse> {
  const { data } = await api.delete<SuccessResponse>(`/credentials/${id}`)
  return data
}

// 获取负载均衡模式
export async function getLoadBalancingMode(): Promise<{ mode: 'priority' | 'balanced' }> {
  const { data } = await api.get<{ mode: 'priority' | 'balanced' }>('/config/load-balancing')
  return data
}

// 设置负载均衡模式
export async function setLoadBalancingMode(mode: 'priority' | 'balanced'): Promise<{ mode: 'priority' | 'balanced' }> {
  const { data } = await api.put<{ mode: 'priority' | 'balanced' }>('/config/load-balancing', { mode })
  return data
}

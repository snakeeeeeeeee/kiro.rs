import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import {
  getCredentials,
  setCredentialDisabled,
  setCredentialPriority,
  resetCredentialFailure,
  forceRefreshToken,
  getCredentialBalance,
  testCredentialConnection,
  addCredential,
  deleteCredential,
  getLoadBalancingMode,
  setLoadBalancingMode,
  getRuntimeSettings,
  setRuntimeSettings,
  getEndpoints,
  testEndpointLatency,
  setCredentialPolicy,
  setCredentialPolicyBatch,
  clearCredentialCooldown,
  clearCredentialCooldownBatch,
  bindDynamicProxy,
  rotateDynamicProxy,
  verifyDynamicProxy,
  clearDynamicProxy,
  dynamicProxyBatchAction,
} from '@/api/credentials'
import type { AddCredentialRequest, BatchCredentialPolicyRequest, CredentialTestRequest, RuntimeSettings, SetCredentialPolicyRequest } from '@/types/api'

// 查询凭据列表
export function useCredentials() {
  return useQuery({
    queryKey: ['credentials'],
    queryFn: getCredentials,
    refetchInterval: 30000, // 每 30 秒刷新一次
  })
}

// 查询凭据余额
export function useCredentialBalance(id: number | null) {
  return useQuery({
    queryKey: ['credential-balance', id],
    queryFn: () => getCredentialBalance(id!),
    enabled: id !== null,
    retry: false, // 余额查询失败时不重试（避免重复请求被封禁的账号）
  })
}

export function useTestCredentialConnection() {
  return useMutation({
    mutationFn: ({ id, request }: { id: number; request: CredentialTestRequest }) =>
      testCredentialConnection(id, request),
  })
}

// 设置禁用状态
export function useSetDisabled() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, disabled }: { id: number; disabled: boolean }) =>
      setCredentialDisabled(id, disabled),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 设置优先级
export function useSetPriority() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, priority }: { id: number; priority: number }) =>
      setCredentialPriority(id, priority),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 重置失败计数
export function useResetFailure() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => resetCredentialFailure(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 强制刷新 Token
export function useForceRefreshToken() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => forceRefreshToken(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 添加新凭据
export function useAddCredential() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (req: AddCredentialRequest) => addCredential(req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 删除凭据
export function useDeleteCredential() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => deleteCredential(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

// 获取负载均衡模式
export function useLoadBalancingMode() {
  return useQuery({
    queryKey: ['loadBalancingMode'],
    queryFn: getLoadBalancingMode,
  })
}

// 设置负载均衡模式
export function useSetLoadBalancingMode() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: setLoadBalancingMode,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['loadBalancingMode'] })
      queryClient.invalidateQueries({ queryKey: ['runtimeSettings'] })
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useRuntimeSettings() {
  return useQuery({
    queryKey: ['runtimeSettings'],
    queryFn: getRuntimeSettings,
  })
}

export function useEndpoints() {
  return useQuery({
    queryKey: ['endpoints'],
    queryFn: getEndpoints,
  })
}

export function useSetRuntimeSettings() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (settings: RuntimeSettings) => setRuntimeSettings(settings),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['runtimeSettings'] })
      queryClient.invalidateQueries({ queryKey: ['endpoints'] })
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
      queryClient.invalidateQueries({ queryKey: ['loadBalancingMode'] })
    },
  })
}

export function useTestEndpointLatency() {
  return useMutation({
    mutationFn: (endpoint: string) => testEndpointLatency(endpoint),
  })
}

export function useSetCredentialPolicy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, policy }: { id: number; policy: SetCredentialPolicyRequest }) =>
      setCredentialPolicy(id, policy),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
      queryClient.invalidateQueries({ queryKey: ['runtimeSettings'] })
    },
  })
}

export function useSetCredentialPolicyBatch() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (request: BatchCredentialPolicyRequest) => setCredentialPolicyBatch(request),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
      queryClient.invalidateQueries({ queryKey: ['runtimeSettings'] })
    },
  })
}

export function useClearCooldown() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => clearCredentialCooldown(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useClearCooldownBatch() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (ids: number[]) => clearCredentialCooldownBatch(ids),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useBindDynamicProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => bindDynamicProxy(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useRotateDynamicProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => rotateDynamicProxy(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useVerifyDynamicProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => verifyDynamicProxy(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useClearDynamicProxy() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: number) => clearDynamicProxy(id),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

export function useDynamicProxyBatchAction() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ action, ids }: { action: 'bind' | 'rotate' | 'verify' | 'clear'; ids: number[] }) =>
      dynamicProxyBatchAction(action, ids),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    },
  })
}

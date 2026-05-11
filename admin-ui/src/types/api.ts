// 凭据状态响应
export interface CredentialsStatusResponse {
  total: number
  available: number
  currentId: number
  credentials: CredentialStatusItem[]
}

// 单个凭据状态
export interface CredentialStatusItem {
  id: number
  priority: number
  disabled: boolean
  failureCount: number
  isCurrent: boolean
  expiresAt: string | null
  authMethod: string | null
  hasProfileArn: boolean
  email?: string
  refreshTokenHash?: string
  apiKeyHash?: string
  maskedApiKey?: string
  successCount: number
  lastUsedAt: string | null
  hasProxy: boolean
  proxyUrl?: string
  refreshFailureCount: number
  disabledReason?: string
  endpoint: string
  inFlight: number
  maxConcurrent: number
  maxConcurrentOverride?: number | null
  rpmOverride?: number | null
  effectiveRpm: number
  usesDefaultPolicy: boolean
  cooldownUntil: string | null
  isCoolingDown: boolean
  availableForDispatch: boolean
  sessionAffinityBindings: number
}

export interface RuntimeStatusResponse {
  globalInFlight: number
  globalMaxConcurrent: number
  perAccountDefaultMaxConcurrent: number
  globalRpm: number
  perAccountDefaultRpm: number
  queueDepth: number
  queueMaxSize: number
  queueTimeoutMs: number
  rateLimitCooldownMs: number
  transientCooldownMs: number
  loadBalancingMode: 'priority' | 'balanced'
  totalCredentials: number
  availableCredentials: number
  dispatchAvailableCredentials: number
  coolingDownCredentials: number
  sessionAffinityBindings: number
  requestMetrics: RuntimeMetricsSnapshot
  credentials: RuntimeCredentialStatus[]
}

export interface RuntimeMetricsSnapshot {
  windowSecs: number
  requestCount: number
  successCount: number
  errorCount: number
  streamCount: number
  retryCount: number
  avgQueueMs: number
  p95QueueMs: number
  avgAcquireMs: number
  p95AcquireMs: number
  avgUpstreamMs: number
  p50UpstreamMs: number
  p95UpstreamMs: number
  avgTotalMs: number
  p95TotalMs: number
  slowModels: ModelLatencySnapshot[]
  statusCounts: StatusCountSnapshot[]
  credentialCounts: CredentialCountSnapshot[]
}

export interface ModelLatencySnapshot {
  model: string
  requestCount: number
  avgUpstreamMs: number
  p95UpstreamMs: number
  avgTotalMs: number
  p95TotalMs: number
}

export interface StatusCountSnapshot {
  status: string
  count: number
}

export interface CredentialCountSnapshot {
  credentialId: number
  count: number
}

export interface RuntimeCredentialStatus {
  id: number
  inFlight: number
  maxConcurrent: number
  maxConcurrentOverride?: number | null
  rpmOverride?: number | null
  effectiveRpm: number
  usesDefaultPolicy: boolean
  cooldownUntil: string | null
  isCoolingDown: boolean
  availableForDispatch: boolean
  sessionAffinityBindings: number
}

// 余额响应
export interface BalanceResponse {
  id: number
  subscriptionTitle: string | null
  currentUsage: number
  usageLimit: number
  remaining: number
  usagePercentage: number
  nextResetAt: number | null
}

// 成功响应
export interface SuccessResponse {
  success: boolean
  message: string
}

// 错误响应
export interface AdminErrorResponse {
  error: {
    type: string
    message: string
  }
}

// 请求类型
export interface SetDisabledRequest {
  disabled: boolean
}

export interface SetPriorityRequest {
  priority: number
}

export interface RuntimeSettings {
  globalMaxConcurrent: number
  perAccountDefaultMaxConcurrent: number
  queueMaxSize: number
  queueTimeoutMs: number
  perAccountDefaultRpm: number
  globalRpm: number
  rateLimitCooldownMs: number
  transientCooldownMs: number
  loadBalancingMode: 'priority' | 'balanced'
  virtualCacheUsageEnabled: boolean
  virtualCacheDefaultTtl: '5m' | '1h'
  virtualCacheUncachedInputTokens: number
  virtualCacheWarmupTokens: number
  virtualCacheMinCreationTokens: number
  virtualCacheMaxCreationTokens: number
  virtualCacheFallbackScope: 'model' | 'none'
}

export interface SetCredentialPolicyRequest {
  maxConcurrentOverride?: number | null
  rpmOverride?: number | null
}

export interface BatchCredentialPolicyRequest extends SetCredentialPolicyRequest {
  ids: number[]
}

export interface BatchCredentialIdsRequest {
  ids: number[]
}

// 添加凭据请求
export interface AddCredentialRequest {
  refreshToken?: string
  authMethod?: 'social' | 'idc' | 'api_key'
  clientId?: string
  clientSecret?: string
  priority?: number
  authRegion?: string
  apiRegion?: string
  machineId?: string
  email?: string
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
  kiroApiKey?: string
  endpoint?: string
}

// 添加凭据响应
export interface AddCredentialResponse {
  success: boolean
  message: string
  credentialId: number
  email?: string
}

// 导出的明文凭据
export interface ExportedCredential {
  id?: number
  accessToken?: string
  refreshToken?: string
  profileArn?: string
  expiresAt?: string
  authMethod?: string
  clientId?: string
  clientSecret?: string
  priority?: number
  region?: string
  authRegion?: string
  apiRegion?: string
  machineId?: string
  email?: string
  subscriptionTitle?: string
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
  disabled?: boolean
  kiroApiKey?: string
  endpoint?: string
}

export interface ExportCredentialsResponse {
  count: number
  credentials: ExportedCredential[]
}

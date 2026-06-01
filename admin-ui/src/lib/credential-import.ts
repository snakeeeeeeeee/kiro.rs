export interface NormalizedCredentialInput {
  email?: string
  label?: string
  nickname?: string
  status?: string
  refreshToken?: string
  accessToken?: string
  expiresAt?: string
  profileArn?: string
  clientId?: string
  clientSecret?: string
  region?: string
  authRegion?: string
  apiRegion?: string
  priority?: number
  machineId?: string
  kiroApiKey?: string
  authMethod?: string
  endpoint?: string
  proxyUrl?: string
  proxyUsername?: string
  proxyPassword?: string
  subscriptionTitle?: string
  allowOverage?: boolean
  overageWeight?: number
  usageCurrent?: number
  usageLimit?: number
  overageStopped?: boolean
}

function objectValue(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null ? value as Record<string, unknown> : null
}

function stringValue(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined
}

function numberValue(value: unknown): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : undefined
  }
  return undefined
}

function booleanValue(value: unknown): boolean | undefined {
  if (typeof value === 'boolean') return value
  if (typeof value === 'string') {
    if (value.trim().toLowerCase() === 'true') return true
    if (value.trim().toLowerCase() === 'false') return false
  }
  return undefined
}

function dateStringValue(value: unknown): string | undefined {
  const text = stringValue(value)
  if (text) return text

  const timestamp = numberValue(value)
  if (timestamp === undefined || timestamp <= 0) return undefined

  const millis = timestamp > 1_000_000_000_000 ? timestamp : timestamp * 1000
  const date = new Date(millis)
  return Number.isNaN(date.getTime()) ? undefined : date.toISOString()
}

function firstDateString(...values: unknown[]) {
  for (const value of values) {
    const result = dateStringValue(value)
    if (result) return result
  }
  return undefined
}

function firstString(...values: unknown[]) {
  for (const value of values) {
    const result = stringValue(value)
    if (result) return result
  }
  return undefined
}

function firstNumber(...values: unknown[]) {
  for (const value of values) {
    const result = numberValue(value)
    if (result !== undefined) return result
  }
  return undefined
}

function firstBoolean(...values: unknown[]) {
  for (const value of values) {
    const result = booleanValue(value)
    if (result !== undefined) return result
  }
  return undefined
}

function parseJsonSequence(raw: string): unknown[] {
  const items: unknown[] = []
  let start = -1
  let depth = 0
  let inString = false
  let escaped = false

  for (let index = 0; index < raw.length; index++) {
    const char = raw[index]

    if (start === -1) {
      if (/\s/.test(char)) continue
      if (char !== '{' && char !== '[') {
        throw new Error(`第 ${index + 1} 个字符不是 JSON 对象或数组的开始`)
      }
      start = index
    }

    if (inString) {
      if (escaped) {
        escaped = false
      } else if (char === '\\') {
        escaped = true
      } else if (char === '"') {
        inString = false
      }
      continue
    }

    if (char === '"') {
      inString = true
    } else if (char === '{' || char === '[') {
      depth++
    } else if (char === '}' || char === ']') {
      depth--
      if (depth < 0) {
        throw new Error(`第 ${index + 1} 个字符附近 JSON 括号不匹配`)
      }
      if (depth === 0 && start !== -1) {
        items.push(JSON.parse(raw.slice(start, index + 1)))
        start = -1
      }
    }
  }

  if (inString || depth !== 0 || start !== -1) {
    throw new Error('JSON 对象未闭合')
  }

  return items
}

function parseJsonOrJsonLines(raw: string): unknown {
  const trimmed = raw.trim()
  if (!trimmed) return []

  try {
    return JSON.parse(trimmed)
  } catch (wholeError) {
    try {
      return raw
        .split(/\r?\n/)
        .map(line => line.trim())
        .filter(Boolean)
        .map(line => JSON.parse(line))
    } catch {
      try {
        return parseJsonSequence(raw)
      } catch (sequenceError) {
        throw new Error(
          `无法解析为完整 JSON、每行一个 JSON 对象或连续 JSON 对象：${sequenceError instanceof Error ? sequenceError.message : String(wholeError)}`
        )
      }
    }
  }
}

function extractImportItems(parsed: unknown): unknown[] {
  if (Array.isArray(parsed)) return parsed.flatMap(extractImportItems)

  const obj = objectValue(parsed)
  if (!obj) return []

  if (Array.isArray(obj.accounts)) return obj.accounts
  if (Array.isArray(obj.credentials)) return obj.credentials
  return [obj]
}

export function normalizeCredentialInput(value: unknown): NormalizedCredentialInput {
  const obj = objectValue(value)
  if (!obj) return {}

  const nested = objectValue(obj.credentials) || {}
  const usageData = objectValue(obj.usageData) || objectValue(obj.usage_data) || {}
  const subscription = objectValue(obj.subscription) || {}
  const usage = objectValue(obj.usage) || {}

  return {
    email: firstString(obj.email, nested.email),
    label: firstString(obj.label, nested.label),
    nickname: firstString(obj.nickname, nested.nickname),
    status: firstString(obj.status, nested.status) || (booleanValue(obj.enabled) === false ? 'disabled' : undefined),
    refreshToken: firstString(obj.refreshToken, obj.refresh_token, nested.refreshToken, nested.refresh_token),
    accessToken: firstString(obj.accessToken, obj.access_token, nested.accessToken, nested.access_token),
    expiresAt: firstDateString(obj.expiresAt, obj.expires_at, nested.expiresAt, nested.expires_at),
    profileArn: firstString(obj.profileArn, obj.profile_arn, nested.profileArn, nested.profile_arn),
    clientId: firstString(obj.clientId, obj.client_id, nested.clientId, nested.client_id),
    clientSecret: firstString(obj.clientSecret, obj.client_secret, nested.clientSecret, nested.client_secret),
    region: firstString(obj.region, nested.region),
    authRegion: firstString(obj.authRegion, obj.auth_region, nested.authRegion, nested.auth_region),
    apiRegion: firstString(obj.apiRegion, obj.api_region, nested.apiRegion, nested.api_region),
    priority: firstNumber(obj.priority, nested.priority),
    machineId: firstString(obj.machineId, obj.machine_id, nested.machineId, nested.machine_id),
    kiroApiKey: firstString(obj.kiroApiKey, obj.kiro_api_key, nested.kiroApiKey, nested.kiro_api_key),
    authMethod: firstString(obj.authMethod, obj.auth_method, nested.authMethod, nested.auth_method),
    endpoint: firstString(obj.endpoint, nested.endpoint),
    proxyUrl: firstString(obj.proxyUrl, obj.proxyURL, obj.proxy_url, nested.proxyUrl, nested.proxyURL, nested.proxy_url),
    proxyUsername: firstString(obj.proxyUsername, obj.proxy_username, nested.proxyUsername, nested.proxy_username),
    proxyPassword: firstString(obj.proxyPassword, obj.proxy_password, nested.proxyPassword, nested.proxy_password),
    subscriptionTitle: firstString(
      obj.subscriptionTitle,
      obj.subscription_title,
      subscription.title,
      subscription.subscriptionTitle,
      usageData.subscriptionTitle,
      usageData.subscription_title,
      nested.subscriptionTitle,
      nested.subscription_title,
    ),
    allowOverage: firstBoolean(
      obj.allowOverage,
      obj.allow_overage,
      obj.allowOverUsage,
      obj.allow_over_usage,
      nested.allowOverage,
      nested.allow_overage,
      nested.allowOverUsage,
      nested.allow_over_usage,
    ),
    overageWeight: firstNumber(obj.overageWeight, obj.overage_weight, nested.overageWeight, nested.overage_weight),
    usageCurrent: firstNumber(
      obj.usageCurrent,
      obj.usage_current,
      usage.current,
      usage.currentUsage,
      usage.usageCurrent,
      usageData.currentUsage,
      usageData.usageCurrent,
    ),
    usageLimit: firstNumber(obj.usageLimit, obj.usage_limit, usage.limit, usage.usageLimit, usageData.usageLimit),
    overageStopped: firstBoolean(obj.overageStopped, obj.overage_stopped, nested.overageStopped, nested.overage_stopped),
  }
}

export function parseCredentialImportInput(raw: string): NormalizedCredentialInput[] {
  const parsed = parseJsonOrJsonLines(raw)
  return extractImportItems(parsed)
    .map(normalizeCredentialInput)
    .filter(credential => Boolean(credential.refreshToken || credential.kiroApiKey || credential.accessToken))
}

export function formatCredentialUsageSnapshot(credential: NormalizedCredentialInput): string | undefined {
  const current = credential.usageCurrent
  const limit = credential.usageLimit

  if (typeof current === 'number' && Number.isFinite(current) && typeof limit === 'number' && Number.isFinite(limit) && limit > 0) {
    return `${current}/${limit}`
  }

  if (typeof limit === 'number' && Number.isFinite(limit) && limit > 0) {
    return `0/${limit}`
  }

  if (typeof current === 'number' && Number.isFinite(current) && current > 0) {
    return `${current}/未知`
  }

  return undefined
}

export function hasCredentialImportSnapshot(credential: NormalizedCredentialInput): boolean {
  return Boolean(formatCredentialUsageSnapshot(credential) || credential.subscriptionTitle?.trim())
}

export function credentialDisplayName(credential: NormalizedCredentialInput, fallback: string) {
  return credential.email || credential.label || credential.nickname || fallback
}

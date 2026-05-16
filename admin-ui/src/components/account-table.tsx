import { Edit3, Globe2, RefreshCw, RotateCw, Snowflake, Trash2 } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Progress } from '@/components/ui/progress'
import type { BalanceResponse, CredentialStatusItem } from '@/types/api'

export type AccountColumnKey =
  | 'auth'
  | 'subscription'
  | 'status'
  | 'dispatch'
  | 'concurrency'
  | 'rpm'
  | 'priority'
  | 'cooldown'
  | 'failures'
  | 'lastUsed'
  | 'endpoint'
  | 'dynamicProxy'
  | 'actions'

export type AccountSortKey =
  | 'priority'
  | 'email'
  | 'status'
  | 'inFlight'
  | 'lastUsedAt'
  | 'failureCount'
  | 'endpoint'

export interface AccountColumn {
  key: AccountColumnKey
  label: string
  sortKey?: AccountSortKey
}

interface AccountTableProps {
  credentials: CredentialStatusItem[]
  selectedIds: Set<number>
  columns: AccountColumn[]
  sortKey: AccountSortKey
  sortOrder: 'asc' | 'desc'
  balanceMap: Map<number, BalanceResponse>
  loadingBalanceIds: Set<number>
  onSort: (key: AccountSortKey) => void
  onToggleSelect: (id: number) => void
  onToggleSelectAll: () => void
  onViewBalance: (id: number) => void
  onRefreshBalance: (id: number) => void
  onEditPolicy: (credential: CredentialStatusItem) => void
  onToggleDisabled: (credential: CredentialStatusItem) => void
  onClearCooldown: (credential: CredentialStatusItem) => void
  onForceRefresh: (id: number) => void
  onDelete: (id: number) => void
  onBindDynamicProxy: (id: number) => void
  onRotateDynamicProxy: (id: number) => void
  onVerifyDynamicProxy: (id: number) => void
  onClearDynamicProxy: (id: number) => void
}

function credentialName(credential: CredentialStatusItem): string {
  return credential.email || credential.maskedApiKey || `凭据 #${credential.id}`
}

function credentialSecondaryName(credential: CredentialStatusItem): string {
  if (credential.email && credential.maskedApiKey) return credential.maskedApiKey
  return `#${credential.id}`
}

function formatRelativeTime(value: string | null): string {
  if (!value) return '-'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  const diffMs = Date.now() - date.getTime()
  const abs = Math.abs(diffMs)
  if (abs < 60_000) return diffMs >= 0 ? '刚刚' : '即将'
  const minutes = Math.round(abs / 60_000)
  if (minutes < 60) return diffMs >= 0 ? `${minutes}分钟前` : `${minutes}分钟后`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return diffMs >= 0 ? `${hours}小时前` : `${hours}小时后`
  const days = Math.round(hours / 24)
  return diffMs >= 0 ? `${days}天前` : `${days}天后`
}

function statusBadge(credential: CredentialStatusItem) {
  if (credential.disabled) {
    return <Badge variant="outline">禁用</Badge>
  }
  if (credential.isCoolingDown) {
    return <Badge variant="warning">冷却中</Badge>
  }
  return <Badge variant="success">正常</Badge>
}

function dispatchBadge(credential: CredentialStatusItem) {
  if (credential.availableForDispatch) {
    return <Badge variant="secondary">可调度</Badge>
  }
  if (credential.inFlight >= credential.maxConcurrent) {
    return <Badge variant="warning">满载</Badge>
  }
  if (credential.isCoolingDown) {
    return <Badge variant="warning">冷却</Badge>
  }
  return <Badge variant="outline">不可调度</Badge>
}

function authLabel(value: string | null | undefined) {
  if (value === 'api_key') return 'API Key'
  if (value === 'idc') return 'Builder ID'
  return 'Social'
}

function dynamicProxyBadge(credential: CredentialStatusItem) {
  const binding = credential.dynamicProxy
  if (!binding) return <Badge variant="outline">未绑定</Badge>
  if (binding.status === 'active') return <Badge variant="success">已绑定</Badge>
  if (binding.status === 'failed' || binding.status === 'expired') return <Badge variant="destructive">{binding.status}</Badge>
  return <Badge variant="warning">{binding.status}</Badge>
}

function formatUsageValue(value: number) {
  return value.toLocaleString('zh-CN', {
    minimumFractionDigits: 0,
    maximumFractionDigits: 1,
  })
}

function quotaView(credential: CredentialStatusItem, balance?: BalanceResponse) {
  const current = balance?.currentUsage ?? credential.usageCurrent
  const limit = balance?.usageLimit ?? credential.usageLimit
  const percentage = limit > 0
    ? Math.min(Math.max(balance?.usagePercentage ?? credential.usagePercentage, 0), 100)
    : 0
  const subscriptionTitle = balance?.subscriptionTitle || credential.subscriptionTitle || '-'

  return {
    current,
    limit,
    percentage,
    subscriptionTitle,
    hasQuota: limit > 0,
  }
}

export function AccountTable({
  credentials,
  selectedIds,
  columns,
  sortKey,
  sortOrder,
  balanceMap,
  loadingBalanceIds,
  onSort,
  onToggleSelect,
  onToggleSelectAll,
  onViewBalance,
  onRefreshBalance,
  onEditPolicy,
  onToggleDisabled,
  onClearCooldown,
  onForceRefresh,
  onDelete,
  onBindDynamicProxy,
  onRotateDynamicProxy,
  onVerifyDynamicProxy,
  onClearDynamicProxy,
}: AccountTableProps) {
  const allSelected = credentials.length > 0 && credentials.every(c => selectedIds.has(c.id))

  return (
    <div className="overflow-hidden rounded-lg border bg-card">
      <div className="max-h-[70vh] overflow-auto">
        <table className="min-w-[1280px] w-full border-collapse text-sm">
          <thead className="sticky top-0 z-20 bg-muted/90 backdrop-blur">
            <tr className="border-b">
              <th className="sticky left-0 z-30 w-12 bg-muted/90 px-4 py-3 text-left">
                <input
                  type="checkbox"
                  className="h-4 w-4 rounded border-input"
                  checked={allSelected}
                  onChange={onToggleSelectAll}
                  aria-label="选择当前页"
                />
              </th>
              <th
                className="sticky left-12 z-30 min-w-[260px] cursor-pointer bg-muted/90 px-4 py-3 text-left font-medium text-muted-foreground"
                onClick={() => onSort('email')}
              >
                账号 {sortKey === 'email' ? (sortOrder === 'asc' ? '↑' : '↓') : ''}
              </th>
              {columns.map(column => (
                <th
                  key={column.key}
                  className="whitespace-nowrap px-4 py-3 text-left font-medium text-muted-foreground"
                  onClick={() => column.sortKey && onSort(column.sortKey)}
                >
                  <span className={column.sortKey ? 'cursor-pointer' : undefined}>
                    {column.label} {column.sortKey === sortKey ? (sortOrder === 'asc' ? '↑' : '↓') : ''}
                  </span>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {credentials.length === 0 ? (
              <tr>
                <td colSpan={columns.length + 2} className="px-4 py-12 text-center text-muted-foreground">
                  没有匹配的账号
                </td>
              </tr>
            ) : (
              credentials.map(credential => {
                const balance = balanceMap.get(credential.id)
                const quota = quotaView(credential, balance)
                const isBalanceLoading = loadingBalanceIds.has(credential.id)
                return (
                  <tr key={credential.id} className="border-b hover:bg-muted/40">
                    <td className="sticky left-0 z-10 bg-card px-4 py-4">
                      <input
                        type="checkbox"
                        className="h-4 w-4 rounded border-input"
                        checked={selectedIds.has(credential.id)}
                        onChange={() => onToggleSelect(credential.id)}
                        aria-label={`选择 ${credentialName(credential)}`}
                      />
                    </td>
                    <td className="sticky left-12 z-10 min-w-[260px] bg-card px-4 py-4">
                      <div className="space-y-1">
                        <div className="font-medium text-foreground">{credentialName(credential)}</div>
                        <div className="text-xs text-muted-foreground">{credentialSecondaryName(credential)}</div>
                      </div>
                    </td>
                    {columns.map(column => (
                      <td key={column.key} className="whitespace-nowrap px-4 py-4 align-middle">
                        {column.key === 'auth' && (
                          <div className="flex flex-wrap gap-1">
                            <Badge variant="secondary">{authLabel(credential.authMethod)}</Badge>
                            {credential.hasProfileArn && <Badge variant="outline">Profile</Badge>}
                          </div>
                        )}
                        {column.key === 'subscription' && (
                          <div className="w-[220px] space-y-2">
                            <div className="flex items-center justify-between gap-2">
                              <span className="max-w-[150px] truncate font-medium" title={quota.subscriptionTitle}>
                                {quota.subscriptionTitle}
                              </span>
                              {isBalanceLoading && <span className="text-xs text-muted-foreground">查询中</span>}
                            </div>
                            {quota.hasQuota ? (
                              <>
                                <Progress value={quota.percentage} className="h-2 rounded" />
                                <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
                                  <span>
                                    {formatUsageValue(quota.current)} / {formatUsageValue(quota.limit)}
                                  </span>
                                  <span>{quota.percentage.toFixed(1)}%</span>
                                </div>
                              </>
                            ) : (
                              <div className="text-xs text-muted-foreground">
                                {isBalanceLoading ? '正在获取额度...' : '额度未查询'}
                              </div>
                            )}
                            <div className="flex flex-wrap gap-1">
                              {credential.isOverUsageLimit && (
                                <Badge variant={credential.overageStopped ? 'destructive' : 'warning'}>
                                  {credential.overageStopped ? '透支停止' : '已满'}
                                </Badge>
                              )}
                              {credential.allowOverage && !credential.overageStopped && (
                                <Badge variant="secondary">透支 x{credential.overageWeight}</Badge>
                              )}
                              <Button
                                size="sm"
                                variant="ghost"
                                className="h-6 px-2"
                                onClick={() => onRefreshBalance(credential.id)}
                                disabled={isBalanceLoading}
                                title="刷新额度"
                              >
                                <RefreshCw className={`h-3.5 w-3.5 ${isBalanceLoading ? 'animate-spin' : ''}`} />
                              </Button>
                              <Button size="sm" variant="ghost" className="h-6 px-2" onClick={() => onViewBalance(credential.id)}>
                                详情
                              </Button>
                            </div>
                          </div>
                        )}
                        {column.key === 'status' && (
                          <div className="flex flex-col gap-1">
                            {statusBadge(credential)}
                            {credential.disabledReason && (
                              <span className="text-xs text-muted-foreground">{credential.disabledReason}</span>
                            )}
                          </div>
                        )}
                        {column.key === 'dispatch' && dispatchBadge(credential)}
                        {column.key === 'concurrency' && (
                          <span className={credential.inFlight >= credential.maxConcurrent ? 'font-semibold text-yellow-600' : 'font-medium'}>
                            {credential.inFlight} / {credential.maxConcurrent}
                            {credential.maxConcurrentOverride != null && <span className="ml-1 text-xs text-muted-foreground">覆盖</span>}
                          </span>
                        )}
                        {column.key === 'rpm' && (
                          <span className="font-medium">
                            {credential.effectiveRpm || '不限'}
                            {credential.rpmOverride != null && <span className="ml-1 text-xs text-muted-foreground">覆盖</span>}
                          </span>
                        )}
                        {column.key === 'priority' && <span className="tabular-nums">{credential.priority}</span>}
                        {column.key === 'cooldown' && (
                          <span className="text-muted-foreground">
                            {credential.cooldownUntil ? formatRelativeTime(credential.cooldownUntil) : '-'}
                          </span>
                        )}
                        {column.key === 'failures' && (
                          <span className={credential.failureCount + credential.refreshFailureCount > 0 ? 'font-semibold text-red-600' : 'text-muted-foreground'}>
                            {credential.failureCount} / {credential.refreshFailureCount}
                          </span>
                        )}
                        {column.key === 'lastUsed' && (
                          <span className="text-muted-foreground">{formatRelativeTime(credential.lastUsedAt)}</span>
                        )}
                        {column.key === 'endpoint' && (
                          <div className="space-y-1">
                            <Badge variant="outline">{credential.endpoint}</Badge>
                            <div className="text-xs text-muted-foreground">{credential.hasProxy ? '代理' : '直连'}</div>
                          </div>
                        )}
                        {column.key === 'dynamicProxy' && (
                          <div className="space-y-1">
                            {dynamicProxyBadge(credential)}
                            {credential.dynamicProxy?.egressIp && (
                              <div className="font-mono text-xs">{credential.dynamicProxy.egressIp}</div>
                            )}
                            {credential.dynamicProxy?.expiresAt && (
                              <div className="text-xs text-muted-foreground">
                                {formatRelativeTime(credential.dynamicProxy.expiresAt)}
                              </div>
                            )}
                            {credential.dynamicProxy?.verifyError && (
                              <div className="max-w-[220px] truncate text-xs text-destructive" title={credential.dynamicProxy.verifyError}>
                                {credential.dynamicProxy.verifyError}
                              </div>
                            )}
                          </div>
                        )}
                        {column.key === 'actions' && (
                          <div className="flex items-center gap-1">
                            <Button size="sm" variant="ghost" onClick={() => onEditPolicy(credential)} title="策略">
                              <Edit3 className="h-4 w-4" />
                            </Button>
                            <Button size="sm" variant="ghost" onClick={() => onToggleDisabled(credential)}>
                              {credential.disabled ? '启用' : '禁用'}
                            </Button>
                            <Button size="sm" variant="ghost" onClick={() => onForceRefresh(credential.id)} title="刷新 Token">
                              <RefreshCw className="h-4 w-4" />
                            </Button>
                            <Button size="sm" variant="ghost" onClick={() => onBindDynamicProxy(credential.id)} title="绑定动态代理">
                              <Globe2 className="h-4 w-4" />
                            </Button>
                            <Button size="sm" variant="ghost" onClick={() => onRotateDynamicProxy(credential.id)} title="换绑动态代理">
                              <RotateCw className="h-4 w-4" />
                            </Button>
                            <Button size="sm" variant="ghost" onClick={() => onVerifyDynamicProxy(credential.id)} title="验证动态代理">
                              验 IP
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              onClick={() => onClearDynamicProxy(credential.id)}
                              disabled={!credential.dynamicProxy}
                              title="清除动态代理"
                            >
                              清 IP
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              onClick={() => onClearCooldown(credential)}
                              disabled={!credential.isCoolingDown}
                              title="清除冷却"
                            >
                              <Snowflake className="h-4 w-4" />
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              className="text-destructive hover:text-destructive"
                              onClick={() => onDelete(credential.id)}
                              disabled={!credential.disabled}
                              title={credential.disabled ? '删除' : '需先禁用'}
                            >
                              <Trash2 className="h-4 w-4" />
                            </Button>
                          </div>
                        )}
                      </td>
                    ))}
                  </tr>
                )
              })
            )}
          </tbody>
        </table>
      </div>
    </div>
  )
}

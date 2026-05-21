import { useState, useEffect, useRef } from 'react'
import { RefreshCw, LogOut, Moon, Sun, Server, Plus, Upload, FileUp, Download, Trash2, RotateCcw, CheckCircle2, Activity, Settings, Columns3, Search, SlidersHorizontal, ShieldCheck, Globe2 } from 'lucide-react'
import { useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { storage } from '@/lib/storage'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { BalanceDialog } from '@/components/balance-dialog'
import { AddCredentialDialog } from '@/components/add-credential-dialog'
import { BatchImportDialog } from '@/components/batch-import-dialog'
import { KamImportDialog } from '@/components/kam-import-dialog'
import { BatchVerifyDialog, type VerifyResult } from '@/components/batch-verify-dialog'
import { RuntimeSettingsDialog } from '@/components/runtime-settings-dialog'
import { PolicyDialog } from '@/components/policy-dialog'
import { CredentialTestDialog } from '@/components/credential-test-dialog'
import { AccountTable, type AccountColumn, type AccountColumnKey, type AccountSortKey } from '@/components/account-table'
import { useCredentials, useDeleteCredential, useResetFailure, useLoadBalancingMode, useSetLoadBalancingMode, useClearCooldown, useClearCooldownBatch, useSetDisabled, useBindDynamicProxy, useRotateDynamicProxy, useVerifyDynamicProxy, useClearDynamicProxy, useDynamicProxyBatchAction } from '@/hooks/use-credentials'
import { getCredentialBalance, forceRefreshToken, exportCredentials, getRuntimeStatus } from '@/api/credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { BalanceResponse, CredentialStatusItem, ExportedCredential, RuntimeStatusResponse } from '@/types/api'

interface DashboardProps {
  onLogout: () => void
}

function formatExportDate(date: Date): string {
  const pad = (value: number) => String(value).padStart(2, '0')
  return [
    date.getFullYear(),
    pad(date.getMonth() + 1),
    pad(date.getDate()),
  ].join('/') + ' ' + [
    pad(date.getHours()),
    pad(date.getMinutes()),
    pad(date.getSeconds()),
  ].join(':')
}

function createExportId(credential: ExportedCredential): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID()
  }
  return `kiro-rs-${credential.id ?? Date.now()}`
}

function formatMs(value?: number): string {
  if (value === undefined || value === null) return '-'
  if (value >= 1000) return `${(value / 1000).toFixed(2)}s`
  return `${Math.round(value)}ms`
}

function formatRpm(value?: number): string {
  if (value === undefined || value === null) return '-'
  if (value >= 100) return Math.round(value).toString()
  if (value >= 10) return value.toFixed(1)
  return value.toFixed(2)
}

function toKamStyleExport(credential: ExportedCredential) {
  const authMethod = credential.authMethod || (credential.kiroApiKey ? 'api_key' : 'social')
  const isApiKey = authMethod === 'api_key' || Boolean(credential.kiroApiKey)
  const provider = isApiKey ? 'API Key' : authMethod === 'idc' ? 'Builder ID' : 'Google'

  return {
    id: createExportId(credential),
    email: credential.email || '',
    password: null,
    label: credential.email ? `Kiro ${provider} 账号` : `Kiro 凭据 #${credential.id ?? ''}`,
    status: credential.disabled ? 'disabled' : 'active',
    addedAt: formatExportDate(new Date()),
    accessToken: credential.accessToken || null,
    refreshToken: credential.refreshToken || null,
    expiresAt: credential.expiresAt || null,
    provider,
    userId: '',
    authMethod,
    clientId: credential.clientId || null,
    clientSecret: credential.clientSecret || null,
    region: credential.authRegion || credential.region || null,
    authRegion: credential.authRegion || null,
    apiRegion: credential.apiRegion || null,
    clientIdHash: null,
    ssoSessionId: null,
    idToken: null,
    startUrl: null,
    profileArn: credential.profileArn || null,
    usageData: credential.subscriptionTitle ? { subscriptionTitle: credential.subscriptionTitle } : null,
    groupId: null,
    tagLinks: [],
    machineId: credential.machineId || null,
    availableModelsCache: null,
    priority: credential.priority || 0,
    proxyUrl: credential.proxyUrl || null,
    endpoint: credential.endpoint || null,
    kiroApiKey: credential.kiroApiKey || null,
  }
}

type SortKey = AccountSortKey
type SortOrder = 'asc' | 'desc'
const UNKNOWN_SUBSCRIPTION_FILTER = '__unknown_subscription__'

const COLUMN_STORAGE_KEY = 'kiro-admin-table-columns'
const BALANCE_AUTO_REFRESH_STORAGE_KEY = 'kiro-admin-balance-auto-refresh'
const BALANCE_AUTO_REFRESH_INTERVAL_STORAGE_KEY = 'kiro-admin-balance-auto-refresh-interval'
const DEFAULT_VISIBLE_COLUMNS = [
  'auth',
  'subscription',
  'status',
  'dispatch',
  'concurrency',
  'rpm',
  'priority',
  'cooldown',
  'failures',
  'lastUsed',
  'endpoint',
  'dynamicProxy',
  'actions',
] as const

const BALANCE_REFRESH_INTERVAL_OPTIONS = [
  { value: 30_000, label: '30s' },
  { value: 60_000, label: '60s' },
  { value: 120_000, label: '2min' },
  { value: 300_000, label: '5min' },
] as const

type ColumnKey = AccountColumnKey

const columnLabels: Record<ColumnKey, string> = {
  auth: '认证',
  subscription: '额度',
  status: '状态',
  dispatch: '调度',
  concurrency: '并发',
  rpm: 'RPM',
  priority: '优先级',
  cooldown: '冷却',
  failures: '失败',
  lastUsed: '最近使用',
  endpoint: '端点/代理',
  dynamicProxy: '动态 IP',
  actions: '操作',
}

function credentialName(credential: CredentialStatusItem): string {
  return credential.email || credential.maskedApiKey || `凭据 #${credential.id}`
}

function normalizeSubscriptionTitle(title: string | null | undefined): string {
  const normalized = (title || '').trim()
  return normalized && normalized !== '-' ? normalized : UNKNOWN_SUBSCRIPTION_FILTER
}

function subscriptionFilterLabel(value: string): string {
  return value === UNKNOWN_SUBSCRIPTION_FILTER ? '未查询' : value
}

function formatClockTime(value: Date | null): string {
  if (!value) return '尚未刷新'
  return value.toLocaleTimeString('zh-CN', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

export function Dashboard({ onLogout }: DashboardProps) {
  const [selectedCredentialId, setSelectedCredentialId] = useState<number | null>(null)
  const [balanceDialogOpen, setBalanceDialogOpen] = useState(false)
  const [addDialogOpen, setAddDialogOpen] = useState(false)
  const [batchImportDialogOpen, setBatchImportDialogOpen] = useState(false)
  const [kamImportDialogOpen, setKamImportDialogOpen] = useState(false)
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set())
  const [verifyDialogOpen, setVerifyDialogOpen] = useState(false)
  const [verifying, setVerifying] = useState(false)
  const [verifyProgress, setVerifyProgress] = useState({ current: 0, total: 0 })
  const [verifyResults, setVerifyResults] = useState<Map<number, VerifyResult>>(new Map())
  const [balanceMap, setBalanceMap] = useState<Map<number, BalanceResponse>>(new Map())
  const [loadingBalanceIds, setLoadingBalanceIds] = useState<Set<number>>(new Set())
  const balanceMapRef = useRef(balanceMap)
  const loadingBalanceIdsRef = useRef(loadingBalanceIds)
  const balanceFetchFailedIdsRef = useRef<Set<number>>(new Set())
  const [autoBalanceRefreshEnabled, setAutoBalanceRefreshEnabled] = useState(() => {
    if (typeof window === 'undefined') return false
    return window.localStorage.getItem(BALANCE_AUTO_REFRESH_STORAGE_KEY) === 'true'
  })
  const [autoBalanceRefreshIntervalMs, setAutoBalanceRefreshIntervalMs] = useState(() => {
    if (typeof window === 'undefined') return 60_000
    const saved = Number(window.localStorage.getItem(BALANCE_AUTO_REFRESH_INTERVAL_STORAGE_KEY))
    return BALANCE_REFRESH_INTERVAL_OPTIONS.some(option => option.value === saved) ? saved : 60_000
  })
  const [lastBalanceRefreshAt, setLastBalanceRefreshAt] = useState<Date | null>(null)
  const [batchRefreshing, setBatchRefreshing] = useState(false)
  const [batchRefreshProgress, setBatchRefreshProgress] = useState({ current: 0, total: 0 })
  const [exporting, setExporting] = useState(false)
  const [runtimeStatus, setRuntimeStatus] = useState<RuntimeStatusResponse | null>(null)
  const [runtimeSettingsOpen, setRuntimeSettingsOpen] = useState(false)
  const [policyDialogOpen, setPolicyDialogOpen] = useState(false)
  const [policyCredential, setPolicyCredential] = useState<CredentialStatusItem | null>(null)
  const [testDialogOpen, setTestDialogOpen] = useState(false)
  const [testCredential, setTestCredential] = useState<CredentialStatusItem | null>(null)
  const [batchPolicyOpen, setBatchPolicyOpen] = useState(false)
  const [columnMenuOpen, setColumnMenuOpen] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')
  const [authFilter, setAuthFilter] = useState('all')
  const [statusFilter, setStatusFilter] = useState('all')
  const [dispatchFilter, setDispatchFilter] = useState('all')
  const [subscriptionFilter, setSubscriptionFilter] = useState('all')
  const [endpointFilter, setEndpointFilter] = useState('all')
  const [proxyFilter, setProxyFilter] = useState('all')
  const [sortKey, setSortKey] = useState<SortKey>('priority')
  const [sortOrder, setSortOrder] = useState<SortOrder>('asc')
  const [itemsPerPage, setItemsPerPage] = useState(50)
  const [visibleColumns, setVisibleColumns] = useState<Set<ColumnKey>>(() => {
    if (typeof window === 'undefined') return new Set(DEFAULT_VISIBLE_COLUMNS)
    try {
      const saved = window.localStorage.getItem(COLUMN_STORAGE_KEY)
      if (!saved) return new Set(DEFAULT_VISIBLE_COLUMNS)
      const parsed = JSON.parse(saved) as ColumnKey[]
      const valid = parsed.filter(key => key in columnLabels)
      return new Set(valid.length > 0 ? valid : DEFAULT_VISIBLE_COLUMNS)
    } catch {
      return new Set(DEFAULT_VISIBLE_COLUMNS)
    }
  })
  const activeColumns: AccountColumn[] = DEFAULT_VISIBLE_COLUMNS
    .filter(key => visibleColumns.has(key))
    .map(key => ({
      key,
      label: columnLabels[key],
      sortKey:
        key === 'priority' ? 'priority' :
        key === 'status' ? 'status' :
        key === 'concurrency' ? 'inFlight' :
        key === 'failures' ? 'failureCount' :
        key === 'lastUsed' ? 'lastUsedAt' :
        key === 'endpoint' ? 'endpoint' :
        undefined,
    }))
  const cancelVerifyRef = useRef(false)
  const [currentPage, setCurrentPage] = useState(1)
  const [darkMode, setDarkMode] = useState(() => {
    if (typeof window !== 'undefined') {
      return document.documentElement.classList.contains('dark')
    }
    return false
  })

  const queryClient = useQueryClient()
  const { data, isLoading, error, refetch } = useCredentials()
  const { mutate: deleteCredential } = useDeleteCredential()
  const { mutate: resetFailure } = useResetFailure()
  const { mutate: setDisabled } = useSetDisabled()
  const { mutate: clearCooldown } = useClearCooldown()
  const { mutate: clearCooldownBatch } = useClearCooldownBatch()
  const { mutate: bindDynamicProxy } = useBindDynamicProxy()
  const { mutate: rotateDynamicProxy } = useRotateDynamicProxy()
  const { mutate: verifyDynamicProxy } = useVerifyDynamicProxy()
  const { mutate: clearDynamicProxy } = useClearDynamicProxy()
  const { mutate: dynamicProxyBatchAction, isPending: dynamicProxyBatching } = useDynamicProxyBatchAction()
  const { data: loadBalancingData } = useLoadBalancingMode()
  const { mutate: setLoadBalancingMode, isPending: isSettingMode } = useSetLoadBalancingMode()

  const fetchRuntimeStatus = async () => {
    try {
      const status = await getRuntimeStatus()
      setRuntimeStatus(status)
    } catch {
      setRuntimeStatus(null)
    }
  }

  const endpointOptions = Array.from(new Set(data?.credentials.map(c => c.endpoint).filter(Boolean) || [])).sort()
  const subscriptionOptions = Array.from(
    new Set((data?.credentials || []).map(credential => normalizeSubscriptionTitle(credential.subscriptionTitle))),
  ).sort((a, b) => {
    if (a === UNKNOWN_SUBSCRIPTION_FILTER) return 1
    if (b === UNKNOWN_SUBSCRIPTION_FILTER) return -1
    return a.localeCompare(b)
  })

  const filteredCredentials = (data?.credentials || [])
    .filter(credential => {
      const query = searchQuery.trim().toLowerCase()
      if (query) {
        const haystack = [
          credentialName(credential),
          credential.id.toString(),
          credential.refreshTokenHash || '',
          credential.apiKeyHash || '',
          credential.endpoint || '',
        ].join(' ').toLowerCase()
        if (!haystack.includes(query)) return false
      }
      if (authFilter !== 'all' && (credential.authMethod || 'social') !== authFilter) return false
      if (statusFilter === 'active' && credential.disabled) return false
      if (statusFilter === 'disabled' && !credential.disabled) return false
      if (statusFilter === 'cooling' && !credential.isCoolingDown) return false
      if (dispatchFilter === 'available' && !credential.availableForDispatch) return false
      if (dispatchFilter === 'full' && credential.inFlight < credential.maxConcurrent) return false
      if (dispatchFilter === 'blocked' && credential.availableForDispatch) return false
      if (subscriptionFilter !== 'all' && normalizeSubscriptionTitle(credential.subscriptionTitle) !== subscriptionFilter) return false
      if (endpointFilter !== 'all' && credential.endpoint !== endpointFilter) return false
      const hasAnyProxy = credential.hasProxy || Boolean(credential.dynamicProxy)
      if (proxyFilter === 'proxy' && !hasAnyProxy) return false
      if (proxyFilter === 'direct' && hasAnyProxy) return false
      if (proxyFilter === 'dynamic' && !credential.dynamicProxy) return false
      return true
    })
    .sort((a, b) => {
      const direction = sortOrder === 'asc' ? 1 : -1
      const value = (credential: CredentialStatusItem) => {
        switch (sortKey) {
          case 'email':
            return credentialName(credential)
          case 'status':
            return credential.disabled ? 1 : credential.isCoolingDown ? 2 : 0
          case 'inFlight':
            return credential.inFlight
          case 'lastUsedAt':
            return credential.lastUsedAt || ''
          case 'failureCount':
            return credential.failureCount + credential.refreshFailureCount
          case 'endpoint':
            return credential.endpoint
          case 'priority':
          default:
            return credential.priority
        }
      }
      const av = value(a)
      const bv = value(b)
      if (typeof av === 'number' && typeof bv === 'number') return (av - bv) * direction
      return String(av).localeCompare(String(bv)) * direction
    })

  // 计算分页
  const totalPages = Math.max(1, Math.ceil(filteredCredentials.length / itemsPerPage))
  const startIndex = (currentPage - 1) * itemsPerPage
  const endIndex = startIndex + itemsPerPage
  const currentCredentials = filteredCredentials.slice(startIndex, endIndex)
  const currentBalanceIds = currentCredentials.map(credential => credential.id)
  const currentBalanceIdsKey = currentBalanceIds.join('|')
  const currentBalanceFetchKey = currentCredentials
    .map(credential => `${credential.id}:${credential.usageLimit > 0 ? 'known' : 'missing'}`)
    .join('|')
  const currentPageBalanceLoading = currentCredentials.some(credential => loadingBalanceIds.has(credential.id))
  const selectedDisabledCount = Array.from(selectedIds).filter(id => {
    const credential = data?.credentials.find(c => c.id === id)
    return Boolean(credential?.disabled)
  }).length

  // 当凭据列表变化时重置到第一页
  useEffect(() => {
    setCurrentPage(1)
  }, [data?.credentials.length, searchQuery, authFilter, statusFilter, dispatchFilter, subscriptionFilter, endpointFilter, proxyFilter, itemsPerPage])

  useEffect(() => {
    if (typeof window === 'undefined') return
    window.localStorage.setItem(COLUMN_STORAGE_KEY, JSON.stringify(Array.from(visibleColumns)))
  }, [visibleColumns])

  useEffect(() => {
    if (typeof window === 'undefined') return
    window.localStorage.setItem(BALANCE_AUTO_REFRESH_STORAGE_KEY, String(autoBalanceRefreshEnabled))
    window.localStorage.setItem(BALANCE_AUTO_REFRESH_INTERVAL_STORAGE_KEY, String(autoBalanceRefreshIntervalMs))
  }, [autoBalanceRefreshEnabled, autoBalanceRefreshIntervalMs])

  useEffect(() => {
    fetchRuntimeStatus()
    const timer = window.setInterval(fetchRuntimeStatus, 5000)
    return () => window.clearInterval(timer)
  }, [])

  useEffect(() => {
    balanceMapRef.current = balanceMap
  }, [balanceMap])

  useEffect(() => {
    loadingBalanceIdsRef.current = loadingBalanceIds
  }, [loadingBalanceIds])

  // 只保留当前仍存在的凭据缓存，避免删除后残留旧数据
  useEffect(() => {
    if (!data?.credentials) {
      setBalanceMap(new Map())
      setLoadingBalanceIds(new Set())
      balanceMapRef.current = new Map()
      loadingBalanceIdsRef.current = new Set()
      balanceFetchFailedIdsRef.current = new Set()
      return
    }

    const validIds = new Set(data.credentials.map(credential => credential.id))

    setBalanceMap(prev => {
      const next = new Map<number, BalanceResponse>()
      prev.forEach((value, id) => {
        if (validIds.has(id)) {
          next.set(id, value)
        }
      })
      const result = next.size === prev.size ? prev : next
      balanceMapRef.current = result
      return result
    })

    setLoadingBalanceIds(prev => {
      if (prev.size === 0) {
        loadingBalanceIdsRef.current = prev
        return prev
      }
      const next = new Set<number>()
      prev.forEach(id => {
        if (validIds.has(id)) {
          next.add(id)
        }
      })
      const result = next.size === prev.size ? prev : next
      loadingBalanceIdsRef.current = result
      return result
    })
    balanceFetchFailedIdsRef.current = new Set(
      Array.from(balanceFetchFailedIdsRef.current).filter(id => validIds.has(id)),
    )
  }, [data?.credentials])

  const toggleDarkMode = () => {
    setDarkMode(!darkMode)
    document.documentElement.classList.toggle('dark')
  }

  const handleViewBalance = (id: number) => {
    setSelectedCredentialId(id)
    setBalanceDialogOpen(true)
  }

  const fetchBalancesForIds = async (
    ids: number[],
    options: { force?: boolean } = {},
  ) => {
    const uniqueIds = Array.from(new Set(ids))
    const candidates = uniqueIds.filter(id => {
      if (loadingBalanceIdsRef.current.has(id)) return false
      if (options.force) return true
      return !balanceMapRef.current.has(id) && !balanceFetchFailedIdsRef.current.has(id)
    })

    if (candidates.length === 0) {
      return { requested: 0, succeeded: 0, failed: 0 }
    }

    if (options.force) {
      candidates.forEach(id => balanceFetchFailedIdsRef.current.delete(id))
    }

    setLoadingBalanceIds(prev => {
      const next = new Set(prev)
      candidates.forEach(id => next.add(id))
      loadingBalanceIdsRef.current = next
      return next
    })

    let succeeded = 0
    let failed = 0
    for (const id of candidates) {
      try {
        const balance = await getCredentialBalance(id)
        succeeded += 1
        balanceFetchFailedIdsRef.current.delete(id)
        setBalanceMap(prev => {
          const next = new Map(prev)
          next.set(id, balance)
          balanceMapRef.current = next
          return next
        })
      } catch {
        failed += 1
        balanceFetchFailedIdsRef.current.add(id)
      } finally {
        setLoadingBalanceIds(prev => {
          const next = new Set(prev)
          next.delete(id)
          loadingBalanceIdsRef.current = next
          return next
        })
      }
    }

    if (succeeded > 0) {
      setLastBalanceRefreshAt(new Date())
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
    }
    return { requested: candidates.length, succeeded, failed }
  }

  useEffect(() => {
    const ids = currentCredentials
      .filter(credential => credential.usageLimit <= 0)
      .map(credential => credential.id)
    if (ids.length > 0) {
      void fetchBalancesForIds(ids)
    }
  }, [currentBalanceFetchKey])

  useEffect(() => {
    if (!autoBalanceRefreshEnabled || currentBalanceIds.length === 0 || typeof window === 'undefined') return

    void fetchBalancesForIds(currentBalanceIds, { force: true })
    const timer = window.setInterval(() => {
      void fetchBalancesForIds(currentBalanceIds, { force: true })
    }, autoBalanceRefreshIntervalMs)

    return () => window.clearInterval(timer)
  }, [autoBalanceRefreshEnabled, autoBalanceRefreshIntervalMs, currentBalanceIdsKey])

  const handleRefreshBalance = async (id: number) => {
    const result = await fetchBalancesForIds([id], { force: true })
    if (result.succeeded > 0 && result.failed === 0) {
      toast.success('额度已刷新')
    } else if (result.failed > 0) {
      toast.error('额度查询失败')
    }
  }

  const handleRefreshVisibleBalances = async () => {
    if (currentCredentials.length === 0) {
      toast.error('当前页没有账号')
      return
    }
    const result = await fetchBalancesForIds(currentCredentials.map(credential => credential.id), { force: true })
    if (result.failed === 0) {
      toast.success(`已刷新 ${result.succeeded} 个账号额度`)
    } else {
      toast.warning(`额度刷新：成功 ${result.succeeded}，失败 ${result.failed}`)
    }
  }

  const handleRefresh = () => {
    refetch()
    fetchRuntimeStatus()
    toast.success('已刷新凭据列表')
  }

  const handleLogout = () => {
    storage.removeApiKey()
    queryClient.clear()
    onLogout()
  }

  // 选择管理
  const toggleSelect = (id: number) => {
    const newSelected = new Set(selectedIds)
    if (newSelected.has(id)) {
      newSelected.delete(id)
    } else {
      newSelected.add(id)
    }
    setSelectedIds(newSelected)
  }

  const deselectAll = () => {
    setSelectedIds(new Set())
  }

  const toggleSort = (key: SortKey) => {
    if (sortKey === key) {
      setSortOrder(order => order === 'asc' ? 'desc' : 'asc')
    } else {
      setSortKey(key)
      setSortOrder('asc')
    }
  }

  const toggleColumn = (key: ColumnKey) => {
    if (key === 'actions') return
    setVisibleColumns(prev => {
      const next = new Set(prev)
      if (next.has(key)) {
        next.delete(key)
      } else {
        next.add(key)
      }
      next.add('actions')
      return next
    })
  }

  const handleToggleCredentialDisabled = (credential: CredentialStatusItem) => {
    setDisabled(
      { id: credential.id, disabled: !credential.disabled },
      {
        onSuccess: () => toast.success(credential.disabled ? '已启用凭据' : '已禁用凭据'),
        onError: error => toast.error(`操作失败: ${extractErrorMessage(error)}`),
      }
    )
  }

  const openPolicyDialog = (credential: CredentialStatusItem) => {
    setPolicyCredential(credential)
    setPolicyDialogOpen(true)
  }

  const handleClearCooldown = (credential: CredentialStatusItem) => {
    clearCooldown(credential.id, {
      onSuccess: () => toast.success('冷却状态已清除'),
      onError: error => toast.error(`清除失败: ${extractErrorMessage(error)}`),
    })
  }

  const handleForceRefreshOne = async (id: number) => {
    try {
      await forceRefreshToken(id)
      queryClient.invalidateQueries({ queryKey: ['credentials'] })
      toast.success('Token 已刷新')
    } catch (error) {
      toast.error(`刷新失败: ${extractErrorMessage(error)}`)
    }
  }

  const handleDeleteOne = (id: number) => {
    if (!confirm(`确定要删除凭据 #${id} 吗？此操作无法撤销。`)) return
    deleteCredential(id, {
      onSuccess: () => toast.success('凭据已删除'),
      onError: error => toast.error(`删除失败: ${extractErrorMessage(error)}`),
    })
  }

  const handleDynamicProxyAction = (
    id: number,
    action: 'bind' | 'rotate' | 'verify' | 'clear',
  ) => {
    const callbacks = {
      onSuccess: () => {
        toast.success(
          action === 'bind' ? '动态代理已绑定' :
          action === 'rotate' ? '动态代理已换绑' :
          action === 'verify' ? '动态代理验证成功' :
          '动态代理已清除'
        )
        fetchRuntimeStatus()
      },
      onError: (error: unknown) => toast.error(`动态代理操作失败: ${extractErrorMessage(error)}`),
    }
    if (action === 'bind') bindDynamicProxy(id, callbacks)
    if (action === 'rotate') rotateDynamicProxy(id, callbacks)
    if (action === 'verify') verifyDynamicProxy(id, callbacks)
    if (action === 'clear') clearDynamicProxy(id, callbacks)
  }

  const handleBatchDynamicProxy = (action: 'bind' | 'rotate' | 'verify' | 'clear') => {
    if (selectedIds.size === 0) {
      toast.error('请先选择凭据')
      return
    }
    dynamicProxyBatchAction(
      { action, ids: Array.from(selectedIds) },
      {
        onSuccess: result => {
          const actionLabel =
            action === 'bind' ? '绑定' :
            action === 'rotate' ? '换绑' :
            action === 'verify' ? '验证' :
            '清除'
          if (result.failed === 0) {
            toast.success(`动态代理${actionLabel}成功：${result.succeeded}/${result.requested}`)
          } else {
            toast.warning(`动态代理${actionLabel}：成功 ${result.succeeded}，失败 ${result.failed}`)
          }
          fetchRuntimeStatus()
          deselectAll()
        },
        onError: error => toast.error(`动态代理批量操作失败: ${extractErrorMessage(error)}`),
      }
    )
  }

  const handleToggleSelectAllCurrentPage = () => {
    const allSelected = currentCredentials.length > 0 && currentCredentials.every(c => selectedIds.has(c.id))
    setSelectedIds(prev => {
      const next = new Set(prev)
      currentCredentials.forEach(credential => {
        if (allSelected) {
          next.delete(credential.id)
        } else {
          next.add(credential.id)
        }
      })
      return next
    })
  }

  // 批量删除（仅删除已禁用项）
  const handleBatchDelete = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要删除的凭据')
      return
    }

    const disabledIds = Array.from(selectedIds).filter(id => {
      const credential = data?.credentials.find(c => c.id === id)
      return Boolean(credential?.disabled)
    })

    if (disabledIds.length === 0) {
      toast.error('选中的凭据中没有已禁用项')
      return
    }

    const skippedCount = selectedIds.size - disabledIds.length
    const skippedText = skippedCount > 0 ? `（将跳过 ${skippedCount} 个未禁用凭据）` : ''

    if (!confirm(`确定要删除 ${disabledIds.length} 个已禁用凭据吗？此操作无法撤销。${skippedText}`)) {
      return
    }

    let successCount = 0
    let failCount = 0

    for (const id of disabledIds) {
      try {
        await new Promise<void>((resolve, reject) => {
          deleteCredential(id, {
            onSuccess: () => {
              successCount++
              resolve()
            },
            onError: (err) => {
              failCount++
              reject(err)
            }
          })
        })
      } catch (error) {
        // 错误已在 onError 中处理
      }
    }

    const skippedResultText = skippedCount > 0 ? `，已跳过 ${skippedCount} 个未禁用凭据` : ''

    if (failCount === 0) {
      toast.success(`成功删除 ${successCount} 个已禁用凭据${skippedResultText}`)
    } else {
      toast.warning(`删除已禁用凭据：成功 ${successCount} 个，失败 ${failCount} 个${skippedResultText}`)
    }

    deselectAll()
  }

  // 批量恢复异常
  const handleBatchResetFailure = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要恢复的凭据')
      return
    }

    const failedIds = Array.from(selectedIds).filter(id => {
      const cred = data?.credentials.find(c => c.id === id)
      return cred && cred.failureCount > 0
    })

    if (failedIds.length === 0) {
      toast.error('选中的凭据中没有失败的凭据')
      return
    }

    let successCount = 0
    let failCount = 0

    for (const id of failedIds) {
      try {
        await new Promise<void>((resolve, reject) => {
          resetFailure(id, {
            onSuccess: () => {
              successCount++
              resolve()
            },
            onError: (err) => {
              failCount++
              reject(err)
            }
          })
        })
      } catch (error) {
        // 错误已在 onError 中处理
      }
    }

    if (failCount === 0) {
      toast.success(`成功恢复 ${successCount} 个凭据`)
    } else {
      toast.warning(`成功 ${successCount} 个，失败 ${failCount} 个`)
    }

    deselectAll()
  }

  const handleBatchSetDisabled = (disabled: boolean) => {
    if (selectedIds.size === 0) {
      toast.error('请先选择凭据')
      return
    }
    let completed = 0
    Array.from(selectedIds).forEach(id => {
      setDisabled(
        { id, disabled },
        {
          onSuccess: () => {
            completed += 1
            if (completed === selectedIds.size) {
              toast.success(disabled ? '已批量禁用' : '已批量启用')
              deselectAll()
            }
          },
          onError: error => {
            toast.error(`操作失败: ${extractErrorMessage(error)}`)
          },
        }
      )
    })
  }

  const handleBatchClearCooldown = () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择凭据')
      return
    }
    clearCooldownBatch(Array.from(selectedIds), {
      onSuccess: () => {
        toast.success('已批量清除冷却')
        deselectAll()
      },
      onError: error => toast.error(`清除失败: ${extractErrorMessage(error)}`),
    })
  }

  // 批量刷新 Token
  const handleBatchForceRefresh = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要刷新的凭据')
      return
    }

    const enabledIds = Array.from(selectedIds).filter(id => {
      const cred = data?.credentials.find(c => c.id === id)
      return cred && !cred.disabled
    })

    if (enabledIds.length === 0) {
      toast.error('选中的凭据中没有启用的凭据')
      return
    }

    setBatchRefreshing(true)
    setBatchRefreshProgress({ current: 0, total: enabledIds.length })

    let successCount = 0
    let failCount = 0

    for (let i = 0; i < enabledIds.length; i++) {
      try {
        await forceRefreshToken(enabledIds[i])
        successCount++
      } catch {
        failCount++
      }
      setBatchRefreshProgress({ current: i + 1, total: enabledIds.length })
    }

    setBatchRefreshing(false)
    queryClient.invalidateQueries({ queryKey: ['credentials'] })

    if (failCount === 0) {
      toast.success(`成功刷新 ${successCount} 个凭据的 Token`)
    } else {
      toast.warning(`刷新 Token：成功 ${successCount} 个，失败 ${failCount} 个`)
    }

    deselectAll()
  }

  // 批量导出明文凭据，每行一个 JSON 对象
  const handleBatchExport = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要导出的凭据')
      return
    }

    setExporting(true)

    try {
      const ids = Array.from(selectedIds)
      const response = await exportCredentials(ids)
      const content = response.credentials
        .map(credential => JSON.stringify(toKamStyleExport(credential)))
        .join('\n')
      const blob = new Blob([content + '\n'], { type: 'text/plain;charset=utf-8' })
      const url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      const timestamp = new Date()
        .toISOString()
        .replace(/[:.]/g, '-')
        .slice(0, 19)

      link.href = url
      link.download = `kiro-credentials-${timestamp}.txt`
      document.body.appendChild(link)
      link.click()
      link.remove()
      URL.revokeObjectURL(url)

      toast.success(`已导出 ${response.count} 个凭据`)
    } catch (error) {
      toast.error('导出失败: ' + extractErrorMessage(error))
    } finally {
      setExporting(false)
    }
  }

  // 批量验活
  const handleBatchVerify = async () => {
    if (selectedIds.size === 0) {
      toast.error('请先选择要验活的凭据')
      return
    }

    // 初始化状态
    setVerifying(true)
    cancelVerifyRef.current = false
    const ids = Array.from(selectedIds)
    setVerifyProgress({ current: 0, total: ids.length })

    let successCount = 0

    // 初始化结果，所有凭据状态为 pending
    const initialResults = new Map<number, VerifyResult>()
    ids.forEach(id => {
      initialResults.set(id, { id, status: 'pending' })
    })
    setVerifyResults(initialResults)
    setVerifyDialogOpen(true)

    // 开始验活
    for (let i = 0; i < ids.length; i++) {
      // 检查是否取消
      if (cancelVerifyRef.current) {
        toast.info('已取消验活')
        break
      }

      const id = ids[i]

      // 更新当前凭据状态为 verifying
      setVerifyResults(prev => {
        const newResults = new Map(prev)
        newResults.set(id, { id, status: 'verifying' })
        return newResults
      })

      try {
        const balance = await getCredentialBalance(id)
        successCount++

        // 更新为成功状态
        setVerifyResults(prev => {
          const newResults = new Map(prev)
          newResults.set(id, {
            id,
            status: 'success',
            usage: `${balance.currentUsage}/${balance.usageLimit}`
          })
          return newResults
        })
      } catch (error) {
        // 更新为失败状态
        setVerifyResults(prev => {
          const newResults = new Map(prev)
          newResults.set(id, {
            id,
            status: 'failed',
            error: extractErrorMessage(error)
          })
          return newResults
        })
      }

      // 更新进度
      setVerifyProgress({ current: i + 1, total: ids.length })

      // 添加延迟防止封号（最后一个不需要延迟）
      if (i < ids.length - 1 && !cancelVerifyRef.current) {
        await new Promise(resolve => setTimeout(resolve, 2000))
      }
    }

    setVerifying(false)

    if (!cancelVerifyRef.current) {
      toast.success(`验活完成：成功 ${successCount}/${ids.length}`)
    }
  }

  // 取消验活
  const handleCancelVerify = () => {
    cancelVerifyRef.current = true
    setVerifying(false)
  }

  // 切换负载均衡模式
  const handleToggleLoadBalancing = () => {
    const currentMode = loadBalancingData?.mode || 'priority'
    const newMode = currentMode === 'priority' ? 'balanced' : 'priority'

    setLoadBalancingMode(newMode, {
      onSuccess: () => {
        const modeName = newMode === 'priority' ? '优先级模式' : '均衡负载模式'
        toast.success(`已切换到${modeName}`)
      },
      onError: (error) => {
        toast.error(`切换失败: ${extractErrorMessage(error)}`)
      }
    })
  }

  const openCredentialTestDialog = (credential: CredentialStatusItem) => {
    setTestCredential(credential)
    setTestDialogOpen(true)
  }

  if (isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background">
        <div className="text-center">
          <div className="animate-spin rounded-full h-12 w-12 border-b-2 border-primary mx-auto mb-4"></div>
          <p className="text-muted-foreground">加载中...</p>
        </div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-background p-4">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6 text-center">
            <div className="text-red-500 mb-4">加载失败</div>
            <p className="text-muted-foreground mb-4">{(error as Error).message}</p>
            <div className="space-x-2">
              <Button onClick={() => refetch()}>重试</Button>
              <Button variant="outline" onClick={handleLogout}>重新登录</Button>
            </div>
          </CardContent>
        </Card>
      </div>
    )
  }

  return (
    <div className="min-h-screen bg-background">
      <header className="sticky top-0 z-50 w-full border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
        <div className="flex h-14 items-center justify-between px-4 md:px-6">
          <div className="flex items-center gap-2">
            <Server className="h-5 w-5" />
            <span className="font-semibold">Kiro Admin</span>
            <Badge variant="outline">单机</Badge>
          </div>
          <div className="flex items-center gap-1 md:gap-2">
            <Button variant="ghost" size="icon" onClick={toggleDarkMode} title="切换主题">
              {darkMode ? <Sun className="h-5 w-5" /> : <Moon className="h-5 w-5" />}
            </Button>
            <Button variant="ghost" size="icon" onClick={handleRefresh} title="刷新">
              <RefreshCw className="h-5 w-5" />
            </Button>
            <Button variant="ghost" size="icon" onClick={handleLogout} title="退出">
              <LogOut className="h-5 w-5" />
            </Button>
          </div>
        </div>
      </header>

      <main className="w-full px-4 py-5 md:px-6">
        <div className="mb-5 grid gap-3 md:grid-cols-2 xl:grid-cols-6">
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium text-muted-foreground">账号池</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex items-end gap-2">
                <span className="text-2xl font-bold">{data?.total || 0}</span>
                <span className="pb-1 text-sm text-muted-foreground">可用 {data?.available || 0}</span>
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium text-muted-foreground">全局并发</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold tabular-nums">
                {runtimeStatus ? `${runtimeStatus.globalInFlight} / ${runtimeStatus.globalMaxConcurrent}` : '-'}
              </div>
              <div className="mt-1 text-xs text-muted-foreground">
                队列 {runtimeStatus ? `${runtimeStatus.queueDepth} / ${runtimeStatus.queueMaxSize}` : '-'}
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium text-muted-foreground">实际 RPM</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="text-2xl font-bold tabular-nums">
                {runtimeStatus ? formatRpm(runtimeStatus.requestMetrics.requestRpm1m) : '-'}
              </div>
              <div className="mt-1 text-xs text-muted-foreground">
                1m 成功 {runtimeStatus ? formatRpm(runtimeStatus.requestMetrics.successRpm1m) : '-'}，5m {runtimeStatus ? formatRpm(runtimeStatus.requestMetrics.requestRpm5m) : '-'}
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium text-muted-foreground">调度状态</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex flex-wrap gap-2">
                <Badge variant="secondary">可调度 {runtimeStatus?.dispatchAvailableCredentials ?? '-'}</Badge>
                <Badge variant={runtimeStatus && runtimeStatus.coolingDownCredentials > 0 ? 'warning' : 'outline'}>
                  冷却 {runtimeStatus?.coolingDownCredentials ?? '-'}
                </Badge>
              </div>
              <div className="mt-2 text-xs text-muted-foreground">当前 #{data?.currentId || '-'}</div>
            </CardContent>
          </Card>
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
                <Activity className="h-4 w-4" />
                运行策略
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex items-center gap-2">
                <Badge variant="outline">
                  {runtimeStatus?.loadBalancingMode === 'balanced' ? '均衡负载' : '优先级'}
                </Badge>
                <Badge variant="secondary">
                  {runtimeStatus?.endpoints.find(endpoint => endpoint.name === runtimeStatus.defaultEndpoint)?.label || runtimeStatus?.defaultEndpoint || '-'}
                </Badge>
                <Badge variant="outline">默认并发 {runtimeStatus?.perAccountDefaultMaxConcurrent ?? '-'}</Badge>
                <Badge variant={runtimeStatus?.sessionAffinityEnabled === false ? 'secondary' : 'outline'}>
                  会话亲和 {runtimeStatus?.sessionAffinityEnabled === false ? '关闭' : runtimeStatus?.sessionAffinityBindings ?? 0}
                </Badge>
              </div>
              <div className="mt-2 text-xs text-muted-foreground">
                全局 RPM {runtimeStatus?.globalRpm || '不限'}，账号 RPM {runtimeStatus?.perAccountDefaultRpm || '不限'}
              </div>
            </CardContent>
          </Card>
          <Card>
            <CardHeader className="pb-2">
              <CardTitle className="text-sm font-medium text-muted-foreground">动态 IP</CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex flex-wrap gap-2">
                <Badge variant={runtimeStatus?.dynamicProxy.enabled ? 'success' : 'outline'}>
                  {runtimeStatus?.dynamicProxy.enabled ? '已启用' : '未启用'}
                </Badge>
                <Badge variant="outline">绑定 {runtimeStatus?.dynamicProxy.bound ?? 0}</Badge>
                <Badge variant={(runtimeStatus?.dynamicProxy.failed ?? 0) > 0 ? 'destructive' : 'outline'}>
                  失败 {runtimeStatus?.dynamicProxy.failed ?? 0}
                </Badge>
              </div>
              <div className="mt-2 text-xs text-muted-foreground">
                未绑定 {runtimeStatus?.dynamicProxy.unbound ?? 0}，即将续绑 {runtimeStatus?.dynamicProxy.expiringSoon ?? 0}
              </div>
            </CardContent>
          </Card>
        </div>

        {runtimeStatus?.requestMetrics.requestCount ? (
          <div className="mb-5 rounded-lg border bg-card p-3">
            <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
              <div className="text-sm font-medium">最近 {Math.round(runtimeStatus.requestMetrics.windowSecs / 60)} 分钟请求耗时</div>
              <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
                <span>成功 {runtimeStatus.requestMetrics.successCount}</span>
                <span>失败 {runtimeStatus.requestMetrics.errorCount}</span>
                <span>流式 {runtimeStatus.requestMetrics.streamCount}</span>
                <span>重试 {runtimeStatus.requestMetrics.retryCount}</span>
                <span>排队 P95 {formatMs(runtimeStatus.requestMetrics.p95QueueMs)}</span>
                <span>拿账号 P95 {formatMs(runtimeStatus.requestMetrics.p95AcquireMs)}</span>
                <span>上游 P95 {formatMs(runtimeStatus.requestMetrics.p95UpstreamMs)}</span>
              </div>
            </div>
            <div className="grid gap-2 md:grid-cols-2 xl:grid-cols-5">
              {runtimeStatus.requestMetrics.slowModels.map(model => (
                <div key={model.model} className="rounded-md border bg-muted/30 px-3 py-2">
                  <div className="truncate text-sm font-medium" title={model.model}>{model.model}</div>
                  <div className="mt-1 text-xs text-muted-foreground">
                    P95 {formatMs(model.p95TotalMs)} · 均值 {formatMs(model.avgTotalMs)} · {model.requestCount} 次
                  </div>
                </div>
              ))}
            </div>
          </div>
        ) : null}

        {runtimeStatus?.modelCooldowns.length ? (
          <div className="mb-5 rounded-lg border border-amber-200 bg-amber-50 p-3 text-amber-950">
            <div className="mb-2 text-sm font-medium">模型冷却</div>
            <div className="grid gap-2 md:grid-cols-2 xl:grid-cols-4">
              {runtimeStatus.modelCooldowns.map(item => (
                <div key={item.model} className="rounded-md border border-amber-200 bg-white/70 px-3 py-2">
                  <div className="truncate text-sm font-medium" title={item.model}>{item.model}</div>
                  <div className="mt-1 text-xs text-amber-800">
                    {formatMs(item.remainingMs)} · {item.reason}
                  </div>
                </div>
              ))}
            </div>
          </div>
        ) : null}

        <div className="space-y-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex flex-wrap items-center gap-2">
              <Button variant="outline" size="sm" onClick={handleRefresh}>
                <RefreshCw className="h-4 w-4" />
                刷新
              </Button>
              <Button variant="outline" size="sm" disabled>
                <RefreshCw className="h-4 w-4" />
                自动刷新 30s
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={handleRefreshVisibleBalances}
                disabled={currentCredentials.length === 0 || currentPageBalanceLoading}
              >
                <RefreshCw className={`h-4 w-4 ${currentPageBalanceLoading ? 'animate-spin' : ''}`} />
                刷新当前页额度
              </Button>
              <Button
                variant={autoBalanceRefreshEnabled ? 'secondary' : 'outline'}
                size="sm"
                onClick={() => setAutoBalanceRefreshEnabled(enabled => !enabled)}
                disabled={currentCredentials.length === 0}
              >
                <RefreshCw className={`h-4 w-4 ${autoBalanceRefreshEnabled && currentPageBalanceLoading ? 'animate-spin' : ''}`} />
                额度自动刷新
              </Button>
              <select
                value={autoBalanceRefreshIntervalMs}
                onChange={event => setAutoBalanceRefreshIntervalMs(Number(event.target.value))}
                disabled={!autoBalanceRefreshEnabled}
                className="h-9 rounded-md border border-input bg-background px-2 text-sm disabled:cursor-not-allowed disabled:opacity-50"
                title="额度自动刷新间隔"
              >
                {BALANCE_REFRESH_INTERVAL_OPTIONS.map(option => (
                  <option key={option.value} value={option.value}>{option.label}</option>
                ))}
              </select>
              <span className="text-xs text-muted-foreground">
                额度刷新：{autoBalanceRefreshEnabled ? `${BALANCE_REFRESH_INTERVAL_OPTIONS.find(option => option.value === autoBalanceRefreshIntervalMs)?.label || `${Math.round(autoBalanceRefreshIntervalMs / 1000)}s`} · ${formatClockTime(lastBalanceRefreshAt)}` : '手动'}
              </span>
              <Button
                variant="outline"
                size="sm"
                onClick={handleToggleLoadBalancing}
                disabled={isSettingMode}
              >
                <ShieldCheck className="h-4 w-4" />
                {loadBalancingData?.mode === 'priority' ? '优先级' : '均衡负载'}
              </Button>
              <Button variant="outline" size="sm" onClick={() => setRuntimeSettingsOpen(true)}>
                <Settings className="h-4 w-4" />
                运行策略
              </Button>
              <div className="relative">
                <Button variant="outline" size="sm" onClick={() => setColumnMenuOpen(open => !open)}>
                  <Columns3 className="h-4 w-4" />
                  列设置
                </Button>
                {columnMenuOpen && (
                  <div className="absolute left-0 z-40 mt-2 w-48 rounded-md border bg-popover p-2 shadow-lg">
                    {DEFAULT_VISIBLE_COLUMNS.map(key => (
                      <button
                        key={key}
                        className="flex w-full items-center justify-between rounded px-2 py-1.5 text-sm hover:bg-accent"
                        onClick={() => toggleColumn(key)}
                      >
                        <span>{columnLabels[key]}</span>
                        <span>{visibleColumns.has(key) ? '✓' : ''}</span>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-2">
              <Button onClick={() => setKamImportDialogOpen(true)} size="sm" variant="outline">
                <FileUp className="h-4 w-4" />
                KAM 导入
              </Button>
              <Button onClick={() => setBatchImportDialogOpen(true)} size="sm" variant="outline">
                <Upload className="h-4 w-4" />
                批量导入
              </Button>
              <Button onClick={handleBatchExport} size="sm" variant="outline" disabled={selectedIds.size === 0 || exporting}>
                <Download className="h-4 w-4" />
                {exporting ? '导出中...' : '导出'}
              </Button>
              <Button onClick={() => setAddDialogOpen(true)} size="sm">
                <Plus className="h-4 w-4" />
                添加账号
              </Button>
            </div>
          </div>

          <div className="grid gap-2 rounded-lg border bg-card p-3 md:grid-cols-[minmax(220px,1.5fr)_repeat(6,minmax(140px,1fr))]">
            <div className="relative">
              <Search className="pointer-events-none absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
              <input
                value={searchQuery}
                onChange={event => setSearchQuery(event.target.value)}
                placeholder="搜索账号、ID、hash、endpoint"
                className="h-10 w-full rounded-md border border-input bg-background pl-9 pr-3 text-sm"
              />
            </div>
            <select value={authFilter} onChange={event => setAuthFilter(event.target.value)} className="h-10 rounded-md border border-input bg-background px-3 text-sm">
              <option value="all">全部认证</option>
              <option value="social">Social</option>
              <option value="idc">Builder ID</option>
              <option value="api_key">API Key</option>
            </select>
            <select value={statusFilter} onChange={event => setStatusFilter(event.target.value)} className="h-10 rounded-md border border-input bg-background px-3 text-sm">
              <option value="all">全部状态</option>
              <option value="active">正常</option>
              <option value="disabled">禁用</option>
              <option value="cooling">冷却中</option>
            </select>
            <select value={dispatchFilter} onChange={event => setDispatchFilter(event.target.value)} className="h-10 rounded-md border border-input bg-background px-3 text-sm">
              <option value="all">全部调度</option>
              <option value="available">可调度</option>
              <option value="full">满载</option>
              <option value="blocked">不可调度</option>
            </select>
            <select value={subscriptionFilter} onChange={event => setSubscriptionFilter(event.target.value)} className="h-10 rounded-md border border-input bg-background px-3 text-sm">
              <option value="all">全部等级</option>
              {subscriptionOptions.map(subscription => (
                <option key={subscription} value={subscription}>{subscriptionFilterLabel(subscription)}</option>
              ))}
            </select>
            <select value={endpointFilter} onChange={event => setEndpointFilter(event.target.value)} className="h-10 rounded-md border border-input bg-background px-3 text-sm">
              <option value="all">全部端点</option>
              {endpointOptions.map(endpoint => (
                <option key={endpoint} value={endpoint}>{endpoint}</option>
              ))}
            </select>
            <select value={proxyFilter} onChange={event => setProxyFilter(event.target.value)} className="h-10 rounded-md border border-input bg-background px-3 text-sm">
              <option value="all">全部代理</option>
              <option value="proxy">有代理</option>
              <option value="dynamic">动态 IP</option>
              <option value="direct">直连</option>
            </select>
          </div>

          {selectedIds.size > 0 && (
            <div className="flex flex-wrap items-center gap-2 rounded-lg border bg-muted/40 p-3">
              <Badge variant="secondary">已选择 {selectedIds.size} 个</Badge>
              <Button size="sm" variant="outline" onClick={() => setBatchPolicyOpen(true)}>
                <SlidersHorizontal className="h-4 w-4" />
                批量策略
              </Button>
              <Button size="sm" variant="outline" onClick={() => handleBatchSetDisabled(false)}>启用</Button>
              <Button size="sm" variant="outline" onClick={() => handleBatchSetDisabled(true)}>禁用</Button>
              <Button size="sm" variant="outline" onClick={handleBatchClearCooldown}>清冷却</Button>
              <Button size="sm" variant="outline" onClick={handleBatchVerify}>
                <CheckCircle2 className="h-4 w-4" />
                验活
              </Button>
              <Button size="sm" variant="outline" onClick={() => handleBatchDynamicProxy('bind')} disabled={dynamicProxyBatching}>
                <Globe2 className="h-4 w-4" />
                绑 IP
              </Button>
              <Button size="sm" variant="outline" onClick={() => handleBatchDynamicProxy('rotate')} disabled={dynamicProxyBatching}>
                换 IP
              </Button>
              <Button size="sm" variant="outline" onClick={() => handleBatchDynamicProxy('verify')} disabled={dynamicProxyBatching}>
                验 IP
              </Button>
              <Button size="sm" variant="outline" onClick={() => handleBatchDynamicProxy('clear')} disabled={dynamicProxyBatching}>
                清 IP
              </Button>
              <Button size="sm" variant="outline" onClick={handleBatchForceRefresh} disabled={batchRefreshing}>
                <RefreshCw className={`h-4 w-4 ${batchRefreshing ? 'animate-spin' : ''}`} />
                {batchRefreshing ? `${batchRefreshProgress.current}/${batchRefreshProgress.total}` : '刷新 Token'}
              </Button>
              <Button size="sm" variant="outline" onClick={handleBatchResetFailure}>
                <RotateCcw className="h-4 w-4" />
                恢复异常
              </Button>
              <Button size="sm" variant="destructive" onClick={handleBatchDelete} disabled={selectedDisabledCount === 0}>
                <Trash2 className="h-4 w-4" />
                删除已禁用
              </Button>
              <Button size="sm" variant="ghost" onClick={deselectAll}>取消选择</Button>
            </div>
          )}

          <AccountTable
            credentials={currentCredentials}
            allowOverUsage={runtimeStatus?.allowOverUsage ?? false}
            selectedIds={selectedIds}
            columns={activeColumns}
            sortKey={sortKey}
            sortOrder={sortOrder}
            balanceMap={balanceMap}
            loadingBalanceIds={loadingBalanceIds}
            onSort={toggleSort}
            onToggleSelect={toggleSelect}
            onToggleSelectAll={handleToggleSelectAllCurrentPage}
            onViewBalance={handleViewBalance}
            onRefreshBalance={handleRefreshBalance}
            onTestConnection={openCredentialTestDialog}
            onEditPolicy={openPolicyDialog}
            onToggleDisabled={handleToggleCredentialDisabled}
            onClearCooldown={handleClearCooldown}
            onForceRefresh={handleForceRefreshOne}
            onDelete={handleDeleteOne}
            onBindDynamicProxy={id => handleDynamicProxyAction(id, 'bind')}
            onRotateDynamicProxy={id => handleDynamicProxyAction(id, 'rotate')}
            onVerifyDynamicProxy={id => handleDynamicProxyAction(id, 'verify')}
            onClearDynamicProxy={id => handleDynamicProxyAction(id, 'clear')}
          />

          <div className="flex flex-wrap items-center justify-between gap-3 border-t pt-4 text-sm text-muted-foreground">
            <div className="flex items-center gap-2">
              <span>显示 {filteredCredentials.length === 0 ? 0 : startIndex + 1} 至 {Math.min(endIndex, filteredCredentials.length)}，共 {filteredCredentials.length} 条</span>
              <select
                value={itemsPerPage}
                onChange={event => setItemsPerPage(Number(event.target.value))}
                className="h-9 rounded-md border border-input bg-background px-2 text-sm"
              >
                <option value={20}>20 / 页</option>
                <option value={50}>50 / 页</option>
                <option value={100}>100 / 页</option>
                <option value={200}>200 / 页</option>
              </select>
            </div>
            <div className="flex items-center gap-2">
              <Button variant="outline" size="sm" onClick={() => setCurrentPage(p => Math.max(1, p - 1))} disabled={currentPage === 1}>
                上一页
              </Button>
              <span>第 {currentPage} / {totalPages} 页</span>
              <Button variant="outline" size="sm" onClick={() => setCurrentPage(p => Math.min(totalPages, p + 1))} disabled={currentPage === totalPages}>
                下一页
              </Button>
            </div>
          </div>
        </div>
      </main>

      {/* 余额对话框 */}
      <BalanceDialog
        credentialId={selectedCredentialId}
        open={balanceDialogOpen}
        onOpenChange={setBalanceDialogOpen}
      />

      {/* 添加凭据对话框 */}
      <AddCredentialDialog
        open={addDialogOpen}
        onOpenChange={setAddDialogOpen}
      />

      {/* 批量导入对话框 */}
      <BatchImportDialog
        open={batchImportDialogOpen}
        onOpenChange={setBatchImportDialogOpen}
      />

      {/* KAM 账号导入对话框 */}
      <KamImportDialog
        open={kamImportDialogOpen}
        onOpenChange={setKamImportDialogOpen}
      />

      {/* 批量验活对话框 */}
      <BatchVerifyDialog
        open={verifyDialogOpen}
        onOpenChange={setVerifyDialogOpen}
        verifying={verifying}
        progress={verifyProgress}
        results={verifyResults}
        onCancel={handleCancelVerify}
      />

      <RuntimeSettingsDialog
        open={runtimeSettingsOpen}
        onOpenChange={setRuntimeSettingsOpen}
      />

      <PolicyDialog
        open={policyDialogOpen}
        onOpenChange={setPolicyDialogOpen}
        credential={policyCredential}
      />

      <CredentialTestDialog
        open={testDialogOpen}
        onOpenChange={setTestDialogOpen}
        credential={testCredential}
      />

      <PolicyDialog
        open={batchPolicyOpen}
        onOpenChange={setBatchPolicyOpen}
        selectedIds={Array.from(selectedIds)}
      />
    </div>
  )
}

import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { useRuntimeSettings, useSetRuntimeSettings } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { RuntimeSettings } from '@/types/api'

interface RuntimeSettingsDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

const numberFields: Array<{
  key: keyof Omit<RuntimeSettings, 'loadBalancingMode' | 'tokenAutoRefreshEnabled' | 'opus47PlainStabilizationMode' | 'opus47DiagnosticsEnabled' | 'virtualCacheUsageEnabled' | 'virtualCacheDefaultTtl' | 'virtualCacheInputMode' | 'virtualCacheCreationMode' | 'virtualCacheFallbackScope' | 'dynamicProxyEnabled' | 'dynamicProxyAutoBindNewAccounts' | 'dynamicProxyProvider' | 'dynamicProxyProtocol' | 'dynamicProxyHost' | 'dynamicProxyUsernameTemplate' | 'dynamicProxyPassword' | 'dynamicProxyRegion' | 'dynamicProxyState' | 'dynamicProxyVerifyUrl'>
  label: string
  hint: string
}> = [
  { key: 'globalMaxConcurrent', label: '全局并发', hint: '1-512' },
  { key: 'perAccountDefaultMaxConcurrent', label: '默认账号并发', hint: '1-64' },
  { key: 'queueMaxSize', label: '队列长度', hint: '0-10000' },
  { key: 'queueTimeoutMs', label: '队列超时 ms', hint: '1000-300000' },
  { key: 'globalRpm', label: '全局 RPM', hint: '0 表示不限速' },
  { key: 'perAccountDefaultRpm', label: '默认账号 RPM', hint: '0 表示不限速' },
  { key: 'rateLimitCooldownMs', label: '429 冷却 ms', hint: '建议 60000' },
  { key: 'transientCooldownMs', label: '瞬态错误冷却 ms', hint: '建议 10000' },
  { key: 'maxRetryAccounts', label: '单请求换号上限', hint: '默认 3，1 表示不换号' },
  { key: 'modelCapacityCooldownMs', label: '模型容量冷却 ms', hint: '建议 10000' },
  { key: 'tokenAutoRefreshIntervalSecs', label: 'Token 刷新扫描秒数', hint: '默认 300' },
  { key: 'tokenAutoRefreshWindowSecs', label: 'Token 提前刷新窗口秒数', hint: '默认 1800' },
  { key: 'sessionAffinityTtlSecs', label: '会话亲和 TTL 秒数', hint: '300-43200，默认 3600' },
  { key: 'virtualCacheUncachedInputTokens', label: '虚拟普通输入 Tokens', hint: '默认 1' },
  { key: 'virtualCacheMinInputTokens', label: '动态普通输入下限', hint: '建议 8' },
  { key: 'virtualCacheMaxInputTokens', label: '动态普通输入上限', hint: '建议 96' },
  { key: 'virtualCacheWarmupTokens', label: '虚拟首轮缓存创建', hint: '建议 18000' },
  { key: 'virtualCacheMinCreationTokens', label: '虚拟最小缓存创建', hint: '建议 128' },
  { key: 'virtualCacheMaxCreationTokens', label: '虚拟最大缓存创建', hint: '建议 1200' },
  { key: 'virtualCacheCreationJitterRatio', label: '动态创建抖动比例', hint: '0-1，例如 0.25' },
  { key: 'virtualCacheBurstEveryTurns', label: '动态突增间隔轮数', hint: '0 表示关闭，建议 7' },
  { key: 'virtualCacheBurstMinTokens', label: '动态突增最小创建', hint: '建议 1500' },
  { key: 'virtualCacheBurstMaxTokens', label: '动态突增最大创建', hint: '建议 3000' },
  { key: 'dynamicProxyPort', label: '动态代理端口', hint: '1-65535' },
  { key: 'dynamicProxyTtlMinutes', label: '动态代理 TTL 分钟', hint: '建议 120' },
  { key: 'dynamicProxyRenewBeforeMs', label: '动态代理提前续绑 ms', hint: '建议 900000' },
  { key: 'dynamicProxyMaxBindRetries', label: '动态代理绑定重试', hint: '建议 3' },
  { key: 'dynamicProxyWorkerIntervalMs', label: '动态代理扫描间隔 ms', hint: '建议 60000' },
  { key: 'dynamicProxyWorkerBatchSize', label: '动态代理批量数量', hint: '建议 20' },
  { key: 'dynamicProxyWorkerConcurrency', label: '动态代理并发数', hint: '建议 3' },
]

export function RuntimeSettingsDialog({ open, onOpenChange }: RuntimeSettingsDialogProps) {
  const { data, isLoading } = useRuntimeSettings()
  const setRuntimeSettings = useSetRuntimeSettings()
  const [form, setForm] = useState<RuntimeSettings | null>(null)

  useEffect(() => {
    if (data && open) {
      setForm(data)
    }
  }, [data, open])

  const updateNumber = (
    key: (typeof numberFields)[number]['key'],
    value: string,
  ) => {
    const next = Number(value)
    setForm(prev => prev ? { ...prev, [key]: Number.isFinite(next) ? next : 0 } : prev)
  }

  const handleSave = () => {
    if (!form) return
    setRuntimeSettings.mutate(form, {
      onSuccess: () => {
        toast.success('运行策略已更新')
        onOpenChange(false)
      },
      onError: error => {
        toast.error(`保存失败: ${extractErrorMessage(error)}`)
      },
    })
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[calc(100dvh-2rem)] w-[calc(100vw-2rem)] max-w-5xl flex-col gap-0 overflow-hidden p-0 sm:max-h-[85vh]">
        <DialogHeader className="border-b px-6 py-5">
          <DialogTitle>运行策略</DialogTitle>
          <DialogDescription>
            修改后立即生效，已有请求不会被中断。
          </DialogDescription>
        </DialogHeader>

        <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
          {isLoading || !form ? (
            <div className="py-10 text-center text-sm text-muted-foreground">加载中...</div>
          ) : (
            <div className="grid gap-4 md:grid-cols-2">
            <div className="space-y-2">
              <label className="text-sm font-medium">负载模式</label>
              <select
                value={form.loadBalancingMode}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, loadBalancingMode: event.target.value as 'priority' | 'balanced' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="priority">优先级模式</option>
                <option value="balanced">均衡负载模式</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">Token 自动刷新</label>
              <select
                value={form.tokenAutoRefreshEnabled ? 'enabled' : 'disabled'}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, tokenAutoRefreshEnabled: event.target.value === 'enabled' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="enabled">启用</option>
                <option value="disabled">关闭</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">Opus 4.7 Plain 稳定模式</label>
              <select
                value={form.opus47PlainStabilizationMode}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, opus47PlainStabilizationMode: event.target.value as 'off' | 'adaptive_low' | 'adaptive_high' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="off">关闭</option>
                <option value="adaptive_low">Adaptive Low</option>
                <option value="adaptive_high">Adaptive High</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">Opus 4.7 诊断日志</label>
              <select
                value={form.opus47DiagnosticsEnabled ? 'enabled' : 'disabled'}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, opus47DiagnosticsEnabled: event.target.value === 'enabled' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="enabled">启用</option>
                <option value="disabled">关闭</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">虚拟缓存 Usage</label>
              <select
                value={form.virtualCacheUsageEnabled ? 'enabled' : 'disabled'}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, virtualCacheUsageEnabled: event.target.value === 'enabled' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="enabled">启用</option>
                <option value="disabled">关闭</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">默认缓存 TTL</label>
              <select
                value={form.virtualCacheDefaultTtl}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, virtualCacheDefaultTtl: event.target.value as '5m' | '1h' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="5m">5 分钟</option>
                <option value="1h">1 小时</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">普通输入模式</label>
              <select
                value={form.virtualCacheInputMode}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, virtualCacheInputMode: event.target.value as 'fixed' | 'estimated_user_delta' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="fixed">固定输入</option>
                <option value="estimated_user_delta">按最新用户输入估算</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">缓存创建模式</label>
              <select
                value={form.virtualCacheCreationMode}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, virtualCacheCreationMode: event.target.value as 'fixed' | 'dynamic' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="fixed">固定下限</option>
                <option value="dynamic">动态变化</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">无 metadata 回退</label>
              <select
                value={form.virtualCacheFallbackScope}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, virtualCacheFallbackScope: event.target.value as 'model' | 'none' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="model">按模型累计</option>
                <option value="none">不累计</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">动态 IP 绑定</label>
              <select
                value={form.dynamicProxyEnabled ? 'enabled' : 'disabled'}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, dynamicProxyEnabled: event.target.value === 'enabled' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="disabled">关闭</option>
                <option value="enabled">启用</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">新账号自动绑定</label>
              <select
                value={form.dynamicProxyAutoBindNewAccounts ? 'enabled' : 'disabled'}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, dynamicProxyAutoBindNewAccounts: event.target.value === 'enabled' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="disabled">关闭</option>
                <option value="enabled">启用</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">动态代理协议</label>
              <select
                value={form.dynamicProxyProtocol}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, dynamicProxyProtocol: event.target.value as 'http' | 'socks5' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="http">HTTP</option>
                <option value="socks5">SOCKS5</option>
              </select>
            </div>

            {([
              ['dynamicProxyProvider', '动态代理供应商', 'novproxy'],
              ['dynamicProxyHost', '动态代理 Host', 'us.novproxy.io'],
              ['dynamicProxyUsernameTemplate', '用户名模板', '支持 {region}/{state}/{sid}/{ttl}'],
              ['dynamicProxyPassword', '动态代理密码', '保存后服务端明文持久化'],
              ['dynamicProxyRegion', '动态代理 Region', '例如 US'],
              ['dynamicProxyState', '动态代理 State', '例如 New Jersey'],
              ['dynamicProxyVerifyUrl', '出口验证 URL', '默认 https://ipinfo.io/json'],
            ] as const).map(([key, label, hint]) => (
              <div key={key} className="space-y-2 md:col-span-2">
                <label className="text-sm font-medium">{label}</label>
                <Input
                  type={key === 'dynamicProxyPassword' ? 'password' : 'text'}
                  value={form[key]}
                  onChange={event => setForm(prev => prev ? { ...prev, [key]: event.target.value } : prev)}
                />
                <p className="text-xs text-muted-foreground">{hint}</p>
              </div>
            ))}

            {numberFields.map(field => (
              <div key={field.key} className="space-y-2">
                <label className="text-sm font-medium">{field.label}</label>
                <Input
                  type="number"
                  min={0}
                  value={form[field.key]}
                  onChange={event => updateNumber(field.key, event.target.value)}
                />
                <p className="text-xs text-muted-foreground">{field.hint}</p>
              </div>
            ))}
          </div>
          )}
        </div>

        <DialogFooter className="border-t bg-background px-6 py-4">
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={handleSave} disabled={!form || setRuntimeSettings.isPending}>
            {setRuntimeSettings.isPending ? '保存中...' : '保存'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

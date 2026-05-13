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
  key: keyof Omit<RuntimeSettings, 'loadBalancingMode' | 'tokenAutoRefreshEnabled' | 'sameAccountRetryRules' | 'opus47PlainStabilizationMode' | 'opus47AntmlProbeCompat' | 'opus47CleanProbeMode' | 'opus47DetectionProfile' | 'opus47SignedThinkingPreservation' | 'opus47ShortThinkingExperiment' | 'opus47DiagnosticsEnabled' | 'opus47RawDebugEnabled' | 'compatUsageShape' | 'compatThinkingModel' | 'compatModelsShape' | 'virtualCacheUsageEnabled' | 'virtualCacheDefaultTtl' | 'virtualCacheInputMode' | 'virtualCacheCreationMode' | 'virtualCacheFallbackScope' | 'dynamicProxyEnabled' | 'dynamicProxyAutoBindNewAccounts' | 'dynamicProxyProvider' | 'dynamicProxyProtocol' | 'dynamicProxyHost' | 'dynamicProxyUsernameTemplate' | 'dynamicProxyPassword' | 'dynamicProxyRegion' | 'dynamicProxyState' | 'dynamicProxyVerifyUrl'>
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
  { key: 'opus47RawDebugMaxChars', label: '4.7 原始日志长度', hint: '1000-200000，仅调试时使用' },
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

  const addRecommendedRetryRule = () => {
    setForm(prev => prev ? {
      ...prev,
      sameAccountRetryRules: [
        ...prev.sameAccountRetryRules,
        {
          enabled: true,
          status: '429',
          reason: 'INSUFFICIENT_MODEL_CAPACITY',
          attempts: 2,
          delayMs: 1500,
          respectRetryAfter: true,
        },
      ],
    } : prev)
  }

  const addRetryRule = () => {
    setForm(prev => prev ? {
      ...prev,
      sameAccountRetryRules: [
        ...prev.sameAccountRetryRules,
        {
          enabled: true,
          status: '500-599',
          reason: '',
          attempts: 1,
          delayMs: 1000,
          respectRetryAfter: true,
        },
      ],
    } : prev)
  }

  const updateRetryRule = (
    index: number,
    patch: Partial<RuntimeSettings['sameAccountRetryRules'][number]>,
  ) => {
    setForm(prev => {
      if (!prev) return prev
      const sameAccountRetryRules = prev.sameAccountRetryRules.map((rule, idx) =>
        idx === index ? { ...rule, ...patch } : rule,
      )
      return { ...prev, sameAccountRetryRules }
    })
  }

  const removeRetryRule = (index: number) => {
    setForm(prev => prev ? {
      ...prev,
      sameAccountRetryRules: prev.sameAccountRetryRules.filter((_, idx) => idx !== index),
    } : prev)
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
              <label className="text-sm font-medium">Opus 4.7 检测 Profile</label>
              <select
                value={form.opus47DetectionProfile}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, opus47DetectionProfile: event.target.value as 'normal' | 'cc_max_like' | 'clean_probe_debug' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="normal">Normal</option>
                <option value="cc_max_like">CC Max Like</option>
                <option value="clean_probe_debug">Clean Probe Debug</option>
              </select>
              <p className="text-xs text-muted-foreground">
                CC Max Like 会统一使用聚合器模型列表、flat usage、native thinking，并关闭 Clean Probe。
              </p>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">Signed Thinking 保留</label>
              <select
                value={form.opus47SignedThinkingPreservation}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, opus47SignedThinkingPreservation: event.target.value as 'off' | 'diagnose' | 'cache_only' | 'history_experiment' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="off">关闭</option>
                <option value="diagnose">仅诊断</option>
                <option value="cache_only">缓存真实签名</option>
                <option value="history_experiment">历史回放实验</option>
              </select>
              <p className="text-xs text-muted-foreground">
                只观察或缓存上游真实 signature，不生成假签名。
              </p>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">短请求 Thinking 实验</label>
              <select
                value={form.opus47ShortThinkingExperiment}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, opus47ShortThinkingExperiment: event.target.value as 'off' | 'adaptive_high' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="off">关闭</option>
                <option value="adaptive_high">Adaptive High</option>
              </select>
              <p className="text-xs text-muted-foreground">
                默认关闭；仅用于 cc_max_like + history_experiment 下的短请求/PDF 签名 A/B。
              </p>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">Opus 4.7 ANTML 探针兼容</label>
              <select
                value={form.opus47AntmlProbeCompat}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, opus47AntmlProbeCompat: event.target.value as 'off' | 'clarify' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="off">关闭</option>
                <option value="clarify">Clarify</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">Opus 4.7 Clean Probe</label>
              <select
                value={form.opus47CleanProbeMode}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, opus47CleanProbeMode: event.target.value as 'off' | 'clean' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="off">关闭</option>
                <option value="clean">Clean</option>
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
              <label className="text-sm font-medium">Opus 4.7 原始调试日志</label>
              <select
                value={form.opus47RawDebugEnabled ? 'enabled' : 'disabled'}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, opus47RawDebugEnabled: event.target.value === 'enabled' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="disabled">关闭</option>
                <option value="enabled">启用</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">Usage 兼容形态</label>
              <select
                value={form.compatUsageShape}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, compatUsageShape: event.target.value as 'anthropic' | 'flat' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="anthropic">Anthropic 标准</option>
                <option value="flat">Flat 四字段</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">Thinking 模型兼容</label>
              <select
                value={form.compatThinkingModel}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, compatThinkingModel: event.target.value as 'native' | 'plain_text' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="native">原生 thinking</option>
                <option value="plain_text">归一 plain text</option>
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">模型列表兼容</label>
              <select
                value={form.compatModelsShape}
                onChange={event =>
                  setForm(prev => prev ? { ...prev, compatModelsShape: event.target.value as 'anthropic' | 'aggregator' } : prev)
                }
                className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
              >
                <option value="anthropic">Anthropic 风格</option>
                <option value="aggregator">聚合器风格</option>
              </select>
            </div>

            <div className="space-y-3 md:col-span-2">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <label className="text-sm font-medium">单号重试规则</label>
                  <p className="text-xs text-muted-foreground">
                    命中规则时先用当前账号重试，耗尽后才进入账号冷却或换号。
                  </p>
                </div>
                <div className="flex gap-2">
                  <Button type="button" variant="outline" size="sm" onClick={addRecommendedRetryRule}>
                    添加推荐规则
                  </Button>
                  <Button type="button" variant="outline" size="sm" onClick={addRetryRule}>
                    添加规则
                  </Button>
                </div>
              </div>

              <div className="overflow-x-auto rounded-md border">
                <table className="w-full min-w-[860px] text-sm">
                  <thead className="bg-muted/50 text-left text-xs text-muted-foreground">
                    <tr>
                      <th className="w-16 px-3 py-2 font-medium">启用</th>
                      <th className="px-3 py-2 font-medium">状态码</th>
                      <th className="px-3 py-2 font-medium">reason</th>
                      <th className="w-28 px-3 py-2 font-medium">次数</th>
                      <th className="w-32 px-3 py-2 font-medium">间隔 ms</th>
                      <th className="w-32 px-3 py-2 font-medium">Retry-After</th>
                      <th className="w-20 px-3 py-2 font-medium">操作</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y">
                    {form.sameAccountRetryRules.length === 0 ? (
                      <tr>
                        <td colSpan={7} className="px-3 py-6 text-center text-sm text-muted-foreground">
                          未配置规则，单号重试关闭。
                        </td>
                      </tr>
                    ) : form.sameAccountRetryRules.map((rule, index) => (
                      <tr key={index} className="align-top">
                        <td className="px-3 py-2">
                          <input
                            type="checkbox"
                            checked={rule.enabled}
                            onChange={event => updateRetryRule(index, { enabled: event.target.checked })}
                            className="mt-2"
                          />
                        </td>
                        <td className="px-3 py-2">
                          <Input
                            value={rule.status}
                            onChange={event => updateRetryRule(index, { status: event.target.value })}
                            placeholder="429 或 408,500-599"
                          />
                        </td>
                        <td className="px-3 py-2">
                          <Input
                            value={rule.reason ?? ''}
                            onChange={event => updateRetryRule(index, { reason: event.target.value })}
                            placeholder="可留空"
                          />
                        </td>
                        <td className="px-3 py-2">
                          <Input
                            type="number"
                            min={0}
                            max={10}
                            value={rule.attempts}
                            onChange={event => updateRetryRule(index, { attempts: Number(event.target.value) || 0 })}
                          />
                        </td>
                        <td className="px-3 py-2">
                          <Input
                            type="number"
                            min={100}
                            value={rule.delayMs}
                            onChange={event => updateRetryRule(index, { delayMs: Number(event.target.value) || 0 })}
                          />
                        </td>
                        <td className="px-3 py-2">
                          <input
                            type="checkbox"
                            checked={rule.respectRetryAfter}
                            onChange={event => updateRetryRule(index, { respectRetryAfter: event.target.checked })}
                            className="mt-2"
                          />
                        </td>
                        <td className="px-3 py-2">
                          <Button type="button" variant="ghost" size="sm" onClick={() => removeRetryRule(index)}>
                            删除
                          </Button>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <p className="text-xs text-muted-foreground">
                状态码支持单值、范围和逗号组合，例如 429、500-599、408,500-599。reason 留空时只匹配状态码。
              </p>
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

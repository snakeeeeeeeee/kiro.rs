import { useEffect, useMemo, useState } from 'react'
import { CheckCircle2, Play, RefreshCw, XCircle } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { useTestCredentialConnection } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { CredentialStatusItem, CredentialTestResponse } from '@/types/api'

interface CredentialTestDialogProps {
  credential: CredentialStatusItem | null
  open: boolean
  onOpenChange: (open: boolean) => void
}

const TEST_MODELS = [
  { label: 'Claude Sonnet 4.6', value: 'claude-sonnet-4-6' },
  { label: 'Claude Haiku 4.5', value: 'claude-haiku-4-5-20251001' },
  { label: 'Claude Opus 4.6', value: 'claude-opus-4-6' },
  { label: 'Claude Opus 4.7', value: 'claude-opus-4-7' },
  { label: 'Claude Opus 4.8', value: 'claude-opus-4-8' },
]

function credentialName(credential: CredentialStatusItem): string {
  return credential.email || credential.maskedApiKey || `凭据 #${credential.id}`
}

function formatMs(value: number): string {
  return value >= 1000 ? `${(value / 1000).toFixed(2)}s` : `${Math.round(value)}ms`
}

export function CredentialTestDialog({ credential, open, onOpenChange }: CredentialTestDialogProps) {
  const [model, setModel] = useState('claude-opus-4-8')
  const [prompt, setPrompt] = useState('hi')
  const [result, setResult] = useState<CredentialTestResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const testCredential = useTestCredentialConnection()

  useEffect(() => {
    if (!open) return
    setPrompt('hi')
    setResult(null)
    setError(null)
  }, [credential?.id, open])

  const selectedModelLabel = useMemo(
    () => TEST_MODELS.find(item => item.value === model)?.label || model,
    [model]
  )

  const handleRun = () => {
    if (!credential) return
    setResult(null)
    setError(null)
    testCredential.mutate(
      {
        id: credential.id,
        request: { model, prompt },
      },
      {
        onSuccess: data => {
          setResult(data)
        },
        onError: err => {
          setError(extractErrorMessage(err))
        },
      }
    )
  }

  const pending = testCredential.isPending
  const responseText = result?.responseText?.trim() || ''

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>测试账号连接</DialogTitle>
        </DialogHeader>

        {credential ? (
          <div className="space-y-4">
            <div className="rounded-md border bg-muted/30 p-3">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <div className="font-medium">{credentialName(credential)}</div>
                  <div className="mt-1 text-xs text-muted-foreground">
                    #{credential.id} · {credential.endpoint} · {credential.hasProxy ? '代理' : '直连'}
                  </div>
                </div>
                <div className="flex flex-wrap gap-2">
                  <Badge variant={credential.disabled ? 'outline' : 'success'}>
                    {credential.disabled ? '禁用' : '启用'}
                  </Badge>
                  <Badge variant={credential.availableForDispatch ? 'secondary' : 'warning'}>
                    {credential.availableForDispatch ? '可调度' : '不可调度'}
                  </Badge>
                </div>
              </div>
            </div>

            <div className="grid gap-3 md:grid-cols-[1fr_1fr]">
              <div className="space-y-2">
                <label className="text-sm font-medium">测试模型</label>
                <select
                  value={model}
                  onChange={event => setModel(event.target.value)}
                  className="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
                  disabled={pending}
                >
                  {TEST_MODELS.map(item => (
                    <option key={item.value} value={item.value}>{item.label}</option>
                  ))}
                </select>
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">测试消息</label>
                <Input
                  value={prompt}
                  onChange={event => setPrompt(event.target.value)}
                  disabled={pending}
                  placeholder="hi"
                />
              </div>
            </div>

            <div className="rounded-md border bg-card p-3">
              <div className="mb-2 flex items-center justify-between gap-2">
                <div className="text-sm font-medium">测试结果</div>
                {pending && <Badge variant="secondary">请求中</Badge>}
                {result && <Badge variant="success">完成</Badge>}
                {error && <Badge variant="destructive">失败</Badge>}
              </div>

              {!pending && !result && !error && (
                <div className="text-sm text-muted-foreground">
                  待测试 · {selectedModelLabel}
                </div>
              )}

              {pending && (
                <div className="flex items-center gap-2 text-sm text-muted-foreground">
                  <RefreshCw className="h-4 w-4 animate-spin" />
                  正在等待上游响应...
                </div>
              )}

              {error && (
                <div className="flex items-start gap-2 text-sm text-destructive">
                  <XCircle className="mt-0.5 h-4 w-4" />
                  <span>{error}</span>
                </div>
              )}

              {result && (
                <div className="space-y-3">
                  <div className="grid gap-2 text-sm md:grid-cols-2">
                    <div className="flex items-center gap-2">
                      <CheckCircle2 className="h-4 w-4 text-green-600" />
                      <span>HTTP {result.status}</span>
                    </div>
                    <div className="text-muted-foreground">耗时 {formatMs(result.latencyMs)}</div>
                    <div className="text-muted-foreground">模型 {result.model}</div>
                    <div className="text-muted-foreground">端点 {result.endpoint} / {result.apiRegion}</div>
                    <div className="md:col-span-2 text-muted-foreground">Prompt: {result.prompt}</div>
                  </div>
                  <pre className="max-h-64 overflow-auto rounded-md bg-muted p-3 whitespace-pre-wrap break-words text-sm leading-6">
                    {responseText || '未返回文本'}
                  </pre>
                </div>
              )}
            </div>
          </div>
        ) : (
          <div className="py-8 text-center text-sm text-muted-foreground">未选择账号</div>
        )}

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            关闭
          </Button>
          <Button onClick={handleRun} disabled={!credential || pending || !prompt.trim()}>
            {pending ? <RefreshCw className="h-4 w-4 animate-spin" /> : <Play className="h-4 w-4" />}
            {result || error ? '重新测试' : '开始测试'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

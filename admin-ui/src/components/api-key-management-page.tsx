import { useMemo, useState } from 'react'
import { Check, Copy, Eye, EyeOff, KeyRound, Plus, RefreshCw, Save, Trash2 } from 'lucide-react'
import { toast } from 'sonner'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { useApiKeys, useCreateApiKey, useDeleteApiKey, useUpdateApiKey } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { ApiKeyItem } from '@/types/api'

function formatDateTime(value: string | null | undefined): string {
  if (!value) return '-'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString('zh-CN', {
    hour12: false,
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

function maskApiKey(key: string): string {
  if (key.length <= 14) return key
  return `${key.slice(0, 7)}${'•'.repeat(18)}${key.slice(-6)}`
}

async function copyToClipboard(value: string) {
  await navigator.clipboard.writeText(value)
}

export function ApiKeyManagementPage() {
  const { data, isLoading, error, refetch } = useApiKeys()
  const { mutate: createApiKey, isPending: creating } = useCreateApiKey()
  const { mutate: updateApiKey, isPending: updating } = useUpdateApiKey()
  const { mutate: deleteApiKey, isPending: deleting } = useDeleteApiKey()
  const [createOpen, setCreateOpen] = useState(false)
  const [newKeyName, setNewKeyName] = useState('')
  const [editingNames, setEditingNames] = useState<Record<number, string>>({})
  const [visibleKeys, setVisibleKeys] = useState<Set<number>>(new Set())
  const [copiedIds, setCopiedIds] = useState<Set<number>>(new Set())

  const keys = data?.keys || []
  const enabledCount = useMemo(() => keys.filter(key => !key.disabled).length, [keys])
  const disabledCount = keys.length - enabledCount

  const toggleVisible = (id: number) => {
    setVisibleKeys(prev => {
      const next = new Set(prev)
      if (next.has(id)) {
        next.delete(id)
      } else {
        next.add(id)
      }
      return next
    })
  }

  const handleCopy = async (item: ApiKeyItem) => {
    try {
      await copyToClipboard(item.key)
      setCopiedIds(prev => new Set(prev).add(item.id))
      window.setTimeout(() => {
        setCopiedIds(prev => {
          const next = new Set(prev)
          next.delete(item.id)
          return next
        })
      }, 1500)
      toast.success('密钥已复制')
    } catch (copyError) {
      toast.error(`复制失败: ${extractErrorMessage(copyError)}`)
    }
  }

  const handleCreate = () => {
    createApiKey(
      { name: newKeyName },
      {
        onSuccess: item => {
          setCreateOpen(false)
          setNewKeyName('')
          setVisibleKeys(prev => new Set(prev).add(item.id))
          toast.success('密钥已生成')
        },
        onError: createError => toast.error(`生成失败: ${extractErrorMessage(createError)}`),
      }
    )
  }

  const handleSaveName = (item: ApiKeyItem) => {
    const name = editingNames[item.id] ?? item.name
    updateApiKey(
      { id: item.id, request: { name } },
      {
        onSuccess: () => {
          setEditingNames(prev => {
            const next = { ...prev }
            delete next[item.id]
            return next
          })
          toast.success('名称已保存')
        },
        onError: updateError => toast.error(`保存失败: ${extractErrorMessage(updateError)}`),
      }
    )
  }

  const handleToggleDisabled = (item: ApiKeyItem, disabled: boolean) => {
    updateApiKey(
      { id: item.id, request: { disabled } },
      {
        onSuccess: () => toast.success(disabled ? '密钥已禁用' : '密钥已启用'),
        onError: updateError => toast.error(`操作失败: ${extractErrorMessage(updateError)}`),
      }
    )
  }

  const handleDelete = (item: ApiKeyItem) => {
    if (!confirm(`确定要删除密钥「${item.name}」吗？此操作无法撤销。`)) return
    deleteApiKey(item.id, {
      onSuccess: () => toast.success('密钥已删除'),
      onError: deleteError => toast.error(`删除失败: ${extractErrorMessage(deleteError)}`),
    })
  }

  if (isLoading) {
    return (
      <div className="flex min-h-[360px] items-center justify-center">
        <div className="text-center">
          <div className="mx-auto mb-4 h-10 w-10 animate-spin rounded-full border-b-2 border-primary" />
          <p className="text-sm text-muted-foreground">加载密钥...</p>
        </div>
      </div>
    )
  }

  if (error) {
    return (
      <Card className="mx-auto max-w-md">
        <CardContent className="pt-6 text-center">
          <div className="mb-2 font-medium text-destructive">密钥列表加载失败</div>
          <p className="mb-4 text-sm text-muted-foreground">{(error as Error).message}</p>
          <Button onClick={() => refetch()}>
            <RefreshCw className="h-4 w-4" />
            重试
          </Button>
        </CardContent>
      </Card>
    )
  }

  return (
    <div className="space-y-4">
      <div className="grid gap-3 md:grid-cols-3">
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">密钥总数</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold tabular-nums">{keys.length}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">启用中</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold tabular-nums">{enabledCount}</div>
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm font-medium text-muted-foreground">已禁用</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="text-2xl font-bold tabular-nums">{disabledCount}</div>
          </CardContent>
        </Card>
      </div>

      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2 text-sm font-medium">
          <KeyRound className="h-4 w-4" />
          <span>外部访问密钥</span>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="sm" onClick={() => refetch()}>
            <RefreshCw className="h-4 w-4" />
            刷新
          </Button>
          <Button size="sm" onClick={() => setCreateOpen(true)}>
            <Plus className="h-4 w-4" />
            生成密钥
          </Button>
        </div>
      </div>

      <div className="overflow-x-auto rounded-lg border bg-card">
        <table className="w-full min-w-[920px] text-sm">
          <thead className="border-b bg-muted/50 text-xs text-muted-foreground">
            <tr>
              <th className="px-3 py-2 text-left font-medium">名称/备注</th>
              <th className="px-3 py-2 text-left font-medium">密钥</th>
              <th className="px-3 py-2 text-left font-medium">状态</th>
              <th className="px-3 py-2 text-left font-medium">最近使用</th>
              <th className="px-3 py-2 text-left font-medium">创建时间</th>
              <th className="px-3 py-2 text-right font-medium">操作</th>
            </tr>
          </thead>
          <tbody>
            {keys.length === 0 ? (
              <tr>
                <td colSpan={6} className="px-3 py-14 text-center text-muted-foreground">
                  暂无外部访问密钥
                </td>
              </tr>
            ) : (
              keys.map(item => {
                const visible = visibleKeys.has(item.id)
                const nameValue = editingNames[item.id] ?? item.name
                const nameChanged = nameValue !== item.name
                const copied = copiedIds.has(item.id)
                return (
                  <tr key={item.id} className="border-b last:border-0 hover:bg-muted/30">
                    <td className="px-3 py-2">
                      <div className="flex min-w-[220px] items-center gap-2">
                        <Input
                          value={nameValue}
                          onChange={event => setEditingNames(prev => ({ ...prev, [item.id]: event.target.value }))}
                          className="h-9"
                          placeholder="未命名密钥"
                        />
                        <Button
                          variant="outline"
                          size="icon"
                          className="h-9 w-9"
                          onClick={() => handleSaveName(item)}
                          disabled={!nameChanged || updating}
                          title="保存名称"
                          aria-label="保存名称"
                        >
                          <Save className="h-4 w-4" />
                        </Button>
                      </div>
                    </td>
                    <td className="px-3 py-2">
                      <div className="flex min-w-[360px] items-center gap-2">
                        <code className="min-w-0 flex-1 truncate rounded border bg-muted/40 px-2 py-1.5 font-mono text-xs">
                          {visible ? item.key : maskApiKey(item.key)}
                        </code>
                        <Button
                          variant="outline"
                          size="icon"
                          className="h-9 w-9"
                          onClick={() => toggleVisible(item.id)}
                          title={visible ? '隐藏密钥' : '查看密钥'}
                          aria-label={visible ? '隐藏密钥' : '查看密钥'}
                        >
                          {visible ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                        </Button>
                        <Button
                          variant="outline"
                          size="icon"
                          className="h-9 w-9"
                          onClick={() => void handleCopy(item)}
                          title="复制密钥"
                          aria-label="复制密钥"
                        >
                          {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
                        </Button>
                      </div>
                    </td>
                    <td className="px-3 py-2">
                      <div className="flex items-center gap-2">
                        <Switch
                          checked={!item.disabled}
                          onCheckedChange={checked => handleToggleDisabled(item, !checked)}
                          disabled={updating}
                          aria-label={item.disabled ? '启用密钥' : '禁用密钥'}
                        />
                        <Badge variant={item.disabled ? 'secondary' : 'success'}>
                          {item.disabled ? '已禁用' : '启用'}
                        </Badge>
                      </div>
                    </td>
                    <td className="px-3 py-2 text-muted-foreground">{formatDateTime(item.lastUsedAt)}</td>
                    <td className="px-3 py-2 text-muted-foreground">{formatDateTime(item.createdAt)}</td>
                    <td className="px-3 py-2 text-right">
                      <Button
                        variant="destructive"
                        size="icon"
                        className="h-9 w-9"
                        onClick={() => handleDelete(item)}
                        disabled={deleting}
                        title="删除密钥"
                        aria-label="删除密钥"
                      >
                        <Trash2 className="h-4 w-4" />
                      </Button>
                    </td>
                  </tr>
                )
              })
            )}
          </tbody>
        </table>
      </div>

      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>生成外部访问密钥</DialogTitle>
          </DialogHeader>
          <div className="space-y-2">
            <label htmlFor="api-key-name" className="text-sm font-medium">
              名称/备注
            </label>
            <Input
              id="api-key-name"
              value={newKeyName}
              onChange={event => setNewKeyName(event.target.value)}
              placeholder="例如：张三 Cursor / 生产网关"
            />
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setCreateOpen(false)}>
              取消
            </Button>
            <Button onClick={handleCreate} disabled={creating}>
              <KeyRound className="h-4 w-4" />
              {creating ? '生成中...' : '生成密钥'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}

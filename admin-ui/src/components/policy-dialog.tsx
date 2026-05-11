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
import { useSetCredentialPolicy, useSetCredentialPolicyBatch } from '@/hooks/use-credentials'
import { extractErrorMessage } from '@/lib/utils'
import type { CredentialStatusItem } from '@/types/api'

interface PolicyDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  credential?: CredentialStatusItem | null
  selectedIds?: number[]
}

export function PolicyDialog({ open, onOpenChange, credential, selectedIds = [] }: PolicyDialogProps) {
  const isBatch = selectedIds.length > 0 && !credential
  const setPolicy = useSetCredentialPolicy()
  const setPolicyBatch = useSetCredentialPolicyBatch()
  const [maxConcurrent, setMaxConcurrent] = useState('')
  const [rpm, setRpm] = useState('')

  useEffect(() => {
    if (!open) return
    setMaxConcurrent(credential?.maxConcurrentOverride?.toString() ?? '')
    setRpm(credential?.rpmOverride?.toString() ?? '')
  }, [credential, open])

  const parseOptionalNumber = (value: string) => {
    const trimmed = value.trim()
    if (!trimmed) return null
    const number = Number(trimmed)
    return Number.isFinite(number) ? number : null
  }

  const handleSave = () => {
    const policy = {
      maxConcurrentOverride: parseOptionalNumber(maxConcurrent),
      rpmOverride: parseOptionalNumber(rpm),
    }

    if (isBatch) {
      setPolicyBatch.mutate(
        { ids: selectedIds, ...policy },
        {
          onSuccess: () => {
            toast.success('批量策略已更新')
            onOpenChange(false)
          },
          onError: error => {
            toast.error(`保存失败: ${extractErrorMessage(error)}`)
          },
        }
      )
    } else if (credential) {
      setPolicy.mutate(
        { id: credential.id, policy },
        {
          onSuccess: () => {
            toast.success('账号策略已更新')
            onOpenChange(false)
          },
          onError: error => {
            toast.error(`保存失败: ${extractErrorMessage(error)}`)
          },
        }
      )
    }
  }

  const pending = setPolicy.isPending || setPolicyBatch.isPending

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{isBatch ? `批量调整 ${selectedIds.length} 个账号` : '账号调度策略'}</DialogTitle>
          <DialogDescription>
            留空表示使用全局默认值；RPM 为 0 表示不限速。
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {!isBatch && credential && (
            <div className="rounded-md border bg-muted/30 p-3 text-sm">
              <div className="font-medium">{credential.email || `#${credential.id}`}</div>
              <div className="mt-1 text-muted-foreground">
                当前生效：并发 {credential.maxConcurrent}，RPM {credential.effectiveRpm || '不限'}
              </div>
            </div>
          )}

          <div className="space-y-2">
            <label className="text-sm font-medium">账号并发覆盖</label>
            <Input
              type="number"
              min={1}
              max={64}
              value={maxConcurrent}
              placeholder="使用全局默认"
              onChange={event => setMaxConcurrent(event.target.value)}
            />
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium">账号 RPM 覆盖</label>
            <Input
              type="number"
              min={0}
              max={10000}
              value={rpm}
              placeholder="使用全局默认"
              onChange={event => setRpm(event.target.value)}
            />
          </div>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button onClick={handleSave} disabled={pending || (!credential && !isBatch)}>
            {pending ? '保存中...' : '保存'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

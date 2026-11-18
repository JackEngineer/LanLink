import { useMutation, useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { api } from './api'

const restartTimeout = 60_000
const healthTimeout = 2_000
const healthInterval = 750

export async function waitForUpgrade(version: string) {
  const deadline = Date.now() + restartTimeout
  while (Date.now() < deadline) {
    const health = await api
      .health(AbortSignal.timeout(healthTimeout))
      .catch(() => undefined)
    if (health?.version === version) return
    if (health?.update_failed_version === version) {
      throw new Error('升级失败，已恢复到原版本。')
    }

    await new Promise((resolve) => setTimeout(resolve, healthInterval))
  }
  throw new Error('暂时无法确认升级结果。LANLink 恢复连接后可重试。')
}

export function useAppUpdate(version: string | undefined, enabled: boolean) {
  const [restarting, setRestarting] = useState(false)
  const check = useQuery({
    queryKey: ['software-update', version],
    queryFn: ({ signal }) => api.checkUpdate(signal),
    enabled: enabled && Boolean(version),
    retry: false,
    refetchOnWindowFocus: false,
  })
  const apply = useMutation({
    mutationFn: async (version: string) => {
      const update = await api.applyUpdate(version)
      setRestarting(true)
      try {
        await waitForUpgrade(update.version)
        window.location.reload()
      } finally {
        setRestarting(false)
      }
    },
    onError: () => {
      void check.refetch()
    },
  })

  return {
    update: check.data,
    checking: check.isFetching,
    checkError: check.error,
    retryCheck: check.refetch,
    installing: apply.isPending,
    restarting,
    installError: apply.error,
    install: apply.mutate,
  }
}

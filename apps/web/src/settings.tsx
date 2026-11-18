import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Check, Copy, FolderDown, RefreshCw } from 'lucide-react'
import { useState } from 'react'
import { api } from './api'
import { DeviceIcon, InlineError } from './components'
import {
  MAX_FILE_BYTES,
  MEBIBYTE,
  formatBytes,
  platformName,
  type Device,
  type Settings,
} from './lanlink'
import type { useAppUpdate } from './use-app-update'
import { agentKeys } from './use-lanlink'

export function SettingsPage({
  device,
  settings,
  version,
  appUpdate,
  loading,
  agentAvailable,
  error,
}: {
  device: Device | undefined
  settings: Settings | undefined
  version: string | undefined
  appUpdate: ReturnType<typeof useAppUpdate>
  loading: boolean
  agentAvailable: boolean
  error: Error | null
}) {
  return (
    <section className="content-scroll" aria-label="LANLink 设置">
      <div className="content-inner settings">
        <InlineError error={error} />
        {loading && <p className="loading">正在读取设置…</p>}
        {device && (
          <div className="settings-group">
            <h2>
              <DeviceIcon platform={device.platform} /> 此设备
            </h2>
            <div className="settings-card">
              <DeviceName device={device} disabled={!agentAvailable} />
              <div className="settings-row">
                <span className="setting-label">系统</span>
                <span className="setting-value">
                  {platformName(device.platform)}
                </span>
              </div>
            </div>
          </div>
        )}
        {settings && (
          <div className="settings-group">
            <h2>
              <FolderDown size={19} aria-hidden="true" /> 文件接收
            </h2>
            <div className="settings-card">
              <FileLimit settings={settings} disabled={!agentAvailable} />
              <DownloadDirectory path={settings.download_dir} />
            </div>
          </div>
        )}
        {version && (
          <AppUpdates
            version={version}
            disabled={!agentAvailable}
            {...appUpdate}
          />
        )}
      </div>
    </section>
  )
}

function AppUpdates({
  version,
  disabled,
  update,
  checking,
  checkError,
  retryCheck,
  installing,
  restarting,
  installError,
  install,
}: ReturnType<typeof useAppUpdate> & { version: string; disabled: boolean }) {
  const available = update?.status === 'available' ? update.version : null
  let message = ''
  if (restarting) message = '正在重启 LANLink…'
  else if (installing) message = '正在下载并校验更新…'
  else if (checking) message = '正在检查更新…'
  else if (update?.status === 'current') message = '当前版本已是最新。'
  else if (update?.status === 'unsupported')
    message = '当前安装暂不支持一键升级。'
  else if (available) message = `发现新版本 ${available}。`

  return (
    <div className="settings-group">
      <h2>
        <RefreshCw size={19} aria-hidden="true" /> 软件更新
      </h2>
      <div className="settings-card">
        <div className="settings-row">
          <span className="setting-label">当前版本</span>
          <div className="setting-field">
            <div className="field-controls">
              <span className="setting-value">LANLink {version}</span>
              {available && (
                <button
                  className="button button-primary"
                  type="button"
                  disabled={disabled || checking || installing}
                  onClick={() => install(available)}
                >
                  一键升级
                </button>
              )}
              {!available && checkError && (
                <button
                  className="button"
                  type="button"
                  disabled={disabled || checking || installing}
                  onClick={() => void retryCheck()}
                >
                  重试
                </button>
              )}
            </div>
            <div className="setting-hint update-status" role="status">
              {message}
            </div>
            <InlineError error={installError ?? checkError} />
          </div>
        </div>
      </div>
    </div>
  )
}

function DeviceName({
  device,
  disabled,
}: {
  device: Device
  disabled: boolean
}) {
  const client = useQueryClient()
  const [draft, setDraft] = useState<string | null>(null)
  const value = draft ?? device.name
  const name = value.trim()
  const invalid = name.length === 0 || Array.from(name).length > 80
  const save = useMutation({
    mutationFn: () => api.rename(name),
    onSuccess: (updated) => {
      client.setQueryData(agentKeys.device, updated)
      setDraft(null)
    },
  })

  return (
    <form
      className="settings-row"
      onSubmit={(event) => {
        event.preventDefault()
        if (!invalid && name !== device.name && !disabled && !save.isPending)
          save.mutate()
      }}
    >
      <label className="setting-label" htmlFor="device-name">
        设备名称
      </label>
      <div className="setting-field">
        <div className="field-controls">
          <input
            id="device-name"
            type="text"
            value={value}
            disabled={disabled || save.isPending}
            aria-invalid={invalid || undefined}
            aria-describedby={invalid ? 'device-name-error' : undefined}
            onChange={(event) => setDraft(event.target.value)}
          />
          <button
            className="button"
            type="submit"
            disabled={
              invalid || name === device.name || disabled || save.isPending
            }
          >
            保存
          </button>
        </div>
        {invalid && (
          <p className="file-error" id="device-name-error">
            请输入 1 至 80 个字符。
          </p>
        )}
        <InlineError error={draft !== null ? save.error : null} />
        {save.isSuccess && draft === null && (
          <p className="saved" role="status">
            <Check size={15} aria-hidden="true" /> 已保存
          </p>
        )}
      </div>
    </form>
  )
}

function FileLimit({
  settings,
  disabled,
}: {
  settings: Settings
  disabled: boolean
}) {
  const client = useQueryClient()
  const [draft, setDraft] = useState<string | null>(null)
  const value = draft ?? String(settings.max_file_size / MEBIBYTE)
  const bytes = Math.round(Number(value) * MEBIBYTE)
  const invalid =
    !Number.isSafeInteger(bytes) || bytes <= 0 || bytes > MAX_FILE_BYTES
  const save = useMutation({
    mutationFn: () => api.saveSettings(bytes),
    onSuccess: (updated) => {
      client.setQueryData(agentKeys.settings, updated)
      setDraft(null)
    },
  })

  return (
    <form
      className="settings-row"
      onSubmit={(event) => {
        event.preventDefault()
        if (
          !invalid &&
          bytes !== settings.max_file_size &&
          !disabled &&
          !save.isPending
        )
          save.mutate()
      }}
    >
      <label className="setting-label" htmlFor="file-limit">
        单文件大小上限
      </label>
      <div className="setting-field">
        <div className="field-controls">
          <div className="number-field">
            <input
              id="file-limit"
              type="number"
              min={1 / MEBIBYTE}
              max={MAX_FILE_BYTES / MEBIBYTE}
              step="any"
              value={value}
              disabled={disabled || save.isPending}
              aria-invalid={invalid || undefined}
              aria-describedby="file-limit-hint"
              onChange={(event) => setDraft(event.target.value)}
            />
            <span>MiB</span>
          </div>
          <button
            className="button"
            type="submit"
            disabled={
              invalid ||
              bytes === settings.max_file_size ||
              disabled ||
              save.isPending
            }
          >
            保存
          </button>
        </div>
        <p
          className={invalid ? 'file-error' : 'setting-hint'}
          id="file-limit-hint"
        >
          最大可设为 {formatBytes(MAX_FILE_BYTES)}。
        </p>
        <InlineError error={draft !== null ? save.error : null} />
        {save.isSuccess && draft === null && (
          <p className="saved" role="status">
            <Check size={15} aria-hidden="true" /> 已保存
          </p>
        )}
      </div>
    </form>
  )
}

function DownloadDirectory({ path }: { path: string }) {
  const [result, setResult] = useState<'copied' | 'error' | null>(null)

  async function copyPath() {
    try {
      await navigator.clipboard.writeText(path)
      setResult('copied')
    } catch {
      setResult('error')
    }
  }

  return (
    <div className="settings-row">
      <span className="setting-label">接收文件目录</span>
      <div className="setting-field">
        <div className="field-controls">
          <code className="download-path">{path}</code>
          <button
            className="icon-button"
            type="button"
            aria-label="复制目录路径"
            title="复制目录路径"
            onClick={() => void copyPath()}
          >
            {result === 'copied' ? (
              <Check size={18} aria-hidden="true" />
            ) : (
              <Copy size={17} aria-hidden="true" />
            )}
          </button>
        </div>
        {result && (
          <p
            className={result === 'copied' ? 'saved' : 'file-error'}
            role="status"
          >
            {result === 'copied'
              ? '路径已复制'
              : '无法访问剪贴板，请选中路径并复制。'}
          </p>
        )}
      </div>
    </div>
  )
}

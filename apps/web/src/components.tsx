import { CircleAlert, Laptop, Monitor, X } from 'lucide-react'
import { type ReactNode, useEffect, useId, useRef } from 'react'
import { errorMessage } from './lanlink'

export function DeviceIcon({
  platform,
  size = 18,
}: {
  platform?: string
  size?: number
}) {
  const Icon = platform?.toLowerCase() === 'windows' ? Monitor : Laptop
  return <Icon size={size} aria-hidden="true" />
}

export function EmptyState({
  icon,
  title,
  children,
}: {
  icon: ReactNode
  title: string
  children?: ReactNode
}) {
  return (
    <div className="empty-state">
      <div className="empty-icon" aria-hidden="true">
        {icon}
      </div>
      <h2>{title}</h2>
      {children && <div className="empty-body">{children}</div>}
    </div>
  )
}

export function InlineError({ error }: { error: unknown }) {
  if (!error) return null
  return (
    <p className="inline-error" role="alert">
      <CircleAlert size={16} aria-hidden="true" />
      {errorMessage(error)}
    </p>
  )
}

export function Modal({
  title,
  children,
  onClose,
}: {
  title: string
  children: ReactNode
  onClose: () => void
}) {
  const id = useId()
  const dialog = useRef<HTMLDialogElement>(null)

  useEffect(() => {
    const element = dialog.current
    if (element && !element.open) element.showModal()
  }, [])

  return (
    <dialog
      ref={dialog}
      className="modal"
      aria-labelledby={id}
      onCancel={(event) => {
        event.preventDefault()
        onClose()
      }}
    >
      <div className="modal-title">
        <h2 id={id}>{title}</h2>
        <button
          className="icon-button"
          type="button"
          aria-label="关闭对话框"
          onClick={onClose}
        >
          <X size={19} aria-hidden="true" />
        </button>
      </div>
      {children}
    </dialog>
  )
}

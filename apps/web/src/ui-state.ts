import { create } from 'zustand'

export type View = 'chat' | 'transfers' | 'settings'

interface Upload {
  id: string
  peerId: string
  file: File
  progress: number
  status: 'uploading' | 'failed'
  error?: string
}

interface UiState {
  view: View
  peerId: string | null
  sidebarOpen: boolean
  pairingId: string | null
  drafts: Record<string, string>
  uploads: Upload[]
  setView: (view: View) => void
  selectPeer: (id: string) => void
  setSidebarOpen: (open: boolean) => void
  setPairingId: (id: string | null) => void
  setDraft: (id: string, value: string) => void
  putUpload: (upload: Upload) => void
  updateUpload: (
    id: string,
    update: Partial<Pick<Upload, 'progress' | 'status' | 'error'>>,
  ) => void
  removeUpload: (id: string) => void
}

export const useUi = create<UiState>((set) => ({
  view: 'chat',
  peerId: null,
  sidebarOpen: false,
  pairingId: null,
  drafts: {},
  uploads: [],
  setView: (view) => set({ view, sidebarOpen: false }),
  selectPeer: (peerId) => set({ peerId, view: 'chat', sidebarOpen: false }),
  setSidebarOpen: (sidebarOpen) => set({ sidebarOpen }),
  setPairingId: (pairingId) => set({ pairingId }),
  setDraft: (id, value) =>
    set((state) => {
      const drafts = { ...state.drafts }
      if (value) drafts[id] = value
      else delete drafts[id]
      return { drafts }
    }),
  putUpload: (upload) =>
    set((state) => ({
      uploads: [...state.uploads.filter(({ id }) => id !== upload.id), upload],
    })),
  updateUpload: (id, update) =>
    set((state) => ({
      uploads: state.uploads.map((upload) =>
        upload.id === id ? { ...upload, ...update } : upload,
      ),
    })),
  removeUpload: (id) =>
    set((state) => ({
      uploads: state.uploads.filter((upload) => upload.id !== id),
    })),
}))

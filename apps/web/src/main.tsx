import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './app'
import './style.css'

const root = document.getElementById('root')
if (!root) throw new Error('LANLink root element is missing.')

const client = new QueryClient({
  defaultOptions: {
    queries: { networkMode: 'always', retry: false },
    mutations: { networkMode: 'always' },
  },
})

createRoot(root).render(
  <StrictMode>
    <QueryClientProvider client={client}>
      <App />
    </QueryClientProvider>
  </StrictMode>,
)

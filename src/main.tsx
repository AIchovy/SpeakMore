import React from 'react'
import ReactDOM from 'react-dom/client'
import { HashRouter } from 'react-router-dom'
import App from './App'
import { AppState } from './lib/app-state'
import './styles.css'
import { lazy, Suspense } from 'react'

const TargetAudit =
  import.meta.env.VITE_TARGET_AUDIT === '1' ? lazy(() => import('./features/target-audit')) : null

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    {TargetAudit ? (
      <Suspense fallback={<p>正在载入原生验证器</p>}>
        <TargetAudit />
      </Suspense>
    ) : (
      <HashRouter>
        <AppState>
          <App />
        </AppState>
      </HashRouter>
    )}
  </React.StrictMode>,
)

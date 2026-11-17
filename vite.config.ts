import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath, URL } from 'node:url'

// Stable vendor chunks shared by the windows. The HUD loads React and the
// small Slot primitive only; Radix components, the router and motion are
// fetched by the windows that use them.
const vendorChunks: [string, RegExp][] = [
  [
    'vendor-react',
    /\/node_modules\/(react|react-dom|scheduler|@radix-ui\/react-(slot|compose-refs))\//,
  ],
  ['vendor-router', /\/node_modules\/(react-router|react-router-dom|cookie|set-cookie-parser)\//],
  ['vendor-tauri', /\/node_modules\/@tauri-apps\/api\//],
  ['vendor-motion', /\/node_modules\/(motion|framer-motion|motion-dom|motion-utils)\//],
  [
    'vendor-radix',
    /\/node_modules\/(@radix-ui|@floating-ui|react-remove-scroll|react-remove-scroll-bar|react-style-singleton|use-callback-ref|use-sidecar|aria-hidden|get-nonce|detect-node-es)\//,
  ],
]

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  clearScreen: false,
  server: { host: '127.0.0.1', port: 1420, strictPort: true },
  build: {
    target: ['es2022', 'chrome111', 'safari16.4'],
    sourcemap: false,
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL('./index.html', import.meta.url)),
        captions: fileURLToPath(new URL('./captions.html', import.meta.url)),
        hud: fileURLToPath(new URL('./hud.html', import.meta.url)),
        meetingHud: fileURLToPath(new URL('./meeting-hud.html', import.meta.url)),
      },
      output: {
        manualChunks(id) {
          const path = id.replaceAll('\\', '/')
          return vendorChunks.find(([, pattern]) => pattern.test(path))?.[0]
        },
      },
    },
  },
})

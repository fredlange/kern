import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// In dev, /api goes to a local `python -m kern.host serve portal.k --port 8080`.
// In the image, nginx does the same rewrite (see ../nginx.conf).
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': { target: 'http://127.0.0.1:8080', rewrite: p => p.replace(/^\/api/, '') },
    },
  },
})

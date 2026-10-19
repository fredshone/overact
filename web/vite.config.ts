import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'

// `base: './'` keeps the build relocatable (GitHub Pages project sites).
// Cross-origin isolation enables SharedArrayBuffer and so the threaded engine.
const isolation = {
  'Cross-Origin-Opener-Policy': 'same-origin',
  'Cross-Origin-Embedder-Policy': 'require-corp',
}

export default defineConfig({
  base: './',
  plugins: [svelte()],
  worker: { format: 'es' },
  build: { target: 'es2022', rollupOptions: { input: { main: 'index.html' } } },
  server: { fs: { allow: ['..'] }, headers: isolation },
  preview: { headers: isolation },
})

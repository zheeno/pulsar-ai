import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  // Tauri serves the webview from a custom protocol; absolute `/assets/...` URLs 404 in release builds.
  base: './',
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
      // @ngx/shared dist is CommonJS. WKWebView (Safari) cannot import named
      // bindings from CJS (`formatCoachSummary is not found`). Point at ESM source.
      '@ngx/shared/coach-format': path.resolve(
        __dirname,
        '../../packages/shared/src/coach-format.ts',
      ),
      '@ngx/shared': path.resolve(__dirname, '../../packages/shared/src/index.ts'),
    },
  },
  optimizeDeps: {
    exclude: ['@ngx/shared'],
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // tauri / cargo write under src-tauri; watching them reloads the UI in a loop.
      ignored: ['**/src-tauri/**', '**/target/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    // macOS 11 WebKit (Safari 14) — es2021 can emit syntax older engines reject.
    target: 'es2020',
    minify: !process.env.TAURI_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
});

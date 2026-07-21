/// <reference types="vitest" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    rollupOptions: {
      output: {
        manualChunks: {
          // Core React runtime — shared by every route; cached after first visit.
          react: ['react', 'react-dom', 'react-router-dom'],
          // Heavy graph deps — only loaded when a run-detail route is visited.
          xyflow: ['@xyflow/react', '@dagrejs/dagre'],
          // Charting — only loaded on Dashboard.
          charts: ['recharts'],
          // Markdown rendering (react-markdown + rehype-highlight + highlight.js)
          // — loaded by the transcript route and by the project Code tab
          // (ProjectCodeTab, itself React.lazy-loaded), isolated from the main entry.
          markdown: ['react-markdown', 'remark-gfm', 'rehype-highlight', 'highlight.js'],
          // CodeMirror 6 — shared by the lazy code/expression editors. Named so
          // the shared core lands in `codemirror-*.js` (never the main entry)
          // instead of an auto-named `index-*` chunk.
          codemirror: [
            '@codemirror/state',
            '@codemirror/view',
            '@codemirror/commands',
            '@codemirror/language',
            '@codemirror/autocomplete',
            '@codemirror/lang-markdown',
            '@codemirror/lang-yaml',
            '@lezer/highlight',
          ],
        },
      },
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:7878',
        changeOrigin: true,
      },
    },
  },
  test: {
    environment: 'node',
    globals: false,
  },
});

import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

const hostApiUrl =
  process.env.WORKFLOW_API_URL?.trim() ||
  `http://${process.env.WORKFLOW_API_HOST?.trim() || '127.0.0.1'}:${
    process.env.WORKFLOW_API_PORT?.trim() || '8788'
  }`;

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': {
        target: hostApiUrl,
        changeOrigin: true,
      },
    },
  },
});
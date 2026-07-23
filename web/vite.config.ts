import react from '@vitejs/plugin-react';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig, loadEnv } from 'vite';

const webDirectory = path.dirname(fileURLToPath(import.meta.url));
const repoDirectory = path.resolve(webDirectory, '..');

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, repoDirectory, '');

  const apiHost =
    env.WORKFLOW_API_HOST?.trim() ||
    process.env.WORKFLOW_API_HOST?.trim() ||
    '127.0.0.1';

  const apiPort =
    env.WORKFLOW_API_PORT?.trim() ||
    process.env.WORKFLOW_API_PORT?.trim() ||
    '8788';

  const hostApiUrl =
    env.WORKFLOW_API_URL?.trim() ||
    process.env.WORKFLOW_API_URL?.trim() ||
    `http://${apiHost}:${apiPort}`;

  const apiProxy = {
    target: hostApiUrl,
    changeOrigin: true,
  };

  return {
    envDir: repoDirectory,
    plugins: [react()],
    server: {
      proxy: {
        '/api': apiProxy,
      },
    },
    preview: {
      proxy: {
        '/api': apiProxy,
      },
    },
  };
});

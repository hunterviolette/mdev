import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'node:path';

function requiredEnv(env: Record<string, string>, key: string): string {
  const value = env[key]?.trim();
  if (!value) {
    throw new Error(`${key} is not set`);
  }
  return value;
}

function httpUrl(host: string, port: string): string {
  return `http://${host}:${port}`;
}

export default defineConfig(({ mode }) => {
  const repoRoot = path.resolve(__dirname, '..');
  const env = loadEnv(mode, repoRoot, '');

  const apiHost = requiredEnv(env, 'WORKFLOW_API_HOST');
  const apiPort = requiredEnv(env, 'WORKFLOW_API_PORT');
  const webHost = requiredEnv(env, 'WORKFLOW_WEB_HOST');
  const webPort = requiredEnv(env, 'WORKFLOW_WEB_PORT');
  const apiUrl = httpUrl(apiHost, apiPort);

  return {
    plugins: [react()],
    envDir: repoRoot,
    server: {
      host: webHost,
      port: Number.parseInt(webPort, 10),
      proxy: {
        '/api': apiUrl,
        '/events': apiUrl,
        '/review': apiUrl,
        '/runs': apiUrl,
        '/settings': apiUrl,
        '/templates': apiUrl,
        '/workflow-builder': apiUrl,
        '/capabilities': apiUrl,
        '/filesystem': apiUrl,
        '/repo-tree': apiUrl,
        '/sap': apiUrl
      }
    }
  };
});

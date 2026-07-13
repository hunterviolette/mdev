import React from 'react';
import ReactDOM from 'react-dom/client';
import { MantineProvider } from '@mantine/core';
import '@mantine/core/styles.css';
import '@xyflow/react/dist/style.css';
import App from './App';

const configuredApiBase = String(import.meta.env.VITE_API_BASE_URL ?? '')
  .trim()
  .replace(/\/$/, '');
const isQaProxyHost = window.location.hostname.endsWith('.qa.localhost');

if (!isQaProxyHost && configuredApiBase) {
  const nativeFetch = window.fetch.bind(window);

  window.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
    const rawUrl =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.toString()
          : input.url;

    if (rawUrl === '/api' || rawUrl.startsWith('/api/')) {
      const target = `${configuredApiBase}${rawUrl.slice('/api'.length)}`;

      if (input instanceof Request) {
        return nativeFetch(new Request(target, input), init);
      }

      return nativeFetch(target, init);
    }

    return nativeFetch(input, init);
  };
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <MantineProvider defaultColorScheme="dark">
      <App />
    </MantineProvider>
  </React.StrictMode>
);

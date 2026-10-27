import React from 'react';
import ReactDOM from 'react-dom/client';
import { MantineProvider, createTheme } from '@mantine/core';
import '@mantine/core/styles.css';
import '@xyflow/react/dist/style.css';
import './responsive.css';
import App from './App';

const theme = createTheme({
  primaryColor: 'blue',
  defaultRadius: 'md',
  spacing: {
    xs: '0.375rem',
    sm: '0.5rem',
    md: '0.75rem',
    lg: '1rem',
    xl: '1.5rem',
  },
  breakpoints: {
    xs: '30em',
    sm: '48em',
    md: '64em',
    lg: '80em',
    xl: '120em',
  },
  components: {
    Paper: {
      defaultProps: { radius: 'md' },
    },
    Card: {
      defaultProps: { radius: 'md' },
    },
    Button: {
      defaultProps: { radius: 'sm' },
    },
  },
});

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

function renderApp() {
  ReactDOM.createRoot(document.getElementById('root')!).render(
    <React.StrictMode>
      <MantineProvider theme={theme} defaultColorScheme="dark">
        <App />
      </MantineProvider>
    </React.StrictMode>
  );
}

async function bootstrap() {
  try {
    const response = await fetch('/api/auth/me', {
      headers: { Accept: 'application/json' },
    });

    if (response.status === 401) {
      window.location.assign('/api/auth/login');
      return;
    }
  } catch {
  }

  renderApp();
}

void bootstrap();

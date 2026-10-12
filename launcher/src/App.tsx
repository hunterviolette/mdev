import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import LogTerminal, { type LogSeverity, type LogTerminalHandle } from './LogTerminal';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';

type Status = { running: boolean; pid: number | null; url: string };

export default function App() {
  const [status, setStatus] = useState<Status>({ running: false, pid: null, url: '' });
  const terminalRef = useRef<LogTerminalHandle>(null);
  const [logsExpanded, setLogsExpanded] = useState(false);
  const [logSeverity, setLogSeverity] = useState<LogSeverity>('ALL');
  const [update, setUpdate] = useState<Update | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const refresh = useCallback(async () => {
    setStatus(await invoke<Status>('api_status'));
  }, []);

  const run = useCallback(async (command: string) => {
    setBusy(true);
    setError('');
    try {
      await invoke(command);
      await refresh();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const checkUpdates = useCallback(async () => {
    setError('');
    setBusy(true);
    try {
      setUpdate(await check());
    } catch (cause) {
      setUpdate(null);
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }, []);

  const installUpdate = async () => {
    if (!update || status.running) return;
    setBusy(true);
    setError('');
    try {
      await update.downloadAndInstall();
      await relaunch();
    } catch (cause) {
      setError(String(cause));
      setBusy(false);
    }
  };

  useEffect(() => {
    void refresh();
    void checkUpdates();
    const interval = window.setInterval(() => void refresh(), 2000);
    return () => {
      window.clearInterval(interval);
    };
  }, [refresh, checkUpdates]);

  return (
    <main className={logsExpanded ? 'logs-expanded' : ''}>
      <header><div><h1>mdev</h1><p>Application Controller</p></div><span className={status.running ? 'online' : 'offline'}>{status.running ? 'Running' : 'Stopped'}</span></header>
      {update && <section><strong>Update available: {update.version}</strong><p>{update.body || 'A newer version of mdev is available.'}</p><button disabled={busy || status.running} onClick={() => void installUpdate()}>Install update</button>{status.running && <small>Stop the API before updating.</small>}</section>}
      <section><div className="line"><strong>API Server</strong><span>{status.url}</span></div><div className="actions"><button disabled={busy || status.running} onClick={() => void run('api_start')}>Start</button><button disabled={busy || !status.running} onClick={() => void run('api_stop')}>Stop</button><button disabled={busy || !status.running} onClick={() => void run('api_restart')}>Restart</button></div><button className="wide" onClick={() => void run('api_open')}>Open mdev</button></section>
      <section className="logs-section">
        <div className="line logs-toolbar">
          <strong>API Logs</strong>
          <div className="log-actions">
            <label className="log-filter-label" htmlFor="log-severity">Severity</label>
            <select id="log-severity" className="log-severity-select" value={logSeverity} onChange={event => setLogSeverity(event.target.value as LogSeverity)}>
              <option value="ALL">All</option>
              <option value="ERROR">Error</option>
              <option value="WARN">Warn+</option>
              <option value="INFO">Info+</option>
              <option value="DEBUG">Debug+</option>
              <option value="TRACE">Trace+</option>
            </select>
            <button onClick={() => terminalRef.current?.follow()} title="Scroll to latest output">Follow</button>
            <button onClick={() => setLogsExpanded(value => !value)} title={logsExpanded ? 'Restore logs' : 'Expand logs'}>{logsExpanded ? 'Restore' : 'Expand'}</button>
            <button onClick={() => terminalRef.current?.clear()} title="Clear retained and displayed logs">Clear</button>
          </div>
        </div>
        <div className="log-terminal-frame">
          <LogTerminal ref={terminalRef} severity={logSeverity} />
        </div>
      </section>
      <footer><button disabled={busy} onClick={() => void checkUpdates()}>{busy ? 'Working...' : 'Check for updates'}</button>{status.pid && <span>PID {status.pid}</span>}</footer>
      {error && <p className="error">{error}</p>}
    </main>
  );
}

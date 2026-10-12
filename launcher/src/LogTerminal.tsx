import { forwardRef, useEffect, useImperativeHandle, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';

export type LogSeverity = 'ALL' | 'ERROR' | 'WARN' | 'INFO' | 'DEBUG' | 'TRACE';
type LogEvent = { stream: string; message: string };
type StoredLog = { message: string; severity: LogSeverity | null; bytes: number };

export type LogTerminalHandle = {
  clear: () => void;
  follow: () => void;
  fit: () => void;
};

type Props = { severity: LogSeverity };

const MAX_EVENTS = 50000;
const MAX_BYTES = 16 * 1024 * 1024;
const ANSI_PATTERN = /\x1b\[[0-?]*[ -/]*[@-~]/g;
const SEVERITY_PATTERN = /^\s*(?:\S+\s+)?(ERROR|WARN|INFO|DEBUG|TRACE)\b/;
const LEVELS: Record<Exclude<LogSeverity, 'ALL'>, number> = {
  ERROR: 1,
  WARN: 2,
  INFO: 3,
  DEBUG: 4,
  TRACE: 5,
};

function parseSeverity(message: string): LogSeverity | null {
  const cleaned = message.replace(ANSI_PATTERN, '');
  const match = cleaned.match(SEVERITY_PATTERN);
  return match ? match[1] as LogSeverity : null;
}

function matchesSeverity(entry: StoredLog, severity: LogSeverity): boolean {
  if (severity === 'ALL') return true;
  if (!entry.severity || entry.severity === 'ALL') return false;
  return LEVELS[entry.severity] <= LEVELS[severity];
}

const LogTerminal = forwardRef<LogTerminalHandle, Props>(function LogTerminal({ severity }, ref) {
  const containerRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const historyRef = useRef<StoredLog[]>([]);
  const bytesRef = useRef(0);
  const severityRef = useRef(severity);
  const renderRef = useRef<() => void>(() => {});
  const queueRef = useRef<string[]>([]);
  const writingRef = useRef(false);
  const generationRef = useRef(0);
  const flushRef = useRef<() => void>(() => {});

  useImperativeHandle(ref, () => ({
    clear: () => {
      historyRef.current = [];
      bytesRef.current = 0;
      renderRef.current();
    },
    follow: () => terminalRef.current?.scrollToBottom(),
    fit: () => fitRef.current?.fit(),
  }), []);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const terminal = new Terminal({
      disableStdin: true,
      cursorBlink: false,
      scrollback: 10000,
      convertEol: true,
      fontFamily: 'Consolas, "Cascadia Code", "JetBrains Mono", monospace',
      fontSize: 12,
      lineHeight: 1.45,
      theme: {
        background: '#0d1118',
        foreground: '#e8edf5',
        cursor: '#0d1118',
        selectionBackground: '#42557488',
        black: '#222936',
        red: '#f87171',
        green: '#6ce5aa',
        yellow: '#facc15',
        blue: '#60a5fa',
        magenta: '#c084fc',
        cyan: '#67e8f9',
        white: '#e8edf5',
        brightBlack: '#64748b',
        brightRed: '#fca5a5',
        brightGreen: '#86efac',
        brightYellow: '#fde047',
        brightBlue: '#93c5fd',
        brightMagenta: '#d8b4fe',
        brightCyan: '#a5f3fc',
        brightWhite: '#ffffff',
      },
    });

    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(container);
    terminalRef.current = terminal;
    fitRef.current = fitAddon;
    fitAddon.fit();

    let active = true;
    let unlisten = () => {};

    const flush = () => {
      if (!active || writingRef.current || queueRef.current.length === 0) return;
      writingRef.current = true;
      const generation = generationRef.current;
      const output = queueRef.current.splice(0, 256).join('');
      const buffer = terminal.buffer.active;
      const following = buffer.viewportY >= buffer.baseY;
      const previousViewport = buffer.viewportY;

      terminal.write(output, () => {
        if (!active) return;
        writingRef.current = false;
        if (generation === generationRef.current) {
          if (following) terminal.scrollToBottom();
          else terminal.scrollToLine(previousViewport);
        }
        flushRef.current();
      });
    };

    flushRef.current = flush;

    renderRef.current = () => {
      generationRef.current++;
      queueRef.current = [];
      terminal.reset();
      for (const entry of historyRef.current) {
        if (matchesSeverity(entry, severityRef.current)) {
          queueRef.current.push(entry.message + '\r\n');
        }
      }
      flushRef.current();
    };

    void listen<LogEvent>('api-log', event => {
      if (!active) return;
      const message = event.payload.message;
      const bytes = new TextEncoder().encode(message).length;
      const entry: StoredLog = {
        message,
        severity: parseSeverity(message),
        bytes,
      };

      historyRef.current.push(entry);
      bytesRef.current += bytes;

      while (historyRef.current.length > MAX_EVENTS || bytesRef.current > MAX_BYTES) {
        const removed = historyRef.current.shift();
        if (removed) bytesRef.current -= removed.bytes;
      }

      if (matchesSeverity(entry, severityRef.current)) {
        queueRef.current.push(message + '\r\n');
        flushRef.current();
      }
    }).then(dispose => {
      if (active) unlisten = dispose;
      else dispose();
    });

    const resizeObserver = new ResizeObserver(() => {
      if (active && container.clientWidth > 0 && container.clientHeight > 0) {
        fitAddon.fit();
      }
    });

    resizeObserver.observe(container);

    return () => {
      active = false;
      unlisten();
      resizeObserver.disconnect();
      generationRef.current++;
      terminalRef.current = null;
      fitRef.current = null;
      terminal.dispose();
    };
  }, []);

  useEffect(() => {
    severityRef.current = severity;
    renderRef.current();
  }, [severity]);

  return <div ref={containerRef} className="log-terminal" />;
});

export default LogTerminal;

import { chromium } from 'playwright';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

async function validate(executablePath: string) {
  const profile = await mkdtemp(join(tmpdir(), 'mdev-browser-validation-'));
  const child = spawn(executablePath, ['--remote-debugging-port=0', `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--new-window', 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true });
  let stderr = '';
  let spawnError: Error | undefined;
  child.stderr?.on('data', (chunk: Buffer) => {
    stderr = (stderr + chunk.toString()).slice(-8192);
  });
  child.on('error', (error: Error) => {
    spawnError = error;
  });
  let browser: Awaited<ReturnType<typeof chromium.connectOverCDP>> | undefined;
  try {
    const deadline = Date.now() + 15000;
    let port = 0;
    while (Date.now() < deadline) {
      if (spawnError) throw new Error(`Browser launch failed: ${spawnError.message}`);
      if (child.exitCode !== null || child.signalCode !== null) throw new Error(`Browser exited before CDP became available (code=${child.exitCode}, signal=${child.signalCode})`);
      try {
        const content = await readFile(join(profile, 'DevToolsActivePort'), 'utf8');
        port = Number(content.split(/\r?\n/)[0]);
        if (Number.isInteger(port) && port > 0) break;
      } catch {}
      await new Promise(resolve => setTimeout(resolve, 150));
    }
    if (!port) throw new Error('Browser did not expose a CDP debugging port');
    const response = await fetch(`http://127.0.0.1:${port}/json/version`, { signal: AbortSignal.timeout(5000) });
    if (!response.ok) throw new Error(`CDP discovery returned HTTP ${response.status}`);
    const metadata = await response.json() as { Browser?: string; webSocketDebuggerUrl?: string };
    if (!metadata.webSocketDebuggerUrl) throw new Error('CDP WebSocket endpoint is unavailable');
    let connectionError: unknown;
    for (let attempt = 0; attempt < 5; attempt++) {
      if (spawnError || child.exitCode !== null || child.signalCode !== null) {
        throw new Error(`Browser terminated during CDP connection (code=${child.exitCode}, signal=${child.signalCode})`);
      }
      try {
        browser = await chromium.connectOverCDP(metadata.webSocketDebuggerUrl, { timeout: 3000 });
        break;
      } catch (error) {
        connectionError = error;
        if (child.exitCode !== null || child.signalCode !== null) break;
        if (attempt < 4) await new Promise(resolve => setTimeout(resolve, 400));
      }
    }
    if (!browser) throw new Error(`Playwright CDP connection failed: ${connectionError instanceof Error ? connectionError.message : String(connectionError)}`);
    const context = browser.contexts()[0];
    if (!context) throw new Error('Playwright could not access the browser context');
    const page = await context.newPage();
    await page.goto('about:blank');
    await page.evaluate(() => document.readyState);
    await page.close();
    return { valid: true, browser: metadata.Browser ?? 'Chromium', version: browser.version(), cdp_supported: true, playwright_connected: true };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    const processStatus = `exitCode=${child.exitCode}, signal=${child.signalCode}, pid=${child.pid ?? 'unknown'}`;
    const browserOutput = stderr.trim();
    throw new Error(`Browser validation failed: ${message}\nProcess: ${processStatus}${browserOutput ? `\nBrowser stderr:\n${browserOutput}` : ''}`);
  } finally {
    await browser?.close().catch(() => undefined);
    if (child.exitCode === null && child.signalCode === null) {
      const exited = new Promise<void>(resolve => child.once('exit', () => resolve()));
      child.kill();
      await Promise.race([exited, new Promise<void>(resolve => setTimeout(resolve, 3000))]);
    }
    await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }).catch(() => undefined);
  }
}

const executable = process.argv[2];
if (!executable) {
  process.stderr.write('Browser executable path is required\n');
  process.exitCode = 1;
} else {
  validate(executable).then(result => {
    process.stdout.write(`${JSON.stringify(result)}\n`);
  }).catch(error => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
}

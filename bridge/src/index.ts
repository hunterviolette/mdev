import readline from 'node:readline';
import { SessionManager } from './session_manager.js';
import type { BridgeCommand, BridgeResponse } from './protocol.js';

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
  terminal: false
});

const manager = new SessionManager();
const sessionQueues = new Map<string, Promise<void>>();
let stdoutQueue = Promise.resolve();
let shuttingDown = false;

async function shutdown(code = 0) {
  if (shuttingDown) {
    return;
  }

  shuttingDown = true;

  try {
    await Promise.allSettled([...sessionQueues.values()]);
    await manager.closeAllSessions();
    await stdoutQueue;
  } finally {
    process.exit(code);
  }
}

function writeResponse(resp: BridgeResponse) {
  stdoutQueue = stdoutQueue.then(() => new Promise<void>((resolve, reject) => {
    process.stdout.write(`${JSON.stringify(resp)}\n`, err => {
      if (err) {
        reject(err);
      } else {
        resolve();
      }
    });
  }));
  return stdoutQueue;
}

async function handleCommand(cmd: BridgeCommand): Promise<BridgeResponse> {
  switch (cmd.cmd) {
    case 'start_session': {
      const data = await manager.startSession(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: data.session_id, data };
    }
    case 'connect_over_cdp': {
      const data = await manager.connectOverCdp(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: data.session_id, data };
    }
    case 'open_page': {
      const data = await manager.openPage(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'probe_page': {
      const data = await manager.probePage(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'close_page': {
      const data = await manager.closePage(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'send_chat': {
      const data = await manager.sendChat(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'upload_file': {
      const data = await manager.uploadFile(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'read_response': {
      const data = await manager.readResponse(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'set_poll_config': {
      const data = await manager.setPollConfig(cmd as any);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'get_cookies': {
      const data = await manager.getCookies(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'set_response_timeout': {
      const data = await manager.setResponseTimeout(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'close_session': {
      const data = await manager.closeSession(cmd.session_id);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    default: {
      const neverCmd: never = cmd;
      throw new Error(`Unsupported command: ${JSON.stringify(neverCmd)}`);
    }
  }
}

function commandSessionId(cmd: BridgeCommand) {
  return 'session_id' in cmd && typeof cmd.session_id === 'string' && cmd.session_id.trim()
    ? cmd.session_id
    : undefined;
}

function dispatchCommand(cmd: BridgeCommand) {
  const sessionId = commandSessionId(cmd);
  const run = async () => {
    try {
      await writeResponse(await handleCommand(cmd));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      await writeResponse({
        id: cmd.id,
        ok: false,
        cmd: cmd.cmd,
        session_id: sessionId,
        error: message
      });
    }
  };

  if (!sessionId) {
    void run();
    return;
  }

  const previous = sessionQueues.get(sessionId) ?? Promise.resolve();
  const current = previous.catch(() => undefined).then(run);
  sessionQueues.set(sessionId, current);
  void current.finally(() => {
    if (sessionQueues.get(sessionId) === current) {
      sessionQueues.delete(sessionId);
    }
  });
}

rl.on('line', line => {
  const trimmed = line.trim();
  if (!trimmed) {
    return;
  }

  let parsed: BridgeCommand | undefined;

  try {
    parsed = JSON.parse(trimmed) as BridgeCommand;
    dispatchCommand(parsed);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    void writeResponse({
      id: parsed?.id ?? 'unknown',
      ok: false,
      cmd: parsed?.cmd,
      session_id: (parsed as { session_id?: string } | undefined)?.session_id,
      error: message
    });
  }
});

rl.on('close', async () => {
  await shutdown(0);
});

process.on('SIGINT', async () => {
  await shutdown(0);
});

process.on('SIGTERM', async () => {
  await shutdown(0);
});

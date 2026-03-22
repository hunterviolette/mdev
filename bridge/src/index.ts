import readline from 'node:readline';
import { SessionManager } from './session_manager.js';
import type { BridgeCommand, BridgeResponse } from './protocol.js';

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
  terminal: false
});

const manager = new SessionManager();

function writeResponse(resp: BridgeResponse) {
  process.stdout.write(JSON.stringify(resp) + '\n');
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

rl.on('line', async (line) => {
  const trimmed = line.trim();
  if (!trimmed) {
    return;
  }

  let parsed: BridgeCommand | undefined;

  try {
    parsed = JSON.parse(trimmed) as BridgeCommand;
    const resp = await handleCommand(parsed);
    writeResponse(resp);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    writeResponse({
      id: parsed?.id ?? 'unknown',
      ok: false,
      cmd: parsed?.cmd,
      session_id: (parsed as { session_id?: string } | undefined)?.session_id,
      error: message
    });
  }
});

rl.on('close', async () => {
  process.exit(0);
});

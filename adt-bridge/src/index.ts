import readline from 'node:readline';
import { SessionManager } from './session_manager.js';
import type { AdtCommand, AdtResponse } from './protocol.js';

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
  terminal: false
});

const manager = new SessionManager();

function writeResponse(resp: AdtResponse) {
  process.stdout.write(JSON.stringify(resp) + '\n');
}

async function handleCommand(cmd: AdtCommand): Promise<AdtResponse> {
  switch (cmd.cmd) {
    case 'connect': {
      const data = await manager.connect(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: data.session_id, data };
    }
    case 'list_package_objects': {
      const data = await manager.listPackageObjects(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'read_object': {
      const data = await manager.readObject(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'lock_object': {
      const data = await manager.lockObject(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'unlock_object': {
      const data = await manager.unlockObject(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'update_object': {
      const data = await manager.updateObject(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'create_object': {
      const data = await manager.createObject(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'create_transport': {
      const data = await manager.createTransport(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'call_endpoint': {
      const data = await manager.callEndpoint(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'syntax_check': {
      const data = await manager.syntaxCheck(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'activate_object': {
      const data = await manager.activateObject(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'activate_package': {
      const data = await manager.activatePackage(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'get_problems': {
      const data = await manager.getProblems(cmd);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    case 'close_session': {
      const data = await manager.closeSession(cmd.session_id);
      return { id: cmd.id, ok: true, cmd: cmd.cmd, session_id: cmd.session_id, data };
    }
    default: {
      const neverCmd: never = cmd;
      throw new Error(`Unhandled command ${(neverCmd as { cmd?: string }).cmd ?? 'unknown'}`);
    }
  }
}

rl.on('line', async (line) => {
  const trimmed = line.trim();
  if (!trimmed) {
    return;
  }

  try {
    const cmd = JSON.parse(trimmed) as AdtCommand;
    const resp = await handleCommand(cmd);
    writeResponse(resp);
  } catch (err) {
    const parsed = (() => {
      try {
        return JSON.parse(trimmed) as Partial<AdtCommand>;
      } catch {
        return {};
      }
    })();

    writeResponse({
      id: typeof parsed.id === 'string' ? parsed.id : 'unknown',
      ok: false,
      cmd: parsed.cmd,
      session_id: typeof parsed.session_id === 'string' ? parsed.session_id : undefined,
      error: err instanceof Error ? err.message : String(err)
    });
  }
});
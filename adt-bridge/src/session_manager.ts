import { randomUUID } from 'node:crypto';
import { AdtClient } from './client.js';
import { parseProblems } from './problem_parser.js';
import type {
  ActivateObjectCommand,
  ActivatePackageCommand,
  CallEndpointCommand,
  ConnectCommand,
  CreateObjectCommand,
  CreateTransportCommand,
  GetProblemsCommand,
  ListPackageObjectsCommand,
  LockObjectCommand,
  ReadObjectCommand,
  SyntaxCheckCommand,
  UnlockObjectCommand,
  UpdateObjectCommand
} from './protocol.js';

export type AdtSessionState = {
  sessionId: string;
  client: AdtClient;
  baseUrl: string;
};

function resolveBaseUrl(cmd: ConnectCommand): string {
  const value = cmd.base_url ?? process.env.ADT_HOST_URL ?? process.env.ADT_HOST ?? '';
  const trimmed = value.trim();
  if (!trimmed) {
    throw new Error('ADT base_url is required');
  }
  return trimmed.replace(/\/$/, '');
}

export class SessionManager {
  private sessions = new Map<string, AdtSessionState>();

  async connect(cmd: ConnectCommand) {
    const sessionId = cmd.session_id ?? randomUUID();
    const baseUrl = resolveBaseUrl(cmd);
    const client = new AdtClient(baseUrl, {
      authType: cmd.auth_type,
      transport: cmd.transport,
      username: cmd.username,
      password: cmd.password,
      authorization: cmd.authorization,
      cookieHeader: cmd.cookie_header,
      negotiateCommand: cmd.negotiate_command,
      client: cmd.client,
      timeoutMs: cmd.timeout_ms,
      sessionKey: sessionId
    });

    const discovery = await client.discovery();
    if (discovery.status < 200 || discovery.status >= 300) {
      throw new Error(`ADT discovery failed (${discovery.status}): ${discovery.body.slice(0, 500)}`);
    }

    this.sessions.set(sessionId, {
      sessionId,
      client,
      baseUrl
    });

    return {
      session_id: sessionId,
      base_url: baseUrl,
      discovery_status: discovery.status,
      discovery_content_type: discovery.headers['content-type'] ?? '',
      discovery_final_url: discovery.headers['x-final-url'] ?? baseUrl
    };
  }

  async listPackageObjects(cmd: ListPackageObjectsCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.listPackageObjects(cmd.package_name, cmd.include_subpackages ?? false);
    if (resp.status < 200 || resp.status >= 300) {
      throw new Error(`list_package_objects failed (${resp.status}): ${resp.body.slice(0, 500)}`);
    }
    return {
      session_id: state.sessionId,
      package_name: cmd.package_name,
      status: resp.status,
      xml: resp.body
    };
  }

  async readObject(cmd: ReadObjectCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.readObject(cmd.object_uri, cmd.accept);
    if (resp.status < 200 || resp.status >= 300) {
      throw new Error(`read_object failed (${resp.status}): ${resp.body.slice(0, 500)}`);
    }
    return {
      session_id: state.sessionId,
      object_uri: cmd.object_uri,
      status: resp.status,
      content_type: resp.headers['content-type'] ?? null,
      body: resp.body,
      headers: resp.headers
    };
  }

  async lockObject(cmd: LockObjectCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.lockObject(cmd.object_uri);
    if (resp.status < 200 || resp.status >= 300) {
      throw new Error(`lock_object failed (${resp.status}): ${resp.body.slice(0, 500)}`);
    }
    const lockHandle =
      /<LOCK_HANDLE>([^<]+)<\/LOCK_HANDLE>/i.exec(resp.body)?.[1]?.trim() ||
      /<lockHandle>([^<]+)<\/lockHandle>/i.exec(resp.body)?.[1]?.trim() ||
      resp.headers['x-lockhandle'] ||
      resp.headers['x-lock-handle'] ||
      null;
    if (!lockHandle) {
      throw new Error(`lock_object succeeded but no lockHandle was returned: ${resp.body.slice(0, 500)}`);
    }
    return {
      session_id: state.sessionId,
      object_uri: cmd.object_uri,
      status: resp.status,
      lock_handle: lockHandle,
      body: resp.body,
      headers: resp.headers
    };
  }

  async unlockObject(cmd: UnlockObjectCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.unlockObject(cmd.object_uri, cmd.lock_handle);
    if (resp.status < 200 || resp.status >= 300) {
      throw new Error(`unlock_object failed (${resp.status}): ${resp.body.slice(0, 500)}`);
    }
    return {
      session_id: state.sessionId,
      object_uri: cmd.object_uri,
      status: resp.status,
      body: resp.body,
      headers: resp.headers
    };
  }

  async updateObject(cmd: UpdateObjectCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.updateObject(cmd.object_uri, cmd.source, cmd.content_type, cmd.lock_handle, cmd.corr_nr, cmd.headers);
    if (resp.status < 200 || resp.status >= 300) {
      throw new Error(`update_object failed (${resp.status}): ${resp.body.slice(0, 500)}`);
    }
    return {
      session_id: state.sessionId,
      object_uri: cmd.object_uri,
      status: resp.status,
      body: resp.body,
      headers: resp.headers
    };
  }

  async createObject(cmd: CreateObjectCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.createObject(cmd.collection_uri, cmd.body, cmd.content_type, cmd.accept, cmd.headers);
    if (resp.status < 200 || resp.status >= 300) {
      throw new Error(`create_object failed (${resp.status}): ${resp.body.slice(0, 500)}`);
    }
    return {
      session_id: state.sessionId,
      collection_uri: cmd.collection_uri,
      status: resp.status,
      body: resp.body,
      headers: resp.headers
    };
  }

  async createTransport(cmd: CreateTransportCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.createTransport(cmd.collection_uri, cmd.body, cmd.content_type, cmd.accept, cmd.headers);
    if (resp.status < 200 || resp.status >= 300) {
      throw new Error(`create_transport failed (${resp.status}): ${resp.body.slice(0, 500)}`);
    }
    return {
      session_id: state.sessionId,
      collection_uri: cmd.collection_uri,
      status: resp.status,
      body: resp.body,
      headers: resp.headers
    };
  }

  async callEndpoint(cmd: CallEndpointCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.callEndpoint(cmd.method, cmd.uri, cmd.body, cmd.content_type, cmd.accept, cmd.headers);
    if (resp.status < 200 || resp.status >= 300) {
      throw new Error(`call_endpoint failed (${resp.status}): ${resp.body.slice(0, 500)}`);
    }
    return {
      session_id: state.sessionId,
      method: cmd.method,
      uri: cmd.uri,
      status: resp.status,
      body: resp.body,
      headers: resp.headers
    };
  }

  async syntaxCheck(cmd: SyntaxCheckCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.syntaxCheck(cmd.object_uri);
    const problems = parseProblems(resp.body);
    return {
      session_id: state.sessionId,
      object_uri: cmd.object_uri,
      status: resp.status,
      ok: resp.status >= 200 && resp.status < 300,
      problems,
      xml: resp.body,
      headers: resp.headers
    };
  }

  async activateObject(cmd: ActivateObjectCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.activateObject(cmd.object_uri);
    const problems = parseProblems(resp.body);
    return {
      session_id: state.sessionId,
      object_uri: cmd.object_uri,
      status: resp.status,
      activated: resp.status >= 200 && resp.status < 300 && problems.length === 0,
      problems,
      xml: resp.body,
      headers: resp.headers
    };
  }

  async activatePackage(cmd: ActivatePackageCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.activatePackage(cmd.package_name);
    const problems = parseProblems(resp.body);
    return {
      session_id: state.sessionId,
      package_name: cmd.package_name,
      status: resp.status,
      activated: resp.status >= 200 && resp.status < 300 && problems.length === 0,
      problems,
      xml: resp.body,
      headers: resp.headers
    };
  }

  async getProblems(cmd: GetProblemsCommand) {
    const state = this.getSession(cmd.session_id);
    if (cmd.xml && cmd.xml.trim()) {
      return {
        session_id: state.sessionId,
        problems: parseProblems(cmd.xml),
        xml: cmd.xml
      };
    }
    if (!cmd.result_uri) {
      throw new Error('get_problems requires result_uri or xml');
    }
    const resp = await state.client.getProblems(cmd.result_uri);
    if (resp.status < 200 || resp.status >= 300) {
      throw new Error(`get_problems failed (${resp.status}): ${resp.body.slice(0, 500)}`);
    }
    return {
      session_id: state.sessionId,
      result_uri: cmd.result_uri,
      status: resp.status,
      problems: parseProblems(resp.body),
      xml: resp.body,
      headers: resp.headers
    };
  }

  async closeSession(sessionId: string) {
    const state = this.getSession(sessionId);
    this.sessions.delete(sessionId);
    await state.client.close();
    return { session_id: state.sessionId, closed: true };
  }

  private getSession(sessionId: string): AdtSessionState {
    const state = this.sessions.get(sessionId);
    if (!state) {
      throw new Error(`Unknown session_id: ${sessionId}`);
    }
    return state;
  }
}
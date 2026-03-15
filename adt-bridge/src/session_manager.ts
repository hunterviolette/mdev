import { randomUUID } from 'node:crypto';
import { AdtClient } from './client.js';
import { parseProblems } from './problem_parser.js';
import type {
  ActivateObjectCommand,
  ActivatePackageCommand,
  ConnectCommand,
  GetProblemsCommand,
  ListPackageObjectsCommand,
  ReadObjectCommand,
  SyntaxCheckCommand,
  UpdateObjectCommand
} from './protocol.js';

export type AdtSessionState = {
  sessionId: string;
  client: AdtClient;
  baseUrl: string;
};

export class SessionManager {
  private sessions = new Map<string, AdtSessionState>();

  async connect(cmd: ConnectCommand) {
    const sessionId = cmd.session_id ?? randomUUID();
    const client = new AdtClient(cmd.base_url, {
      username: cmd.username,
      password: cmd.password,
      authorization: cmd.authorization,
      client: cmd.client,
      timeoutMs: cmd.timeout_ms
    });

    const discovery = await client.discovery();
    if (discovery.status < 200 || discovery.status >= 300) {
      throw new Error(`ADT discovery failed (${discovery.status}): ${discovery.body.slice(0, 500)}`);
    }

    this.sessions.set(sessionId, {
      sessionId,
      client,
      baseUrl: cmd.base_url
    });

    return {
      session_id: sessionId,
      base_url: cmd.base_url,
      discovery_status: discovery.status
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

  async updateObject(cmd: UpdateObjectCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.updateObject(cmd.object_uri, cmd.source, cmd.content_type, cmd.lock_handle);
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

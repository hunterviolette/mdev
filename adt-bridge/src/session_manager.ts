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

function mergeProblems<T extends { severity?: string; message?: string; line?: number; column?: number; code?: string }>(base: T[], extra: T[]): T[] {
  const out: T[] = [];
  const seen = new Set<string>();

  for (const item of [...base, ...extra]) {
    const key = [
      item.severity ?? '',
      item.code ?? '',
      item.line ?? '',
      item.column ?? '',
      item.message ?? ''
    ].join('|');
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    out.push(item);
  }

  return out;
}

function hasLocationProblems(items: Array<{ line?: number; column?: number }>): boolean {
  return items.some(item => Number.isFinite(item.line) || Number.isFinite(item.column));
}

type AdtResolvedRoute = {
  routeFamily: 'source_main_at_root' | 'direct_resource' | 'root_to_source_main';
  lockUri: string;
  writeUri: string;
};

function resolveAdtRoute(objectUri: string): AdtResolvedRoute {
  const trimmed = objectUri.trim();
  if (!trimmed) {
    return {
      routeFamily: 'direct_resource',
      lockUri: trimmed,
      writeUri: trimmed,
    };
  }

  const [base, suffix = ''] = trimmed.split(/([?#].*)/, 2);
  const normalizedBase = base.replace(/\/$/, '');

  if (/\/source\/main$/i.test(normalizedBase)) {
    return {
      routeFamily: 'source_main_at_root',
      lockUri: normalizedBase.replace(/\/source\/main$/i, ''),
      writeUri: `${normalizedBase}${suffix}`,
    };
  }

  if (/\/includes\//i.test(normalizedBase)) {
    return {
      routeFamily: 'direct_resource',
      lockUri: normalizedBase.replace(/\/includes\/.*$/i, ''),
      writeUri: `${normalizedBase}${suffix}`,
    };
  }

  if (/\/(programs\/programs|ddic\/ddl\/sources|oo\/classes)\//i.test(normalizedBase)) {
    return {
      routeFamily: 'root_to_source_main',
      lockUri: normalizedBase,
      writeUri: `${normalizedBase}/source/main${suffix}`,
    };
  }

  return {
    routeFamily: 'direct_resource',
    lockUri: normalizedBase,
    writeUri: `${normalizedBase}${suffix}`,
  };
}

function previewText(value: string | undefined, max = 240): string {
  return (value ?? '').replace(/[\r\n\t]+/g, ' ').slice(0, max);
}

function isDdlSourceUri(uri: string): boolean {
  return /\/ddic\/ddl\/sources\//i.test(uri);
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
    const route = resolveAdtRoute(cmd.object_uri);
    const sourceUri = route.writeUri;
    const lockUri = route.lockUri;
    const headers = { ...(cmd.headers ?? {}) } as Record<string, string>;
    delete headers['If-Match'];
    delete headers['if-match'];

    console.error(
      `[adt-bridge] updateObject start objectUri=${cmd.object_uri} routeFamily=${route.routeFamily} sourceUri=${sourceUri} lockUri=${lockUri} contentType=${cmd.content_type ?? ''} inputCorrNr=${cmd.corr_nr ?? ''} sourceBytes=${cmd.source.length}`
    );

    let lockHandle: string | undefined;
    let corrNr: string | undefined = cmd.corr_nr?.trim() || undefined;

    try {
      const lockResp = isDdlSourceUri(lockUri)
        ? await state.client.lockDdlSource(lockUri)
        : await state.client.lockObject(lockUri);
      lockHandle =
        /<LOCK_HANDLE>([^<]+)<\/LOCK_HANDLE>/i.exec(lockResp.body)?.[1]?.trim() ||
        /<lockHandle>([^<]+)<\/lockHandle>/i.exec(lockResp.body)?.[1]?.trim() ||
        lockResp.headers['x-lock-handle'] ||
        lockResp.headers['x-lockhandle'] ||
        undefined;
      corrNr =
        /<CORRNR>([^<]+)<\/CORRNR>/i.exec(lockResp.body)?.[1]?.trim() ||
        /<corrNr>([^<]+)<\/corrNr>/i.exec(lockResp.body)?.[1]?.trim() ||
        corrNr;

      console.error(
        `[adt-bridge] updateObject lock status=${lockResp.status} routeFamily=${route.routeFamily} lockUri=${lockUri} sourceUri=${sourceUri} lockHandle=${lockHandle ?? ''} corrNr=${corrNr ?? ''} body=${previewText(lockResp.body)}`
      );
      if (lockResp.status >= 400) {
        throw new Error(`updateObject lock failed (${lockResp.status}) for ${lockUri}: ${previewText(lockResp.body)}`);
      }
      if (!lockHandle) {
        throw new Error(`updateObject lock did not return LOCK_HANDLE for ${lockUri}`);
      }

      let sourceToWrite = cmd.source;
      if (isDdlSourceUri(lockUri)) {
        const fmtResp = await state.client.formatDdlIdentifiers(cmd.source);
        console.error(
          `[adt-bridge] updateObject ddl-formatter status=${fmtResp.status} sourceUri=${sourceUri} body=${previewText(fmtResp.body)}`
        );
        if (fmtResp.status >= 200 && fmtResp.status < 300 && fmtResp.body.trim()) {
          sourceToWrite = fmtResp.body;
        }
      }

      const resp = await state.client.updateObject(
        sourceUri,
        sourceToWrite,
        cmd.content_type,
        lockHandle,
        corrNr,
        headers
      );

      console.error(
        `[adt-bridge] updateObject put status=${resp.status} routeFamily=${route.routeFamily} sourceUri=${sourceUri} lockHandle=${lockHandle} corrNr=${corrNr ?? ''} body=${previewText(resp.body)}`
      );
      if (resp.status >= 400) {
        throw new Error(`updateObject put failed (${resp.status}) for ${sourceUri}: ${previewText(resp.body)}`);
      }

      const problems = parseProblems(resp.body);
      return {
        session_id: state.sessionId,
        object_uri: cmd.object_uri,
        status: resp.status,
        ok: resp.status >= 200 && resp.status < 300 && problems.length === 0,
        problems,
        xml: resp.body,
        headers: {
          ...resp.headers,
          'x-adt-route-family': route.routeFamily,
          'x-adt-lock-uri': lockUri,
          'x-adt-write-uri': sourceUri,
        }
      };
    } finally {
      if (lockHandle) {
        console.error(`[adt-bridge] updateObject unlock routeFamily=${route.routeFamily} lockUri=${lockUri} sourceUri=${sourceUri} lockHandle=${lockHandle}`);
        await state.client.unlockObject(lockUri, lockHandle).catch(() => undefined);
      }
    }
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
      ok: resp.status >= 200 && resp.status < 300 && problems.length === 0,
      problems,
      xml: resp.body,
      headers: resp.headers
    };
  }

  async activateObject(cmd: ActivateObjectCommand) {
    const state = this.getSession(cmd.session_id);
    const resp = await state.client.activateObject(cmd.object_uri);
    let problems = parseProblems(resp.body);

    let checkruns:
      | {
          status?: number;
          xml?: string;
          problems?: ReturnType<typeof parseProblems>;
          error?: string;
        }
      | undefined;

    const activationHttpFailed = resp.status < 200 || resp.status >= 300;
    const activationFailed = activationHttpFailed || problems.length > 0;

    if (!activationHttpFailed && activationFailed) {
      try {
        const checkResp = await state.client.runCheckruns(cmd.object_uri);
        console.error(`[sap_adt] http label=run_checkruns method=POST status=${checkResp.status} url=/sap/bc/adt/checkruns?reporters=abapCheckRun`);
        const checkProblems = parseProblems(checkResp.body);
        checkruns = {
          status: checkResp.status,
          xml: checkResp.body,
          problems: checkProblems
        };

        if (checkProblems.length > 0) {
          if (hasLocationProblems(checkProblems)) {
            problems = checkProblems;
          } else if (problems.length === 0) {
            problems = checkProblems;
          } else {
            problems = mergeProblems(problems, checkProblems);
          }
        }
      } catch (err) {
        console.error(`[sap_adt] http label=run_checkruns method=POST status=0 url=/sap/bc/adt/checkruns?reporters=abapCheckRun error=${err instanceof Error ? err.message : String(err)}`);
        checkruns = {
          error: err instanceof Error ? err.message : String(err)
        };
      }
    }

    const activated = resp.status >= 200 && resp.status < 300 && problems.length === 0;

    return {
      session_id: state.sessionId,
      object_uri: cmd.object_uri,
      status: resp.status,
      activated,
      problems,
      xml: resp.body,
      headers: resp.headers,
      debug: {
        command: 'activate_object',
        checkruns
      }
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
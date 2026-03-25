import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

type AuthType = 'basic' | 'header' | 'negotiate' | 'cookie';
type TransportType = 'fetch' | 'curl';

type AuthConfig = {
  authType?: AuthType;
  transport?: TransportType;
  username?: string;
  password?: string;
  authorization?: string;
  cookieHeader?: string;
  negotiateCommand?: string;
  client?: string;
  timeoutMs?: number;
  sessionKey?: string;
};

export type AdtHttpResponse = {
  status: number;
  headers: Record<string, string>;
  body: string;
};

type CurlResult = {
  status: number;
  headers: Record<string, string>;
  body: string;
};

export class AdtClient {
  readonly baseUrl: string;
  readonly auth: AuthConfig;
  private cookieJarPath?: string;
  private csrfToken?: string;

  constructor(baseUrl: string, auth: AuthConfig) {
    this.baseUrl = baseUrl.replace(/\/$/, '');
    this.auth = auth;
  }

  private getAuthType(): AuthType {
    if (this.auth.authType) {
      return this.auth.authType;
    }
    if (this.auth.authorization && this.auth.authorization.trim()) {
      return 'header';
    }
    if (this.auth.cookieHeader && this.auth.cookieHeader.trim()) {
      return 'cookie';
    }
    return 'basic';
  }

  private getTransport(): TransportType {
    return this.auth.transport ?? 'curl';
  }

  private resolveUrl(pathOrUrl: string): string {
    return /^https?:\/\//i.test(pathOrUrl) ? pathOrUrl : `${this.baseUrl}${pathOrUrl.startsWith('/') ? '' : '/'}${pathOrUrl}`;
  }

  private isMutatingMethod(method: string): boolean {
    switch (method.toUpperCase()) {
      case 'POST':
      case 'PUT':
      case 'PATCH':
      case 'DELETE':
        return true;
      default:
        return false;
    }
  }

  private async fetchCsrfToken(): Promise<string | undefined> {
    const suffix = this.auth.client && this.auth.client.trim() ? `?sap-client=${encodeURIComponent(this.auth.client.trim())}` : '';
    const resp = await this.fetchText('GET', `/sap/bc/adt/discovery${suffix}`, undefined, {
      Accept: 'application/xml, text/xml, */*',
      'X-CSRF-Token': 'Fetch'
    });
    const token = resp.headers['x-csrf-token'];
    if (token && token.trim()) {
      this.csrfToken = token.trim();
      return this.csrfToken;
    }
    return undefined;
  }

  private buildBaseHeaders(extra?: Record<string, string>): Record<string, string> {
    const headers: Record<string, string> = {
      Accept: '*/*',
      'User-Agent': 'mdev-adt-bridge/0.1',
      ...extra
    };

    if (this.auth.client && this.auth.client.trim()) {
      headers['sap-client'] = this.auth.client.trim();
    }

    return headers;
  }

  private async ensureCookieJarPath(): Promise<string> {
    if (this.cookieJarPath) {
      return this.cookieJarPath;
    }
    const dir = await mkdtemp(path.join(os.tmpdir(), `mdev-adt-${this.auth.sessionKey ?? 'session'}-`));
    this.cookieJarPath = path.join(dir, 'cookies.txt');
    await writeFile(this.cookieJarPath, '', 'utf8');
    return this.cookieJarPath;
  }

  private ensureAdtContent(resp: AdtHttpResponse, url: string) {
    if (resp.status >= 200 && resp.status < 300) {
      return;
    }
    throw new Error(`ADT request failed (${resp.status}) for ${url}: ${resp.body.slice(0, 500)}`);
  }

  private async fetchWithCurl(method: string, pathOrUrl: string, body?: string, headers?: Record<string, string>): Promise<AdtHttpResponse> {
    const url = this.resolveUrl(pathOrUrl);
    const requestHeaders = this.buildBaseHeaders(headers);
    const authType = this.getAuthType();
    const timeoutMs = Math.max(1000, this.auth.timeoutMs ?? 60000);
    const workDir = await mkdtemp(path.join(os.tmpdir(), 'mdev-adt-curl-'));
    const headerFile = path.join(workDir, 'headers.txt');
    const bodyFile = path.join(workDir, 'body.txt');
    const args: string[] = [
      '--silent',
      '--show-error',
      '--location',
      '--max-time',
      String(Math.ceil(timeoutMs / 1000)),
      '--dump-header',
      headerFile,
      '--output',
      bodyFile,
      '--request',
      method
    ];

    let authAttached = false;

    if (authType === 'cookie' && this.auth.cookieHeader && this.auth.cookieHeader.trim()) {
      requestHeaders.Cookie = this.auth.cookieHeader.trim();
      authAttached = true;
      const cookieJar = await this.ensureCookieJarPath();
      args.push('--cookie', cookieJar, '--cookie-jar', cookieJar);
    } else if (authType === 'header' && this.auth.authorization && this.auth.authorization.trim()) {
      requestHeaders.Authorization = this.auth.authorization.trim();
      authAttached = true;
    } else if (authType === 'basic' && this.auth.username) {
      args.push('--user', `${this.auth.username}:${this.auth.password ?? ''}`);
      authAttached = true;
    } else if (authType === 'negotiate') {
      args.push('--negotiate', '--user', ':');
      authAttached = true;
    }

    for (const [key, value] of Object.entries(requestHeaders)) {
      args.push('--header', `${key}: ${value}`);
    }

    if (body !== undefined) {
      args.push('--data-binary', body);
    }

    args.push(url);

    try {
      const result = await new Promise<CurlResult>((resolve, reject) => {
        const child = spawn('curl', args, { stdio: ['ignore', 'pipe', 'pipe'] });
        let stderr = '';
        child.stderr.on('data', chunk => {
          stderr += String(chunk);
        });
        child.on('error', reject);
        child.on('close', async code => {
          try {
            const rawHeaders = await readFile(headerFile, 'utf8').catch(() => '');
            const responseBody = await readFile(bodyFile, 'utf8').catch(() => '');
            const headerBlocks = rawHeaders.split(/\r?\n\r?\n/).filter(Boolean);
            const lastBlock = headerBlocks.length > 0 ? headerBlocks[headerBlocks.length - 1] : '';
            const lines = lastBlock.split(/\r?\n/).filter(Boolean);
            const statusLine = lines.shift() ?? '';
            const statusMatch = statusLine.match(/\s(\d{3})(?:\s|$)/);
            const status = statusMatch ? Number.parseInt(statusMatch[1], 10) : (code === 0 ? 200 : 0);
            const responseHeaders: Record<string, string> = {};
            for (const line of lines) {
              const idx = line.indexOf(':');
              if (idx <= 0) {
                continue;
              }
              responseHeaders[line.slice(0, idx).trim().toLowerCase()] = line.slice(idx + 1).trim();
            }
            responseHeaders['x-final-url'] = url;
            responseHeaders['x-redirected'] = 'true';
            responseHeaders['x-request-auth-type'] = authType;
            responseHeaders['x-request-auth-attached'] = authAttached ? 'true' : 'false';
            responseHeaders['x-request-authorization-scheme'] = authAttached ? (authType === 'basic' ? 'Basic' : authType === 'negotiate' ? 'Negotiate' : authType === 'header' ? 'Custom' : 'Cookie') : 'none';
            if (code !== 0 && status === 0) {
              reject(new Error(stderr.trim() || `curl exited with ${code}`));
              return;
            }
            resolve({ status, headers: responseHeaders, body: responseBody });
          } catch (err) {
            reject(err);
          }
        });
      });
      return result;
    } finally {
      await rm(workDir, { recursive: true, force: true });
    }
  }

  private async fetchWithNode(method: string, pathOrUrl: string, body?: string, headers?: Record<string, string>): Promise<AdtHttpResponse> {
    const url = this.resolveUrl(pathOrUrl);
    const requestHeaders = this.buildBaseHeaders(headers);
    const authType = this.getAuthType();
    let authAttached = false;

    if (authType === 'header' && this.auth.authorization && this.auth.authorization.trim()) {
      requestHeaders.Authorization = this.auth.authorization.trim();
      authAttached = true;
    } else if (authType === 'cookie' && this.auth.cookieHeader && this.auth.cookieHeader.trim()) {
      requestHeaders.Cookie = this.auth.cookieHeader.trim();
      authAttached = true;
    } else if (authType === 'basic' && this.auth.username) {
      const raw = `${this.auth.username}:${this.auth.password ?? ''}`;
      requestHeaders.Authorization = `Basic ${Buffer.from(raw).toString('base64')}`;
      authAttached = true;
    }

    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), Math.max(1000, this.auth.timeoutMs ?? 60000));

    try {
      const resp = await fetch(url, {
        method,
        headers: requestHeaders,
        body,
        signal: controller.signal,
        redirect: 'manual'
      });

      const text = await resp.text();
      const responseHeaders: Record<string, string> = {};
      resp.headers.forEach((value, key) => {
        responseHeaders[key.toLowerCase()] = value;
      });
      responseHeaders['x-final-url'] = resp.url;
      responseHeaders['x-redirected'] = String(resp.redirected);
      responseHeaders['x-request-auth-type'] = authType;
      responseHeaders['x-request-auth-attached'] = authAttached ? 'true' : 'false';
      responseHeaders['x-request-authorization-scheme'] = authAttached ? (authType === 'negotiate' ? 'Negotiate' : authType === 'basic' ? 'Basic' : authType === 'cookie' ? 'Cookie' : 'Custom') : 'none';

      return {
        status: resp.status,
        headers: responseHeaders,
        body: text
      };
    } finally {
      clearTimeout(timeout);
      controller.abort();
    }
  }

  private async fetchText(method: string, pathOrUrl: string, body?: string, headers?: Record<string, string>): Promise<AdtHttpResponse> {
    const finalHeaders: Record<string, string> = {
      ...(headers ?? {})
    };

    if (this.isMutatingMethod(method)) {
      if (!this.csrfToken) {
        await this.fetchCsrfToken();
      }
      if (this.csrfToken && !finalHeaders['X-CSRF-Token'] && !finalHeaders['x-csrf-token']) {
        finalHeaders['X-CSRF-Token'] = this.csrfToken;
      }
    }

    let resp = this.getTransport() === 'curl'
      ? await this.fetchWithCurl(method, pathOrUrl, body, finalHeaders)
      : await this.fetchWithNode(method, pathOrUrl, body, finalHeaders);

    const respToken = resp.headers['x-csrf-token'];
    if (respToken && respToken.trim()) {
      this.csrfToken = respToken.trim();
    }

    const csrfFailed =
      this.isMutatingMethod(method) &&
      resp.status === 403 &&
      /csrf token validation failed/i.test(resp.body);

    if (csrfFailed) {
      this.csrfToken = undefined;
      await this.fetchCsrfToken();

      const retryHeaders: Record<string, string> = {
        ...(headers ?? {})
      };
      if (this.csrfToken) {
        retryHeaders['X-CSRF-Token'] = this.csrfToken;
      }

      resp = this.getTransport() === 'curl'
        ? await this.fetchWithCurl(method, pathOrUrl, body, retryHeaders)
        : await this.fetchWithNode(method, pathOrUrl, body, retryHeaders);

      const retryToken = resp.headers['x-csrf-token'];
      if (retryToken && retryToken.trim()) {
        this.csrfToken = retryToken.trim();
      }
    }

    return resp;
  }

  async discovery() {
    const suffix = this.auth.client && this.auth.client.trim() ? `?sap-client=${encodeURIComponent(this.auth.client.trim())}` : '';
    const url = `/sap/bc/adt/discovery${suffix}`;
    const resp = await this.fetchText('GET', url, undefined, { Accept: 'application/xml, text/xml, */*' });
    if (this.getTransport() === 'curl') {
      this.ensureAdtContent(resp, resp.headers['x-final-url'] ?? this.resolveUrl(url));
    }
    return resp;
  }

  async listPackageObjects(packageName: string, includeSubpackages = false) {
    const attempts: Array<Array<[string, string]>> = [];

    if (includeSubpackages) {
      attempts.push([
        ['packagename', packageName],
        ['type', 'all']
      ]);
    } else {
      attempts.push([
        ['packagename', packageName],
        ['type', 'package']
      ]);
    }

    attempts.push([
      ['packagename', packageName]
    ]);
    attempts.push([
      ['packagename', packageName],
      ['type', 'all']
    ]);
    attempts.push([
      ['packagename', packageName],
      ['type', 'package']
    ]);
    attempts.push([
      ['packagename', packageName],
      ['type', 'flat']
    ]);

    let lastResp: AdtHttpResponse | undefined;

    for (const pairs of attempts) {
      const qp = new URLSearchParams();
      for (const [key, value] of pairs) {
        qp.set(key, value);
      }
      if (this.auth.client && this.auth.client.trim()) {
        qp.set('sap-client', this.auth.client.trim());
      }

      const resp = await this.fetchText(
        'GET',
        `/sap/bc/adt/packages/$tree?${qp.toString()}`,
        undefined,
        { Accept: 'application/xml, text/xml, */*' }
      );

      lastResp = resp;
      const body = resp.body ?? '';
      const looksEmptyTree = /<[^>]*packageTree\b[^>]*\/>/i.test(body) && !/\buri\s*=\s*"/i.test(body);
      if (!looksEmptyTree) {
        return resp;
      }
    }

    return lastResp ?? this.fetchText(
      'GET',
      `/sap/bc/adt/packages/$tree?packagename=${encodeURIComponent(packageName)}`,
      undefined,
      { Accept: 'application/xml, text/xml, */*' }
    );
  }

  async readObject(objectUri: string, accept = 'text/plain, application/xml, */*') {
    return this.fetchText('GET', objectUri, undefined, { Accept: accept });
  }

  async lockObject(objectUri: string) {
    const path = `${objectUri}${objectUri.includes('?') ? '&' : '?'}_action=LOCK&accessMode=MODIFY`;
    return this.fetchText('POST', path, undefined, {
      'X-sap-adt-sessiontype': 'stateful',
      Accept: 'application/xml, text/xml, */*'
    });
  }

  async unlockObject(objectUri: string, lockHandle: string) {
    const path = `${objectUri}${objectUri.includes('?') ? '&' : '?'}lockHandle=${encodeURIComponent(lockHandle)}`;
    return this.fetchText('POST', path, '', {
      'X-sap-adt-sessiontype': 'stateless',
      Accept: 'application/xml, text/xml, */*'
    });
  }

  async updateObject(objectUri: string, source: string, contentType = 'text/plain; charset=utf-8', lockHandle?: string, corrNr?: string, extraHeaders?: Record<string, string>) {
    const headers: Record<string, string> = {
      'Content-Type': contentType,
      Accept: 'application/xml, text/plain, */*',
      ...(extraHeaders ?? {})
    };
    let finalUri = objectUri;
    const params: string[] = [];
    if (lockHandle && lockHandle.trim()) {
      const trimmed = lockHandle.trim();
      headers['X-sap-adt-lockhandle'] = trimmed;
      headers['X-lockHandle'] = trimmed;
      params.push(`lockHandle=${encodeURIComponent(trimmed)}`);
    }
    if (corrNr && corrNr.trim()) {
      params.push(`corrNr=${encodeURIComponent(corrNr.trim())}`);
    }
    if (params.length > 0) {
      finalUri = `${objectUri}${objectUri.includes('?') ? '&' : '?'}${params.join('&')}`;
    }
    return this.fetchText('PUT', finalUri, source, headers);
  }

  async createObject(collectionUri: string, body: string, contentType = 'application/xml; charset=utf-8', accept = 'application/xml, text/xml, */*', extraHeaders?: Record<string, string>) {
    return this.fetchText('POST', collectionUri, body, {
      'Content-Type': contentType,
      Accept: accept,
      ...(extraHeaders ?? {})
    });
  }

  async createTransport(collectionUri: string, body: string, contentType = 'application/xml; charset=utf-8', accept = 'application/xml, text/xml, */*', extraHeaders?: Record<string, string>) {
    return this.createObject(collectionUri, body, contentType, accept, extraHeaders);
  }

  async callEndpoint(method: string, uri: string, body?: string, contentType?: string, accept?: string, headers?: Record<string, string>) {
    const mergedHeaders: Record<string, string> = {
      ...(headers ?? {})
    };
    if (contentType) {
      mergedHeaders['Content-Type'] = contentType;
    }
    if (accept) {
      mergedHeaders.Accept = accept;
    }
    return this.fetchText(method, uri, body, mergedHeaders);
  }

  async syntaxCheck(objectUri: string) {
    const path = `${objectUri}${objectUri.includes('?') ? '&' : '?'}check=true`;
    return this.fetchText('GET', path, undefined, {
      Accept: 'application/xml, text/xml, */*'
    });
  }

  async activateObject(objectUri: string) {
    const body = `<?xml version="1.0" encoding="UTF-8"?><adtcore:objectReferences xmlns:adtcore="http://www.sap.com/adt/core"><adtcore:objectReference adtcore:uri="${escapeXmlAttr(objectUri)}" /></adtcore:objectReferences>`;
    return this.fetchText(
      'POST',
      '/sap/bc/adt/activation?method=activate&preauditRequested=true',
      body,
      {
        'Content-Type': 'application/xml; charset=utf-8',
        Accept: 'application/xml, text/xml, */*'
      }
    );
  }

  async runCheckruns(objectUri: string) {
    const body = `<?xml version="1.0" encoding="UTF-8"?><chkrun:checkObjectList xmlns:chkrun="http://www.sap.com/adt/checkrun" xmlns:adtcore="http://www.sap.com/adt/core"><chkrun:checkObject adtcore:uri="${escapeXmlAttr(objectUri)}" chkrun:version="inactive"/></chkrun:checkObjectList>`;
    return this.fetchText(
      'POST',
      '/sap/bc/adt/checkruns?reporters=abapCheckRun',
      body,
      {
        'Content-Type': 'application/vnd.sap.adt.checkobjects+xml',
        Accept: 'application/vnd.sap.adt.checkmessages+xml'
      }
    );
  }

  async activatePackage(packageName: string) {
    const body = `<?xml version="1.0" encoding="UTF-8"?><adtcore:activationRequest xmlns:adtcore="http://www.sap.com/adt/core"><adtcore:objectReference uri="/sap/bc/adt/packages/${escapeXmlAttr(encodeURIComponent(packageName))}" /></adtcore:activationRequest>`;
    return this.fetchText(
      'POST',
      '/sap/bc/adt/activation',
      body,
      {
        'Content-Type': 'application/xml; charset=utf-8',
        Accept: 'application/xml, text/xml, */*'
      }
    );
  }

  async getProblems(resultUri: string) {
    return this.fetchText('GET', resultUri, undefined, {
      Accept: 'application/xml, text/xml, */*'
    });
  }

  async close() {
    if (!this.cookieJarPath) {
      return;
    }
    const dir = path.dirname(this.cookieJarPath);
    this.cookieJarPath = undefined;
    await rm(dir, { recursive: true, force: true });
  }
}

function escapeXmlAttr(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

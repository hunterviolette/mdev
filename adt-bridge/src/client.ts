import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
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
    return this.auth.transport ?? 'fetch';
  }

  private resolveUrl(pathOrUrl: string): string {
    return /^https?:\/\//i.test(pathOrUrl) ? pathOrUrl : `${this.baseUrl}${pathOrUrl.startsWith('/') ? '' : '/'}${pathOrUrl}`;
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
    const baseDir = path.join(os.tmpdir(), 'mdev-adt-bridge');
    await mkdir(baseDir, { recursive: true });
    const dir = await mkdtemp(path.join(baseDir, `${this.auth.sessionKey ?? 'session'}-`));
    this.cookieJarPath = path.join(dir, 'cookies.txt');
    return this.cookieJarPath;
  }

  private async buildCurlArgs(method: string, url: string, headerFile: string, bodyFile: string, body?: string, headers?: Record<string, string>): Promise<{ args: string[]; authAttached: boolean; authType: string }> {
    const requestHeaders = this.buildBaseHeaders(headers);
    const args: string[] = ['--silent', '--show-error', '--location', '--dump-header', headerFile, '--output', bodyFile, '--request', method, '--cookie', await this.ensureCookieJarPath(), '--cookie-jar', await this.ensureCookieJarPath()];
    let authAttached = false;
    const authType = this.getAuthType();

    if (authType === 'basic' && this.auth.username) {
      args.push('--user', `${this.auth.username}:${this.auth.password ?? ''}`);
      authAttached = true;
    } else if (authType === 'header' && this.auth.authorization && this.auth.authorization.trim()) {
      requestHeaders.Authorization = this.auth.authorization.trim();
      authAttached = true;
    } else if (authType === 'negotiate') {
      args.push('--negotiate', '--user', ':');
      authAttached = true;
    }
    if (authType === 'cookie' && this.auth.cookieHeader && this.auth.cookieHeader.trim()) {
      requestHeaders.Cookie = this.auth.cookieHeader.trim();
      authAttached = true;
    }

    for (const [key, value] of Object.entries(requestHeaders)) {
      args.push('--header', `${key}: ${value}`);
    }

    if (body !== undefined) {
      args.push('--data-binary', '@-');
    }

    if (this.auth.timeoutMs && this.auth.timeoutMs > 0) {
      args.push('--max-time', String(Math.max(1, Math.ceil(this.auth.timeoutMs / 1000))));
    }

    args.push('--write-out', '\n__CURL_STATUS__:%{http_code}\n__CURL_EFFECTIVE_URL__:%{url_effective}\n__CURL_CONTENT_TYPE__:%{content_type}\n');
    args.push(url);
    return { args, authAttached, authType };
  }

  private parseCurlHeaders(rawHeaders: string): Record<string, string> {
    const lines = rawHeaders.replace(/\r/g, '').split('\n');
    const headerBlocks: string[][] = [];
    let current: string[] = [];

    for (const line of lines) {
      if (!line.trim()) {
        if (current.length > 0) {
          headerBlocks.push(current);
          current = [];
        }
        continue;
      }
      current.push(line);
    }
    if (current.length > 0) {
      headerBlocks.push(current);
    }

    const finalBlock = headerBlocks.length > 0 ? headerBlocks[headerBlocks.length - 1] : [];
    const headerMap: Record<string, string> = {};
    for (const line of finalBlock.slice(1)) {
      const idx = line.indexOf(':');
      if (idx <= 0) {
        continue;
      }
      const key = line.slice(0, idx).trim().toLowerCase();
      const value = line.slice(idx + 1).trim();
      if (key === 'set-cookie' && headerMap[key]) {
        headerMap[key] = `${headerMap[key]}\n${value}`;
      } else {
        headerMap[key] = value;
      }
    }
    return headerMap;
  }

  private ensureAdtContent(resp: CurlResult, url: string) {
    const contentType = (resp.headers['content-type'] ?? '').toLowerCase();
    const body = resp.body;
    const looksLikeHtml = contentType.includes('text/html') || /^\s*<!doctype html/i.test(body) || /^\s*<html\b/i.test(body);
    const looksLikeSamlLogin = /login\.microsoftonline\.com/i.test(body) || /<form[^>]+saml2/i.test(body) || /name=\"SAMLRequest\"/i.test(body) || /document\.forms\[0\]\.submit\(\)/i.test(body);
    const looksLikeXml = /^\s*<\?xml\b/i.test(body) || /<(?:app:)?service\b/i.test(body) || /<(?:atom:)?feed\b/i.test(body) || /<(?:atom:)?entry\b/i.test(body);

    if (looksLikeHtml || looksLikeSamlLogin || !looksLikeXml) {
      throw new Error(`Authentication failed: endpoint did not return ADT XML content (status=${resp.status}, content-type=${contentType || 'unknown'}, url=${url})`);
    }
  }

  private runCurl(args: string[], body?: string): Promise<{ stdout: string; stderr: string; exitCode: number }> {
    return new Promise((resolve, reject) => {
      const child = spawn('curl', args, {
        stdio: ['pipe', 'pipe', 'pipe']
      });

      let stdout = '';
      let stderr = '';

      child.stdout.setEncoding('utf8');
      child.stderr.setEncoding('utf8');

      child.stdout.on('data', chunk => {
        stdout += chunk;
      });

      child.stderr.on('data', chunk => {
        stderr += chunk;
      });

      child.on('error', err => {
        reject(err);
      });

      child.on('close', code => {
        resolve({
          stdout,
          stderr,
          exitCode: code ?? -1
        });
      });

      if (body !== undefined) {
        child.stdin.write(body);
      }
      child.stdin.end();
    });
  }

  private async fetchWithCurl(method: string, pathOrUrl: string, body?: string, headers?: Record<string, string>): Promise<AdtHttpResponse> {
    const url = this.resolveUrl(pathOrUrl);
    const workDir = await mkdtemp(path.join(os.tmpdir(), 'mdev-adt-curl-'));
    const headerFile = path.join(workDir, 'headers.txt');
    const bodyFile = path.join(workDir, 'body.txt');

    try {
      const { args, authAttached, authType } = await this.buildCurlArgs(method, url, headerFile, bodyFile, body, headers);
      const result = await this.runCurl(args, body);
      const stderr = result.stderr ?? '';
      const stdout = result.stdout ?? '';

      if (result.exitCode !== 0) {
        throw new Error(`curl exited with code ${result.exitCode} for ${url}: ${stderr || stdout.slice(0, 500)}`);
      }

      const rawHeaders = await readFile(headerFile, 'utf8');
      const responseBody = await readFile(bodyFile, 'utf8');
      const responseHeaders = this.parseCurlHeaders(rawHeaders);
      const statusMatch = stdout.match(/__CURL_STATUS__:(\d{3})/);
      const effectiveUrlMatch = stdout.match(/__CURL_EFFECTIVE_URL__:(.*)/);
      const contentTypeMatch = stdout.match(/__CURL_CONTENT_TYPE__:(.*)/);
      const status = statusMatch ? Number.parseInt(statusMatch[1], 10) : NaN;

      if (!Number.isFinite(status)) {
        throw new Error(`curl request failed to produce an HTTP status for ${url}: ${stderr || stdout.slice(0, 500)}`);
      }

      responseHeaders['x-final-url'] = effectiveUrlMatch ? effectiveUrlMatch[1].trim() : url;
      if (contentTypeMatch && contentTypeMatch[1].trim()) {
        responseHeaders['content-type'] = contentTypeMatch[1].trim().toLowerCase();
      }
      responseHeaders['x-request-auth-type'] = authType;
      responseHeaders['x-request-auth-attached'] = authAttached ? 'true' : 'false';
      responseHeaders['x-request-authorization-scheme'] = authAttached ? (authType === 'negotiate' ? 'Negotiate' : authType === 'basic' ? 'Basic' : authType === 'cookie' ? 'Cookie' : 'Custom') : 'none';

      return {
        status,
        headers: responseHeaders,
        body: responseBody
      };
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
      responseHeaders['x-request-authorization-scheme'] = authAttached ? (authType === 'basic' ? 'Basic' : authType === 'cookie' ? 'Cookie' : 'Custom') : 'none';

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
    if (this.getTransport() === 'curl') {
      return this.fetchWithCurl(method, pathOrUrl, body, headers);
    }
    return this.fetchWithNode(method, pathOrUrl, body, headers);
  }

  async discovery() {
    const suffix = this.auth.client && this.auth.client.trim() ? `?sap-client=${encodeURIComponent(this.auth.client.trim())}` : '';
    const resp = await this.fetchText('GET', `/sap/bc/adt/discovery${suffix}`, undefined, { Accept: 'application/xml, text/xml, */*' });
    if (this.getTransport() === 'curl') {
      this.ensureAdtContent(resp, resp.headers['x-final-url'] ?? this.resolveUrl(`/sap/bc/adt/discovery${suffix}`));
    }
    return resp;
  }

  async listPackageObjects(packageName: string, includeSubpackages = false) {
    const qp = new URLSearchParams();
    qp.set('packagename', packageName);
    if (includeSubpackages) {
      qp.set('type', 'all');
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
    return resp;
  }

  async readObject(objectUri: string, accept = 'text/plain, application/xml, */*') {
    return this.fetchText('GET', objectUri, undefined, { Accept: accept });
  }

  async updateObject(objectUri: string, source: string, contentType = 'text/plain; charset=utf-8', lockHandle?: string, extraHeaders?: Record<string, string>) {
    const headers: Record<string, string> = {
      'Content-Type': contentType,
      Accept: 'application/xml, text/plain, */*',
      ...(extraHeaders ?? {})
    };
    if (lockHandle && lockHandle.trim()) {
      headers['X-lockHandle'] = lockHandle.trim();
    }
    return this.fetchText('PUT', objectUri, source, headers);
  }

  async createObject(collectionUri: string, body: string, contentType = 'application/xml; charset=utf-8', accept = 'application/xml, text/xml, */*', extraHeaders?: Record<string, string>) {
    return this.fetchText('POST', collectionUri, body, {
      'Content-Type': contentType,
      Accept: accept,
      ...(extraHeaders ?? {})
    });
  }

  async createTransport(collectionUri: string, body: string, contentType = 'application/xml; charset=utf-8', accept = 'application/xml, text/xml, */*', extraHeaders?: Record<string, string>) {
    return this.fetchText('POST', collectionUri, body, {
      'Content-Type': contentType,
      Accept: accept,
      ...(extraHeaders ?? {})
    });
  }

  async callEndpoint(method: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE', uri: string, body?: string, contentType?: string, accept = 'application/xml, text/plain, */*', extraHeaders?: Record<string, string>) {
    const headers: Record<string, string> = {
      Accept: accept,
      ...(extraHeaders ?? {})
    };
    if (contentType && contentType.trim()) {
      headers['Content-Type'] = contentType;
    }
    return this.fetchText(method, uri, body, headers);
  }

  async syntaxCheck(objectUri: string) {
    const body = `<?xml version="1.0" encoding="UTF-8"?><checkrun><object uri="${escapeXmlAttr(objectUri)}" /></checkrun>`;
    return this.fetchText(
      'POST',
      '/sap/bc/adt/checkruns',
      body,
      {
        'Content-Type': 'application/xml; charset=utf-8',
        Accept: 'application/xml, text/xml, */*'
      }
    );
  }

  async activateObject(objectUri: string) {
    const body = `<?xml version="1.0" encoding="UTF-8"?><adtcore:activationRequest xmlns:adtcore="http://www.sap.com/adt/core"><adtcore:objectReference uri="${escapeXmlAttr(objectUri)}" /></adtcore:activationRequest>`;
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

 
type AuthConfig = {
  username?: string;
  password?: string;
  authorization?: string;
  client?: string;
  timeoutMs?: number;
};

export type AdtHttpResponse = {
  status: number;
  headers: Record<string, string>;
  body: string;
};

export class AdtClient {
  readonly baseUrl: string;
  readonly auth: AuthConfig;

  constructor(baseUrl: string, auth: AuthConfig) {
    this.baseUrl = baseUrl.replace(/\/$/, '');
    this.auth = auth;
  }

  private buildHeaders(extra?: Record<string, string>): Record<string, string> {
    const headers: Record<string, string> = {
      Accept: '*/*',
      'User-Agent': 'mdev-adt-bridge/0.1',
      ...extra
    };

    if (this.auth.authorization && this.auth.authorization.trim()) {
      headers.Authorization = this.auth.authorization.trim();
    } else if (this.auth.username) {
      const raw = `${this.auth.username}:${this.auth.password ?? ''}`;
      headers.Authorization = `Basic ${Buffer.from(raw).toString('base64')}`;
    }

    if (this.auth.client && this.auth.client.trim()) {
      headers['sap-client'] = this.auth.client.trim();
    }

    return headers;
  }

  private async fetchText(method: string, pathOrUrl: string, body?: string, headers?: Record<string, string>): Promise<AdtHttpResponse> {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), Math.max(1000, this.auth.timeoutMs ?? 60000));

    try {
      const url = /^https?:\/\//i.test(pathOrUrl) ? pathOrUrl : `${this.baseUrl}${pathOrUrl.startsWith('/') ? '' : '/'}${pathOrUrl}`;
      const resp = await fetch(url, {
        method,
        headers: this.buildHeaders(headers),
        body,
        signal: controller.signal
      });

      const text = await resp.text();
      const headerMap: Record<string, string> = {};
      resp.headers.forEach((value, key) => {
        headerMap[key.toLowerCase()] = value;
      });

      return {
        status: resp.status,
        headers: headerMap,
        body: text
      };
    } finally {
      clearTimeout(timeout);
    }
  }

  async discovery() {
    return this.fetchText('GET', '/sap/bc/adt/discovery');
  }

  async listPackageObjects(packageName: string, includeSubpackages = false) {
    const qp = includeSubpackages ? '?includesubpackages=true' : '';
    return this.fetchText(
      'GET',
      `/sap/bc/adt/packages/${encodeURIComponent(packageName)}/objectstructure${qp}`,
      undefined,
      { Accept: 'application/xml, text/xml, */*' }
    );
  }

  async readObject(objectUri: string, accept = 'text/plain, application/xml, */*') {
    return this.fetchText('GET', objectUri, undefined, { Accept: accept });
  }

  async updateObject(objectUri: string, source: string, contentType = 'text/plain; charset=utf-8', lockHandle?: string) {
    const headers: Record<string, string> = {
      'Content-Type': contentType,
      Accept: 'application/xml, text/plain, */*'
    };
    if (lockHandle && lockHandle.trim()) {
      headers['X-lockHandle'] = lockHandle.trim();
    }
    return this.fetchText('PUT', objectUri, source, headers);
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
}

function escapeXmlAttr(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/"/g, '&quot;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

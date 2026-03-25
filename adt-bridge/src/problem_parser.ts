export type AdtProblem = {
  severity: string;
  message: string;
  line?: number;
  column?: number;
  object_uri?: string;
  code?: string;
};

function decodeXml(s: string): string {
  return s
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&amp;/g, '&');
}

function attr(tag: string, name: string): string | undefined {
  const rx = new RegExp(`${name}="([^"]*)"`, 'i');
  return tag.match(rx)?.[1];
}

function parseStartLocation(uri: string | undefined): { line?: number; column?: number } {
  if (!uri) {
    return {};
  }

  const match = uri.match(/#start=(\d+),(\d+)/i);
  if (!match) {
    return {};
  }

  const line = Number.parseInt(match[1] ?? '', 10);
  const column = Number.parseInt(match[2] ?? '', 10);
  return {
    line: Number.isFinite(line) ? line : undefined,
    column: Number.isFinite(column) ? column : undefined
  };
}

export function parseProblems(xml: string): AdtProblem[] {
  const out: AdtProblem[] = [];

  const messageTag = /<(?:adtcore:)?message\b([^>]*)>([\s\S]*?)<\/(?:adtcore:)?message>/gi;
  for (const m of xml.matchAll(messageTag)) {
    const attrs = m[1] ?? '';
    const body = decodeXml((m[2] ?? '').trim());
    const line = Number.parseInt(attr(attrs, 'line') ?? '', 10);
    const column = Number.parseInt(attr(attrs, 'column') ?? '', 10);
    out.push({
      severity: (attr(attrs, 'severity') ?? 'error').toLowerCase(),
      message: body,
      line: Number.isFinite(line) ? line : undefined,
      column: Number.isFinite(column) ? column : undefined,
      object_uri: attr(attrs, 'objectUri') ?? attr(attrs, 'uri'),
      code: attr(attrs, 'code')
    });
  }

  if (out.length > 0) {
    return out;
  }

  const checkMessageTag = /<(?:[^\s>]+:)?checkMessage\b([^>]*)>([\s\S]*?)<\/(?:[^\s>]+:)?checkMessage>/gi;
  for (const m of xml.matchAll(checkMessageTag)) {
    const attrs = m[1] ?? '';
    const bodyXml = m[2] ?? '';
    const uri = attr(attrs, 'uri');
    const startLoc = parseStartLocation(uri);
    const shortText = attr(attrs, 'shortText')
      ?? bodyXml.match(/<(?:[^\s>]+:)?shortText\b[^>]*>([\s\S]*?)<\/(?:[^\s>]+:)?shortText>/i)?.[1]
      ?? bodyXml.match(/<(?:[^\s>]+:)?txt\b[^>]*>([\s\S]*?)<\/(?:[^\s>]+:)?txt>/i)?.[1]
      ?? '';
    const body = decodeXml(shortText.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim());
    if (!body) {
      continue;
    }
    out.push({
      severity: (attr(attrs, 'severity') ?? attr(attrs, 'type') ?? 'error').toLowerCase(),
      message: body,
      line: startLoc.line,
      column: startLoc.column,
      object_uri: uri,
      code: attr(attrs, 'code') ?? attr(attrs, 'msgid')
    });
  }

  if (out.length > 0) {
    return out;
  }

  const genericTag = /<(?:[^\s>]+:)?(?:problem|item|entry|msg)\b([^>]*)>([\s\S]*?)<\/(?:[^\s>]+:)?(?:problem|item|entry|msg)>/gi;
  for (const m of xml.matchAll(genericTag)) {
    const attrs = m[1] ?? '';
    const rawBody = m[2] ?? '';
    const shortText = rawBody.match(/<(?:[^\s>]+:)?shortText\b[^>]*>[\s\S]*?<(?:[^\s>]+:)?txt\b[^>]*>([\s\S]*?)<\/(?:[^\s>]+:)?txt>[\s\S]*?<\/(?:[^\s>]+:)?shortText>/i)?.[1];
    const body = decodeXml(((shortText ?? rawBody) ?? '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim());
    if (!body) {
      continue;
    }
    const line = Number.parseInt(attr(attrs, 'line') ?? '', 10);
    const column = Number.parseInt(attr(attrs, 'column') ?? '', 10);
    out.push({
      severity: (attr(attrs, 'severity') ?? attr(attrs, 'type') ?? 'error').toLowerCase(),
      message: body,
      line: Number.isFinite(line) ? line : undefined,
      column: Number.isFinite(column) ? column : undefined,
      object_uri: attr(attrs, 'objectUri') ?? attr(attrs, 'uri') ?? attr(attrs, 'href'),
      code: attr(attrs, 'code')
    });
  }

  return out;
}

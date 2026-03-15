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

  const genericTag = /<(?:[^\s>]+:)?(?:problem|item|entry)\b([^>]*)>([\s\S]*?)<\/(?:[^\s>]+:)?(?:problem|item|entry)>/gi;
  for (const m of xml.matchAll(genericTag)) {
    const attrs = m[1] ?? '';
    const body = decodeXml((m[2] ?? '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim());
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
      object_uri: attr(attrs, 'objectUri') ?? attr(attrs, 'uri'),
      code: attr(attrs, 'code')
    });
  }

  return out;
}

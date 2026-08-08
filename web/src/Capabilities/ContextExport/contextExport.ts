import type { RepoTreeEntry } from '../../api';
import type { FileTreeEntry, FileTreePathState } from './FileTree';

export type ContextPathState = FileTreePathState;

export type ContextPathResolution = {
  state: ContextPathState;
  reason: string;
};

export type ContextExportConfig = {
  includeDirectories: string[];
  includeFiles: string[];
  excludeDirectories: string[];
  excludeFiles: string[];
  excludeRegex: string[];
  includeOverrideRegex: string[];
};

export type ResolvedContextExport = {
  entries: FileTreeEntry[];
  includedFiles: string[];
};

function normalize(path: string) {
  return path.trim().replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
}

function isWithin(path: string, directory: string) {
  return path === directory || path.startsWith(`${directory}/`);
}

function compilePatterns(patterns: string[]) {
  return patterns.flatMap((pattern) => {
    try {
      return [new RegExp(pattern)];
    } catch {
      return [];
    }
  });
}

export function resolveContextExport(entries: RepoTreeEntry[], config: ContextExportConfig): ResolvedContextExport {
  const normalizedEntries = entries.map((entry) => ({ ...entry, path: normalize(entry.path) })).filter((entry) => entry.path);
  const includeDirectories = config.includeDirectories.map(normalize).filter(Boolean);
  const includeFiles = new Set(config.includeFiles.map(normalize).filter(Boolean));
  const excludeDirectories = config.excludeDirectories.map(normalize).filter(Boolean);
  const excludeFiles = new Set(config.excludeFiles.map(normalize).filter(Boolean));
  const excludes = compilePatterns(config.excludeRegex);
  const overrides = compilePatterns(config.includeOverrideRegex);
  const files = normalizedEntries.filter((entry) => entry.kind === 'file').map((entry) => entry.path);
  const fileResolutions = new Map<string, ContextPathResolution>();

  const resolveFile = (path: string): ContextPathResolution => {
    const normalized = normalize(path);
    const cached = fileResolutions.get(normalized);
    if (cached) return cached;

    const explicitlyIncluded = includeFiles.has(normalized);
    const includedDirectory = includeDirectories.find((directory) => isWithin(normalized, directory));
    const explicitlyExcluded = excludeFiles.has(normalized);
    const excludedDirectory = excludeDirectories.find((directory) => isWithin(normalized, directory));
    const override = overrides.find((pattern) => pattern.test(normalized));
    const exclusion = excludes.find((pattern) => pattern.test(normalized));
    let resolution: ContextPathResolution;

    if (!explicitlyIncluded && !includedDirectory) {
      resolution = { state: 'neutral', reason: 'Not selected' };
    } else if (override) {
      resolution = { state: 'included', reason: `Included by override ${override.source}` };
    } else if (explicitlyExcluded) {
      resolution = { state: 'excluded', reason: 'Explicitly excluded' };
    } else if (excludedDirectory) {
      resolution = { state: 'excluded', reason: `Excluded by directory ${excludedDirectory}` };
    } else if (exclusion) {
      resolution = { state: 'excluded', reason: `Excluded by ${exclusion.source}` };
    } else if (explicitlyIncluded) {
      resolution = { state: 'included', reason: 'Explicitly included' };
    } else {
      resolution = { state: 'included', reason: `Included by directory ${includedDirectory}` };
    }

    fileResolutions.set(normalized, resolution);
    return resolution;
  };

  const resolveDirectory = (path: string): ContextPathResolution => {
    const descendantFiles = files.filter((file) => isWithin(file, path));

    if (descendantFiles.length === 0) {
      return { state: 'neutral', reason: 'No files' };
    }

    const states = descendantFiles.map((file) => resolveFile(file).state);
    const included = states.filter((state) => state === 'included').length;
    const excluded = states.filter((state) => state === 'excluded').length;
    const neutral = states.filter((state) => state === 'neutral').length;

    if (included === 0 && excluded === 0) {
      return { state: 'neutral', reason: 'Not selected' };
    }

    if (included === 0 && neutral === 0) {
      return { state: 'excluded', reason: `${excluded} excluded files` };
    }

    if (excluded > 0 || neutral > 0) {
      return {
        state: 'mixed',
        reason: `${included} included, ${excluded} excluded, ${neutral} not selected`
      };
    }

    return { state: 'included', reason: `${included} included files` };
  };

  const resolvedEntries = normalizedEntries.map((entry): FileTreeEntry => {
    const resolution = entry.kind === 'dir' ? resolveDirectory(entry.path) : resolveFile(entry.path);
    return {
      ...entry,
      state: resolution.state,
      reason: resolution.reason,
      selected: entry.kind === 'dir' ? includeDirectories.includes(entry.path) : includeFiles.has(entry.path)
    };
  });

  return {
    entries: resolvedEntries,
    includedFiles: files.filter((file) => resolveFile(file).state === 'included')
  };
}

import { useMemo } from 'react';
import type { RepoTreeEntry } from '../../api';
import { FileTree } from './FileTree';
import {
  resolveContextExport,
  type ContextPathResolution,
  type ContextPathState
} from './contextExport';

export type { ContextPathResolution, ContextPathState };

type Props = {
  entries: RepoTreeEntry[];
  includeDirectories: string[];
  includeFiles: string[];
  excludeDirectories: string[];
  excludeFiles: string[];
  excludeRegex: string[];
  includeOverrideRegex: string[];
  onToggleDirectory: (path: string) => void;
  onToggleFile: (path: string) => void;
};

export function ContextExportTree(props: Props) {
  const resolved = useMemo(
    () => resolveContextExport(props.entries, {
      includeDirectories: props.includeDirectories,
      includeFiles: props.includeFiles,
      excludeDirectories: props.excludeDirectories,
      excludeFiles: props.excludeFiles,
      excludeRegex: props.excludeRegex,
      includeOverrideRegex: props.includeOverrideRegex
    }),
    [
      props.entries,
      props.includeDirectories,
      props.includeFiles,
      props.excludeDirectories,
      props.excludeFiles,
      props.excludeRegex,
      props.includeOverrideRegex
    ]
  );

  return (
    <FileTree
      entries={resolved.entries}
      onDirectoryClick={props.onToggleDirectory}
      onFileClick={props.onToggleFile}
      searchable
      showExpandCollapse
      showLegend
    />
  );
}

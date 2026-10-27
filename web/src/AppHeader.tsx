import type { CSSProperties, MouseEvent, ReactNode } from 'react';
import { Box, Button, Group, Paper, Stack, Tabs, type ButtonProps, type PaperProps } from '@mantine/core';
import './AppHeader.css';

export type AppHeaderView = 'workflows' | 'supervisor' | 'templates' | 'runtime' | 'settings';

export const appSurfaceStyle: CSSProperties = {
  background: 'linear-gradient(135deg, rgba(28,126,214,0.08), rgba(255,255,255,0.025))',
  borderColor: 'rgba(139,148,158,0.26)',
};

export type AppSurfaceProps = PaperProps & {
  children?: ReactNode;
  style?: CSSProperties;
};

export type AppHeaderActionProps = ButtonProps & {
  children?: ReactNode;
  onClick?: React.MouseEventHandler<HTMLButtonElement>;
};

export function AppSurface(props: AppSurfaceProps) {
  const { children, style, ...paperProps } = props;
  return (
    <Paper withBorder radius="md" {...paperProps} style={{ ...appSurfaceStyle, ...(style ?? {}) }}>
      {children}
    </Paper>
  );
}

export function AppHeaderAction(props: AppHeaderActionProps) {
  return <Button size="xs" variant="light" {...props} />;
}

const navigation: { value: AppHeaderView; label: string; href: string }[] = [
  { value: 'workflows', label: 'Workflows', href: '/workflows' },
  { value: 'supervisor', label: 'Supervisors', href: '/supervisors' },
  { value: 'templates', label: 'Templates', href: '/templates' },
  { value: 'runtime', label: 'Runtime', href: '/runtime' },
];

export function AppHeader(props: {
  active: AppHeaderView;
  onChange: (value: AppHeaderView) => void;
  onActiveTabClick?: () => void;
  actions?: ReactNode;
  children?: ReactNode;
  contextLabel?: ReactNode;
}) {
  const navigate = (event: MouseEvent<HTMLAnchorElement | HTMLButtonElement>, value: AppHeaderView) => {
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0) return;
    event.preventDefault();
    if (value === 'supervisor' && props.active === 'supervisor' && props.onActiveTabClick) {
      props.onActiveTabClick();
      return;
    }
    props.onChange(value);
  };

  const settingsActive = props.active === 'settings';

  return (
    <Box component="header" className="mdev-header">
      <div className="mdev-header__primary">
        <a
          className="mdev-header__brand"
          href="/workflows"
          aria-label="mdev home"
          onClick={(event) => navigate(event, 'workflows')}
        >
          <span className="mdev-header__mark" aria-hidden="true">m</span>
          <span className="mdev-header__wordmark">m<span>dev</span></span>
        </a>
        <div className="mdev-header__divider" aria-hidden="true" />
        <Tabs value={props.active} className="mdev-header__tabs">
          <Tabs.List className="mdev-header__nav" aria-label="Application navigation">
            {navigation.map((item) => (
              <Tabs.Tab
                key={item.value}
                value={item.value}
                className="mdev-header__nav-item"
                renderRoot={(rootProps) => <a {...rootProps} href={item.href} />}
                onClick={(event) => navigate(event, item.value)}
              >
                {item.label}
              </Tabs.Tab>
            ))}
          </Tabs.List>
        </Tabs>
        <div className="mdev-header__utilities">
          {props.actions ? <Group gap="xs" wrap="nowrap" className="mdev-header__actions">{props.actions}</Group> : null}
          <a
            href="/settings"
            className={`mdev-header__settings${settingsActive ? ' mdev-header__settings--active' : ''}`}
            aria-current={settingsActive ? 'page' : undefined}
            title="Application settings"
            onClick={(event) => navigate(event, 'settings')}
          >
            <svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7Z" />
              <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2 2-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1 1.56V20h-2.82v-.09a1.7 1.7 0 0 0-1-1.56 1.7 1.7 0 0 0-1.88.34l-.06.06-2-2 .06-.06A1.7 1.7 0 0 0 7.5 15a1.7 1.7 0 0 0-1.56-1H5.8v-2.82h.09a1.7 1.7 0 0 0 1.56-1 1.7 1.7 0 0 0-.34-1.88l-.06-.06 2-2 .06.06A1.7 1.7 0 0 0 11 6.64a1.7 1.7 0 0 0 1-1.56V5h2.82v.09a1.7 1.7 0 0 0 1 1.56 1.7 1.7 0 0 0 1.88-.34l.06-.06 2 2-.06.06a1.7 1.7 0 0 0-.34 1.88 1.7 1.7 0 0 0 1.56 1H21v2.82h-.09a1.7 1.7 0 0 0-1.51.99Z" transform="translate(-1 -1)" />
            </svg>
            <span>Settings</span>
          </a>
        </div>
      </div>
      {props.children ? (
        <div className="mdev-header__secondary">
          <div className="mdev-header__context" aria-hidden="true">
            <span className="mdev-header__context-dot" />
            {props.contextLabel ?? 'Workspace'}
          </div>
          <div className="mdev-header__secondary-content">{props.children}</div>
        </div>
      ) : null}
    </Box>
  );
}

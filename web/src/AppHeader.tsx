import type { CSSProperties, ReactNode } from 'react';
import { Box, Button, Group, Paper, Stack, Tabs, type ButtonProps, type PaperProps } from '@mantine/core';

export type AppHeaderView = 'workflows' | 'supervisor' | 'templates' | 'runtime';

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
    <Paper
      withBorder
      radius="md"
      {...paperProps}
      style={{
        ...appSurfaceStyle,
        ...(style ?? {}),
      }}
    >
      {children}
    </Paper>
  );
}

export function AppHeaderAction(props: AppHeaderActionProps) {
  return <Button size="xs" variant="light" {...props} />;
}

export function AppHeader(props: {
  active: AppHeaderView;
  onChange: (value: AppHeaderView) => void;
  onActiveDoubleClick?: () => void;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <AppSurface px="md" style={{ overflow: 'hidden' }}>
      <Stack gap={0}>
        <Group justify="space-between" align="center" wrap="wrap" gap="sm" mih={44}>
          <Tabs
            value={props.active}
            onChange={(value) => {
              if (value === 'workflows' || value === 'supervisor' || value === 'templates' || value === 'runtime') {
                props.onChange(value);
              }
            }}
          >
            <Tabs.List style={{ borderBottom: 0 }}>
              <Tabs.Tab value="workflows">Workflows</Tabs.Tab>
              <Tabs.Tab
                value="supervisor"
                onDoubleClick={() => {
                  if (props.active === 'supervisor') props.onActiveDoubleClick?.();
                }}
              >
                Supervisors
              </Tabs.Tab>
              <Tabs.Tab value="templates">Templates</Tabs.Tab>
              <Tabs.Tab value="runtime">Runtime</Tabs.Tab>
            </Tabs.List>
          </Tabs>

          {props.actions ? <Group gap="xs" wrap="nowrap">{props.actions}</Group> : null}
        </Group>

        {props.children ? (
          <Box
            py="sm"
            style={{
              borderTop: '1px solid rgba(139,148,158,0.20)',
            }}
          >
            {props.children}
          </Box>
        ) : null}
      </Stack>
    </AppSurface>
  );
}

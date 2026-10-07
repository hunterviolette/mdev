import { useCallback, useEffect, useMemo, useState } from 'react';
import { WorkflowShell } from './WorkflowShell';

type AppRoute = {
  path: string;
  workflowRunId: string | null;
  workflowView: 'workflow' | 'changes' | 'commits' | 'repository' | 'capabilities' | null;
  supervisorRunId: string | null;
  supervisorView: 'planner' | null;
};

function shortRouteId(value: string | null): string {
  return value ? value.slice(0, 8) : '';
}

function routeTitle(route: AppRoute): string {
  if (route.workflowRunId) return 'Workflow';
  if (route.path === '/supervisors' || route.path.startsWith('/supervisors/')) return 'Supervisor';
  if (route.path === '/templates') return 'Templates';
  if (route.path === '/runtime') return 'Runtime';
  if (route.path === '/workflows' || route.path === '/') return 'Workflow list';
  return 'Workflow Web';
}


function parseRoute(): AppRoute {
  const path = window.location.pathname;
  const workflowMatch = path.match(/^\/workflows\/([^/]+)(?:\/(changes|commits|repository|capabilities))?$/);
  const supervisorModalMatch = path.match(/^\/supervisors\/([^/]+)\/planner$/);
  const supervisorMatch = path.match(/^\/supervisors\/([^/]+)/);
  return {
    path,
    workflowRunId: workflowMatch?.[1] ? decodeURIComponent(workflowMatch[1]) : null,
    workflowView: workflowMatch?.[1]
      ? ((workflowMatch[2] ?? 'workflow') as 'workflow' | 'changes' | 'commits' | 'repository' | 'capabilities')
      : null,
    supervisorRunId: supervisorMatch?.[1] ? decodeURIComponent(supervisorMatch[1]) : null,
    supervisorView: supervisorModalMatch ? 'planner' : null
  };
}

export default function App() {
  const [route, setRoute] = useState<AppRoute>(() => parseRoute());

  useEffect(() => {
    document.title = routeTitle(route);
  }, [route]);

  useEffect(() => {
    const onPopState = () => setRoute(parseRoute());
    window.addEventListener('popstate', onPopState);
    return () => window.removeEventListener('popstate', onPopState);
  }, []);

  const navigate = useCallback((path: string) => {
    if (window.location.pathname === path) {
      return;
    }
    window.history.pushState({}, '', path);
    setRoute(parseRoute());
  }, []);

  const initialRoute = useMemo(() => route, [route.path, route.workflowRunId, route.workflowView, route.supervisorRunId, route.supervisorView]);

  return <WorkflowShell route={initialRoute} navigate={navigate} />;
}

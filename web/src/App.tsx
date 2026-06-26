import { useCallback, useEffect, useMemo, useState } from 'react';
import { WorkflowShell } from './WorkflowShell';

type AppRoute = {
  path: string;
  workflowRunId: string | null;
  supervisorRunId: string | null;
};

function parseRoute(): AppRoute {
  const path = window.location.pathname;
  const workflowMatch = path.match(/^\/workflows\/([^/]+)/);
  const supervisorMatch = path.match(/^\/supervisors\/([^/]+)/);
  return {
    path,
    workflowRunId: workflowMatch?.[1] ? decodeURIComponent(workflowMatch[1]) : null,
    supervisorRunId: supervisorMatch?.[1] ? decodeURIComponent(supervisorMatch[1]) : null
  };
}

export default function App() {
  const [route, setRoute] = useState<AppRoute>(() => parseRoute());

  useEffect(() => {
    const onPopState = () => setRoute(parseRoute());
    window.addEventListener('popstate', onPopState);
    return () => window.removeEventListener('popstate', onPopState);
  }, []);

  const navigate = useCallback((path: string) => {
    if (window.location.pathname !== path) {
      window.history.pushState({}, '', path);
    }
    setRoute(parseRoute());
  }, []);

  const initialRoute = useMemo(() => route, [route.path, route.workflowRunId, route.supervisorRunId]);

  return <WorkflowShell route={initialRoute} navigate={navigate} />;
}

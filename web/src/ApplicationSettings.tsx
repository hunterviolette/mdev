import { useCallback, useEffect, useState } from 'react';
import { Alert, Button, Code, Divider, Group, Loader, Paper, Stack, Text, TextInput, Title, Radio, Badge } from '@mantine/core';

type EnvironmentFileStatus = {
  path: string;
  effective_path: string;
  exists: boolean;
  readable: boolean;
  configured: boolean;
  restart_required: boolean;
};

type BrowserOption = {
  id: string;
  name: string;
  executable_path: string;
  source: 'detected' | 'custom' | 'legacy';
  selected: boolean;
};

type BrowserValidation = {
  valid: boolean;
  browser: string;
  version: string;
  cdp_supported: boolean;
  playwright_connected: boolean;
};

async function requestBrowsers(): Promise<BrowserOption[]> {
  const response = await fetch('/api/app-settings/browsers', { credentials: 'same-origin' });
  if (!response.ok) throw new Error((await response.text()) || `Request failed: ${response.status}`);
  return response.json() as Promise<BrowserOption[]>;
}

async function addBrowser(path: string): Promise<BrowserOption & { validation: BrowserValidation }> {
  const response = await fetch('/api/app-settings/browsers', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ executable_path: path })
  });
  if (!response.ok) throw new Error((await response.text()) || `Request failed: ${response.status}`);
  return response.json() as Promise<BrowserOption & { validation: BrowserValidation }>;
}

async function deleteBrowser(id: string): Promise<void> {
  const response = await fetch(`/api/app-settings/browsers/${encodeURIComponent(id)}`, {
    method: 'DELETE',
    credentials: 'same-origin'
  });
  if (!response.ok) throw new Error((await response.text()) || `Request failed: ${response.status}`);
}

type AppSettings = {
  browser: {
    edge_executable_path?: string;
    chrome_executable_path?: string;
    [key: string]: unknown;
  };
  [key: string]: unknown;
};

async function requestEnvironmentFile(method: 'GET' | 'PUT', path?: string): Promise<EnvironmentFileStatus> {
  const response = await fetch('/api/app-settings/environment-file', {
    method,
    headers: method === 'PUT' ? { 'Content-Type': 'application/json' } : undefined,
    body: method === 'PUT' ? JSON.stringify({ path: path ?? '' }) : undefined,
    credentials: 'same-origin'
  });
  if (!response.ok) throw new Error((await response.text()) || `Request failed: ${response.status}`);
  return response.json() as Promise<EnvironmentFileStatus>;
}

async function requestAppSettings(path?: string): Promise<AppSettings> {
  const updating = path !== undefined;
  const response = await fetch('/api/app-settings', {
    method: updating ? 'PATCH' : 'GET',
    headers: updating ? { 'Content-Type': 'application/json' } : undefined,
    body: updating ? JSON.stringify({ patch: { browser: { edge_executable_path: path } } }) : undefined,
    credentials: 'same-origin'
  });
  if (!response.ok) throw new Error((await response.text()) || `Request failed: ${response.status}`);
  return response.json() as Promise<AppSettings>;
}

export function ApplicationSettings() {
  const [envPath, setEnvPath] = useState('');
  const [envStatus, setEnvStatus] = useState<EnvironmentFileStatus | null>(null);
  const [browserPath, setBrowserPath] = useState('');
  const [savedBrowserPath, setSavedBrowserPath] = useState('');
  const [browsers, setBrowsers] = useState<BrowserOption[]>([]);
  const [customPath, setCustomPath] = useState('');
  const [addingBrowser, setAddingBrowser] = useState(false);
  const [browserValidation, setBrowserValidation] = useState<BrowserValidation | null>(null);
  const [validatingBrowser, setValidatingBrowser] = useState(false);
  const [loading, setLoading] = useState(true);
  const [savingEnv, setSavingEnv] = useState(false);
  const [savingBrowser, setSavingBrowser] = useState(false);
  const [envError, setEnvError] = useState<string | null>(null);
  const [browserError, setBrowserError] = useState<string | null>(null);
  const [envSaved, setEnvSaved] = useState(false);
  const [browserSaved, setBrowserSaved] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setEnvError(null);
    setBrowserError(null);
    try {
      const [envResult, appResult, browsersResult] = await Promise.allSettled([
        requestEnvironmentFile('GET'),
        requestAppSettings(),
        requestBrowsers()
      ]);
      if (envResult.status === 'fulfilled') {
        setEnvStatus(envResult.value);
        setEnvPath(envResult.value.path);
      } else {
        setEnvError(envResult.reason instanceof Error ? envResult.reason.message : String(envResult.reason));
      }
      if (browsersResult.status === 'fulfilled') {
        setBrowsers(browsersResult.value);
      } else {
        setBrowserError(browsersResult.reason instanceof Error ? browsersResult.reason.message : String(browsersResult.reason));
      }
      if (appResult.status === 'fulfilled') {
        const value = appResult.value.browser?.edge_executable_path ?? '';
        setBrowserPath(value);
        setSavedBrowserPath(value);
      } else {
        setBrowserError(appResult.reason instanceof Error ? appResult.reason.message : String(appResult.reason));
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);

  const saveEnvironment = async () => {
    setSavingEnv(true);
    setEnvError(null);
    setEnvSaved(false);
    try {
      const next = await requestEnvironmentFile('PUT', envPath);
      setEnvStatus(next);
      setEnvPath(next.path);
      setEnvSaved(true);
    } catch (reason) {
      setEnvError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSavingEnv(false);
    }
  };

  const validateAndAddBrowser = async () => {
    setValidatingBrowser(true);
    setBrowserError(null);
    setBrowserValidation(null);
    try {
      const result = await addBrowser(customPath.trim());
      setBrowserValidation(result.validation);
      setBrowsers(await requestBrowsers());
      setBrowserPath(result.executable_path);
      setBrowserSaved(false);
      setCustomPath('');
      setAddingBrowser(false);
    } catch (reason) {
      setBrowserError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setValidatingBrowser(false);
    }
  };

  const removeCustomBrowser = async (browser: BrowserOption) => {
    setBrowserError(null);
    try {
      await deleteBrowser(browser.id);
      setBrowsers(await requestBrowsers());
      if (browserPath === browser.executable_path) {
        setBrowserPath('');
        setSavedBrowserPath('');
      }
      setBrowserValidation(null);
    } catch (reason) {
      setBrowserError(reason instanceof Error ? reason.message : String(reason));
    }
  };

  const saveBrowser = async () => {
    setSavingBrowser(true);
    setBrowserError(null);
    setBrowserSaved(false);
    try {
      const next = await requestAppSettings(browserPath.trim());
      const value = next.browser?.edge_executable_path ?? '';
      setBrowserPath(value);
      setSavedBrowserPath(value);
      setBrowserSaved(true);
      setBrowsers(await requestBrowsers());
    } catch (reason) {
      setBrowserError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setSavingBrowser(false);
    }
  };

  return (
    <Paper withBorder radius="md" p="lg">
      <Stack gap="lg">
        <Title order={3}>Application Settings</Title>
        {loading ? <Loader size="sm" /> : (
          <>
            <Stack gap="sm">
              <Title order={4}>Environment Configuration</Title>
              <Text size="sm" c="dimmed">Select the environment file loaded when MDEV starts. The application-root .env file takes precedence when present.</Text>
              <TextInput
                label="Environment file path"
                description="Enter an absolute path to an existing .env file. Leave empty to use the application default."
                placeholder="C:\\Users\\YourName\\mdev\\.env"
                value={envPath}
                onChange={(event) => { setEnvPath(event.currentTarget.value); setEnvSaved(false); }}
                disabled={savingEnv}
              />
              {envStatus && (
                <Stack gap={4}>
                  <Text size="sm">Effective path: <Code>{envStatus.effective_path}</Code></Text>
                  <Text size="sm" c={envStatus.readable ? 'green' : 'dimmed'}>{envStatus.readable ? 'Environment file is readable' : 'Environment file is not currently readable'}</Text>
                </Stack>
              )}
              {envError && <Alert color="red">{envError}</Alert>}
              {envSaved && <Alert color="green">Environment configuration saved. Restart MDEV to load the selected environment file.</Alert>}
              <Group justify="flex-end">
                <Button onClick={() => void saveEnvironment()} loading={savingEnv}>Save Environment Configuration</Button>
              </Group>
            </Stack>
            <Divider />
            <Stack gap="sm">
              <Title order={4}>Browser Configuration</Title>
              <Text size="sm" c="dimmed">Select the installed Chromium-based browser used for MDEV browser automation. Compatible browsers include Microsoft Edge, Google Chrome, Chromium and Brave.</Text>
              <Radio.Group value={browserPath} onChange={(value) => { setBrowserPath(value); setBrowserSaved(false); setBrowserValidation(null); }}>
                <Stack gap="xs">
                  <Radio value="" label="Automatic browser selection" description="Use MDEV's built-in browser discovery order" />
                  {browsers.map((browser) => (
                    <Group key={browser.id} justify="space-between" align="center" wrap="nowrap">
                      <Radio
                        value={browser.executable_path}
                        label={<Group gap="xs"><Text size="sm">{browser.name}</Text><Badge size="xs" color={browser.source === 'custom' ? 'blue' : 'gray'}>{browser.source}</Badge></Group>}
                        description={<Text size="xs" style={{ overflowWrap: 'anywhere' }}>{browser.executable_path}</Text>}
                      />
                      {browser.source === 'custom' && <Button size="xs" variant="subtle" color="red" onClick={() => void removeCustomBrowser(browser)} disabled={savingBrowser || validatingBrowser}>Remove</Button>}
                    </Group>
                  ))}
                </Stack>
              </Radio.Group>
              {browsers.length === 0 && <Text size="sm" c="dimmed">No installed browsers were discovered. Add a compatible browser executable below.</Text>}
              {!addingBrowser ? (
                <Group><Button variant="light" onClick={() => setAddingBrowser(true)}>Add browser</Button><Button variant="subtle" onClick={() => void refresh()}>Refresh detected browsers</Button></Group>
              ) : (
                <Stack gap="xs">
                  <TextInput
                    label="Custom browser executable path"
                    description="The executable must launch a CDP server and pass a real Playwright connection test."
                    placeholder="C:\\Path\\To\\Browser.exe"
                    value={customPath}
                    onChange={(event) => { setCustomPath(event.currentTarget.value); setBrowserValidation(null); }}
                    disabled={validatingBrowser}
                  />
                  <Group>
                    <Button loading={validatingBrowser} disabled={!customPath.trim()} onClick={() => void validateAndAddBrowser()}>Validate and add</Button>
                    <Button variant="default" disabled={validatingBrowser} onClick={() => { setAddingBrowser(false); setCustomPath(''); }}>Cancel</Button>
                  </Group>
                </Stack>
              )}
              {browserValidation && <Alert color="green">{browserValidation.browser} {browserValidation.version} passed CDP and Playwright validation. Save the browser configuration to use it for new sessions.</Alert>}
              {browserError && <Alert color="red">{browserError}</Alert>}
              {browserSaved && <Alert color="green">Browser configuration saved. New browser sessions will use the selected executable. MDEV does not need to restart.</Alert>}
              {browserPath !== savedBrowserPath && <Text size="xs" c="yellow">Unsaved browser configuration changes</Text>}
              <Text size="sm" c="dimmed">Existing browser sessions continue using their current browser until they are closed and relaunched. The selected path is shared through application settings.</Text>
              <Group justify="flex-end">
                <Button onClick={() => void saveBrowser()} loading={savingBrowser} disabled={browserPath === savedBrowserPath}>Save Browser Configuration</Button>
              </Group>
            </Stack>
            <Divider />
            <Group justify="flex-end">
              <Button variant="default" onClick={() => void refresh()} disabled={savingEnv || savingBrowser}>Reload Settings</Button>
            </Group>
          </>
        )}
      </Stack>
    </Paper>
  );
}

// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AppShell } from './AppShell';
import { SettingsView } from './UtilityViews';
import { generateThemeTokens } from '../lib/theme';
import { useUiStore } from '../stores/ui';

const api = vi.hoisted(() => ({ invoke: vi.fn(), settings: vi.fn(), update: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: api.invoke }));
vi.mock('../lib/tauri', async (importOriginal) => ({
  ...await importOriginal<typeof import('../lib/tauri')>(),
  isTauriRuntime: true,
  getSettings: api.settings,
  updateSettings: api.update,
}));
vi.mock('./BackgroundSettings', () => ({ BackgroundSettings: () => null }));
vi.mock('../lib/icons', () => ({ Icon: () => null }));
vi.mock('../lib/platform-permissions', () => ({ getAllFilesAccess: async () => 'not-applicable' }));

const clients: QueryClient[] = [];
beforeEach(() => {
  vi.resetAllMocks();
  vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Mozilla/5.0 (Linux; Android 16)');
  vi.stubGlobal('matchMedia', () => ({ matches: false, addEventListener: vi.fn(), removeEventListener: vi.fn() }));
  useUiStore.setState({ themeMode: 'dark', themePalette: 'matcha', androidDynamicColor: false, androidDynamicSeed: null });
  api.settings.mockResolvedValue({ theme: 'dark', colorScheme: 'matcha', customThemeSeed: '#3F6654', androidDynamicColor: false, safeReading: true, syncPolicy: 'automatic' });
  api.update.mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup();
  clients.splice(0).forEach((client) => client.clear());
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function show(settings = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  clients.push(client);
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[settings ? '/settings' : '/mail']}>
        <AppShell accounts={[]} selectedAccountId={null} mailboxes={[]} messageCount={0} onSelectAccount={vi.fn()} onAddAccount={vi.fn()}>
          {settings && <SettingsView accounts={[]} onAddAccount={vi.fn()} onRemoveAccount={async () => {}} />}
        </AppShell>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

it('recovers from an IPC rejection without calling a supported device unsupported', async () => {
  api.invoke.mockRejectedValueOnce(new Error('Command plugin:dynamic-color|palette not allowed by ACL'))
    .mockResolvedValue({ available: true, seedHex: '#527ABC' });
  show();
  await screen.findByText('暂时无法读取系统配色，请重试。');
  expect(screen.queryByText(/此设备不支持/)).toBeNull();
  const control = screen.getByRole('switch', { name: '系统动态配色' }) as HTMLButtonElement;
  expect(control.disabled).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: '重新读取' }));
  await waitFor(() => expect(control.disabled).toBe(false));
  fireEvent.click(control);
  await waitFor(() => expect(control.getAttribute('aria-checked')).toBe('true'));
  expect(api.invoke).toHaveBeenCalledWith('plugin:dynamic-color|palette');
  expect(api.update).toHaveBeenCalledWith({ androidDynamicColor: true });
  expect(useUiStore.getState().androidDynamicSeed).toBe('#527ABC');
  expect(document.documentElement.style.getPropertyValue('--md-sys-color-primary'))
    .toBe(generateThemeTokens('#527ABC', true)['--md-sys-color-primary']);
});

it('only reports unsupported when the native API explicitly returns unavailable', async () => {
  api.invoke.mockResolvedValue({ available: false });
  show();
  await screen.findByText(/此设备不支持 Android 12 动态配色/);
  expect((screen.getByRole('switch', { name: '系统动态配色' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: '重新读取' })).toBeNull();
});

it('keeps an enabled palette visible and allows turning it off after a read failure', async () => {
  useUiStore.setState({ androidDynamicColor: true, androidDynamicSeed: '#527ABC' });
  api.settings.mockResolvedValue({ theme: 'dark', colorScheme: 'matcha', customThemeSeed: '#3F6654', androidDynamicColor: true, safeReading: true, syncPolicy: 'automatic' });
  api.invoke.mockRejectedValue(new Error('temporarily unavailable'));
  show();
  await screen.findByText('暂时无法读取系统配色，请重试。');
  const control = screen.getByRole('switch', { name: '系统动态配色' }) as HTMLButtonElement;
  expect(control.getAttribute('aria-checked')).toBe('true');
  expect(control.disabled).toBe(false);
  expect(useUiStore.getState().androidDynamicSeed).toBe('#527ABC');
  fireEvent.click(control);
  expect(control.getAttribute('aria-checked')).toBe('false');
  expect(api.update).toHaveBeenCalledWith({ androidDynamicColor: false });
});

it('does not enable dynamic color with a missing or malformed seed', async () => {
  api.invoke.mockResolvedValueOnce({ available: true }).mockResolvedValue({ available: true, seedHex: 'invalid' });
  show();
  await screen.findByText('暂时无法读取系统配色，请重试。');
  fireEvent.click(screen.getByRole('button', { name: '重新读取' }));
  await screen.findByText('暂时无法读取系统配色，请重试。');
  expect((screen.getByRole('switch', { name: '系统动态配色' }) as HTMLButtonElement).disabled).toBe(true);
  expect(useUiStore.getState().androidDynamicSeed).toBeNull();
});

it('retries startup failures on focus and refreshes a changed system color', async () => {
  useUiStore.setState({ androidDynamicColor: true, androidDynamicSeed: '#527ABC' });
  api.invoke.mockRejectedValueOnce(new Error('temporarily unavailable'))
    .mockResolvedValueOnce({ available: true, seedHex: '#AD536B' })
    .mockResolvedValue({ available: true, seedHex: '#478F65' });
  show(false);
  await waitFor(() => expect(api.invoke).toHaveBeenCalledTimes(1));
  expect(useUiStore.getState().androidDynamicSeed).toBe('#527ABC');
  fireEvent.focus(window);
  await waitFor(() => expect(useUiStore.getState().androidDynamicSeed).toBe('#AD536B'));
  fireEvent.focus(window);
  await waitFor(() => expect(useUiStore.getState().androidDynamicSeed).toBe('#478F65'));
  expect(document.documentElement.style.getPropertyValue('--md-sys-color-primary'))
    .toBe(generateThemeTokens('#478F65', true)['--md-sys-color-primary']);
});

it('does not expose Android dynamic color on desktop', async () => {
  vi.spyOn(navigator, 'userAgent', 'get').mockReturnValue('Mozilla/5.0 (Macintosh)');
  show();
  await screen.findByText('配色组合');
  expect(screen.queryByRole('switch', { name: '系统动态配色' })).toBeNull();
  expect(api.invoke).not.toHaveBeenCalled();
});

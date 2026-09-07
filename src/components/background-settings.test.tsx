// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { BackgroundSettings } from './BackgroundSettings';

const api = vi.hoisted(() => ({ get: vi.fn(), startup: vi.fn(), update: vi.fn(), open: vi.fn() }));
vi.mock('../lib/background', () => ({ getBackgroundStatus: api.get, setLaunchAtLogin: api.startup, openBackgroundSettings: api.open }));
vi.mock('../lib/tauri', () => ({ isTauriRuntime: true, updateSettings: api.update, appErrorMessage: (error: Error) => error.message }));
vi.mock('../lib/icons', () => ({ Icon: () => null }));
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));
beforeEach(() => { vi.clearAllMocks(); });
afterEach(cleanup);
function show() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><BackgroundSettings /></QueryClientProvider>);
  return client;
}

it('keeps the real startup state when the OS rejects registration', async () => {
  api.get.mockResolvedValue({ platform: 'macos', backgroundMail: true, autostartSupported: true, launchAtLogin: false });
  api.startup.mockRejectedValue(new Error('登录启动设置失败'));
  const client = show();
  const control = await screen.findByRole('switch', { name: '登录时启动' });
  fireEvent.click(control);
  expect((await screen.findByRole('alert')).textContent).toContain('登录启动设置失败');
  expect(control.getAttribute('aria-checked')).toBe('false');
  expect(api.startup).toHaveBeenCalledWith(true);
  client.clear();
});

it('shows Android service expiry and only opens battery settings after a click', async () => {
  api.get.mockResolvedValue({ platform: 'android', backgroundMail: true, autostartSupported: false, quotaExpired: true, serviceRunning: false, batteryUnrestricted: false });
  api.open.mockResolvedValue(undefined);
  const client = show();
  await screen.findByText(/本轮后台运行时间已用完/);
  expect(screen.queryByRole('switch', { name: '登录时启动' })).toBeNull();
  expect(api.open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: '管理后台权限' }));
  await waitFor(() => expect(api.open).toHaveBeenCalledOnce());
  client.clear();
});

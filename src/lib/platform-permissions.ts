import { invoke } from '@tauri-apps/api/core';
import { isTauriRuntime } from './tauri';

export type AllFilesAccess = 'granted' | 'not-granted' | 'not-applicable';

function isAndroidRuntime() {
  return isTauriRuntime && /android/i.test(navigator.userAgent);
}

export async function getAllFilesAccess(): Promise<AllFilesAccess> {
  if (!isAndroidRuntime()) return 'not-applicable';
  const result = await invoke<{ granted: boolean }>('plugin:all-files-access|status');
  return result.granted ? 'granted' : 'not-granted';
}

export async function requestAllFilesAccess(): Promise<AllFilesAccess> {
  if (!isAndroidRuntime()) return 'not-applicable';
  await invoke('plugin:all-files-access|request');
  return getAllFilesAccess();
}

export async function requestNotificationAccess(): Promise<'granted' | 'denied' | 'not-applicable'> {
  if (!isTauriRuntime) return 'not-applicable';
  const notifications = await import('@tauri-apps/plugin-notification');
  if (await notifications.isPermissionGranted()) return 'granted';
  return (await notifications.requestPermission()) === 'granted' ? 'granted' : 'denied';
}

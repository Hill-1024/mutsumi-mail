import { invoke } from '@tauri-apps/api/core';

export interface BackgroundStatus {
  platform: string;
  backgroundMail: boolean;
  autostartSupported: boolean;
  launchAtLogin?: boolean;
  serviceRunning?: boolean;
  engineReady?: boolean;
  online?: boolean;
  quotaExpired?: boolean;
  batteryUnrestricted?: boolean;
  notificationsAllowed?: boolean;
}

export const getBackgroundStatus = () => invoke<BackgroundStatus>('get_background_status');
export const setLaunchAtLogin = (enabled: boolean) => invoke<void>('set_launch_at_login', { enabled });
export const openBackgroundSettings = () => invoke<void>('open_background_settings');

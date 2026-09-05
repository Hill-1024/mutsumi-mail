import {
  Hct,
  SchemeTonalSpot,
  argbFromHex,
  hexFromArgb,
  type DynamicScheme,
} from '@material/material-color-utilities';
import { invoke } from '@tauri-apps/api/core';
import { isTauriRuntime } from './tauri';

export type ThemePaletteId = 'matcha' | 'mutsumi' | 'lavender' | 'ocean' | 'sunset' | 'custom';

export interface ThemePalettePreset {
  id: Exclude<ThemePaletteId, 'custom'>;
  name: string;
  seed: string;
  description: string;
}

export const THEME_PALETTES: ThemePalettePreset[] = [
  { id: 'matcha', name: '抹茶', seed: '#879A6C', description: '柔和自然的灰绿色' },
  { id: 'mutsumi', name: '睦头模式', seed: '#76885F', description: '若叶睦专属角色主题' },
  { id: 'lavender', name: '薰衣草', seed: '#6750A4', description: '经典 Material 紫' },
  { id: 'ocean', name: '海洋', seed: '#006A6A', description: '清晰沉静的青蓝色' },
  { id: 'sunset', name: '日落', seed: '#984061', description: '温暖克制的玫红色' },
];

export const DEFAULT_CUSTOM_SEED = '#3F6654';

const token = (name: string) => `--md-sys-color-${name}`;
const color = (value: number) => hexFromArgb(value).toUpperCase();

export function schemeTokens(scheme: DynamicScheme): Record<string, string> {
  return {
    [token('primary')]: color(scheme.primary),
    [token('on-primary')]: color(scheme.onPrimary),
    [token('primary-container')]: color(scheme.primaryContainer),
    [token('on-primary-container')]: color(scheme.onPrimaryContainer),
    [token('secondary')]: color(scheme.secondary),
    [token('on-secondary')]: color(scheme.onSecondary),
    [token('secondary-container')]: color(scheme.secondaryContainer),
    [token('on-secondary-container')]: color(scheme.onSecondaryContainer),
    [token('tertiary')]: color(scheme.tertiary),
    [token('on-tertiary')]: color(scheme.onTertiary),
    [token('tertiary-container')]: color(scheme.tertiaryContainer),
    [token('on-tertiary-container')]: color(scheme.onTertiaryContainer),
    [token('error')]: color(scheme.error),
    [token('on-error')]: color(scheme.onError),
    [token('error-container')]: color(scheme.errorContainer),
    [token('on-error-container')]: color(scheme.onErrorContainer),
    [token('background')]: color(scheme.background),
    [token('on-background')]: color(scheme.onBackground),
    [token('surface')]: color(scheme.surface),
    [token('on-surface')]: color(scheme.onSurface),
    [token('surface-variant')]: color(scheme.surfaceVariant),
    [token('on-surface-variant')]: color(scheme.onSurfaceVariant),
    [token('outline')]: color(scheme.outline),
    [token('outline-variant')]: color(scheme.outlineVariant),
    [token('surface-dim')]: color(scheme.surfaceDim),
    [token('surface-bright')]: color(scheme.surfaceBright),
    [token('surface-container-lowest')]: color(scheme.surfaceContainerLowest),
    [token('surface-container-low')]: color(scheme.surfaceContainerLow),
    [token('surface-container')]: color(scheme.surfaceContainer),
    [token('surface-container-high')]: color(scheme.surfaceContainerHigh),
    [token('surface-container-highest')]: color(scheme.surfaceContainerHighest),
    [token('inverse-surface')]: color(scheme.inverseSurface),
    [token('inverse-on-surface')]: color(scheme.inverseOnSurface),
    [token('inverse-primary')]: color(scheme.inversePrimary),
  };
}

export function generateThemeTokens(seed: string, dark: boolean): Record<string, string> {
  const scheme = new SchemeTonalSpot(Hct.fromInt(argbFromHex(seed)), dark, 0);
  return schemeTokens(scheme);
}

export function applyThemeTokens(seed: string, dark: boolean) {
  const root = document.documentElement;
  const tokens = generateThemeTokens(seed, dark);
  for (const [property, value] of Object.entries(tokens)) root.style.setProperty(property, value);
  root.style.colorScheme = dark ? 'dark' : 'light';
  document
    .querySelector<HTMLMetaElement>('meta[name="theme-color"]')
    ?.setAttribute('content', tokens[token('background')]);
}

export function paletteSeed(paletteId: ThemePaletteId, customSeed: string) {
  if (paletteId === 'custom') return customSeed;
  return THEME_PALETTES.find((palette) => palette.id === paletteId)?.seed ?? THEME_PALETTES[0].seed;
}

export interface AndroidDynamicColorResult {
  available: boolean;
  seedHex?: string;
}

export async function getAndroidDynamicColor(): Promise<AndroidDynamicColorResult> {
  if (!isTauriRuntime || !/android/i.test(navigator.userAgent)) return { available: false };
  return invoke<AndroidDynamicColorResult>('plugin:dynamic-color|palette');
}

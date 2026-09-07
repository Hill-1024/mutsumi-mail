import { create } from 'zustand';
import { sanitizeSeed, THEME_PALETTES } from '../lib/theme';
import type { ThemeMode, ThemePaletteId } from '../types';

const THEME_STORAGE_KEY = 'mutsumi_theme_mode';
const THEME_PALETTE_STORAGE_KEY = 'mutsumi_theme_palette';
const THEME_CUSTOM_SEED_STORAGE_KEY = 'mutsumi_theme_custom_seed';
const THEME_DYNAMIC_STORAGE_KEY = 'mutsumi_theme_android_dynamic';

function readPreference(key: string): string | null {
  try { return typeof window === 'undefined' ? null : window.localStorage.getItem(key); }
  catch { return null; }
}
function writePreference(key: string, value: string) {
  try { window.localStorage.setItem(key, value); } catch { /* Keep the in-memory preference usable. */ }
}
function initialPalette(): ThemePaletteId {
  const saved = readPreference(THEME_PALETTE_STORAGE_KEY);
  if (saved === 'custom') return saved;
  return THEME_PALETTES.find((palette) => palette.id === saved)?.id ?? 'matcha';
}

const getInitialTheme = (): ThemeMode => {
  if (typeof window === 'undefined') return 'dark';
  const saved = readPreference(THEME_STORAGE_KEY);
  if (saved === 'light' || saved === 'dark' || saved === 'system') return saved;
  return 'dark';
};

export interface ComposeDraftState {
  accountId?: string;
  to?: string;
  cc?: string;
  bcc?: string;
  subject?: string;
  bodyText?: string;
  inReplyTo?: string;
  references?: string[];
}

export interface UiState {
  themeMode: ThemeMode;
  themePalette: ThemePaletteId;
  customThemeSeed: string;
  androidDynamicColor: boolean;
  androidDynamicSeed: string | null;
  safeReading: boolean;
  selectedMailboxId: string;
  selectedMessageId: string | null;
  searchOpen: boolean;
  composeOpen: boolean;
  composeDraft: ComposeDraftState | null;
  navPage: 'mail' | 'search' | 'outbox' | 'settings';
  syncMessage: string | null;
  setThemeMode: (themeMode: ThemeMode) => void;
  setThemePalette: (themePalette: ThemePaletteId) => void;
  setCustomThemeSeed: (customThemeSeed: string) => void;
  setAndroidDynamicColor: (androidDynamicColor: boolean) => void;
  setAndroidDynamicSeed: (androidDynamicSeed: string | null) => void;
  toggleSafeReading: () => void;
  setSafeReading: (value: boolean) => void;
  selectMailbox: (id: string) => void;
  selectMessage: (id: string | null) => void;
  setSearchOpen: (value: boolean) => void;
  setComposeOpen: (value: boolean) => void;
  openComposeWithDraft: (draft: ComposeDraftState) => void;
  clearComposeDraft: () => void;
  setNavPage: (page: UiState['navPage']) => void;
  setSyncMessage: (message: string | null) => void;
}

export const useUiStore = create<UiState>((set) => ({
  themeMode: getInitialTheme(),
  themePalette: initialPalette(),
  customThemeSeed: sanitizeSeed(
    typeof window !== 'undefined'
      ? readPreference(THEME_CUSTOM_SEED_STORAGE_KEY)
      : null,
  ),
  androidDynamicColor:
    typeof window !== 'undefined' &&
    readPreference(THEME_DYNAMIC_STORAGE_KEY) === 'true',
  androidDynamicSeed: null,
  safeReading: true,
  selectedMailboxId: 'inbox',
  selectedMessageId: null,
  searchOpen: false,
  composeOpen: false,
  composeDraft: null,
  navPage: 'mail',
  syncMessage: null,
  setThemeMode: (themeMode) => {
    if (typeof window !== 'undefined') {
      writePreference(THEME_STORAGE_KEY, themeMode);
    }
    set({ themeMode });
  },
  setThemePalette: (themePalette) => {
    if (typeof window !== 'undefined') {
      writePreference(THEME_PALETTE_STORAGE_KEY, themePalette);
    }
    set({ themePalette });
  },
  setCustomThemeSeed: (value) => {
    const customThemeSeed = sanitizeSeed(value);
    if (typeof window !== 'undefined') {
      writePreference(THEME_CUSTOM_SEED_STORAGE_KEY, customThemeSeed);
    }
    set({ customThemeSeed });
  },
  setAndroidDynamicColor: (androidDynamicColor) => {
    if (typeof window !== 'undefined') {
      writePreference(THEME_DYNAMIC_STORAGE_KEY, String(androidDynamicColor));
    }
    set({ androidDynamicColor });
  },
  setAndroidDynamicSeed: (androidDynamicSeed) => set({ androidDynamicSeed }),
  toggleSafeReading: () => set((state) => ({ safeReading: !state.safeReading })),
  setSafeReading: (safeReading) => set({ safeReading }),
  selectMailbox: (selectedMailboxId) =>
    set({ selectedMailboxId, navPage: 'mail', selectedMessageId: null }),
  selectMessage: (selectedMessageId) => set({ selectedMessageId }),
  setSearchOpen: (searchOpen) => set({ searchOpen }),
  setComposeOpen: (composeOpen) =>
    set((state) => ({ composeOpen, composeDraft: composeOpen ? state.composeDraft : null })),
  openComposeWithDraft: (composeDraft) => set({ composeDraft, composeOpen: true }),
  clearComposeDraft: () => set({ composeDraft: null }),
  setNavPage: (navPage) => set({ navPage }),
  setSyncMessage: (syncMessage) => set({ syncMessage }),
}));

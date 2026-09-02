import { create } from 'zustand';
import type { ThemeMode } from '../types';

interface UiState {
  themeMode: ThemeMode;
  isOffline: boolean;
  selectedMailboxId: string;
  selectedMessageId: string | null;
  searchOpen: boolean;
  composeOpen: boolean;
  navPage: 'mail' | 'search' | 'outbox' | 'settings';
  syncMessage: string | null;
  setThemeMode: (themeMode: ThemeMode) => void;
  toggleOffline: () => void;
  selectMailbox: (id: string) => void;
  selectMessage: (id: string | null) => void;
  setSearchOpen: (value: boolean) => void;
  setComposeOpen: (value: boolean) => void;
  setNavPage: (page: UiState['navPage']) => void;
  setSyncMessage: (message: string | null) => void;
}

export const useUiStore = create<UiState>((set) => ({
  themeMode: 'dark',
  isOffline: false,
  selectedMailboxId: 'inbox',
  selectedMessageId: null,
  searchOpen: false,
  composeOpen: false,
  navPage: 'mail',
  syncMessage: null,
  setThemeMode: (themeMode) => set({ themeMode }),
  toggleOffline: () => set((state) => ({ isOffline: !state.isOffline })),
  selectMailbox: (selectedMailboxId) => set({ selectedMailboxId, navPage: 'mail', selectedMessageId: null }),
  selectMessage: (selectedMessageId) => set({ selectedMessageId }),
  setSearchOpen: (searchOpen) => set({ searchOpen }),
  setComposeOpen: (composeOpen) => set({ composeOpen }),
  setNavPage: (navPage) => set({ navPage, selectedMessageId: null }),
  setSyncMessage: (syncMessage) => set({ syncMessage }),
}));

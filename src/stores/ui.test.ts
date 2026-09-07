import { afterEach, expect, it, vi } from 'vitest';

afterEach(() => { vi.restoreAllMocks(); localStorage.clear(); });
it('validates an unknown persisted palette instead of asserting it is supported', async () => {
  vi.resetModules(); localStorage.setItem('mutsumi_theme_palette', 'corrupt');
  const { useUiStore } = await import('./ui');
  expect(useUiStore.getState().themePalette).toBe('matcha');
});
it('keeps theme state usable when browser preference storage is unavailable', async () => {
  vi.resetModules();
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('blocked'); });
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('blocked'); });
  const { useUiStore } = await import('./ui');
  useUiStore.getState().setThemeMode('light');
  expect(useUiStore.getState().themeMode).toBe('light');
});

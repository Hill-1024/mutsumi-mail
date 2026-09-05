import { describe, expect, it } from 'vitest';
import { generateThemeTokens, paletteSeed } from './theme';

describe('MD3 theme generation', () => {
  it('generates complete and distinct light and dark tonal-spot roles', () => {
    const light = generateThemeTokens('#879A6C', false);
    const dark = generateThemeTokens('#879A6C', true);

    for (const role of [
      '--md-sys-color-primary',
      '--md-sys-color-on-primary',
      '--md-sys-color-surface',
      '--md-sys-color-surface-container',
      '--md-sys-color-outline-variant',
    ]) {
      expect(light[role]).toMatch(/^#[0-9A-F]{6}$/);
      expect(dark[role]).toMatch(/^#[0-9A-F]{6}$/);
      expect(light[role]).not.toBe(dark[role]);
    }
  });

  it('uses a custom seed only for the custom palette', () => {
    expect(paletteSeed('custom', '#123456')).toBe('#123456');
    expect(paletteSeed('matcha', '#123456')).toBe('#879A6C');
    expect(paletteSeed('mutsumi', '#123456')).toBe('#76885F');
  });
});

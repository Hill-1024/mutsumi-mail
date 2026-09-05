import { describe, expect, it } from 'vitest';
import { installContextMenuGuard } from './context-menu';

describe('context menu guard', () => {
  it('blocks ordinary application surfaces', () => {
    const dispose = installContextMenuGuard();
    const surface = document.createElement('div');
    document.body.append(surface);
    const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });

    expect(surface.dispatchEvent(event)).toBe(false);
    expect(event.defaultPrevented).toBe(true);
    dispose();
    surface.remove();
  });

  it('keeps the native menu for copy and paste fields', () => {
    const dispose = installContextMenuGuard();
    const input = document.createElement('input');
    document.body.append(input);
    const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });

    expect(input.dispatchEvent(event)).toBe(true);
    expect(event.defaultPrevented).toBe(false);
    dispose();
    input.remove();
  });
});

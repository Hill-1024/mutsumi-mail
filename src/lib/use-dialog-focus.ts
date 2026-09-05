import { useEffect, useRef } from 'react';

/** Keep keyboard navigation within a modal and restore the invoking control. */
export function useDialogFocus<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const dialog = ref.current;
    if (!dialog) return;
    const controls = () => Array.from(dialog.querySelectorAll<HTMLElement>(
      'button, input, select, textarea, a[href], [tabindex]',
    )).filter((element) => element.tabIndex >= 0 && !element.matches(':disabled') && !element.closest('[hidden]'));
    if (!dialog.contains(document.activeElement)) controls()[0]?.focus();
    const trap = (event: KeyboardEvent) => {
      if (event.key !== 'Tab') return;
      const items = controls();
      const first = items[0];
      const last = items.at(-1);
      if (!first) { event.preventDefault(); return; }
      if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) {
        event.preventDefault(); last?.focus();
      } else if (!event.shiftKey && (document.activeElement === last || !dialog.contains(document.activeElement))) {
        event.preventDefault(); first.focus();
      }
    };
    document.addEventListener('keydown', trap);
    return () => { document.removeEventListener('keydown', trap); if (previous?.isConnected) previous.focus(); };
  }, []);
  return ref;
}

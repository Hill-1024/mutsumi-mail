const textInputTypes = new Set(['', 'email', 'number', 'password', 'search', 'tel', 'text', 'url']);

export function shouldKeepNativeContextMenu(target: EventTarget | null, selection: Selection | null) {
  if (!(target instanceof Element)) return false;
  const editable = target.closest('textarea, [contenteditable]:not([contenteditable="false"])');
  if (editable) return true;

  const input = target.closest('input');
  if (input instanceof HTMLInputElement && textInputTypes.has(input.type)) return true;

  if (!selection || selection.isCollapsed || selection.rangeCount === 0) return false;
  try {
    return selection.getRangeAt(0).intersectsNode(target);
  } catch {
    return false;
  }
}

export function installContextMenuGuard(documentRoot: Document = document) {
  const guard = (event: MouseEvent) => {
    if (!shouldKeepNativeContextMenu(event.target, documentRoot.getSelection())) {
      event.preventDefault();
    }
  };
  documentRoot.addEventListener('contextmenu', guard);
  return () => documentRoot.removeEventListener('contextmenu', guard);
}

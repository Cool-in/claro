import { invoke } from '@tauri-apps/api/core';

// Keep normal controls and the translated result usable. Every other background
// area can move Settings, including the sidebar, cards and page headings.
export function mountSettingsDrag(root: HTMLElement) {
  let activePointer: number | null = null;
  let position = { x: 0, y: 0 };
  let started: Promise<unknown> = Promise.resolve();
  let framePending = false;
  const interactive = 'button,input,select,textarea,label,a,[contenteditable="true"],#manual-result,#notice';

  root.addEventListener('pointerdown', event => {
    const target = event.target as Element;
    if (event.button !== 0 || !target.closest('.settings-shell') || target.closest(interactive)) return;
    event.preventDefault();
    activePointer = event.pointerId;
    position = { x: event.screenX, y: event.screenY };
    root.setPointerCapture(event.pointerId);
    document.body.classList.add('settings-dragging');
    started = invoke('start_settings_drag', position).catch(() => {
      if (activePointer !== null && root.hasPointerCapture(activePointer)) root.releasePointerCapture(activePointer);
      activePointer = null;
      document.body.classList.remove('settings-dragging');
    });
  });
  root.addEventListener('pointermove', event => {
    if (activePointer === null) return;
    position = { x: event.screenX, y: event.screenY };
    if (framePending) return;
    framePending = true;
    requestAnimationFrame(async () => {
      framePending = false;
      await started;
      if (activePointer !== null) invoke('move_settings_drag', position).catch(() => {});
    });
  });
  async function finish(event: PointerEvent) {
    if (activePointer === null) return;
    if (event.type === 'pointerup') position = { x: event.screenX, y: event.screenY };
    const pointer = activePointer;
    activePointer = null;
    if (root.hasPointerCapture(pointer)) root.releasePointerCapture(pointer);
    document.body.classList.remove('settings-dragging');
    await started;
    await invoke('move_settings_drag', position).catch(() => {});
    await invoke('end_settings_drag').catch(() => {});
  }
  root.addEventListener('pointerup', finish);
  root.addEventListener('pointercancel', finish);
  root.addEventListener('lostpointercapture', finish);
}

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

const languageCountries: Record<string, string> = {
  Spanish: 'es', English: 'gb', French: 'fr', German: 'de', Italian: 'it',
  Portuguese: 'pt', Dutch: 'nl', Polish: 'pl', Ukrainian: 'ua', Russian: 'ru',
  Arabic: 'sa', Hindi: 'in', Chinese: 'cn', Japanese: 'jp', Korean: 'kr',
  Turkish: 'tr', Swedish: 'se', Danish: 'dk', Norwegian: 'no', Finnish: 'fi',
  Czech: 'cz', Greek: 'gr', Romanian: 'ro', Hungarian: 'hu', Indonesian: 'id',
  Vietnamese: 'vn', Thai: 'th', Hebrew: 'il',
};
const flags = import.meta.glob<string>(
  '/node_modules/flag-icons/flags/4x3/{es,gb,fr,de,it,pt,nl,pl,ua,ru,sa,in,cn,jp,kr,tr,se,dk,no,fi,cz,gr,ro,hu,id,vn,th,il}.svg',
  { eager: true, query: '?url', import: 'default' },
);
const flag = (language: string) => flags[`/node_modules/flag-icons/flags/4x3/${languageCountries[language] || 'es'}.svg`];
const chevron = '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="m3 4.5 3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>';
const copy = '<svg viewBox="0 0 16 16" aria-hidden="true"><rect x="5.5" y="5.5" width="7" height="8" rx="1.5"/><path d="M10 3.5v-1A1.5 1.5 0 0 0 8.5 1h-6A1.5 1.5 0 0 0 1 2.5v7A1.5 1.5 0 0 0 2.5 11h1"/></svg>';
type Translation = { state: string; target?: string; mode?: string; text?: string; error?: string };

export function mountPopup(root: HTMLElement, mark: string, close: string) {
  document.body.classList.add('popup-body');
  root.innerHTML = `<section class="translation-panel" aria-label="Claro translation">
    <header class="popup-header">
      <span class="tiny-mark" role="img" aria-label="Claro">${mark}</span>
      <button class="target-picker" id="target-picker" aria-haspopup="menu" aria-expanded="false" aria-controls="language-menu" aria-label="Translate into Spanish"><img alt="" id="target-flag" src="${flag('Spanish')}">${chevron}</button>
      <span class="popup-status" id="popup-status" role="status"></span>
      <span class="header-space"></span>
      <button class="icon-button" id="copy" aria-label="Copy translation" disabled>${copy}</button>
      <button class="icon-button" id="dismiss" aria-label="Dismiss translation">${close}</button>
    </header>
    <div id="language-menu" class="language-menu" role="menu" aria-label="Translation options" hidden><button type="button" id="follow-selection" role="menuitem" title="Reset the saved popup position">↳ Follow selection</button><div class="menu-divider"></div>${Object.keys(languageCountries).map(language => `<button type="button" role="menuitemradio" aria-checked="${language === 'Spanish'}" data-language="${language}" tabindex="-1"><img alt="" src="${flag(language)}"><span>${language}</span><span class="language-check" aria-hidden="true">✓</span></button>`).join('')}</div>
    <div class="translation-text" id="translation" aria-live="polite"></div>
  </section>`;
  const panel = root.querySelector<HTMLElement>('.translation-panel')!;
  const text = root.querySelector<HTMLElement>('#translation')!;
  const picker = root.querySelector<HTMLButtonElement>('#target-picker')!;
  const menu = root.querySelector<HTMLElement>('#language-menu')!;
  const status = root.querySelector<HTMLElement>('#popup-status')!;
  const copyButton = root.querySelector<HTMLButtonElement>('#copy')!;
  let target = 'Spanish';
  let current: Translation = { state: 'loading' };
  let menuOpen = false;
  const canvas = document.createElement('canvas');
  const measure = canvas.getContext('2d')!;

  function size() {
    const computed = getComputedStyle(text);
    measure.font = `${computed.fontWeight} ${computed.fontSize} ${computed.fontFamily}`;
    const longest = Math.max(0, ...(text.textContent || '').split('\n').map(line => measure.measureText(line).width));
    const minimum = current.state === 'loading' ? 220 : current.state === 'error' ? 236 : 176;
    const width = menuOpen ? 228 : Math.min(324, Math.max(minimum, Math.ceil(longest + 26)));
    panel.style.width = `${width}px`;
    // Measure intrinsic content, never a flex item stretched by an old window size.
    panel.style.height = 'auto';
    const height = Math.ceil(panel.getBoundingClientRect().height);
    invoke('resize_popup', { width: width + 12, height: height + 12 }).catch(() => {});
    invoke('popup_rendered', { state: current.state }).catch(() => {});
  }
  function setTarget(language: string) {
    target = language;
    root.querySelector<HTMLImageElement>('#target-flag')!.src = flag(language);
    picker.setAttribute('aria-label', `Translate into ${language}`);
    picker.title = `${language} · change language`;
    menu.querySelectorAll<HTMLElement>('[data-language]').forEach(option => option.setAttribute('aria-checked', String(option.dataset.language === language)));
  }
  async function setMenu(open: boolean, keepInteraction = false) {
    if (open) await invoke('set_popup_picker', { open: true });
    menuOpen = open;
    menu.hidden = !open;
    text.hidden = open;
    picker.setAttribute('aria-expanded', String(open));
    if (!open && !keepInteraction) invoke('set_popup_picker', { open: false }).catch(() => {});
    requestAnimationFrame(size);
  }
  function render(payload: Translation) {
    current = payload;
    if (payload.state === 'empty') { text.textContent = ''; setMenu(false); return; }
    setTarget(payload.target || target);
    copyButton.disabled = payload.state !== 'ready';
    status.textContent = payload.state === 'loading' ? 'Translating…' : payload.state === 'error' ? 'Try again' : '';
    status.classList.toggle('working', payload.state === 'loading');
    text.className = `translation-text ${payload.state}`;
    if (payload.state === 'loading') text.innerHTML = '<span class="shimmer"></span>';
    else text.textContent = payload.text || payload.error || '';
    requestAnimationFrame(size);
  }
  root.querySelector('.popup-header')!.addEventListener('mousedown', event => {
    const mouse=event as MouseEvent;
    if(mouse.button!==0||(event.target as HTMLElement).closest('button'))return;
    mouse.preventDefault();invoke('start_popup_drag').catch(()=>{});
  });
  root.querySelector('.popup-header')!.setAttribute('title','Drag to move');
  root.querySelector('#follow-selection')!.addEventListener('click',async()=>{
    await setMenu(false);try{await invoke('follow_selection');}catch(error){render({state:'error',target,error:String(error)});}
  });
  picker.addEventListener('click', () => setMenu(!menuOpen).catch(error => render({ state: 'error', target, error: String(error) })));
  menu.querySelectorAll<HTMLButtonElement>('[data-language]').forEach(option => {
    option.addEventListener('click', async () => {
      const language = option.dataset.language!;
      await setMenu(false,true);
      if (language === target) { invoke('set_popup_picker',{open:false}); return; }
      try { await invoke('change_popup_target', { target: language }); }
      catch (error) { render({ state: 'error', target, error: String(error) }); }
      finally { invoke('set_popup_picker',{open:false}).catch(()=>{}); }
    });
    option.addEventListener('keydown', event => {
      const options = Array.from(menu.querySelectorAll<HTMLButtonElement>('[data-language]'));
      const index = options.indexOf(option);
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        options[(index + (event.key === 'ArrowDown' ? 1 : -1) + options.length) % options.length].focus();
      }
    });
  });
  picker.addEventListener('keydown', event => {
    if (event.key === 'ArrowDown') { event.preventDefault(); setMenu(true).then(() => menu.querySelector<HTMLElement>('[aria-checked="true"]')?.focus()); }
  });
  copyButton.addEventListener('click', async () => {
    try { await invoke('copy_translation', { text: current.text || '' }); copyButton.title = 'Copied'; }
    catch { copyButton.title = 'Could not copy. Try again.'; }
  });
  root.querySelector('#dismiss')!.addEventListener('click', () => invoke('dismiss_popup'));
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape') { event.preventDefault(); if (menuOpen) setMenu(false); else invoke('dismiss_popup'); }
  });
  listen<Translation>('translation', ({ payload }) => render(payload)).then(() => invoke('popup_ready'));
}

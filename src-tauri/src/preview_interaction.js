(() => {
  if (window.top !== window) {
    let bindings = [];
    window.addEventListener('message', event => {
      if (event.source === window.top && event.data?.type === 'yuzora-tab-bindings' && Array.isArray(event.data.bindings)) bindings = event.data.bindings.slice(0, 11);
    });
    document.addEventListener('keydown', event => {
      if (event.isComposing || event.keyCode === 229 || event.repeat || event.getModifierState('AltGraph') || event.target.closest?.('[role="dialog"], dialog[open], [data-shortcut-capture]')) return;
      const binding = bindings.find(item => item.key === event.key.toUpperCase() && item.ctrl === event.ctrlKey && item.meta === event.metaKey && item.alt === event.altKey && item.shift === event.shiftKey);
      if (!binding) return;
      event.preventDefault(); event.stopImmediatePropagation();
      window.top.postMessage({ type: 'yuzora-tab-command', id: binding.id }, '*');
    }, true);
    window.top.postMessage({ type: 'yuzora-tab-ready' }, '*');
    return;
  }
  if (window.__yuzoraBrowser) return;
  const state = { bindings: [], commands: [], selection: null, selecting: false };
  const frames = new Set();
  window.addEventListener('message', event => {
    if (!event.source || event.source === window) return;
    if (event.data?.type === 'yuzora-tab-ready' && frames.size < 64) frames.add(event.source);
    if (event.data?.type === 'yuzora-tab-command' && frames.has(event.source) && state.bindings.some(binding => binding.id === event.data.id) && state.commands.length < 8) state.commands.push(event.data.id);
  });
  const cleanups = [];
  const overlays = [];
  const stop = () => {
    state.selecting = false;
    cleanups.splice(0).forEach(clean => clean());
    overlays.splice(0).forEach(overlay => overlay.remove());
  };
  const selector = element => {
    const root = element.getRootNode();
    const parts = [];
    for (let node = element; node && node.nodeType === 1; node = node.parentElement) {
      if (node.id) {
        const id = '#' + CSS.escape(node.id);
        if (root.querySelectorAll(id).length === 1) { parts.unshift(id); break; }
      }
      const siblings = node.parentElement ? [...node.parentElement.children].filter(item => item.localName === node.localName) : [];
      parts.unshift(node.localName + (siblings.length > 1 ? `:nth-of-type(${siblings.indexOf(node) + 1})` : ''));
    }
    const own = parts.join(' > ');
    return root.host ? selector(root.host) + ' >>> ' + own : own;
  };
  const snapshot = element => {
    const clone = element.cloneNode(true);
    const scrub = node => {
      for (const attr of [...node.attributes]) {
        if (/^on/i.test(attr.name) || /^(value|checked|selected|srcdoc)$/i.test(attr.name)) node.removeAttribute(attr.name);
      }
      if (node.matches('input, textarea, select, [contenteditable]')) node.textContent = '';
    };
    scrub(clone);
    clone.querySelectorAll('script, style').forEach(node => node.remove());
    clone.querySelectorAll('*').forEach(scrub);
    const style = element.ownerDocument.defaultView.getComputedStyle(element);
    const styles = {};
    for (const name of ['display','position','width','height','margin','padding','gap','flex-direction','align-items','justify-content','grid-template-columns','font-family','font-size','font-weight','line-height','color','background-color','border','border-radius']) styles[name] = style.getPropertyValue(name).slice(0, 512);
    const rect = element.getBoundingClientRect();
    let targetSelector = selector(element);
    let view = element.ownerDocument.defaultView;
    while (view !== window && view.frameElement) { targetSelector = selector(view.frameElement) + ' >> ' + targetSelector; view = view.parent; }
    const html = clone.outerHTML;
    return { url: location.href.slice(0, 4096), selector: targetSelector.slice(0, 2048), text: (clone.textContent || '').trim().slice(0, 4096), html: html.slice(0, 24576), truncated: html.length > 24576, width: Math.round(rect.width), height: Math.round(rect.height), styles };
  };
  const installKeys = doc => {
    if (doc.__yuzoraKeys) return;
    doc.__yuzoraKeys = true;
    doc.addEventListener('keydown', event => {
      if (event.isComposing || event.keyCode === 229 || event.repeat || event.getModifierState('AltGraph')) return;
      if (event.target.closest?.('[role="dialog"], dialog[open], [data-shortcut-capture]')) return;
      const binding = state.bindings.find(item => item.key === event.key.toUpperCase() && item.ctrl === event.ctrlKey && item.meta === event.metaKey && item.alt === event.altKey && item.shift === event.shiftKey);
      if (!binding) return;
      event.preventDefault(); event.stopImmediatePropagation();
      if (state.commands.length < 8) state.commands.push(binding.id);
    }, true);
  };
  const documents = () => {
    const result = [];
    const walk = doc => {
      result.push(doc);
      doc.querySelectorAll('iframe').forEach(frame => { try { if (frame.contentDocument) walk(frame.contentDocument); } catch {} });
    };
    walk(document);
    return result;
  };
  state.start = () => {
    stop(); state.selection = null; state.selecting = true;
    for (const doc of documents()) {
      const overlay = doc.createElement('div');
      overlay.style.cssText = 'position:fixed;pointer-events:none;z-index:2147483647;border:2px solid #7c3aed;background:rgba(124,58,237,.12);display:none;box-sizing:border-box';
      doc.documentElement.append(overlay); overlays.push(overlay);
      const target = event => event.target.__yuzoraFrameTarget || event.composedPath().find(node => node instanceof doc.defaultView.Element && node !== overlay);
      const move = event => {
        const element = target(event); if (!element) return;
        const rect = element.getBoundingClientRect();
        overlays.forEach(item => item.style.display = 'none');
        Object.assign(overlay.style, { display: 'block', left: rect.left + 'px', top: rect.top + 'px', width: rect.width + 'px', height: rect.height + 'px' });
      };
      const click = event => {
        const element = target(event); if (!element) return;
        event.preventDefault(); event.stopImmediatePropagation();
        state.selection = snapshot(element); stop();
      };
      const key = event => { if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); stop(); } };
      doc.querySelectorAll('iframe').forEach(frame => {
        try { if (frame.contentDocument) return; } catch {}
        const shield = doc.createElement('div');
        shield.__yuzoraFrameTarget = frame;
        shield.style.cssText = 'position:fixed;z-index:2147483646;background:transparent';
        const place = () => { const rect = frame.getBoundingClientRect(); Object.assign(shield.style, { left: rect.left + 'px', top: rect.top + 'px', width: rect.width + 'px', height: rect.height + 'px' }); };
        place(); doc.documentElement.append(shield);
        doc.addEventListener('scroll', place, true);
        doc.defaultView.addEventListener('resize', place);
        cleanups.push(() => { shield.remove(); doc.removeEventListener('scroll', place, true); doc.defaultView.removeEventListener('resize', place); });
      });
      const press = event => { event.preventDefault(); event.stopImmediatePropagation(); };
      doc.addEventListener('pointerdown', press, true);
      cleanups.push(() => doc.removeEventListener('pointerdown', press, true));
      doc.addEventListener('pointermove', move, true); doc.addEventListener('click', click, true); doc.addEventListener('keydown', key, true);
      cleanups.push(() => { doc.removeEventListener('pointermove', move, true); doc.removeEventListener('click', click, true); doc.removeEventListener('keydown', key, true); });
    }
  };
  state.poll = bindings => {
    state.bindings = bindings;
    installKeys(document);
    frames.forEach(frame => { try { frame.postMessage({ type: 'yuzora-tab-bindings', bindings }, '*'); } catch { frames.delete(frame); } });
    const result = { commands: state.commands.splice(0), selection: state.selection, selecting: state.selecting };
    state.selection = null;
    return result;
  };
  state.stop = stop;
  window.__yuzoraBrowser = state;
})();

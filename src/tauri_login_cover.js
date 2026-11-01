// Presentation only: no fields, cookies, storage, credentials or native IPC.
(() => {
  if (window.top !== window || location.protocol !== 'https:') return;
  const host = location.hostname;
  if (!(host === 'vk.ru' || host.endsWith('.vk.ru') || host === 'vk.com' || host.endsWith('.vk.com'))) return;
  if (window.__vkFastLoginCover) return;
  // Cover potential feed documents before their first paint. Native cookie
  // detection reveals an anonymous login form without hiding the window.
  let checking = (host === 'vk.ru' || host === 'vk.com') && ['/', '/feed', '/feed/'].includes(location.pathname);
  let overlay;
  const css = (node, properties) => {
    for (const [name, value] of Object.entries(properties)) node.style.setProperty(name, value, 'important');
  };
  const install = () => {
    const root = document.documentElement;
    if (!root) return false;
    if (!overlay || !overlay.isConnected) {
      overlay = document.createElement('div');
      overlay.id = 'vk-fast-login-cover';
      overlay.setAttribute('role', 'status');
      css(overlay, {position:'fixed', inset:'0', 'z-index':'2147483647', background:'#0f1115', color:'#e5ebf5', font:'14px system-ui,sans-serif', 'align-items':'center', 'justify-content':'center', 'flex-direction':'column', gap:'18px', visibility:'visible'});
      const spinner = document.createElement('i');
      spinner.setAttribute('aria-hidden', 'true');
      css(spinner, {width:'38px', height:'38px', border:'3px solid #1b2c43', 'border-top-color':'#3670c4', 'border-radius':'50%'});
      if (!window.matchMedia?.('(prefers-reduced-motion: reduce)')?.matches) {
        spinner.animate?.([{transform:'rotate(0deg)'}, {transform:'rotate(360deg)'}], {duration:800, iterations:Infinity});
      }
      const brand = document.createElement('strong');
      brand.textContent = 'VK Fast';
      css(brand, {'font-size':'19px', 'letter-spacing':'.2px'});
      const status = document.createElement('span');
      status.textContent = __STATUS_TEXT__;
      css(status, {color:'#9ca6b5'});
      overlay.append(spinner, brand, status);
      root.append(overlay);
    }
    root.toggleAttribute('data-vk-fast-login-check', checking);
    css(overlay, {display:checking ? 'flex' : 'none'});
    return true;
  };
  window.__vkFastLoginCover = (value) => { checking = Boolean(value); install(); };
  if (!install()) {
    const observer = new MutationObserver(() => { if (install()) observer.disconnect(); });
    observer.observe(document, {childList:true});
  }
})();

(() => {
  const config = window.REQGLIDE_SITE || {};
  const configScript = document.querySelector('script[src$="site-config.js"]');
  const siteBase = new URL('.', configScript?.src || document.baseURI);
  const toggle = document.querySelector('.nav-toggle');
  const nav = document.querySelector('#navigation');
  function closeNavigation() {
    toggle?.setAttribute('aria-expanded', 'false');
    toggle?.setAttribute('aria-label', 'Open navigation');
    nav?.classList.remove('is-open');
  }
  toggle?.addEventListener('click', () => {
    const open = toggle.getAttribute('aria-expanded') !== 'true';
    toggle.setAttribute('aria-expanded', String(open));
    toggle.setAttribute('aria-label', open ? 'Close navigation' : 'Open navigation');
    nav?.classList.toggle('is-open', open);
  });
  nav?.querySelectorAll('a').forEach(link => link.addEventListener('click', closeNavigation));
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape' && toggle?.getAttribute('aria-expanded') === 'true') {
      closeNavigation();
      toggle.focus();
    }
  });

  const examples = {
    request: {
      filename: 'hello-api.sh',
      code: "curl --request GET \\\n  'https://api.example.com/v1/projects' \\\n  --header 'Accept: application/json' \\\n  --header 'Authorization: Bearer {{access_token}}'",
      caption: 'Paste cURL into the URL bar. Your request is ready to edit.'
    },
    environment: {
      filename: 'your-environment',
      code: '# Stage environment\n\nbaseUrl       https://api.example.com\naccess_token  ••••••••••••••••••••••••  [Secret]\n\n# Use {{baseUrl}} and {{access_token}} in requests.\n# Secret values stay in your OS credential store.',
      caption: 'Switch context without rebuilding your requests.'
    },
    script: {
      filename: 'post-response.js',
      code: 'pm.test("Request succeeded", () => {\n  pm.response.to.have.status(200);\n});\n\npm.test("Projects are returned", () => {\n  pm.expect(pm.response.json()).to.have.property("projects");\n});',
      caption: 'Keep repeatable response checks beside your requests.'
    }
  };
  let currentExample = 'request';
  const tabs = [...document.querySelectorAll('[data-example]')];
  function selectExample(key, focus = false) {
    if (!examples[key]) return;
    currentExample = key;
    tabs.forEach(tab => {
      const selected = tab.dataset.example === key;
      tab.setAttribute('aria-selected', String(selected));
      tab.tabIndex = selected ? 0 : -1;
      if (selected && focus) tab.focus();
    });
    document.querySelector('#example-code').textContent = examples[key].code;
    document.querySelector('#example-filename').textContent = examples[key].filename;
    document.querySelector('#example-caption').textContent = examples[key].caption;
    document.querySelector('#example-panel').setAttribute('aria-labelledby', `tab-${key}`);
  }
  tabs.forEach((tab, index) => {
    tab.addEventListener('click', () => selectExample(tab.dataset.example));
    tab.addEventListener('keydown', event => {
      const indexes = { ArrowRight: (index + 1) % tabs.length, ArrowLeft: (index + tabs.length - 1) % tabs.length, Home: 0, End: tabs.length - 1 };
      if (event.key in indexes) {
        event.preventDefault();
        selectExample(tabs[indexes[event.key]].dataset.example, true);
      }
    });
  });
  let statusTimer;
  document.querySelector('#copy-example')?.addEventListener('click', async () => {
    const status = document.querySelector('#copy-status');
    try {
      const value = examples[currentExample].code;
      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(value);
      } else {
        const input = document.createElement('textarea');
        input.value = value;
        input.style.position = 'fixed';
        input.style.opacity = '0';
        document.body.append(input);
        input.select();
        const copied = document.execCommand('copy');
        input.remove();
        if (!copied) throw new Error('Copy unavailable');
        document.querySelector('#copy-example').focus();
      }
      status.textContent = 'Example copied. Make it your own.';
    } catch {
      status.textContent = 'Select the example text and copy it with Ctrl+C.';
    }
    status.classList.add('visible');
    window.clearTimeout(statusTimer);
    statusTimer = window.setTimeout(() => status.classList.remove('visible'), 4000);
  });

  const release = config.release;
  if (release) {
    document.querySelectorAll('[data-release-version]').forEach(element => element.textContent = `v${release.version}`);
    const exe = release.assets.find(asset => asset.kind === 'nsis');
    const msi = release.assets.find(asset => asset.kind === 'msi');
    if (exe) {
      document.querySelectorAll('[data-download-primary]').forEach(element => { element.href = new URL(exe.url, siteBase).href; element.setAttribute('download', exe.file); });
      document.querySelectorAll('[data-download-size]').forEach(element => element.textContent = `${(exe.bytes / 1048576).toFixed(1)} MB · Windows x64`);
      document.querySelectorAll('[data-checksum]').forEach(element => element.textContent = exe.sha256);
    }
    if (msi) document.querySelectorAll('[data-download-msi]').forEach(element => { element.href = new URL(msi.url, siteBase).href; element.setAttribute('download', msi.file); });
    if (release.checksumsUrl) document.querySelectorAll('[data-checksums-link]').forEach(element => element.href = new URL(release.checksumsUrl, siteBase).href);
  }
})();

// Static, same-origin enhancement. The service page and QR work without JS.
document.addEventListener('click', async (event) => {
  const button = event.target.closest('[data-copy-target]');
  if (!button) return;
  const field = document.getElementById(button.dataset.copyTarget);
  const status = document.querySelector('[data-copy-status]');
  if (!field || !status) return;
  try {
    await navigator.clipboard.writeText(field.value);
    status.textContent = button.dataset.copyTarget === 'service-link' ? 'Service link copied.' : 'Agent request copied.';
  } catch {
    field.focus();
    field.select();
    status.textContent = 'Select and copy the highlighted text.';
  }
});

let fileSaveUrl;
window.addEventListener('pagehide', () => { if (fileSaveUrl) URL.revokeObjectURL(fileSaveUrl); });

// No file or invitation is fetched until the recipient explicitly clicks.
document.querySelector('[data-file-download]')?.addEventListener('click', async (event) => {
  const button = event.currentTarget;
  const status = document.querySelector('[data-download-status]');
  const invitation = document.getElementById('file-invitation');
  const config = document.getElementById('file-download-config');
  if (!status || !config) return;
  let controller;
  let timer;
  try {
    const file = JSON.parse(config.textContent);
    const url = new URL(file.url);
    if (!/^https:\/\/[a-z2-7]{52}\.relay(?:-[a-z0-9]+)*\.froglet\.dev$/.test(url.origin)
      || !/^\/v1\/provider\/services\/[a-zA-Z0-9._-]+\/files\/[a-f0-9]{64}\/download$/.test(url.pathname)
      || url.search || url.hash || url.username || url.password || !Number.isSafeInteger(file.size_bytes)
      || file.size_bytes < 0 || file.size_bytes > 8388608 || !/^[a-f0-9]{64}$/.test(file.sha256)) throw new Error('Download metadata is invalid.');
    if (Date.now()/1000 >= file.expires_at) throw new Error('This file share has expired.');
    const token = invitation?.value.trim() ?? '';
    if (file.invite && !/^[\x21-\x7e]{32,256}$/.test(token)) throw new Error('Enter the invitation token supplied by the provider.');
    button.disabled = true;
    if (fileSaveUrl) { URL.revokeObjectURL(fileSaveUrl); fileSaveUrl = undefined; }
    document.querySelector('[data-file-save]')?.remove();
    status.textContent = 'Downloading and checking the file…';
    controller = new AbortController();
    timer = setTimeout(() => controller.abort(), 55000);
    const response = await fetch(url, {method:'GET', credentials:'omit', redirect:'error', cache:'no-store', referrerPolicy:'no-referrer',
      headers: token ? {'x-froglet-access-token': token} : {}, signal:controller.signal});
    if (invitation) invitation.value = '';
    if (!response.ok) throw new Error(({403:'Access was denied. Ask the provider for an invitation.',404:'This file is no longer available.',410:'The share is paused, expired, or no longer active.',429:'The provider is busy or its transfer allowance is exhausted.',503:'The provider cannot serve the file now.'})[response.status] ?? 'The provider could not complete the download.');
    if (response.headers.get('content-length') !== String(file.size_bytes) || !response.body) throw new Error('The received file length does not match the signed metadata.');
    const reader = response.body.getReader();
    const chunks = [];
    let length = 0;
    while (true) {
      const part = await reader.read();
      if (part.done) break;
      length += part.value.length;
      if (length > file.size_bytes) {await reader.cancel(); throw new Error('The file exceeded its approved size.');}
      chunks.push(part.value);
    }
    if (length !== file.size_bytes) throw new Error('The download was interrupted; no file was saved.');
    const blob = new Blob(chunks, {type:'application/octet-stream'});
    const digest = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', await blob.arrayBuffer())), b => b.toString(16).padStart(2,'0')).join('');
    if (digest !== file.sha256) throw new Error('Checksum mismatch; no file was saved.');
    fileSaveUrl = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = fileSaveUrl; link.download = file.filename; link.dataset.fileSave = '';
    link.textContent = 'Save verified file'; button.parentElement.append(link); link.click();
    status.textContent = 'Size and SHA-256 verified. If saving did not start, choose Save verified file.';
  } catch (error) {
    controller?.abort();
    status.textContent = error.name === 'AbortError' ? 'The download timed out. Retrying consumes another transfer allowance.' : (error.message || 'Download failed.');
  } finally {
    clearTimeout(timer);
    if (invitation) invitation.value = '';
    button.disabled = false;
  }
});

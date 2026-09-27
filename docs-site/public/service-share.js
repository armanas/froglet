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

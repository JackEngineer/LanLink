const button = document.getElementById('copy-install');
const prompt = document.getElementById('install-prompt');
const status = document.getElementById('copy-install-status');
button.hidden = false;
button.addEventListener('click', async () => {
  button.disabled = true;
  try {
    await navigator.clipboard.writeText(prompt.value);
    status.textContent = '已复制。请粘贴给你的 AI 助手。';
  } catch {
    prompt.focus();
    prompt.select();
    status.textContent = '无法自动复制。已选中文字，请手动复制。';
  } finally {
    button.disabled = false;
  }
});

// Copy-to-clipboard for buttons carrying data-copy; data-done is the confirmation label.
document.addEventListener("click", (event) => {
  const button = event.target.closest("[data-copy]");
  if (!button) return;
  const label = button.textContent;
  const done = () => {
    button.textContent = button.dataset.done || label;
    setTimeout(() => (button.textContent = label), 1500);
  };
  if (navigator.clipboard) {
    navigator.clipboard.writeText(button.dataset.copy).then(done);
  } else {
    const field = document.createElement("textarea");
    field.value = button.dataset.copy;
    document.body.appendChild(field);
    field.select();
    document.execCommand("copy");
    field.remove();
    done();
  }
});

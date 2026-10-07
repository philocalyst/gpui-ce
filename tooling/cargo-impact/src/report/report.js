"use strict";
(() => {
  const search = document.getElementById("search");
  const articles = [...document.querySelectorAll("article.consumer")];
  const rows = [...document.querySelectorAll("#results tbody tr")];
  const buttons = [...document.querySelectorAll("button[data-filter]")];
  let filter = "all";
  function update() {
    const query = search.value.trim().toLocaleLowerCase();
    let visible = 0;
    const matches = new Set();
    for (const article of articles) {
      const show = (filter === "all" || article.dataset.status === filter) && article.textContent.toLocaleLowerCase().includes(query);
      article.hidden = !show;
      if (show) { visible += 1; matches.add(article.id); }
    }
    for (const row of rows) row.hidden = !matches.has(row.dataset.consumer);
    for (const button of buttons) button.setAttribute("aria-pressed", String(button.dataset.filter === filter));
    document.getElementById("visible-count").textContent = `${visible} of ${articles.length} consumers`;
  }
  search.addEventListener("input", update);
  for (const button of buttons) button.addEventListener("click", () => { filter = button.dataset.filter; update(); });
  for (const button of document.querySelectorAll("button[data-copy]")) {
    button.addEventListener("click", async () => {
      const draft = document.getElementById(button.dataset.copy);
      const status = document.getElementById("copy-status");
      try {
        if (navigator.clipboard && window.isSecureContext) await navigator.clipboard.writeText(draft.value);
        else { draft.focus(); draft.select(); if (!document.execCommand("copy")) throw new Error("Clipboard unavailable"); button.focus(); }
        status.textContent = "Issue draft copied. Review it before submitting.";
      } catch (_) { status.textContent = "Clipboard unavailable. Use Download draft instead."; }
      window.setTimeout(() => { status.textContent = ""; }, 5000);
    });
  }
  update();
})();

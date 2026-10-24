(function () {
  var UI = window.KILN_DOC_UI || {};
  var LANGS = ["en", "ko", "ja", "zh-CN"];
  var select = document.getElementById("lang");
  var tocList = document.getElementById("toc");
  var observer = null;

  function current() { return document.querySelector(".doc:not([hidden])"); }

  function buildToc() {
    var doc = current();
    tocList.innerHTML = "";
    if (observer) observer.disconnect();
    var heads = Array.prototype.slice.call(doc.querySelectorAll("h2[id], h3[id]"));
    heads.forEach(function (h) {
      if (!h.querySelector(".hl")) {
        var a = document.createElement("a");
        a.className = "hl"; a.href = "#" + h.id; a.textContent = "#"; a.setAttribute("aria-hidden", "true"); a.tabIndex = -1;
        h.appendChild(a);
      }
      var li = document.createElement("li"), link = document.createElement("a");
      link.href = "#" + h.id;
      link.textContent = h.firstChild ? h.firstChild.textContent : h.textContent;
      if (h.tagName === "H3") link.className = "sub";
      li.appendChild(link); tocList.appendChild(li);
    });
    if (!("IntersectionObserver" in window)) return;
    var links = tocList.querySelectorAll("a");
    observer = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (!e.isIntersecting) return;
        links.forEach(function (l) { l.classList.toggle("on", l.getAttribute("href") === "#" + e.target.id); });
      });
    }, { rootMargin: "-70px 0px -70% 0px" });
    heads.forEach(function (h) { observer.observe(h); });
  }

  function addCopy() {
    document.querySelectorAll(".doc pre").forEach(function (pre) {
      if (pre.querySelector(".cp")) return;
      var b = document.createElement("button");
      b.type = "button"; b.className = "cp"; b.textContent = "Copy";
      b.addEventListener("click", function () {
        var text = pre.querySelector("code").textContent;
        var ok = function () { b.textContent = "✓"; setTimeout(function () { b.textContent = "Copy"; }, 1400); };
        var fallback = function () { var r = document.createRange(); r.selectNodeContents(pre.querySelector("code")); var s = getSelection(); s.removeAllRanges(); s.addRange(r); };
        if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).then(ok, fallback); else fallback();
      });
      pre.appendChild(b);
    });
  }

  function apply(lang) {
    if (LANGS.indexOf(lang) < 0) lang = "en";
    document.documentElement.lang = lang;
    document.querySelectorAll("[data-lang]").forEach(function (el) { el.hidden = el.dataset.lang !== lang; });
    var ui = UI[lang] || UI.en || {};
    document.querySelectorAll("[data-ui]").forEach(function (el) { if (ui[el.dataset.ui]) el.textContent = ui[el.dataset.ui]; });
    document.getElementById("toc-h").textContent = ui.toc || "On this page";
    if (ui.title && !window.KILN_ROUTES) document.title = ui.title + " · Kiln";
    select.value = lang;
    if (window.KilnDownloads) KilnDownloads.refresh();
    buildToc();
    if (window.KilnGlaze) requestAnimationFrame(function () { KilnGlaze.refresh(); });
    try { localStorage.setItem("kiln-site-lang", lang); } catch (e) {}
  }

  select.addEventListener("change", function () { if (window.KILN_ROUTES) location.assign(window.KILN_ROUTES[select.value] + location.hash); else apply(select.value); });

  var side = document.getElementById("side"), menu = document.getElementById("menu-btn");
  menu.addEventListener("click", function () {
    var open = !side.classList.contains("open");
    side.classList.toggle("open", open);
    menu.setAttribute("aria-expanded", open ? "true" : "false");
  });
  side.addEventListener("click", function (e) { if (e.target.closest("a")) { side.classList.remove("open"); menu.setAttribute("aria-expanded", "false"); } });

  addCopy();
  var saved = null;
  try { saved = localStorage.getItem("kiln-site-lang"); } catch (e) {}
  apply(window.KILN_ROUTES ? document.documentElement.lang : (saved || "en"));
})();

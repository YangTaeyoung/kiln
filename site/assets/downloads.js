/* Recommend a platform across every download entry point; all links stay usable. */
(function () {
  "use strict";
  function detect(nav) {
    var hints = nav.userAgentData || {};
    var platform = hints.platform || nav.platform || "";
    var ua = nav.userAgent || "";
    // iPadOS can identify as a desktop Mac. Mobile devices have no Kiln build.
    if (hints.mobile || /Android|iPhone|iPad|iPod|Windows Phone/i.test(ua) ||
        (/Mac/i.test(platform) && nav.maxTouchPoints > 1)) return null;
    if (/Windows|Win32|Win64|WinCE/i.test(platform)) return "windows";
    if (/macOS|MacIntel|MacPPC|Macintosh/i.test(platform)) return "mac";
    if (/Windows NT/i.test(ua)) return "windows";
    if (/Macintosh|Mac OS X/i.test(ua)) return "mac";
    return null;
  }

  var icons = {
    mac: '<path d="M16.4 12.6c0-2.4 2-3.5 2-3.6-1.1-1.6-2.8-1.8-3.4-1.8-1.4-.2-2.8.8-3.5.8-.7 0-1.8-.8-3-.8-1.5 0-3 .9-3.8 2.3-1.6 2.8-.4 7 1.2 9.3.8 1.1 1.7 2.4 2.9 2.3 1.2 0 1.6-.7 3-.7s1.8.7 3 .7c1.3 0 2.1-1.1 2.8-2.3.9-1.3 1.3-2.6 1.3-2.6s-2.5-1-2.5-3.6ZM14.1 5.6c.6-.8 1.1-1.9 1-3-.9 0-2.1.6-2.7 1.4-.6.7-1.1 1.8-1 2.9 1 .1 2.1-.5 2.7-1.3Z"/>',
    windows: '<path d="M3 5.1 10.4 4v7.2H3V5.1Zm8.4-1.2L21 2.5v8.7h-9.6V3.9ZM3 12.7h7.4V20L3 18.9v-6.2Zm8.4 0H21v8.8l-9.6-1.4v-7.4Z"/>'
  };
  function refresh() {
    var root = document.documentElement, os = detect(navigator);
    var t = (window.KILN_I18N || {})[root.lang] || {};
    var labels = {
      mac: t.dl_mac || root.getAttribute("data-download-mac-text") || "Download for macOS",
      windows: t.dl_windows || root.getAttribute("data-download-windows-text") || "Download for Windows",
      generic: t.nav_download || root.getAttribute("data-download-generic-text") || "Download"
    };
    root.dataset.downloadOs = os || "other";
    document.querySelectorAll("[data-download-primary]").forEach(function (link) {
      link.querySelector("span").textContent = labels[os || "generic"];
      // Keep a download icon for unknown platforms; known platforms get their OS mark.
      if (os) {
        var svg = link.querySelector("svg");
        svg.setAttribute("viewBox", "0 0 24 24");
        svg.setAttribute("fill", "currentColor");
        svg.setAttribute("stroke", "none");
        svg.style.fill = "currentColor";
        svg.innerHTML = icons[os];
      }
    });
    document.querySelectorAll("[data-download-options]").forEach(function (row) {
      var options = row.querySelectorAll("[data-download-platform]");
      options.forEach(function (link) {
        var selected = link.dataset.downloadPlatform === os;
        link.querySelector("span").textContent = selected || !os ? labels[link.dataset.downloadPlatform] : (link.dataset.downloadPlatform === "mac" ? "macOS" : "Windows");
        link.classList.toggle("ink", selected && !row.closest(".final"));
        link.classList.toggle("ghost", !!row.closest(".final") && !!os && !selected);
        link.toggleAttribute("data-download-recommended", selected);
      });
      if (os) row.insertBefore(row.querySelector('[data-download-platform="' + os + '"]'), row.firstChild);
    });
  }
  window.KilnDownloads = { detect: detect, refresh: refresh };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", refresh);
  else refresh();
})();

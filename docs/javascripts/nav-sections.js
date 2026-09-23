/* Keep the Read the Docs sidebar in one audience at a time. */
(function () {
  function setAudience() {
    var menu = document.querySelector('.wy-menu-vertical');
    if (!menu) return;

    var contributorPath = /\/contributing\//.test(window.location.pathname);
    var captions = menu.querySelectorAll(':scope > .caption');
    captions.forEach(function (caption) {
      var label = caption.textContent.trim().toLowerCase();
      var isContributor = label.indexOf('contributor') !== -1;
      var isUser = label.indexOf('user guide') !== -1;
      if (!isContributor && !isUser) return;
      var visible = isContributor === contributorPath;
      caption.hidden = !visible;
      if (caption.nextElementSibling) caption.nextElementSibling.hidden = !visible;
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', setAudience);
  } else {
    setAudience();
  }
})();

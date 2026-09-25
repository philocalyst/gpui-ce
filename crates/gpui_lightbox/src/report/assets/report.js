// Lightbox report viewer: shots with overlays and a pixel loupe, golden
// swipe comparisons, film scrubbing against curves. Vanilla JS, works from file://.
(() => {
  'use strict';

  const data = JSON.parse(document.getElementById('lightbox-data').textContent);
  const items = data.items;
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));
  const SVG = 'http://www.w3.org/2000/svg';

  function el(tag, attributes = {}, ...children) {
    const node = document.createElement(tag);
    for (const [key, value] of Object.entries(attributes)) {
      if (value == null || value === false) continue;
      if (key === 'class') node.className = value;
      else if (key === 'html') node.innerHTML = value;
      else if (key === 'style') node.style.cssText = value;
      else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
      else node.setAttribute(key, value === true ? '' : value);
    }
    for (const child of children.flat()) {
      if (child == null || child === false) continue;
      node.append(child.nodeType ? child : document.createTextNode(String(child)));
    }
    return node;
  }

  function svg(tag, attributes = {}) {
    const node = document.createElementNS(SVG, tag);
    for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, value);
    return node;
  }

  const store = {
    get(key) { try { return localStorage.getItem(key); } catch { return null; } },
    set(key, value) { try { localStorage.setItem(key, value); } catch { /* private mode */ } },
  };

  const trim = (value, digits = 2) => {
    const rounded = Number(value.toFixed(digits));
    return String(rounded);
  };
  const compact = (count) => count >= 1e6 ? (count / 1e6).toFixed(1) + 'M'
    : count >= 1e3 ? (count / 1e3).toFixed(1) + 'k' : String(count);

  // Theme: auto → light → dark.
  const themeButton = $('#theme');
  const themes = ['auto', 'light', 'dark'];
  let theme = store.get('lightbox-theme') || 'auto';
  function applyTheme() {
    if (theme === 'auto') document.documentElement.removeAttribute('data-theme');
    else document.documentElement.setAttribute('data-theme', theme);
    themeButton.textContent = theme[0].toUpperCase() + theme.slice(1);
    themeButton.title = 'Theme: ' + (theme === 'auto' ? 'follows the system' : theme);
  }
  applyTheme();
  themeButton.addEventListener('click', () => {
    theme = themes[(themes.indexOf(theme) + 1) % themes.length];
    store.set('lightbox-theme', theme);
    applyTheme();
  });

  // Filter.
  const filter = $('#filter');
  filter.addEventListener('input', () => {
    const query = filter.value.trim().toLowerCase();
    for (const node of $$('[data-name]')) {
      node.classList.toggle('hidden', query !== '' && !node.dataset.name.includes(query));
    }
  });

  // Viewer.
  const viewer = $('#viewer');
  const toggles = JSON.parse(store.get('lightbox-toggles') || 'null')
    || { texts: false, boxes: false, lint: true, loupe: true };
  let current = null;
  let cleanup = [];

  function saveToggles() { store.set('lightbox-toggles', JSON.stringify(toggles)); }

  function listFor(node) {
    const list = node && node.dataset.list;
    if (!list) return [node.dataset.open];
    return $$('[data-list]')
      .filter((other) => other.dataset.list === list && !other.classList.contains('hidden'))
      .map((other) => other.dataset.open);
  }

  document.addEventListener('click', (event) => {
    const node = event.target.closest('[data-open]');
    if (!node || !node.dataset.open || viewer.contains(node)) return;
    event.preventDefault();
    open(node.dataset.open, listFor(node), node.dataset.view);
  });
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && !current) {
      const node = document.activeElement && document.activeElement.closest('[data-open]');
      if (node) { event.preventDefault(); open(node.dataset.open, listFor(node), node.dataset.view); }
    }
  });

  function open(id, list = [id], view) {
    const item = items[id];
    if (!item) return;
    if (view === 'lint') toggles.lint = true;
    current = { id, item, list, index: Math.max(0, list.indexOf(id)), frame: null, playing: false };
    viewer.hidden = false;
    document.body.style.overflow = 'hidden';
    history.replaceState(null, '', '#view/' + encodeURIComponent(id));
    render();
  }

  function close() {
    for (const undo of cleanup) undo();
    cleanup = [];
    viewer.hidden = true;
    viewer.replaceChildren();
    document.body.style.overflow = '';
    loupe.hide();
    current = null;
    history.replaceState(null, '', location.pathname + location.search);
  }

  function step(delta) {
    if (!current || current.list.length < 2) return;
    const count = current.list.length;
    const next = current.list[(current.index + delta + count) % count];
    open(next, current.list);
  }

  function button(label, attributes = {}) {
    return el('button', { class: 'button', type: 'button', ...attributes }, label);
  }

  function toggle(label, key, hotkey, disabled) {
    return button(label, {
      'aria-pressed': String(Boolean(toggles[key]) && !disabled),
      title: `${label} (${hotkey})`,
      disabled: disabled || null,
      onclick: () => { toggles[key] = !toggles[key]; saveToggles(); render(); },
    });
  }

  function bar(item, controls) {
    const position = current.list.length > 1
      ? el('span', { class: 'crumb mono' }, `${current.index + 1} / ${current.list.length}`) : null;
    return el('div', { class: 'bar' },
      el('span', { class: 'title' }, item.title),
      el('span', { class: 'crumb' }, `${item.suite} · ${item.subtitle || item.kind}`),
      position,
      el('span', { class: 'grow' }),
      ...controls,
      el('span', { class: 'sep' }),
      button('‹', { title: 'Previous (←)', onclick: () => step(-1), disabled: current.list.length < 2 || null }),
      button('›', { title: 'Next (→)', onclick: () => step(1), disabled: current.list.length < 2 || null }),
      button('✕', { title: 'Close (Esc)', onclick: close }));
  }

  function render() {
    for (const undo of cleanup) undo();
    cleanup = [];
    loupe.hide();
    const item = current.item;
    const views = { shot: renderShot, golden: renderGolden, film: renderFilm, image: renderImage };
    viewer.replaceChildren(...views[item.kind](item));
  }

  // Sizing: "fit" fills the stage (up to 2 CSS px per logical px); numbers are
  // CSS px per logical px.
  let zoom = store.get('lightbox-zoom') || 'fit';
  function zoomControls() {
    return ['fit', '1', '2', '4'].map((value) => button(value === 'fit' ? 'Fit' : value + '×', {
      'aria-pressed': String(zoom === value),
      title: value === 'fit' ? 'Fit (0)' : `${value} CSS px per logical px (${value})`,
      onclick: () => { zoom = value; store.set('lightbox-zoom', zoom); render(); },
    }));
  }

  function place(canvas, stage, width, height, scale) {
    const apply = () => {
      const available = stage.getBoundingClientRect();
      const factor = zoom === 'fit'
        ? Math.min((available.width - 64) / width, (available.height - 64) / height, 2)
        : Number(zoom);
      canvas.style.width = Math.max(1, Math.round(width * factor)) + 'px';
      canvas.style.height = Math.max(1, Math.round(height * factor)) + 'px';
      canvas.classList.toggle('pixelated', factor / scale >= 1.5);
    };
    requestAnimationFrame(apply);
    window.addEventListener('resize', apply);
    cleanup.push(() => window.removeEventListener('resize', apply));
  }

  // Loupe: a pixel magnifier that follows the cursor over any viewer image.
  const loupe = (() => {
    const size = 184;
    const cells = 15;
    const ratio = window.devicePixelRatio || 1;
    const canvas = el('canvas', { width: size * ratio, height: size * ratio });
    const readout = el('div', { class: 'readout' });
    const node = el('div', { class: 'loupe', hidden: true }, canvas, readout);
    document.body.append(node);
    const context = canvas.getContext('2d');
    return {
      hide() { node.hidden = true; },
      attach(image, scale) {
        const move = (event) => {
          if (!toggles.loupe || !image.naturalWidth) { node.hidden = true; return; }
          const bounds = image.getBoundingClientRect();
          const fx = (event.clientX - bounds.left) / bounds.width;
          const fy = (event.clientY - bounds.top) / bounds.height;
          if (fx < 0 || fy < 0 || fx >= 1 || fy >= 1) { node.hidden = true; return; }
          const x = Math.floor(fx * image.naturalWidth);
          const y = Math.floor(fy * image.naturalHeight);
          const half = (cells - 1) / 2;
          const cell = canvas.width / cells;
          context.imageSmoothingEnabled = false;
          context.fillStyle = getComputedStyle(document.body).backgroundColor;
          context.fillRect(0, 0, canvas.width, canvas.height);
          context.drawImage(image, x - half, y - half, cells, cells, 0, 0, canvas.width, canvas.height);
          context.strokeStyle = 'rgba(128, 128, 128, .28)';
          context.lineWidth = 1;
          context.beginPath();
          for (let line = 1; line < cells; line++) {
            context.moveTo(Math.round(line * cell) + .5, 0); context.lineTo(Math.round(line * cell) + .5, canvas.height);
            context.moveTo(0, Math.round(line * cell) + .5); context.lineTo(canvas.width, Math.round(line * cell) + .5);
          }
          context.stroke();
          context.strokeStyle = getComputedStyle(document.documentElement).getPropertyValue('--accent');
          context.lineWidth = 2 * ratio;
          context.strokeRect(half * cell, half * cell, cell, cell);
          let color = '';
          try {
            const [r, g, b] = context.getImageData(Math.floor((half + .5) * cell), Math.floor((half + .5) * cell), 1, 1).data;
            color = '#' + [r, g, b].map((channel) => channel.toString(16).padStart(2, '0')).join('');
          } catch { /* file:// images can't be read back; the position still shows */ }
          readout.replaceChildren(
            el('span', {}, `${trim(x / scale, 1)}, ${trim(y / scale, 1)}`),
            el('span', {}, color ? el('i', { class: 'swatch', style: `background:${color}` }) : null, color || `px ${x}, ${y}`));
          node.hidden = false;
          const left = event.clientX + 24 + size > innerWidth ? event.clientX - 24 - size : event.clientX + 24;
          const top = event.clientY + 24 + size + 30 > innerHeight ? event.clientY - 24 - size - 30 : event.clientY + 24;
          node.style.left = left + 'px';
          node.style.top = top + 'px';
        };
        const leave = () => { node.hidden = true; };
        image.addEventListener('mousemove', move);
        image.addEventListener('mouseleave', leave);
        cleanup.push(() => {
          image.removeEventListener('mousemove', move);
          image.removeEventListener('mouseleave', leave);
        });
      },
    };
  })();

  function details(rows) {
    return el('dl', {}, ...rows.filter(([, value]) => value != null && value !== '')
      .flatMap(([label, value]) => [el('dt', {}, label), el('dd', {}, value)]));
  }

  function originSection(origin) {
    if (!origin || (!origin.test && !origin.location)) return null;
    return el('section', {}, el('h4', {}, 'Source'),
      details([['test', origin.test], ['location', el('span', { class: 'mono' }, origin.location || '')]]));
  }

  // A shot: the image, overlays for painted text, boxes and lint violations.
  function renderShot(item) {
    const [width, height] = item.size;
    const lint = item.lint;
    const image = el('img', { src: item.image, alt: item.title, draggable: 'false' });
    const overlay = svg('svg', { class: 'overlay', viewBox: `0 0 ${width} ${height}`, preserveAspectRatio: 'none' });
    const badges = el('div', { style: 'position:absolute;inset:0;pointer-events:none' });
    const canvas = el('div', { class: 'canvas' }, image, overlay, badges);
    const stage = el('div', { class: 'stage' }, canvas);
    place(canvas, stage, width, height, item.scale || 1);
    loupe.attach(image, item.scale || 1);

    const shapes = { t: [], q: [], v: [] };
    const box = (kind, [x, y, w, h], className) => {
      const rect = svg('rect', { x, y, width: Math.max(w, .5), height: Math.max(h, .5), class: className });
      overlay.append(rect);
      shapes[kind].push(rect);
      return rect;
    };
    if (toggles.boxes) item.quads.forEach((quad) => box('q', quad, 'q'));
    if (toggles.texts) item.texts.forEach((text) => box('t', text, text[9] ? 't clipped' : 't'));
    if (toggles.lint && lint) {
      lint.violations.forEach((violation, ix) => {
        box('v', violation.box, 'v ' + violation.severity);
        badges.append(el('span', {
          class: 'pin ' + violation.severity,
          style: `left:${violation.box[0] / width * 100}%;top:${violation.box[1] / height * 100}%`,
        }, ix + 1));
      });
    }
    const highlight = (kind, ix, on) => {
      const shape = shapes[kind][ix];
      if (shape) shape.classList.toggle('hot', on);
    };

    const aside = el('aside', {},
      el('section', {}, el('h4', {}, 'Shot'), details([
        ['size', el('span', { class: 'mono' }, `${trim(width)}×${trim(height)} @${trim(item.scale || 1)}×`)],
        ['appearance', item.appearance],
        ['clock', item.time != null ? el('span', { class: 'mono' }, `${trim(item.time)} ms`) : null],
        ['painted', `${item.texts.length} text lines · ${item.quads.length} boxes`],
        ['golden', item.golden ? el('a', { href: '#', 'data-open': item.golden, onclick: (event) => { event.preventDefault(); open(item.golden, [item.golden]); } }, items[item.golden] ? (items[item.golden].passed ? 'matches — compare' : 'mismatch — compare') : '') : null],
      ])));

    if (lint) {
      const list = el('section', {}, el('h4', {}, el('span', {}, `Lint · ${lint.spec}`),
        el('span', { class: lint.violations.length ? 'fail' : 'pass' }, lint.violations.length ? `${lint.violations.length} violations` : 'clean')));
      lint.violations.forEach((violation, ix) => {
        list.append(el('div', {
          class: 'list-item',
          onmouseenter: () => { if (toggles.lint) highlight('v', ix, true); },
          onmouseleave: () => highlight('v', ix, false),
        },
        el('span', { class: 'n ' + violation.severity }, ix + 1),
        el('div', { class: 'body' },
          el('div', {}, el('span', { class: 'rule ' + violation.severity }, violation.rule), ' ', el('b', {}, violation.subject)),
          el('div', { class: 'msg' }, violation.message))));
      });
      list.append(el('div', { class: 'msg', style: 'color:var(--faint);font-size:11.5px;margin-top:8px' },
        `Checked ${lint.checked.texts} texts and ${lint.checked.quads} boxes: ${lint.checked.rules.join(', ')}.`,
        ...lint.checked.skipped.map((skipped) => el('div', { style: 'color:var(--warn)' }, 'Skipped ' + skipped))));
      list.append(el('div', { style: 'margin-top:8px' }, el('a', { href: lint.annotated, target: '_blank', class: 'hint' }, 'Annotated PNG ↗')));
      aside.append(list);
    }

    if (item.texts.length) {
      const texts = el('section', {}, el('h4', {}, el('span', {}, 'Painted text'), el('span', {}, String(item.texts.length))));
      item.texts.forEach((text, ix) => {
        const [, , , , content, family, size, weight, color, clipped] = text;
        texts.append(el('div', {
          class: 'list-item text-item',
          onmouseenter: () => { if (!toggles.texts) { toggles.texts = true; render(); } highlight('t', ix, true); },
          onmouseleave: () => highlight('t', ix, false),
        },
        el('span', { class: 'sw', style: `background:${color}` }),
        el('div', { class: 'body' },
          el('div', { class: 't1' }, content || '·'),
          el('div', { class: 't2' }, `${family} ${trim(size)}px ${weight} · ${color}${clipped ? ' · clipped' : ''}`))));
      });
      aside.append(texts);
    }
    aside.append(originSection(item.origin) || '');

    return [
      bar(item, [
        toggle('Text', 'texts', 'T'),
        toggle('Boxes', 'boxes', 'B'),
        toggle('Lint', 'lint', 'V', !lint),
        toggle('Loupe', 'loupe', 'L'),
        el('span', { class: 'sep' }),
        ...zoomControls(),
      ]),
      stage,
      aside,
    ];
  }

  // A golden comparison: swipe between golden and shot, or view the heatmap.
  let goldenMode = 'swipe';
  function renderGolden(item) {
    const [width, height] = item.pixels;
    const modes = item.expected ? ['swipe', 'diff', 'expected', 'actual'] : ['actual'];
    if (!modes.includes(goldenMode)) goldenMode = modes[0];
    const canvas = el('div', { class: 'canvas' });
    const stage = el('div', { class: 'stage' }, canvas);
    const scale = 2;
    place(canvas, stage, width / scale, height / scale, 1);

    if (goldenMode === 'swipe') {
      const base = el('img', { src: item.expected, alt: 'golden', draggable: 'false' });
      const top = el('img', { src: item.actual, alt: 'shot', class: 'top-image', draggable: 'false' });
      const handle = el('div', { class: 'handle' });
      const swipe = el('div', { class: 'swipe', style: 'width:100%;height:100%' }, base, top, handle,
        el('span', { class: 'tag left' }, 'golden'), el('span', { class: 'tag right' }, 'shot'));
      const set = (fraction) => {
        const percent = Math.min(100, Math.max(0, fraction * 100));
        top.style.clipPath = `inset(0 0 0 ${percent}%)`;
        handle.style.left = percent + '%';
      };
      set(.5);
      swipe.addEventListener('mousemove', (event) => {
        const bounds = swipe.getBoundingClientRect();
        set((event.clientX - bounds.left) / bounds.width);
      });
      canvas.append(swipe);
    } else {
      const source = { diff: item.diff || item.actual, expected: item.expected, actual: item.actual }[goldenMode];
      const image = el('img', { src: source, alt: goldenMode, draggable: 'false' });
      canvas.append(image);
      loupe.attach(image, scale);
    }

    const stats = item.stats || {};
    const aside = el('aside', {},
      el('section', {}, el('h4', {}, el('span', {}, 'Golden'), el('span', { class: item.passed ? 'pass' : 'fail' }, item.updated ? 'recorded' : item.passed ? 'match' : 'mismatch')),
        el('p', { style: 'margin:0 0 10px' }, item.message),
        details([
          ['golden', el('span', { class: 'mono' }, item.golden)],
          ['differing', stats.total ? `${compact(stats.differing)} px (${(stats.fraction * 100).toFixed(4)}%)` : null],
          ['any change', stats.total ? `${compact(stats.changed)} px` : null],
          ['max ΔE', stats.total ? trim(stats.max_delta_e, 3) : null],
          ['region', stats.bounds ? el('span', { class: 'mono' }, `${trim(stats.bounds.w)}×${trim(stats.bounds.h)} at (${trim(stats.bounds.x)}, ${trim(stats.bounds.y)})`) : null],
          ['tolerance', `ΔE ≤ ${trim(item.tolerance.max_delta_e, 3)} per pixel; ≤ ${trim(item.tolerance.max_fraction * 100, 4)}% of pixels over`],
        ])),
      el('section', {}, el('h4', {}, 'Heatmap'), el('p', { class: 'msg', style: 'margin:0;color:var(--muted);font-size:12px' },
        'The shot dimmed to gray; blue pixels changed by less than the tolerance (noise), amber to red changed by more.')),
      originSection(item.origin) || '');
    return [
      bar(item, [
        ...modes.map((mode) => button(mode[0].toUpperCase() + mode.slice(1), {
          'aria-pressed': String(goldenMode === mode),
          onclick: () => { goldenMode = mode; render(); },
        })),
        item.comparison ? el('a', { class: 'button', href: item.comparison, target: '_blank' }, 'Side by side ↗') : null,
        el('span', { class: 'sep' }),
        toggle('Loupe', 'loupe', 'L', goldenMode === 'swipe'),
        ...zoomControls(),
      ]),
      stage,
      aside,
    ];
  }

  // A film: scrub frames against the tracked curves; play the APNG.
  function renderFilm(item) {
    const [width, height] = item.size;
    const frames = item.frames;
    if (current.frame == null) {
      const last = frames.map((frame) => frame.changed > 0).lastIndexOf(true);
      current.frame = Math.max(0, last);
    }
    const image = el('img', { alt: item.title, draggable: 'false' });
    const overlay = svg('svg', { class: 'overlay', viewBox: `0 0 ${width} ${height}`, preserveAspectRatio: 'none' });
    const canvas = el('div', { class: 'canvas' }, image, overlay);
    const stage = el('div', { class: 'stage' }, canvas);
    place(canvas, stage, width, height, item.scale);
    loupe.attach(image, item.scale);

    const range = el('input', { type: 'range', min: 0, max: frames.length - 1, value: current.frame, 'aria-label': 'Frame' });
    const label = el('span', { class: 'mono', style: 'font-size:11.5px;white-space:nowrap' });
    const curves = el('section', {}, el('h4', {}, 'Curves'));
    for (const curve of item.curves) {
      const block = el('div', { class: 'curve-block' },
        el('div', { class: 'curve-head' },
          el('span', { style: 'display:inline-flex;align-items:center;gap:6px' }, el('i', { class: 'key ' + curve.class }), curve.name),
          el('span', {}, curve.range || '')),
        el('div', { html: curve.svg }));
      curves.append(block);
    }
    for (const group of $$('g[data-frame]', curves)) {
      group.style.cursor = 'pointer';
      group.addEventListener('click', () => show(Number(group.dataset.frame)));
    }

    function show(ix) {
      current.frame = Math.max(0, Math.min(frames.length - 1, ix));
      current.playing = false;
      const frame = frames[current.frame];
      image.src = frame.image;
      range.value = current.frame;
      label.textContent = `#${String(current.frame).padStart(2, '0')} · ${trim(frame.t)} ms · ${frame.changed ? compact(frame.changed) + ' px changed' : current.frame ? 'still' : 'first frame'}`;
      overlay.replaceChildren();
      if (frame.box) {
        const [x, y, w, h] = frame.box;
        overlay.append(svg('rect', { x, y, width: w, height: h, class: 'changed' }));
      }
      for (const cursor of $$('line.cursor', curves)) {
        const x = Number(cursor.dataset.left) + frame.t / Number(cursor.dataset.end) * Number(cursor.dataset.width);
        cursor.setAttribute('x1', x);
        cursor.setAttribute('x2', x);
        cursor.style.opacity = 1;
      }
      playButton.textContent = 'Play';
    }
    const playButton = button('Play', {
      title: 'Play the animated PNG (Space)',
      onclick: () => {
        current.playing = !current.playing;
        if (current.playing) {
          image.src = item.animation;
          overlay.replaceChildren();
          playButton.textContent = 'Pause';
        } else show(current.frame);
      },
    });
    range.addEventListener('input', () => show(Number(range.value)));
    current.show = show;

    const findings = el('section', {}, el('h4', {}, el('span', {}, 'Findings'), el('span', {}, String(item.findings.length))));
    if (!item.findings.length) findings.append(el('div', { class: 'msg', style: 'color:var(--muted)' }, 'No freezes, jumps, reversals or overshoot.'));
    for (const finding of item.findings) {
      findings.append(el('div', { class: 'list-item', style: 'cursor:pointer', onclick: () => show(finding.frame) },
        el('span', { class: 'n ' + finding.status }, finding.frame),
        el('div', { class: 'body' }, el('div', { class: 'rule ' + finding.status }, finding.kind), el('div', { class: 'msg' }, finding.message))));
    }
    const assertions = el('section', {}, el('h4', {}, 'Assertions'));
    if (!item.assertions.length) assertions.append(el('div', { class: 'msg', style: 'color:var(--muted)' }, 'None made.'));
    for (const assertion of item.assertions) {
      assertions.append(el('div', { class: 'list-item' },
        el('span', { class: assertion.passed ? 'pass' : 'fail', style: 'font:600 11px var(--mono);width:30px;flex:none' }, assertion.passed ? 'pass' : 'fail'),
        el('div', { class: 'body' }, el('div', { class: 'mono', style: 'font-size:12px' }, assertion.name), el('div', { class: 'msg' }, assertion.message))));
    }
    const aside = el('aside', {},
      el('section', {}, el('h4', {}, el('span', {}, 'Frame'), el('a', { href: item.strip, target: '_blank', class: 'hint', style: 'text-transform:none;letter-spacing:0' }, 'strip PNG ↗')),
        el('div', { class: 'scrub' }, button('‹', { title: 'Previous frame (,)', onclick: () => show(current.frame - 1) }), range, button('›', { title: 'Next frame (.)', onclick: () => show(current.frame + 1) })),
        label),
      curves, findings, assertions, originSection(item.origin) || '');
    requestAnimationFrame(() => show(current.frame));
    return [bar(item, [playButton, toggle('Loupe', 'loupe', 'L'), el('span', { class: 'sep' }), ...zoomControls()]), stage, aside];
  }

  function renderImage(item) {
    const image = el('img', { src: item.image, alt: item.title, draggable: 'false' });
    const canvas = el('div', { class: 'canvas' }, image);
    const stage = el('div', { class: 'stage' }, canvas);
    const scale = 2;
    const ready = () => place(canvas, stage, image.naturalWidth / scale, image.naturalHeight / scale, 1);
    if (image.complete && image.naturalWidth) ready(); else image.addEventListener('load', ready, { once: true });
    loupe.attach(image, scale);
    const aside = el('aside', {}, el('section', {}, el('h4', {}, 'Image'),
      details([['file', el('a', { class: 'mono', href: item.image, target: '_blank' }, item.image)]])), originSection(item.origin) || '');
    return [bar(item, [toggle('Loupe', 'loupe', 'L'), el('span', { class: 'sep' }), ...zoomControls()]), stage, aside];
  }

  document.addEventListener('keydown', (event) => {
    const typing = event.target.closest && event.target.closest('input, textarea');
    if (event.key === '/' && !typing) { event.preventDefault(); filter.focus(); return; }
    if (event.key === 'Escape') {
      if (current) close(); else if (typing) { filter.value = ''; filter.dispatchEvent(new Event('input')); filter.blur(); }
      return;
    }
    if (!current || typing || event.metaKey || event.ctrlKey || event.altKey) return;
    const key = event.key.toLowerCase();
    const flips = { t: 'texts', b: 'boxes', v: 'lint', l: 'loupe' };
    if (event.key === 'ArrowLeft') step(-1);
    else if (event.key === 'ArrowRight') step(1);
    else if (flips[key]) { toggles[flips[key]] = !toggles[flips[key]]; saveToggles(); render(); }
    else if (['0', '1', '2', '4'].includes(key)) { zoom = key === '0' ? 'fit' : key; store.set('lightbox-zoom', zoom); render(); }
    else if (current.item.kind === 'film' && (key === ',' || key === '.')) current.show(current.frame + (key === ',' ? -1 : 1));
    else if (current.item.kind === 'film' && key === ' ') { event.preventDefault(); $('.bar .button', viewer).click(); }
    else return;
    event.preventDefault();
  });

  // Deep links: #view/<id>, on load and when the hash changes.
  function follow() {
    if (!location.hash.startsWith('#view/')) {
      if (current) close();
      return;
    }
    const id = decodeURIComponent(location.hash.slice('#view/'.length));
    if (current && current.id === id) return;
    const node = $$('[data-open]').find((candidate) => candidate.dataset.open === id);
    open(id, node ? listFor(node) : [id]);
  }
  window.addEventListener('hashchange', follow);
  follow();
})();

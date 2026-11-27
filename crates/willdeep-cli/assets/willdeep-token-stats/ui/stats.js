(() => {
  const { locale, SOURCES, WEEK, today, monthOf, changeMonth, weekOffset, datesBetween,
    requestRange, fullDate, monthDate, weekday, query } = tokenStats;
  const number = new Intl.NumberFormat(locale);
  const compact = new Intl.NumberFormat(locale, { notation: 'compact', maximumFractionDigits: 0 });
  const compactActivity = new Intl.NumberFormat(locale, { notation: 'compact', maximumFractionDigits: 1 });
  const shortDate = new Intl.DateTimeFormat(locale, { month: 'long', day: 'numeric', timeZone: 'UTC' });
  const HEAT_LEVELS = 4;
  const TOOLTIP_GAP = 8;
  const percent = new Intl.NumberFormat(locale, { style: 'percent', maximumFractionDigits: 1 });
  const t = (key, values = {}) => strings[key].replace(/\{(\w+)\}/g, (_, name) => values[name]);
  const num = (value) => Number.isFinite(value) ? number.format(value) : t('dash');
  const rate = (value) => Number.isFinite(value) ? percent.format(value) : t('dash');
  const cost = (totals) => totals?.calls > 0 ? t('unpriced') : t('dash');
  const byId = (id) => document.getElementById(id);
  const content = byId('content');
  const status = byId('status');
  let currentDay = today();
  const state = { view: 'overview', source: 'all', month: monthOf(currentDay), date: currentDay,
    mode: 'heat', texture: false, sequence: 0, result: null };

  function node(tag, className, ...children) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    element.append(...children);
    return element;
  }

  function button(label, action, className) {
    const element = node('button', className, label);
    element.type = 'button';
    element.addEventListener('click', action);
    return element;
  }

  function region(heading, ...children) {
    const title = node('h2', '', heading);
    title.tabIndex = -1;
    return node('section', '', node('div', 'section-head', title), ...children);
  }

  function metric(label, value, extra) {
    const element = node('div', 'metric', node('dt', '', t(label)), node('dd', '', value));
    if (extra) element.lastElementChild.append(node('small', '', extra));
    return element;
  }

  function metrics(...items) { return node('dl', 'panel metrics', ...items); }
  function notice(text) { return node('p', 'notice', text); }
  const emptyText = () => t(state.source === 'all' ? 'empty' : 'filteredEmpty');

  function table(heading, columns, rows) {
    const header = columns.map((key) => node('th', '', t(key)));
    header.forEach((cell) => { cell.scope = 'col'; });
    const body = rows.map((row) => {
      const first = node('th', '', row[0]);
      first.scope = 'row';
      return node('tr', '', first, ...row.slice(1).map((value) => node('td', '', value)));
    });
    const scroller = node('div', 'scroll table-box', node('table', '',
      node('thead', '', node('tr', '', ...header)), node('tbody', '', ...body)));
    scroller.tabIndex = 0;
    scroller.setAttribute('role', 'region');
    scroller.setAttribute('aria-label', heading);
    return scroller;
  }

  const indexDays = (response) => new Map(response.days.map(({ date, totals }) => [date, totals]));
  function dayReadout(date, totals) {
    if (!totals?.calls && totals?.tokens == null) return t('missingReadout', { date: fullDate(date), state: t('none') });
    if (totals?.tokens == null) return t('missingReadout', { date: fullDate(date), state: t('unknownUsage') }) +
      '，' + t('readoutTail', { sessions: num(totals.sessions), cost: cost(totals) });
    return t('readout', { date: fullDate(date), tokens: num(totals.tokens), sessions: num(totals.sessions), cost: cost(totals) });
  }

  function openDay(date) {
    state.date = date;
    state.month = monthOf(date);
    go('day');
  }

  function dayList(response, days) {
    return table(t('activity'), ['date', 'tokens', 'sessions', 'cost'], days.map((date) => {
      const totals = response.get(date);
      const tokens = totals?.tokens == null ? t(totals?.calls ? 'unknownUsage' : 'none') : num(totals.tokens);
      return [button(fullDate(date), () => openDay(date)), tokens, num(totals?.sessions), cost(totals)];
    }));
  }

  function wireDateKeys(container, columns, columnMajor) {
    const cells = Array.from(container.querySelectorAll('button[data-date]:not(:disabled)'));
    const deltas = columnMajor ? { ArrowLeft: -columns, ArrowRight: columns, ArrowUp: -1, ArrowDown: 1 }
      : { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -columns, ArrowDown: columns };
    const startCell = cells.find((cell) => cell.dataset.date === state.date) || cells.at(-1);
    if (startCell) startCell.tabIndex = 0;
    container.addEventListener('focusin', (event) => {
      if (!cells.includes(event.target)) return;
      for (const cell of cells) cell.tabIndex = cell === event.target ? 0 : -1;
    });
    container.addEventListener('keydown', (event) => {
      const index = cells.indexOf(event.target);
      if (index === -1 || !(event.key in deltas || ['Home', 'End'].includes(event.key))) return;
      event.preventDefault();
      if (columnMajor) {
        const row = weekOffset(cells[index].dataset.date);
        if ((event.key === 'ArrowUp' && row === 0) || (event.key === 'ArrowDown' && row === WEEK - 1)) return;
      }
      const target = event.key === 'Home' ? cells[0] : event.key === 'End' ? cells.at(-1) : cells[index + deltas[event.key]];
      target?.focus();
    });
  }

  function activityFacts(day, exact = false) {
    const totals = day?.totals;
    if (!totals?.calls) return [t('none')];
    const facts = [t('hoverCalls', { count: num(totals.calls) })];
    facts.push(totals.tokens == null ? t('unknownUsage') : t('hoverTokens', {
      count: exact ? num(totals.tokens) : compactActivity.format(totals.tokens),
    }));
    if (state.source === 'all') {
      for (const source of SOURCES.slice(1)) {
        const count = day.source_calls?.[source];
        if (count > 0) facts.push(t('hoverSource', { source: t(source), count: num(count) }));
      }
    }
    if (totals.cache_rate != null) facts.push(t('hoverCache', { rate: rate(totals.cache_rate) }));
    return facts;
  }

  function positionActivityTooltip(tooltip, cell) {
    const anchor = cell.getBoundingClientRect();
    const { width, height } = tooltip.getBoundingClientRect();
    const left = Math.max(TOOLTIP_GAP, Math.min(anchor.left + (anchor.width - width) / 2,
      document.documentElement.clientWidth - width - TOOLTIP_GAP));
    const above = anchor.top - height - TOOLTIP_GAP;
    tooltip.style.left = `${left}px`;
    const below = anchor.bottom + TOOLTIP_GAP;
    const top = above >= TOOLTIP_GAP ? above : Math.min(below, document.documentElement.clientHeight - height - TOOLTIP_GAP);
    tooltip.style.top = `${Math.max(TOOLTIP_GAP, top)}px`;
  }

  function heatmap(response, days, max) {
    const initialOffset = weekOffset(days[0]);
    const weeks = Math.ceil((initialOffset + days.length) / WEEK);
    const step = Math.max(1, Math.ceil(max / HEAT_LEVELS));
    const grid = node('div', 'heat');
    grid.style.setProperty('--weeks', weeks);
    const scroller = node('div', 'scroll heat-scroll', grid);
    const tooltip = node('div', 'activity-tooltip');
    tooltip.setAttribute('aria-hidden', 'true');
    tooltip.hidden = true;
    let hovered = null;
    let focused = null;
    const syncTooltip = () => {
      const cell = hovered ?? focused;
      tooltip.hidden = !cell;
      if (!cell) return;
      const date = cell.dataset.date;
      const heading = shortDate.format(new Date(`${date}T00:00:00Z`));
      tooltip.replaceChildren(node('strong', '', heading), ...activityFacts(response.get(date))
        .map((fact) => node('span', '', t('hoverJoin'), fact)));
      positionActivityTooltip(tooltip, cell);
    };
    scroller.addEventListener('scroll', () => {
      hovered = null;
      if (focused) {
        const cell = focused.getBoundingClientRect();
        const bounds = scroller.getBoundingClientRect();
        if (cell.left < bounds.left || cell.right > bounds.right) focused = null;
      }
      syncTooltip();
    });
    grid.addEventListener('keydown', (event) => {
      if (event.key === 'Escape' && !tooltip.hidden) {
        event.stopPropagation();
        hovered = focused = null;
        syncTooltip();
      }
    });
    let lastMonth = '';
    const specialLevels = new Set(['none']);
    days.forEach((date, index) => {
      const offset = initialOffset + index;
      const column = Math.floor(offset / WEEK) + 1;
      const month = monthOf(date);
      if (month !== lastMonth) {
        lastMonth = month;
        const label = node('span', 'heat-label', t('localMonth', { month: num(Number(date.slice(5, 7))) }));
        label.style.gridArea = `${WEEK + 1} / ${column} / auto / span ${Math.min(2, weeks - column + 1)}`;
        grid.append(label);
      }
      const day = response.get(date);
      const tokens = day?.totals.tokens;
      const level = tokens == null ? (day?.totals.calls ? 'unknown' : 'none') : tokens === 0 ? 'zero' : Math.ceil(tokens / step);
      if (typeof level === 'string') specialLevels.add(level);
      const cell = button('', () => openDay(date));
      cell.setAttribute('aria-label', [fullDate(date), ...activityFacts(day, true)].join(t('hoverJoin')));
      cell.tabIndex = -1;
      cell.dataset.level = level;
      cell.dataset.date = date;
      cell.style.gridArea = `${offset % WEEK + 1} / ${column}`;
      cell.append(node('span', 'mark'));
      cell.addEventListener('pointerenter', (event) => {
        if (event.pointerType === 'touch') return;
        hovered = cell;
        syncTooltip();
      });
      cell.addEventListener('pointerleave', () => { hovered = null; syncTooltip(); });
      cell.addEventListener('focus', () => { focused = cell; hovered = null; syncTooltip(); });
      cell.addEventListener('blur', () => { focused = null; syncTooltip(); });
      grid.append(cell);
    });
    wireDateKeys(grid, WEEK, true);
    const legend = node('figcaption', 'legend muted');
    for (const [level, label] of [['none', 'none'], ['unknown', 'unknownUsage'], ['zero', 'zero']]) {
      if (!specialLevels.has(level)) continue;
      const item = node('span', 'legend-item', node('span', 'mark'), t(label));
      item.dataset.level = level;
      legend.append(item);
    }
    if (max > 0) {
      const ramp = node('span', 'legend-ramp', t('low'));
      for (let level = 1; (level - 1) * step < max; level++) {
        const mark = node('span', 'mark');
        mark.dataset.level = level;
        mark.title = t('band', { start: num((level - 1) * step + 1), end: num(Math.min(level * step, max)) });
        ramp.append(mark);
      }
      ramp.append(t('high'));
      legend.append(ramp);
    }
    requestAnimationFrame(() => { scroller.scrollLeft = scroller.scrollWidth; });
    return node('figure', state.texture ? 'textured' : '', scroller, tooltip, legend,
      node('p', 'sr-only', t('heatHelp')));
  }

  function overview(response, range) {
    const days = datesBetween(range.start, range.end);
    const indexed = indexDays(response);
    const known = response.days.filter(({ totals }) => Number.isFinite(totals.tokens));
    const peak = known.reduce((best, entry) => !best || entry.totals.tokens > best.totals.tokens ? entry : best, null);
    const chart = region(t('activity'));
    chart.className = 'activity';
    const sources = node('div', 'activity-controls');
    sources.setAttribute('role', 'group');
    sources.setAttribute('aria-label', t('source'));
    for (const source of SOURCES) {
      const control = button(t(source === 'all' ? 'allActivity' : source), () => {
        if (state.source === source) return;
        state.source = source;
        load({ focus: control.id });
      });
      control.id = `activity-source-${source}`;
      control.setAttribute('aria-pressed', String(source === state.source));
      sources.append(control);
    }
    const modes = node('div', 'modes activity-controls');
    for (const mode of ['heat', 'table', 'texture']) {
      const control = button(t(mode), () => {
        if (mode === 'texture') state.texture = !state.texture;
        else state.mode = mode;
        renderResult(response, range);
        byId(`mode-${mode}`)?.focus();
      });
      control.id = `mode-${mode}`;
      control.setAttribute('aria-pressed', String(mode === 'texture' ? state.texture : state.mode === mode));
      control.hidden = mode === 'texture' && state.mode !== 'heat';
      modes.append(control);
    }
    chart.firstElementChild.append(node('div', 'activity-toolbar', sources, modes));
    const activityDays = new Map(response.days.map((day) => [day.date, day]));
    chart.append(state.mode === 'table' ? node('div', 'panel', dayList(indexed, days))
      : heatmap(activityDays, days, peak?.totals.tokens ?? 0));
    const totals = response.totals;
    return [metrics(metric('tokens', num(totals.tokens)), metric('peak', num(peak?.totals.tokens), peak && fullDate(peak.date)),
      metric('active_days', num(totals.active_days)), metric('sessions', num(totals.sessions))), chart];
  }

  function calendar(response, range) {
    const indexed = indexDays(response);
    const days = datesBetween(range.start, range.end);
    const heading = monthDate(range.start);
    const section = region(heading);
    const actions = node('div', 'month-actions');
    for (const [label, delta] of [['previous', -1], ['current', 0], ['next', 1]]) {
      const control = button(t(label), () => {
        state.month = delta ? changeMonth(state.month, delta) : monthOf(currentDay);
        load({ focus: `month-${label}` });
      });
      control.id = `month-${label}`;
      control.disabled = delta >= 0 && state.month >= monthOf(currentDay);
      actions.append(control);
    }
    section.firstElementChild.append(actions);
    const offset = weekOffset(range.start);
    const headings = Array.from({ length: WEEK }, (_, index) => {
      const label = weekday(tokenStats.addDays(range.start, index - offset));
      const cell = node('th', '', label);
      cell.scope = 'col';
      return cell;
    });
    const body = node('tbody');
    for (let start = -offset; start < days.length; start += WEEK) {
      const row = node('tr');
      for (let column = 0; column < WEEK; column++) {
        const date = days[start + column];
        const totals = indexed.get(date);
        const cell = node('td');
        if (date) {
          const dayNumber = num(Number(date.slice(-2)));
          const control = button('', () => openDay(date), 'month-day');
          control.tabIndex = -1;
          control.dataset.date = date;
          control.disabled = date > currentDay;
          control.setAttribute('aria-label', dayReadout(date, totals));
          control.setAttribute('aria-pressed', String(date === state.date));
          if (date === currentDay) {
            control.classList.add('today');
            control.setAttribute('aria-current', 'date');
          }
          const fullTokens = totals?.tokens == null ? t('dash') : t('tokenAmount', { value: num(totals.tokens) });
          const compactTokens = totals?.tokens == null ? t('dash') : compact.format(totals.tokens);
          const sessionCount = !totals?.calls ? t('dash') : t('sessionAmount', { value: num(totals.sessions) });
          control.append(node('strong', '', dayNumber), node('span', 'month-tokens full', fullTokens),
            node('span', 'month-tokens compact', compactTokens), node('span', '', sessionCount), node('span', '', cost(totals)));
          cell.append(control);
        }
        row.append(cell);
      }
      body.append(row);
    }
    const grid = node('table', 'month-grid', node('thead', '', node('tr', '', ...headings)), body);
    grid.setAttribute('aria-label', heading);
    wireDateKeys(grid, WEEK, false);
    section.append(node('div', 'panel', grid));
    const totals = response.totals;
    return [metrics(metric('monthly', num(totals.tokens)), metric('sessions', num(totals.sessions)),
      metric('calls', num(totals.calls)), metric('cost', cost(totals))), section];
  }

  const groupName = (group) => node('span', 'identity', group.model || t('modelUnknown'),
    node('small', 'muted identity', group.provider || t('providerUnknown')));

  function agents(response) {
    const columns = ['model', 'calls', 'worker_runs', 'sessions', 'unknown_run_calls', 'errors', 'cancelled'];
    const rows = [...response.agents].sort((a, b) => b.totals.calls - a.totals.calls);
    const section = region(t('agents'));
    if (!rows.length) {
      section.append(node('div', 'panel', notice(t('noAgents'))));
      return [section];
    }
    const missing = rows.reduce((sum, row) => sum + (row.totals.unknown_run_calls ?? 0), 0);
    if (missing) section.append(notice(t('runCoverage', { count: num(missing) })));
    section.append(notice(t('runOutcome')), node('div', 'panel', table(t('agents'), columns,
      rows.map((row) => [groupName(row), ...columns.slice(1).map((key) => num(row.totals[key]))]))));
    return [section];
  }

  function shareImage(date, source, totals) {
    const lines = [
      [t('tokens'), num(totals.tokens)], [t('sessions'), num(totals.sessions)],
      [t('calls'), num(totals.calls)], [t('cost'), cost(totals)],
    ];
    const canvas = node('canvas');
    const width = 720;
    const margin = 40;
    const rowHeight = 48;
    canvas.width = width;
    canvas.height = 152 + rowHeight * lines.length;
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('Unable to create canvas context');
    ctx.fillStyle = '#ffffff';
    ctx.fillRect(0, 0, width, canvas.height);
    ctx.fillStyle = '#182432';
    ctx.font = 'bold 32px system-ui';
    ctx.fillText(t('title'), margin, 58);
    ctx.font = '20px system-ui';
    ctx.fillText(`${fullDate(date)}　${t(source)}`, margin, 95);
    lines.forEach(([label, value], index) => {
      const y = 147 + index * rowHeight;
      ctx.strokeStyle = '#d6dbe2';
      ctx.beginPath();
      ctx.moveTo(margin, y - 29);
      ctx.lineTo(width - margin, y - 29);
      ctx.stroke();
      ctx.textAlign = 'left';
      ctx.fillText(label, margin, y);
      ctx.textAlign = 'right';
      ctx.fillText(value, width - margin, y);
    });
    return canvas.toDataURL('image/png');
  }

  function day(response, range) {
    const totals = response.totals;
    const section = region(fullDate(range.start));
    const sharePanel = node('div', 'image-panel');
    const share = button(t('share'), () => {
      try {
        const url = shareImage(range.start, state.source, totals);
        const image = node('img');
        image.src = url;
        image.alt = dayReadout(range.start, totals);
        const download = node('a', 'download', t('download'));
        download.href = url;
        download.download = `token-stats-${range.start}.png`;
        const close = button(t('close'), () => { sharePanel.replaceChildren(); share.focus(); });
        sharePanel.replaceChildren(image, notice(t('imageHelp')), download, close);
        download.focus();
      } catch (error) {
        console.warn('token-stats: PNG encoding failed', { name: error?.name });
        sharePanel.replaceChildren(notice(t('imageError')));
      }
    });
    share.disabled = !totals.calls;
    section.firstElementChild.append(share);
    const pricing = totals.unpriced_calls > 0 ? t('unpricedCount', { count: num(totals.unpriced_calls) }) : null;
    section.append(metrics(metric('tokens', num(totals.tokens)), metric('sessions', num(totals.sessions)),
      metric('calls', num(totals.calls)), metric('cost', cost(totals), pricing)), sharePanel);
    const columns = ['model', 'input', 'output', 'cache_read', 'cache_rate', 'calls', 'cost'];
    if (response.models.length) {
      const rows = [...response.models].sort((a, b) => (b.totals.tokens ?? -1) - (a.totals.tokens ?? -1))
        .map((row) => [groupName(row), num(row.totals.input), num(row.totals.output), num(row.totals.cache_read),
          rate(row.totals.cache_rate), num(row.totals.calls), cost(row.totals)]);
      return [section, region(t('modelDetails'), node('div', 'panel', table(t('modelDetails'), columns, rows)), notice(t('cacheHelp')))];
    }
    return [section];
  }

  const renderers = { overview, calendar, day, agents };
  const views = ['overview', 'calendar', 'agents'];

  function renderResult(response, range) {
    const notices = [];
    if (response.skipped_rows > 0) notices.push(notice(t('skipped', { count: num(response.skipped_rows) })));
    const totals = response.totals;
    if (!totals.calls) notices.push(notice(emptyText()));
    else if (state.view !== 'agents' && totals.known_usage_calls < totals.calls) {
      notices.push(notice(t('coverage', { known: num(totals.known_usage_calls), calls: num(totals.calls) })));
    }
    content.replaceChildren(...notices, ...renderers[state.view](response, range));
  }

  function renderChrome(range) {
    for (const control of byId('navigation').children) {
      if (control.dataset.view === (state.view === 'day' ? 'calendar' : state.view)) control.setAttribute('aria-current', 'page');
      else control.removeAttribute('aria-current');
    }
    const rangeLabel = state.view === 'day' ? fullDate(range.start) : state.view === 'calendar' ? monthDate(range.start)
      : t('range', { start: fullDate(range.start), end: fullDate(range.end) });
    byId('range').textContent = state.view === 'agents' ? `${t('recent')} (${rangeLabel})` : rangeLabel;
    byId('source-filter').hidden = state.view === 'overview';
    byId('source-select').value = state.source;
    byId('back').hidden = state.view !== 'day';
  }

  function focusTarget(target) {
    if (!target) return;
    let element = target === 'heading' ? content.querySelector('h2') : byId(target);
    if (target === 'date') element = content.querySelector(`[data-date='${state.date}']`);
    if (!element || element.disabled) element = content.querySelector('h2, .message button');
    element?.focus();
  }

  async function load({ focus, refresh = false } = {}) {
    currentDay = today();
    const range = requestRange(state.view, currentDay, state.month, state.date);
    const view = state.view;
    const source = state.source;
    const sequence = ++state.sequence;
    renderChrome(range);
    if (state.result?.view !== view) content.replaceChildren();
    content.setAttribute('aria-busy', 'true');
    content.inert = true;
    status.textContent = t('loading');
    try {
      const response = await query(range, source);
      if (sequence !== state.sequence) return;
      state.result = { view };
      renderResult(response, range);
      status.textContent = refresh ? t('refreshed') : '';
    } catch (error) {
      if (sequence !== state.sequence) return;
      state.result = null;
      const kind = error?.kind || 'error';
      console.warn('token-stats: query failed', { kind, detail: range.detail });
      const message = node('section', 'panel message', node('p', '', t(kind)), button(t('retry'), () => load({ refresh: true })));
      content.replaceChildren(message);
      status.textContent = t(kind);
    } finally {
      if (sequence === state.sequence) {
        content.setAttribute('aria-busy', 'false');
        content.inert = false;
        focusTarget(focus);
      }
    }
  }

  function go(view, focus = 'heading') {
    if (state.view === view) return;
    state.view = view;
    load({ focus });
  }

  document.title = t('title');
  byId('header').append(node('h1', '', t('title')));
  const nav = byId('navigation');
  nav.setAttribute('aria-label', t('nav'));
  for (const view of views) {
    const control = button(t(view), () => go(view));
    control.dataset.view = view;
    nav.append(control);
  }
  const back = button(t('back'), () => go('calendar', 'date'));
  back.id = 'back';
  const select = node('select');
  for (const source of SOURCES) {
    const option = node('option', '', t(source));
    option.value = source;
    select.append(option);
  }
  select.addEventListener('change', () => { state.source = select.value; load(); });
  const range = node('span', 'range');
  range.id = 'range';
  select.id = 'source-select';
  const sourceFilter = node('label', '', t('source'), select);
  sourceFilter.id = 'source-filter';
  byId('filters').append(back, range, sourceFilter, button(t('refresh'), () => load({ refresh: true })));
  load();
})();

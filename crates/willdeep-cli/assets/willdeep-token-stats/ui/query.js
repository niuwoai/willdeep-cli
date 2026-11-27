const tokenStats = (() => {
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const locale = 'zh-CN';
  const dateParts = new Intl.DateTimeFormat('en-CA', {
    year: 'numeric', month: '2-digit', day: '2-digit', timeZone,
  });
  const fullDate = new Intl.DateTimeFormat(locale, { dateStyle: 'long', timeZone: 'UTC' });
  const monthDate = new Intl.DateTimeFormat(locale, { year: 'numeric', month: 'long', timeZone: 'UTC' });
  const weekday = new Intl.DateTimeFormat(locale, { weekday: 'short', timeZone: 'UTC' });
  const WEEK = 7;
  const OVERVIEW_MONTHS = 8;
  const AGENT_DAYS = 90;
  const SOURCES = ['all', 'cli', 'tui', 'mobile', 'unknown'];
  const DENIED = /permission|authoriz|approv|forbidden|disabled|denied|not enabled|权限|授权|禁用|停用|未启用/i;

  class QueryError extends Error {
    constructor(kind) {
      super(`token-stats.query: ${kind}`);
      this.kind = kind;
    }
  }

  function today() {
    const parts = Object.fromEntries(dateParts.formatToParts().map(({ type, value }) => [type, value]));
    return `${parts.year}-${parts.month}-${parts.day}`;
  }

  // ISO strings here represent wall-calendar days. UTC arithmetic steps over DST safely.
  const parseDate = (date) => new Date(`${date}T00:00:00Z`);
  const iso = (date) => date.toISOString().slice(0, 10);
  const addDays = (date, offset) => {
    const result = parseDate(date);
    result.setUTCDate(result.getUTCDate() + offset);
    return iso(result);
  };
  const monthOf = (date) => `${date.slice(0, 7)}-01`;
  const changeMonth = (month, offset) => {
    const result = parseDate(month);
    result.setUTCMonth(result.getUTCMonth() + offset);
    return iso(result);
  };
  const monthEnd = (month) => addDays(changeMonth(month, 1), -1);
  const weekOffset = (date) => (parseDate(date).getUTCDay() + WEEK - 1) % WEEK;
  const datesBetween = (start, end) => {
    const dates = [];
    for (let date = start; date <= end; date = addDays(date, 1)) dates.push(date);
    return dates;
  };

  function requestRange(view, currentDay, month, date) {
    if (view === 'day') return { start: date, end: date, detail: 'day' };
    if (view === 'calendar') return { start: month, end: monthEnd(month), detail: 'summary' };
    if (view === 'agents') return { start: addDays(currentDay, 1 - AGENT_DAYS), end: currentDay, detail: 'agents' };
    return { start: changeMonth(monthOf(currentDay), 1 - OVERVIEW_MONTHS), end: currentDay, detail: 'summary' };
  }

  function decode(raw) {
    let result = typeof raw === 'string' ? JSON.parse(raw) : raw;
    if (result?.isError || result?.is_error || result?.ok === false) {
      const text = result.content?.find?.((entry) => typeof entry?.text === 'string')?.text;
      throw new QueryError(DENIED.test(text || result.error || '') ? 'denied' : 'error');
    }
    if (Array.isArray(result?.content)) {
      const text = result.content.find((entry) => entry?.type === 'text' && typeof entry.text === 'string')?.text;
      result = JSON.parse(text);
    }
    if (result?.ok === false) throw new QueryError(DENIED.test(result.error || '') ? 'denied' : 'error');
    if (!result?.totals || !Array.isArray(result.days) || !Array.isArray(result.agents) || !Array.isArray(result.models)) {
      throw new QueryError('error');
    }
    return result;
  }

  async function query(range, source) {
    if (typeof window.willdeep?.executeCommand !== 'function') throw new QueryError('missing');
    try {
      const raw = await window.willdeep.executeCommand('token-stats.query', { ...range, timezone: timeZone, source });
      return decode(raw);
    } catch (error) {
      if (error instanceof QueryError) throw error;
      throw new QueryError(DENIED.test(String(error?.message || error)) ? 'denied' : 'error');
    }
  }

  return {
    locale, SOURCES, WEEK, today, addDays, monthOf, changeMonth, weekOffset, datesBetween, requestRange, query, decode,
    fullDate: (date) => fullDate.format(parseDate(date)),
    monthDate: (date) => monthDate.format(parseDate(date)),
    weekday: (date) => weekday.format(parseDate(date)),
  };
})();

import { describe, expect, test } from 'vitest'
import { renderFilterBar, yearMonthDayFilter } from './FilterBar'

describe('filter bar', () => {
  test('shows selected year, month, and day from indexed albums', () => {
    const html = renderFilterBar({ from: '2024-03-01' }, ['2025-05-08', '2024-03-01'])

    expect(html).toContain('2024 年')
    expect(html).toContain('03 月')
    expect(html).toContain('01 日')
  })

  test('disables month selection until a year is selected', () => {
    const html = renderFilterBar({}, ['2026-08-04', '2025-01-01'])

    expect(html).toContain('全部年份')
    expect(html).toContain('全部月份')
    expect(html).toContain('全部日期')
    expect(html).toContain('aria-label="选择月份"')
    expect(html).toContain('data-part="month" aria-label="选择月份" aria-haspopup="listbox" aria-expanded="false" disabled')
    expect(html).toContain('data-part="day" aria-label="选择日期" aria-haspopup="listbox" aria-expanded="false" disabled')
  })

  test('creates complete year, month, and day boundaries', () => {
    expect(yearMonthDayFilter('2024', '', '')).toEqual({ from: '2024-01-01', to: '2024-12-31' })
    expect(yearMonthDayFilter('2024', '02', '')).toEqual({ from: '2024-02-01', to: '2024-02-29' })
    expect(yearMonthDayFilter('2024', '02', '29')).toEqual({ from: '2024-02-29', to: '2024-02-29' })
  })
})

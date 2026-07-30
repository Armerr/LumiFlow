import { describe, expect, test } from 'vitest'
import { renderFilterBar } from './FilterBar'

describe('filter bar', () => {
  test('shows selected year and month with a reset button', () => {
    const html = renderFilterBar({ from: '2024-03-01' })

    expect(html).toContain('2024 ▾')
    expect(html).toContain('03 月 ▾')
    expect(html).toContain('清空筛选')
  })

  test('shows neutral labels and hides reset when filter is empty', () => {
    const html = renderFilterBar({})

    expect(html).toContain('全部年份 ▾')
    expect(html).toContain('全部月份 ▾')
    expect(html).not.toContain('清空筛选')
  })
})

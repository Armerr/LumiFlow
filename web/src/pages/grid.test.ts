import { describe, expect, test } from 'vitest'
import { renderGridHeader } from './grid'

describe('grid album header', () => {
  test('renders the deterministic album name and optional AI description', () => {
    const html = renderGridHeader({
      name: '2026年6月2日 · 上海',
      description: '清晨的城市与朋友。',
    }, 2)

    expect(html).toContain('2026年6月2日 · 上海')
    expect(html).toContain('清晨的城市与朋友。')
    expect(html).toContain('2 张照片')
  })

  test('escapes album names and AI descriptions before rendering', () => {
    const html = renderGridHeader({
      name: '<img src=x onerror=alert(1)>',
      description: '<script>alert("description")</script>',
    }, 0)

    expect(html).not.toContain('<img')
    expect(html).not.toContain('<script>')
    expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;')
    expect(html).toContain('&lt;script&gt;alert("description")&lt;/script&gt;')
  })
})

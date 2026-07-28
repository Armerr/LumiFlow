import { describe, expect, test } from 'vitest'
import { getSwipeTargetId, renderOriginalDownloadLink, renderPhotoInfo, shouldBlockNativeDetailNavigation } from './detail'

describe('detail swipe navigation', () => {
  test('uses a low horizontal threshold for phone swipes', () => {
    expect(getSwipeTargetId({ startX: 320, endX: 250, startY: 420, endY: 426 }, 2, 6)).toBe(3)
    expect(getSwipeTargetId({ startX: 160, endX: 232, startY: 420, endY: 416 }, 2, 6)).toBe(1)
    expect(getSwipeTargetId({ startX: 320, endX: 286, startY: 420, endY: 421 }, 2, 6)).toBeNull()
    expect(getSwipeTargetId({ startX: 320, endX: 250, startY: 420, endY: 520 }, 2, 6)).toBeNull()
  })

  test('does not navigate beyond detail bounds', () => {
    expect(getSwipeTargetId({ startX: 320, endX: 250, startY: 420, endY: 426 }, 5, 6)).toBeNull()
    expect(getSwipeTargetId({ startX: 160, endX: 232, startY: 420, endY: 416 }, 0, 6)).toBeNull()
  })

  test('blocks native browser edge navigation during detail swipes', () => {
    expect(shouldBlockNativeDetailNavigation({ startX: 2, endX: 8, startY: 420, endY: 421 }, 390)).toBe(true)
    expect(shouldBlockNativeDetailNavigation({ startX: 388, endX: 382, startY: 420, endY: 421 }, 390)).toBe(true)
    expect(shouldBlockNativeDetailNavigation({ startX: 2, endX: 4, startY: 420, endY: 462 }, 390)).toBe(false)
    expect(shouldBlockNativeDetailNavigation({ startX: 120, endX: 60, startY: 420, endY: 421 }, 390)).toBe(false)
  })
})

describe('detail original download', () => {
  test('renders same-origin original download link for mobile detail UI', () => {
    const html = renderOriginalDownloadLink('Test Album', {
      name: 'DSCF6138.HIF',
      width: 5152,
      height: 7728,
      size_bytes: 8178892,
      format: 'HIF',
    })

    expect(html).toContain('class="original-download-link"')
    expect(html).toContain('href="/api/photos/Test%20Album/DSCF6138.HIF?download=1"')
    expect(html).toContain('download="DSCF6138.HIF"')
    expect(html).toContain('下载原图')
  })
})

describe('photo metadata rendering', () => {
  test('groups useful basic, device, shooting, tag, and tone fields', () => {
    const html = renderPhotoInfo({
      make: 'FUJIFILM',
      model: 'X-T5',
      lens: 'XF56mmF1.2 R WR',
      focal_length: '56mm',
      aperture: 'f/2.8',
      shutter_speed: '1/1250s',
      iso: 160,
      date_taken: '2026/6/2 11:56:45',
      timezone: 'UTC+9',
      gps: { lat: 35.6586, lon: 139.7454 },
      dimensions: { width: 5152, height: 7728 },
      file_size: 8178892,
      format: 'HIF',
      flash: null,
      software: 'Digital Camera X-T5 Ver4.31',
      orientation: 1,
      artist: 'INNEI',
      color_space: 'sRGB',
      image_description: null,
      user_comment: null,
      tags: ['日本', '东京', '浅草'],
      tone: { type: '高调', brightness: 79, contrast: 55, shadows: 11, highlights: 69 },
    }, {
      id: 0,
      name: 'DSCF6138.HIF',
      width: 5152,
      height: 7728,
      size_bytes: 8178892,
      format: 'HIF',
    })

    expect(html).toContain('基本信息')
    expect(html).toContain('DSCF6138')
    expect(html).toContain('5152 × 7728')
    expect(html).toContain('39 MP')
    expect(html).toContain('UTC+9')
    expect(html).toContain('拍摄信息')
    expect(html).toContain('拍摄设备')
    expect(html).toContain('拍摄地址')
    expect(html).toContain('35.6586')
    expect(html).toContain('139.7454')
    expect(html).toContain('设备信息')
    expect(html).toContain('FUJIFILM X-T5')
    expect(html).toContain('拍摄参数')
    expect(html).toContain('56mm')
    expect(html).toContain('f/2.8')
    expect(html).toContain('1/1250s')
    expect(html).toContain('ISO 160')
    expect(html).toContain('标签')
    expect(html).toContain('日本')
    expect(html).toContain('影调分析')
    expect(html).toContain('高调')
    expect(html).toContain('79%')
  })

  test('rounds small megapixel counts instead of showing zero', () => {
    const html = renderPhotoInfo(null, {
      id: 0,
      name: 'studio-test-editorial-v2-01.jpg',
      width: 1200,
      height: 800,
      size_bytes: 393479,
      format: 'JPG',
    })

    expect(html).toContain('1 MP')
    expect(html).not.toContain('0 MP')
  })
})
